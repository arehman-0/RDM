//! External tools the app needs for video downloads — yt-dlp, a JavaScript
//! runtime for it (QuickJS-NG) and FFmpeg — found, installed and updated in
//! ONE place. No Tauri in here: the UI is reached through [`ComponentEvents`].
//!
//! * **Resolution order** per tool: the per-user components dir (writable,
//!   updatable) → a copy bundled next to the exe → `PATH`.
//! * **Pinned manifest**: versions, URLs and SHA-256 hashes live in
//!   `components.json`, embedded at build time. Nothing else names a URL.
//! * **Install**: download to a `.part` file (resumable with `Range`), verify
//!   the SHA-256, and only then move it into place (rename; the previous copy
//!   is kept as `.old`). A zip is verified as a whole, then only the listed
//!   files are extracted (by file name, never by archive path).
//! * **yt-dlp update**: at most daily; the new binary must match the
//!   release's `SHA2-256SUMS` and pass a `--version` smoke test, otherwise the
//!   previous one is restored.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tracing::{info, warn};

use crate::history::write_json_atomic;
use crate::process;

/// The pinned manifest, embedded in the binary.
pub const MANIFEST_JSON: &str = include_str!("../components.json");
const MANIFEST_VERSION: u32 = 1;
/// How often yt-dlp is checked for an update automatically.
pub const UPDATE_INTERVAL_SECS: u64 = 24 * 60 * 60;
/// A forced check (after an extraction failure) is still limited to this.
pub const FORCED_UPDATE_MIN_SECS: u64 = 10 * 60;
const SMOKE_TIMEOUT: Duration = Duration::from_secs(60);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub version: u32,
    pub components: Vec<ComponentSpec>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComponentSpec {
    /// Stable id, also the folder name (`yt-dlp`, `qjs`, `ffmpeg`).
    pub id: String,
    pub name: String,
    pub version: String,
    /// Executable file name inside the component's folder.
    pub exe: String,
    pub url: String,
    /// SHA-256 (hex) of the downloaded file (the archive, for a zip).
    pub sha256: String,
    pub size: u64,
    pub license: String,
    pub homepage: String,
    pub purpose: String,
    /// Present when `url` is a zip: which files to take out of it.
    #[serde(default)]
    pub archive: Option<ArchiveSpec>,
    /// Present when the tool can be updated past the pinned version.
    #[serde(default)]
    pub update: Option<UpdateSpec>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArchiveSpec {
    /// Folder inside the zip's top-level directory to take files from.
    pub dir: String,
    /// A file is extracted when its name equals, or ends with, one of these.
    pub include: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateSpec {
    /// GNU-style checksum list of the latest release.
    pub sums_url: String,
    pub asset_url: String,
    /// The line to look for in the checksum list.
    pub asset_name: String,
}

fn is_plain_name(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.contains(['/', '\\', ':'])
        && s.chars().all(|c| !c.is_control())
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Manifest {
    /// Parse + validate. `allow_http` is for tests against a local server;
    /// the embedded manifest must be https only.
    pub fn parse(json: &str, allow_http: bool) -> Result<Self, String> {
        let m: Manifest = serde_json::from_str(json).map_err(|e| format!("manifest: {e}"))?;
        if m.version != MANIFEST_VERSION {
            return Err(format!("manifest: unsupported version {}", m.version));
        }
        let url_ok =
            |u: &str| u.starts_with("https://") || (allow_http && u.starts_with("http://"));
        let mut seen = Vec::new();
        for c in &m.components {
            let bad = |what: &str| Err(format!("manifest: component {:?}: {what}", c.id));
            if !is_plain_name(&c.id) || seen.contains(&c.id) {
                return bad("id must be unique and a plain name");
            }
            seen.push(c.id.clone());
            if !is_plain_name(&c.exe) {
                return bad("exe must be a plain file name");
            }
            if !is_sha256(&c.sha256) {
                return bad("sha256 must be 64 hex characters");
            }
            if !url_ok(&c.url) {
                return bad("url must be https");
            }
            if let Some(a) = &c.archive {
                if !is_plain_name(&a.dir) || a.include.is_empty() {
                    return bad("archive needs a plain dir and an include list");
                }
            }
            if let Some(u) = &c.update {
                if !url_ok(&u.sums_url) || !url_ok(&u.asset_url) || !is_plain_name(&u.asset_name) {
                    return bad("update URLs must be https and assetName a plain name");
                }
            }
        }
        Ok(m)
    }

    pub fn embedded() -> Self {
        // Checked by a unit test; a broken manifest can't ship.
        Self::parse(MANIFEST_JSON, false).expect("embedded components.json is valid")
    }

    pub fn get(&self, id: &str) -> Option<&ComponentSpec> {
        self.components.iter().find(|c| c.id == id)
    }
}

/// The tools the rest of the app asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    YtDlp,
    Ffmpeg,
    Qjs,
}

impl Tool {
    pub fn id(self) -> &'static str {
        match self {
            Tool::YtDlp => "yt-dlp",
            Tool::Ffmpeg => "ffmpeg",
            Tool::Qjs => "qjs",
        }
    }
}

// ---------------------------------------------------------------------------
// Where tools are looked for
// ---------------------------------------------------------------------------

/// Search locations, in priority order. Plain data, so tests build their own.
#[derive(Clone, Debug)]
pub struct Locations {
    /// Per-user, writable: `<components>/<id>/<exe>`.
    pub components: PathBuf,
    /// Shipped with the app: `<dir>/<id>/<exe>` or `<dir>/<exe>`.
    pub bundled: Vec<PathBuf>,
    /// The directories of `PATH`.
    pub path: Vec<PathBuf>,
}

impl Locations {
    /// The real locations: `components` under the per-user local data dir,
    /// the exe's own folder (and its `resources`/`components` subfolders),
    /// then `PATH`.
    pub fn system(local_data_dir: &Path) -> Self {
        let mut bundled = Vec::new();
        if let Some(dir) = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf))
        {
            bundled.push(dir.join("components"));
            bundled.push(dir.join("resources").join("components"));
            bundled.push(dir);
        }
        Self {
            components: local_data_dir.join("components"),
            bundled,
            path: std::env::var_os("PATH")
                .map(|p| std::env::split_paths(&p).collect())
                .unwrap_or_default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Components,
    Bundled,
    Path,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub path: PathBuf,
    pub source: Source,
}

/// The file name to look for on this OS (`yt-dlp.exe` → `yt-dlp` on Unix).
fn os_exe_name(exe: &str) -> String {
    #[cfg(windows)]
    {
        exe.to_string()
    }
    #[cfg(unix)]
    {
        exe.strip_suffix(".exe").unwrap_or(exe).to_string()
    }
}

/// Find `spec` in `loc`: components dir → bundled → PATH.
pub fn resolve(spec: &ComponentSpec, loc: &Locations) -> Option<Resolved> {
    let own = loc.components.join(&spec.id).join(&spec.exe);
    if own.is_file() {
        return Some(Resolved {
            path: own,
            source: Source::Components,
        });
    }
    for dir in &loc.bundled {
        for candidate in [dir.join(&spec.id).join(&spec.exe), dir.join(&spec.exe)] {
            if candidate.is_file() {
                return Some(Resolved {
                    path: candidate,
                    source: Source::Bundled,
                });
            }
        }
    }
    let name = os_exe_name(&spec.exe);
    loc.path
        .iter()
        .map(|d| d.join(&name))
        .find(|p| p.is_file())
        .map(|path| Resolved {
            path,
            source: Source::Path,
        })
}

/// A JavaScript runtime yt-dlp can run (it needs one for YouTube).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JsRuntime {
    /// Our own QuickJS-NG (components dir, bundled, or PATH).
    QuickJs(PathBuf),
    /// `deno` on PATH.
    Deno,
    /// `node` on PATH.
    Node,
}

/// Pick the runtime: our QuickJS first, then `deno`, then `node` from `path`.
pub fn find_js_runtime(qjs: Option<PathBuf>, path: &[PathBuf]) -> Option<JsRuntime> {
    let on_path = |exe: &str| {
        let name = os_exe_name(exe);
        path.iter().any(|d| d.join(&name).is_file())
    };
    if let Some(qjs) = qjs {
        Some(JsRuntime::QuickJs(qjs))
    } else if on_path("deno.exe") {
        Some(JsRuntime::Deno)
    } else if on_path("node.exe") {
        Some(JsRuntime::Node)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Errors, events, persisted state
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ComponentError {
    /// Couldn't reach the server / connection dropped (retry resumes).
    Network(String),
    Http(u16),
    /// The download doesn't match the pinned hash; it was deleted.
    HashMismatch {
        expected: String,
        actual: String,
    },
    Io(String),
    Archive(String),
    /// The checksum list has no line for the asset.
    NoChecksum(String),
    Unknown(String),
}

impl std::fmt::Display for ComponentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network(e) => write!(f, "download failed ({e}). Check your connection and retry; it will continue where it stopped."),
            Self::Http(s) => write!(f, "the download server answered HTTP {s}"),
            Self::HashMismatch { expected, actual } => write!(
                f,
                "the downloaded file is not the expected one (SHA-256 {actual}, expected {expected}); it was deleted"
            ),
            Self::Io(e) => write!(f, "could not write the file: {e}"),
            Self::Archive(e) => write!(f, "could not unpack the archive: {e}"),
            Self::NoChecksum(n) => write!(f, "the release has no checksum for {n}"),
            Self::Unknown(id) => write!(f, "unknown component {id}"),
        }
    }
}

fn io_err(e: impl std::fmt::Display) -> ComponentError {
    ComponentError::Io(e.to_string())
}

/// `component-progress` event payload.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentProgress {
    pub id: String,
    pub name: String,
    /// `downloading` | `verifying` | `installing` | `done` | `error`
    pub phase: String,
    pub downloaded: u64,
    pub total: u64,
    pub error: Option<String>,
}

/// How the manager reports to the UI (Tauri events in `main.rs`).
pub trait ComponentEvents: Send + Sync {
    fn progress(&self, p: &ComponentProgress);
}

/// No-op events for tests.
#[cfg(test)]
pub struct NoEvents;
#[cfg(test)]
impl ComponentEvents for NoEvents {
    fn progress(&self, _p: &ComponentProgress) {}
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct State {
    /// "Keep yt-dlp up to date" (default on; `None` = never set).
    auto_update: Option<bool>,
    /// Epoch seconds of the last update check.
    last_update_check: u64,
    /// What we installed: id → version label.
    installed: HashMap<String, String>,
}

/// One row of `get_components_status`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentStatus {
    pub id: String,
    pub name: String,
    /// The pinned version the app would install.
    pub pinned_version: String,
    /// What is installed in the components dir (if we installed it).
    pub installed_version: Option<String>,
    /// `components` | `bundled` | `path`, or `None` when missing everywhere.
    pub source: Option<Source>,
    pub size: u64,
    pub license: String,
    pub purpose: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "result")]
pub enum UpdateOutcome {
    /// Nothing newer (or not due yet); `version` is what's in use.
    UpToDate {
        version: String,
    },
    Updated {
        from: String,
        to: String,
    },
    /// The new binary failed its smoke test; the previous one was restored.
    RolledBack {
        reason: String,
    },
    /// Not checked (e.g. checked recently, or updates are off).
    Skipped {
        reason: String,
    },
}

// ---------------------------------------------------------------------------
// The manager
// ---------------------------------------------------------------------------

pub struct Components {
    manifest: Manifest,
    loc: Locations,
    client: reqwest::Client,
    events: Arc<dyn ComponentEvents>,
    state: Mutex<State>,
    /// One install/update at a time.
    busy: tokio::sync::Mutex<()>,
}

impl Components {
    pub fn new(
        manifest: Manifest,
        loc: Locations,
        client: reqwest::Client,
        events: Arc<dyn ComponentEvents>,
    ) -> Self {
        let state = std::fs::read(loc.components.join("state.json"))
            .ok()
            .and_then(|d| serde_json::from_slice::<State>(&d).ok())
            .unwrap_or_default();
        Self {
            manifest,
            loc,
            client,
            events,
            state: Mutex::new(state),
            busy: tokio::sync::Mutex::new(()),
        }
    }

    fn save_state(&self) {
        let state = lock(&self.state).clone();
        if let Err(e) = write_json_atomic(&self.loc.components.join("state.json"), &state) {
            warn!(error = %e, "could not save component state");
        }
    }

    fn tmp_dir(&self) -> PathBuf {
        self.loc.components.join(".tmp")
    }

    pub fn resolve(&self, tool: Tool) -> Option<Resolved> {
        self.manifest
            .get(tool.id())
            .and_then(|spec| resolve(spec, &self.loc))
    }

    /// The JavaScript runtime yt-dlp will use, if there is one.
    pub fn js_runtime(&self) -> Option<JsRuntime> {
        find_js_runtime(self.resolve(Tool::Qjs).map(|r| r.path), &self.loc.path)
    }

    pub fn status(&self) -> Vec<ComponentStatus> {
        let state = lock(&self.state);
        self.manifest
            .components
            .iter()
            .map(|c| {
                let found = resolve(c, &self.loc);
                let ours = found
                    .as_ref()
                    .is_some_and(|r| r.source == Source::Components);
                ComponentStatus {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    pinned_version: c.version.clone(),
                    installed_version: ours.then(|| state.installed.get(&c.id).cloned()).flatten(),
                    source: found.map(|r| r.source),
                    size: c.size,
                    license: c.license.clone(),
                    purpose: c.purpose.clone(),
                }
            })
            .collect()
    }

    /// Components that are not found anywhere.
    pub fn missing(&self) -> Vec<ComponentSpec> {
        self.manifest
            .components
            .iter()
            .filter(|c| resolve(c, &self.loc).is_none())
            .cloned()
            .collect()
    }

    pub fn auto_update(&self) -> bool {
        lock(&self.state).auto_update.unwrap_or(true)
    }

    pub fn set_auto_update(&self, on: bool) {
        lock(&self.state).auto_update = Some(on);
        self.save_state();
    }

    fn emit(
        &self,
        spec: &ComponentSpec,
        phase: &str,
        downloaded: u64,
        total: u64,
        error: Option<String>,
    ) {
        self.events.progress(&ComponentProgress {
            id: spec.id.clone(),
            name: spec.name.clone(),
            phase: phase.into(),
            downloaded,
            total,
            error,
        });
    }

    /// Install every missing component (the first-run setup). Stops at the
    /// first failure; calling again resumes the interrupted download.
    pub async fn install_missing(&self) -> Result<(), ComponentError> {
        let _guard = self.busy.lock().await;
        for spec in self.missing() {
            if let Err(e) = self.install(&spec).await {
                warn!(component = %spec.id, "component install failed: {e}");
                self.emit(&spec, "error", 0, spec.size, Some(e.to_string()));
                return Err(e);
            }
        }
        Ok(())
    }

    /// Download → verify → move into place.
    async fn install(&self, spec: &ComponentSpec) -> Result<(), ComponentError> {
        let part = self
            .tmp_dir()
            .join(format!("{}-{}.part", spec.id, &spec.sha256[..12]));
        self.fetch(spec, &spec.url, &part, Some(spec.size)).await?;
        self.emit(spec, "verifying", spec.size, spec.size, None);
        verify(&part, &spec.sha256).await?;
        self.emit(spec, "installing", spec.size, spec.size, None);
        self.place(spec, &part).await?;
        lock(&self.state)
            .installed
            .insert(spec.id.clone(), spec.version.clone());
        self.save_state();
        info!(component = %spec.id, version = %spec.version, "component installed");
        self.emit(spec, "done", spec.size, spec.size, None);
        Ok(())
    }

    /// Download `url` into `part`, continuing an earlier partial file.
    async fn fetch(
        &self,
        spec: &ComponentSpec,
        url: &str,
        part: &Path,
        expect: Option<u64>,
    ) -> Result<(), ComponentError> {
        if let Some(dir) = part.parent() {
            tokio::fs::create_dir_all(dir).await.map_err(io_err)?;
        }
        let have = tokio::fs::metadata(part)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        let total = expect.unwrap_or(0);
        if expect.is_some_and(|e| e > 0 && e == have) {
            return Ok(()); // complete; the hash check decides if it's good
        }
        let mut req = self.client.get(url);
        if have > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| ComponentError::Network(e.to_string()))?;
        let status = resp.status();
        let (mut file, mut done) = match status.as_u16() {
            206 => (
                tokio::fs::OpenOptions::new()
                    .append(true)
                    .open(part)
                    .await
                    .map_err(io_err)?,
                have,
            ),
            // The server ignored the range (or there was none): start over.
            200 => (tokio::fs::File::create(part).await.map_err(io_err)?, 0),
            // Nothing left after our offset: the file is already complete.
            416 if have > 0 => return Ok(()),
            s => return Err(ComponentError::Http(s)),
        };
        let total = if total > 0 {
            total
        } else {
            done + resp.content_length().unwrap_or(0)
        };
        self.emit(spec, "downloading", done, total, None);
        let mut last = std::time::Instant::now();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| ComponentError::Network(e.to_string()))?;
            file.write_all(&chunk).await.map_err(io_err)?;
            done += chunk.len() as u64;
            if last.elapsed() >= Duration::from_millis(250) {
                last = std::time::Instant::now();
                self.emit(spec, "downloading", done, total.max(done), None);
            }
        }
        file.flush().await.map_err(io_err)?;
        file.sync_all().await.map_err(io_err)?;
        Ok(())
    }

    /// Move a verified download into `<components>/<id>/`.
    async fn place(&self, spec: &ComponentSpec, part: &Path) -> Result<(), ComponentError> {
        let dir = self.loc.components.join(&spec.id);
        match &spec.archive {
            None => {
                tokio::fs::create_dir_all(&dir).await.map_err(io_err)?;
                swap_file(part, &dir.join(&spec.exe)).map_err(io_err)
            }
            Some(archive) => {
                let staging = self.tmp_dir().join(format!("{}-new", spec.id));
                let (zip, out, a, exe) = (
                    part.to_path_buf(),
                    staging.clone(),
                    archive.clone(),
                    spec.exe.clone(),
                );
                tokio::task::spawn_blocking(move || extract(&zip, &out, &a, &exe))
                    .await
                    .map_err(|e| ComponentError::Archive(e.to_string()))??;
                // Swap the whole folder, keeping nothing half-written visible.
                let old = self.loc.components.join(format!("{}.old", spec.id));
                let _ = std::fs::remove_dir_all(&old);
                if dir.exists() {
                    std::fs::rename(&dir, &old).map_err(io_err)?;
                }
                std::fs::rename(&staging, &dir).map_err(io_err)?;
                let _ = std::fs::remove_dir_all(&old);
                let _ = std::fs::remove_file(part);
                Ok(())
            }
        }
    }

    /// `<tool> --version`, trimmed (also the post-update smoke test).
    pub async fn version_of(&self, exe: &Path) -> Result<String, String> {
        let run = process::command(exe)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output();
        match tokio::time::timeout(SMOKE_TIMEOUT, run).await {
            Err(_) => Err("it did not answer --version in time".into()),
            Ok(Err(e)) => Err(format!("it could not be started: {e}")),
            Ok(Ok(out)) if !out.status.success() => Err("--version failed".into()),
            Ok(Ok(out)) => {
                let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if v.is_empty() {
                    Err("--version printed nothing".into())
                } else {
                    Ok(v)
                }
            }
        }
    }

    /// The yt-dlp version in use (runs it), if any.
    pub async fn ytdlp_version(&self) -> Option<String> {
        let r = self.resolve(Tool::YtDlp)?;
        self.version_of(&r.path).await.ok()
    }

    /// Check for a newer yt-dlp when one is due: daily when `forced` is
    /// false (and updates are on), at most every ten minutes when forced.
    pub async fn update_ytdlp_if_due(&self, forced: bool, now: u64) -> UpdateOutcome {
        if !forced && !self.auto_update() {
            return UpdateOutcome::Skipped {
                reason: "automatic updates are off".into(),
            };
        }
        let since = now.saturating_sub(lock(&self.state).last_update_check);
        let min = if forced {
            FORCED_UPDATE_MIN_SECS
        } else {
            UPDATE_INTERVAL_SECS
        };
        if since < min {
            return UpdateOutcome::Skipped {
                reason: "checked recently".into(),
            };
        }
        self.update_ytdlp(now).await
    }

    /// Check now. The new binary replaces the one in the components dir only
    /// after its hash matches the release's checksum list; if it then fails
    /// `--version`, the previous binary is put back.
    pub async fn update_ytdlp(&self, now: u64) -> UpdateOutcome {
        let _guard = self.busy.lock().await;
        let outcome = self.update_ytdlp_inner().await;
        lock(&self.state).last_update_check = now;
        self.save_state();
        match outcome {
            Ok(o) => o,
            Err(e) => {
                warn!("yt-dlp update check failed: {e}");
                UpdateOutcome::Skipped {
                    reason: e.to_string(),
                }
            }
        }
    }

    async fn update_ytdlp_inner(&self) -> Result<UpdateOutcome, ComponentError> {
        let spec = self
            .manifest
            .get(Tool::YtDlp.id())
            .ok_or_else(|| ComponentError::Unknown("yt-dlp".into()))?
            .clone();
        let Some(update) = spec.update.clone() else {
            return Ok(UpdateOutcome::Skipped {
                reason: "this build has no update source".into(),
            });
        };
        let Some(current) = resolve(&spec, &self.loc) else {
            return Ok(UpdateOutcome::Skipped {
                reason: "yt-dlp is not installed yet".into(),
            });
        };
        let current_version = self
            .version_of(&current.path)
            .await
            .unwrap_or_else(|_| "unknown".into());

        let sums = self
            .client
            .get(&update.sums_url)
            .send()
            .await
            .map_err(|e| ComponentError::Network(e.to_string()))?;
        if !sums.status().is_success() {
            return Err(ComponentError::Http(sums.status().as_u16()));
        }
        let sums = sums
            .text()
            .await
            .map_err(|e| ComponentError::Network(e.to_string()))?;
        let wanted = parse_sums(&sums, &update.asset_name)
            .ok_or_else(|| ComponentError::NoChecksum(update.asset_name.clone()))?;
        let have = sha256_file(current.path.clone()).await.map_err(io_err)?;
        if have == wanted {
            return Ok(UpdateOutcome::UpToDate {
                version: current_version,
            });
        }

        let part = self
            .tmp_dir()
            .join(format!("yt-dlp-{}.part", &wanted[..12]));
        self.fetch(&spec, &update.asset_url, &part, None).await?;
        self.emit(&spec, "verifying", 0, 0, None);
        verify(&part, &wanted).await?;

        // Always into OUR dir (a bundled / PATH copy is never modified).
        let dir = self.loc.components.join(&spec.id);
        tokio::fs::create_dir_all(&dir).await.map_err(io_err)?;
        let target = dir.join(&spec.exe);
        let had_previous = target.is_file();
        swap_file(&part, &target).map_err(io_err)?;
        match self.version_of(&target).await {
            Ok(new_version) => {
                lock(&self.state)
                    .installed
                    .insert(spec.id.clone(), new_version.clone());
                info!(from = %current_version, to = %new_version, "yt-dlp updated");
                self.emit(&spec, "done", 0, 0, None);
                Ok(UpdateOutcome::Updated {
                    from: current_version,
                    to: new_version,
                })
            }
            Err(reason) => {
                // Roll back: the previous binary (kept as `.old`) returns.
                let _ = std::fs::remove_file(&target);
                if had_previous {
                    std::fs::rename(old_path(&target), &target).map_err(io_err)?;
                }
                warn!("new yt-dlp failed its smoke test ({reason}); rolled back");
                self.emit(&spec, "done", 0, 0, None);
                Ok(UpdateOutcome::RolledBack { reason })
            }
        }
    }

    /// Extra yt-dlp arguments so it uses OUR runtime and ffmpeg rather than
    /// whatever is (or isn't) on PATH.
    pub fn yt_dlp_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        match self.js_runtime() {
            Some(JsRuntime::QuickJs(qjs)) => {
                args.push("--js-runtimes".into());
                args.push(format!("quickjs:{}", qjs.display()));
            }
            // Not enabled by default in yt-dlp, so it has to be named.
            Some(JsRuntime::Node) => {
                args.push("--js-runtimes".into());
                args.push("node".into());
            }
            // yt-dlp looks for deno on PATH by itself.
            Some(JsRuntime::Deno) | None => {}
        }
        if let Some(dir) = self
            .resolve(Tool::Ffmpeg)
            .and_then(|f| f.path.parent().map(Path::to_path_buf))
        {
            args.push("--ffmpeg-location".into());
            args.push(dir.display().to_string());
        }
        args
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn old_path(target: &Path) -> PathBuf {
    let mut s = target.as_os_str().to_owned();
    s.push(".old");
    PathBuf::from(s)
}

/// Put `new` at `target`, keeping the previous `target` as `<target>.old`.
fn swap_file(new: &Path, target: &Path) -> std::io::Result<()> {
    if target.exists() {
        let old = old_path(target);
        let _ = std::fs::remove_file(&old);
        std::fs::rename(target, &old)?;
    }
    std::fs::rename(new, target)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

async fn sha256_file(path: PathBuf) -> std::io::Result<String> {
    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let mut f = std::fs::File::open(&path)?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect())
    })
    .await
    .unwrap_or_else(|e| Err(std::io::Error::other(e)))
}

/// Check `part` against `expected`; a wrong file is deleted on the spot.
async fn verify(part: &Path, expected: &str) -> Result<(), ComponentError> {
    let actual = sha256_file(part.to_path_buf()).await.map_err(io_err)?;
    if actual.eq_ignore_ascii_case(expected) {
        return Ok(());
    }
    let _ = tokio::fs::remove_file(part).await;
    Err(ComponentError::HashMismatch {
        expected: expected.to_ascii_lowercase(),
        actual,
    })
}

/// The hash for `name` in a GNU-style `SHA2-256SUMS` list.
pub fn parse_sums(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.split_once(char::is_whitespace)?;
        let file = file.trim().trim_start_matches('*');
        (file == name && is_sha256(hash)).then(|| hash.to_ascii_lowercase())
    })
}

/// Extract the wanted files of `zip` into the fresh folder `out`. Files are
/// written under their own file name only, so no archive path can escape.
fn extract(zip: &Path, out: &Path, spec: &ArchiveSpec, exe: &str) -> Result<(), ComponentError> {
    let arch = |e: &dyn std::fmt::Display| ComponentError::Archive(e.to_string());
    let _ = std::fs::remove_dir_all(out);
    std::fs::create_dir_all(out).map_err(io_err)?;
    let file = std::fs::File::open(zip).map_err(io_err)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| arch(&e))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| arch(&e))?;
        if entry.is_dir() {
            continue;
        }
        let Some(path) = entry.enclosed_name() else {
            continue;
        };
        // <top-level folder>/<dir>/<file>
        let parts: Vec<String> = path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let [_, dir, name] = parts.as_slice() else {
            continue;
        };
        let wanted = dir == &spec.dir
            && is_plain_name(name)
            && spec
                .include
                .iter()
                .any(|inc| name == inc || (inc.starts_with('.') && name.ends_with(inc.as_str())));
        if !wanted {
            continue;
        }
        let mut dest = std::fs::File::create(out.join(name)).map_err(io_err)?;
        std::io::copy(&mut entry, &mut dest).map_err(io_err)?;
    }
    if !out.join(exe).is_file() {
        return Err(ComponentError::Archive(format!(
            "{exe} is not in the archive"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Process-wide access for youtube.rs / muxer.rs / stream_dl callers
// ---------------------------------------------------------------------------

static GLOBAL: OnceLock<Arc<Components>> = OnceLock::new();

/// Install the app's manager (once, at startup).
pub fn set_global(c: Arc<Components>) {
    let _ = GLOBAL.set(c);
}

pub fn global() -> Option<Arc<Components>> {
    GLOBAL.get().cloned()
}

/// The executable to run for `tool`: the resolved path, or — when there is no
/// manager (unit tests) or the tool is missing — its bare name, which the OS
/// looks up on PATH (and fails cleanly if absent).
pub fn tool_program(tool: Tool) -> OsString {
    global()
        .and_then(|c| c.resolve(tool))
        .map(|r| r.path.into_os_string())
        .unwrap_or_else(|| OsString::from(tool.id()))
}

/// See [`Components::yt_dlp_args`]; empty without a manager.
pub fn yt_dlp_args() -> Vec<String> {
    global().map(|c| c.yt_dlp_args()).unwrap_or_default()
}

/// Is `tool` available from any location?
pub fn has_tool(tool: Tool) -> bool {
    global().is_some_and(|c| c.resolve(tool).is_some())
}

/// Is there a JavaScript runtime for yt-dlp (QuickJS, or deno/node on PATH)?
pub fn has_js_runtime() -> bool {
    global().is_some_and(|c| c.js_runtime().is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;
    use axum::body::Body;
    use axum::extract::{Path as UrlPath, State};
    use axum::http::{header, HeaderMap, StatusCode};
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;
    use axum::Router;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A tiny range-capable file server that records `Range` headers and can
    /// cut one response in half (a dropped connection).
    #[derive(Clone, Default)]
    struct Srv {
        files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
        ranges: Arc<Mutex<Vec<String>>>,
        cut_next: Arc<AtomicBool>,
    }

    async fn serve(
        State(s): State<Srv>,
        UrlPath(name): UrlPath<String>,
        headers: HeaderMap,
    ) -> Response {
        let Some(data) = lock(&s.files).get(&name).cloned() else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let range = headers
            .get(header::RANGE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        if let Some(r) = &range {
            lock(&s.ranges).push(r.clone());
        }
        let start = range
            .as_deref()
            .and_then(|r| r.strip_prefix("bytes="))
            .and_then(|r| r.strip_suffix('-'))
            .and_then(|n| n.parse::<usize>().ok())
            .unwrap_or(0);
        if start >= data.len() && start > 0 {
            return StatusCode::RANGE_NOT_SATISFIABLE.into_response();
        }
        let status = if start > 0 {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        };
        let rest = data[start..].to_vec();
        if s.cut_next.swap(false, Ordering::SeqCst) {
            let half = rest[..rest.len() / 2].to_vec();
            // Deliver half, give the client time to read it, then drop.
            let stream = futures_util::stream::unfold(0u8, move |step| {
                let half = half.clone();
                async move {
                    match step {
                        0 => Some((Ok::<_, std::io::Error>(bytes::Bytes::from(half)), 1)),
                        1 => {
                            tokio::time::sleep(Duration::from_millis(300)).await;
                            Some((Err(std::io::Error::other("connection cut")), 2))
                        }
                        _ => None,
                    }
                }
            });
            return (status, Body::from_stream(stream)).into_response();
        }
        (status, rest).into_response()
    }

    async fn start(srv: Srv) -> String {
        let app = Router::new().route("/:name", get(serve)).with_state(srv);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    fn sha(data: &[u8]) -> String {
        Sha256::digest(data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// A fake tool: a script whose `--version` prints `version`, or fails.
    fn script(version: Option<&str>) -> Vec<u8> {
        #[cfg(windows)]
        let s = match version {
            Some(v) => format!("@echo off\r\necho {v}\r\n"),
            None => "@echo off\r\nexit /b 1\r\n".to_string(),
        };
        #[cfg(unix)]
        let s = match version {
            Some(v) => format!("#!/bin/sh\necho {v}\n"),
            None => "#!/bin/sh\nexit 1\n".to_string(),
        };
        s.into_bytes()
    }

    #[cfg(windows)]
    const TOOL_EXE: &str = "yt-dlp.cmd";
    #[cfg(unix)]
    const TOOL_EXE: &str = "yt-dlp";

    fn manifest_for(base: &str, file: &str, hash: &str, size: usize) -> Manifest {
        let json = serde_json::json!({
            "version": 1,
            "components": [{
                "id": "yt-dlp", "name": "yt-dlp", "version": "1.0", "exe": TOOL_EXE,
                "url": format!("{base}/{file}"), "sha256": hash, "size": size,
                "license": "Unlicense", "homepage": "https://example.com", "purpose": "test",
                "update": { "sumsUrl": format!("{base}/SUMS"), "assetUrl": format!("{base}/latest"), "assetName": "yt-dlp.exe" }
            }]
        });
        Manifest::parse(&json.to_string(), true).unwrap()
    }

    fn locations(root: &Path) -> Locations {
        Locations {
            components: root.join("components"),
            bundled: vec![root.join("bundled")],
            path: vec![root.join("path")],
        }
    }

    fn manager(manifest: Manifest, root: &Path) -> Components {
        Components::new(
            manifest,
            locations(root),
            reqwest::Client::new(),
            Arc::new(NoEvents),
        )
    }

    #[test]
    fn embedded_manifest_parses_and_pins_everything() {
        let m = Manifest::embedded();
        for id in ["yt-dlp", "qjs", "ffmpeg"] {
            let c = m.get(id).unwrap_or_else(|| panic!("{id} missing"));
            assert!(c.url.starts_with("https://github.com/"), "{}", c.url);
            assert!(is_sha256(&c.sha256) && c.size > 0);
        }
        assert!(m.get("yt-dlp").unwrap().update.is_some());
        assert!(m.get("ffmpeg").unwrap().archive.is_some());

        let ok = |v: serde_json::Value| Manifest::parse(&v.to_string(), false);
        let base = serde_json::json!({
            "id": "a", "name": "A", "version": "1", "exe": "a.exe", "url": "https://x.example/a.exe",
            "sha256": "0".repeat(64), "size": 1, "license": "MIT", "homepage": "h", "purpose": "p"
        });
        assert!(ok(serde_json::json!({ "version": 1, "components": [base] })).is_ok());
        let with = |k: &str, v: serde_json::Value| {
            let mut c = base.clone();
            c[k] = v;
            ok(serde_json::json!({ "version": 1, "components": [c] }))
        };
        assert!(with("url", "http://x.example/a.exe".into()).is_err());
        assert!(with("sha256", "abc".into()).is_err());
        assert!(with("exe", "..\\evil.exe".into()).is_err());
        assert!(with("id", "../x".into()).is_err());
        assert!(with("surprise", 1.into()).is_err());
        assert!(ok(serde_json::json!({ "version": 2, "components": [] })).is_err());
        assert!(ok(serde_json::json!({ "version": 1, "components": [base, base] })).is_err());
        assert!(Manifest::parse("not json", false).is_err());
    }

    #[test]
    fn resolver_prefers_components_then_bundled_then_path() {
        let root = temp_dir("comp_resolve");
        let loc = locations(&root);
        let m = manifest_for("http://127.0.0.1:1", "x", &"0".repeat(64), 1);
        let spec = m.get("yt-dlp").unwrap();
        assert_eq!(resolve(spec, &loc), None);

        let on_path = root.join("path").join(os_exe_name(TOOL_EXE));
        let bundled = root.join("bundled").join("yt-dlp").join(TOOL_EXE);
        let own = root.join("components").join("yt-dlp").join(TOOL_EXE);
        for p in [&on_path, &bundled, &own] {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        assert_eq!(
            resolve(spec, &loc).unwrap(),
            Resolved {
                path: own.clone(),
                source: Source::Components
            }
        );
        std::fs::remove_file(&own).unwrap();
        assert_eq!(
            resolve(spec, &loc).unwrap(),
            Resolved {
                path: bundled.clone(),
                source: Source::Bundled
            }
        );
        std::fs::remove_file(&bundled).unwrap();
        // A flat bundled copy (next to the exe) also counts.
        let flat = root.join("bundled").join(TOOL_EXE);
        std::fs::write(&flat, b"x").unwrap();
        assert_eq!(resolve(spec, &loc).unwrap().source, Source::Bundled);
        std::fs::remove_file(&flat).unwrap();
        assert_eq!(
            resolve(spec, &loc).unwrap(),
            Resolved {
                path: on_path,
                source: Source::Path
            }
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn js_runtime_prefers_quickjs_then_deno_then_node() {
        let root = temp_dir("comp_js");
        let path = vec![root.join("a"), root.join("b")];
        for d in &path {
            std::fs::create_dir_all(d).unwrap();
        }
        assert_eq!(find_js_runtime(None, &path), None);
        assert_eq!(find_js_runtime(None, &[]), None);

        std::fs::write(path[1].join(os_exe_name("node.exe")), b"x").unwrap();
        assert_eq!(find_js_runtime(None, &path), Some(JsRuntime::Node));
        std::fs::write(path[0].join(os_exe_name("deno.exe")), b"x").unwrap();
        assert_eq!(find_js_runtime(None, &path), Some(JsRuntime::Deno));
        let qjs = root.join("qjs.exe");
        assert_eq!(
            find_js_runtime(Some(qjs.clone()), &path),
            Some(JsRuntime::QuickJs(qjs))
        );
        // A directory with the right name is not a runtime.
        let dirs = vec![root.join("c")];
        std::fs::create_dir_all(dirs[0].join(os_exe_name("deno.exe"))).unwrap();
        assert_eq!(find_js_runtime(None, &dirs), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn yt_dlp_args_name_the_js_runtime() {
        let root = temp_dir("comp_js_args");
        let c = manager(Manifest::embedded(), &root);
        assert_eq!(c.js_runtime(), None);
        assert!(c.yt_dlp_args().is_empty());

        // node has to be asked for; deno is yt-dlp's default.
        let path = root.join("path");
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join(os_exe_name("node.exe")), b"x").unwrap();
        assert_eq!(c.yt_dlp_args(), ["--js-runtimes", "node"]);
        std::fs::write(path.join(os_exe_name("deno.exe")), b"x").unwrap();
        assert!(c.yt_dlp_args().is_empty());

        // Our own QuickJS wins and is passed by full path.
        let qjs = root.join("components").join("qjs").join("qjs.exe");
        std::fs::create_dir_all(qjs.parent().unwrap()).unwrap();
        std::fs::write(&qjs, b"x").unwrap();
        assert_eq!(
            c.yt_dlp_args(),
            [
                "--js-runtimes".to_string(),
                format!("quickjs:{}", qjs.display())
            ]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn installs_a_verified_download_and_reports_status() {
        let root = temp_dir("comp_install");
        let tool = script(Some("1.0"));
        let srv = Srv::default();
        lock(&srv.files).insert("tool".into(), tool.clone());
        let base = start(srv).await;
        let c = manager(manifest_for(&base, "tool", &sha(&tool), tool.len()), &root);

        assert_eq!(c.missing().len(), 1);
        assert_eq!(c.status()[0].source, None);
        c.install_missing().await.unwrap();
        let r = c.resolve(Tool::YtDlp).unwrap();
        assert_eq!(r.source, Source::Components);
        assert_eq!(std::fs::read(&r.path).unwrap(), tool);
        assert_eq!(c.version_of(&r.path).await.unwrap(), "1.0");
        let st = &c.status()[0];
        assert_eq!(
            (st.source, st.installed_version.as_deref()),
            (Some(Source::Components), Some("1.0"))
        );
        assert!(c.missing().is_empty());
        // State survives a restart.
        let again = manager(manifest_for(&base, "tool", &sha(&tool), tool.len()), &root);
        assert_eq!(again.status()[0].installed_version.as_deref(), Some("1.0"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn hash_mismatch_is_rejected_and_nothing_is_installed() {
        let root = temp_dir("comp_hash");
        let srv = Srv::default();
        lock(&srv.files).insert("tool".into(), b"tampered bytes".to_vec());
        let base = start(srv).await;
        let c = manager(
            manifest_for(&base, "tool", &sha(b"the real file"), 14),
            &root,
        );

        let e = c.install_missing().await.unwrap_err();
        assert!(matches!(e, ComponentError::HashMismatch { .. }), "{e:?}");
        assert!(e.to_string().contains("not the expected one"));
        assert!(c.resolve(Tool::YtDlp).is_none());
        assert!(!root
            .join("components")
            .join("yt-dlp")
            .join(TOOL_EXE)
            .exists());
        // The bad download is gone, not left to be "resumed".
        let leftovers: Vec<_> = std::fs::read_dir(root.join("components").join(".tmp"))
            .unwrap()
            .flatten()
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn interrupted_download_resumes_with_a_range_request() {
        let root = temp_dir("comp_resume");
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let srv = Srv::default();
        lock(&srv.files).insert("tool".into(), data.clone());
        srv.cut_next.store(true, Ordering::SeqCst);
        let base = start(srv.clone()).await;
        let c = manager(manifest_for(&base, "tool", &sha(&data), data.len()), &root);

        let e = c.install_missing().await.unwrap_err();
        assert!(matches!(e, ComponentError::Network(_)), "{e:?}");
        assert!(c.resolve(Tool::YtDlp).is_none());
        let part = std::fs::read_dir(root.join("components").join(".tmp"))
            .unwrap()
            .flatten()
            .next()
            .expect("partial file kept for resume")
            .path();
        let have = std::fs::metadata(&part).unwrap().len();
        assert!(have > 0 && have < data.len() as u64, "partial = {have}");

        // Retry: continues from the partial file instead of starting over.
        c.install_missing().await.unwrap();
        assert_eq!(lock(&srv.ranges).as_slice(), [format!("bytes={have}-")]);
        assert_eq!(
            std::fs::read(c.resolve(Tool::YtDlp).unwrap().path).unwrap(),
            data
        );
        assert!(!part.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_corrupt_partial_file_is_cleaned_up_then_redownloaded() {
        let root = temp_dir("comp_corrupt");
        let data = script(Some("1.0"));
        let srv = Srv::default();
        lock(&srv.files).insert("tool".into(), data.clone());
        let base = start(srv).await;
        let m = manifest_for(&base, "tool", &sha(&data), data.len());
        let c = manager(m.clone(), &root);
        // A full-size but wrong leftover from an earlier run.
        let tmp = root.join("components").join(".tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        let part = tmp.join(format!(
            "yt-dlp-{}.part",
            &m.get("yt-dlp").unwrap().sha256[..12]
        ));
        std::fs::write(&part, vec![b'x'; data.len()]).unwrap();

        assert!(matches!(
            c.install_missing().await,
            Err(ComponentError::HashMismatch { .. })
        ));
        assert!(!part.exists());
        c.install_missing().await.unwrap();
        assert_eq!(
            std::fs::read(c.resolve(Tool::YtDlp).unwrap().path).unwrap(),
            data
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn zip_component_extracts_only_the_listed_files() {
        use std::io::Write;
        let root = temp_dir("comp_zip");
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            for (name, data) in [
                ("ffmpeg-1/bin/ffmpeg.exe", &b"FFMPEG"[..]),
                ("ffmpeg-1/bin/avcodec-1.dll", b"DLL"),
                ("ffmpeg-1/bin/ffplay.exe", b"PLAY"),
                ("ffmpeg-1/doc/manual.html", b"DOC"),
                ("ffmpeg-1/bin/nested/evil.dll", b"NESTED"),
                ("../escape.dll", b"ESCAPE"),
            ] {
                zip.start_file(name, opts).unwrap();
                zip.write_all(data).unwrap();
            }
            zip.finish().unwrap();
        }
        let zip_bytes = buf.into_inner();
        let srv = Srv::default();
        lock(&srv.files).insert("ffmpeg.zip".into(), zip_bytes.clone());
        let base = start(srv).await;
        let json = serde_json::json!({ "version": 1, "components": [{
            "id": "ffmpeg", "name": "FFmpeg", "version": "1", "exe": "ffmpeg.exe",
            "url": format!("{base}/ffmpeg.zip"), "sha256": sha(&zip_bytes), "size": zip_bytes.len(),
            "license": "LGPL", "homepage": "h", "purpose": "p",
            "archive": { "dir": "bin", "include": ["ffmpeg.exe", ".dll"] }
        }]});
        let c = manager(Manifest::parse(&json.to_string(), true).unwrap(), &root);
        c.install_missing().await.unwrap();

        let dir = root.join("components").join("ffmpeg");
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["avcodec-1.dll", "ffmpeg.exe"]);
        assert_eq!(std::fs::read(dir.join("ffmpeg.exe")).unwrap(), b"FFMPEG");
        assert!(
            !root.join("escape.dll").exists()
                && !root.join("components").join("escape.dll").exists()
        );
        // The verified archive and staging folder are gone.
        assert_eq!(
            std::fs::read_dir(root.join("components").join(".tmp"))
                .unwrap()
                .count(),
            0
        );
        // ffmpeg's folder is what yt-dlp is pointed at.
        let args = c.yt_dlp_args();
        assert_eq!(
            args,
            ["--ffmpeg-location".to_string(), dir.display().to_string()]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn checksum_list_parsing() {
        let sums = format!(
            "{}  yt-dlp\n{} *yt-dlp.exe\nnot-a-hash  yt-dlp_x86.exe\n",
            "a".repeat(64),
            "B".repeat(64)
        );
        assert_eq!(parse_sums(&sums, "yt-dlp.exe"), Some("b".repeat(64)));
        assert_eq!(parse_sums(&sums, "yt-dlp"), Some("a".repeat(64)));
        assert_eq!(parse_sums(&sums, "yt-dlp_x86.exe"), None);
        assert_eq!(parse_sums(&sums, "missing.exe"), None);
    }

    /// Install v1, then offer `new` as the latest release.
    async fn updated_with(root: &Path, new: Vec<u8>, sums_for: &[u8]) -> (Components, Srv) {
        let v1 = script(Some("1.0"));
        let srv = Srv::default();
        lock(&srv.files).insert("tool".into(), v1.clone());
        lock(&srv.files).insert("latest".into(), new);
        lock(&srv.files).insert(
            "SUMS".into(),
            format!("{}  yt-dlp.exe\n", sha(sums_for)).into_bytes(),
        );
        let base = start(srv.clone()).await;
        let c = manager(manifest_for(&base, "tool", &sha(&v1), v1.len()), root);
        c.install_missing().await.unwrap();
        (c, srv)
    }

    #[tokio::test]
    async fn update_installs_a_newer_verified_binary_and_keeps_the_old_one() {
        let root = temp_dir("comp_update");
        let v2 = script(Some("2.0"));
        let (c, _srv) = updated_with(&root, v2.clone(), &v2).await;
        let out = c.update_ytdlp(1000).await;
        assert_eq!(
            out,
            UpdateOutcome::Updated {
                from: "1.0".into(),
                to: "2.0".into()
            }
        );
        let target = c.resolve(Tool::YtDlp).unwrap().path;
        assert_eq!(std::fs::read(&target).unwrap(), v2);
        assert_eq!(
            std::fs::read(old_path(&target)).unwrap(),
            script(Some("1.0"))
        );
        assert_eq!(c.status()[0].installed_version.as_deref(), Some("2.0"));
        // Checking again: already the latest.
        assert_eq!(
            c.update_ytdlp(2000).await,
            UpdateOutcome::UpToDate {
                version: "2.0".into()
            }
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_bad_update_is_rolled_back() {
        let root = temp_dir("comp_rollback");
        let broken = script(None); // `--version` fails
        let (c, _srv) = updated_with(&root, broken.clone(), &broken).await;
        let out = c.update_ytdlp(1000).await;
        assert!(matches!(out, UpdateOutcome::RolledBack { .. }), "{out:?}");
        let target = c.resolve(Tool::YtDlp).unwrap().path;
        assert_eq!(std::fs::read(&target).unwrap(), script(Some("1.0")));
        assert_eq!(c.version_of(&target).await.unwrap(), "1.0");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn an_update_that_does_not_match_the_checksum_list_is_refused() {
        let root = temp_dir("comp_update_hash");
        let (c, _srv) = updated_with(&root, script(Some("6.6.6")), &script(Some("2.0"))).await;
        let out = c.update_ytdlp(1000).await;
        assert!(
            matches!(&out, UpdateOutcome::Skipped { reason } if reason.contains("not the expected one")),
            "{out:?}"
        );
        let target = c.resolve(Tool::YtDlp).unwrap().path;
        assert_eq!(c.version_of(&target).await.unwrap(), "1.0");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_checks_are_daily_and_respect_the_toggle() {
        let root = temp_dir("comp_due");
        let v1 = script(Some("1.0"));
        let (c, _srv) = updated_with(&root, v1.clone(), &v1).await;
        let day = UPDATE_INTERVAL_SECS;
        assert!(c.auto_update());
        assert_eq!(
            c.update_ytdlp_if_due(false, 10 * day).await,
            UpdateOutcome::UpToDate {
                version: "1.0".into()
            }
        );
        // Same day: skipped. Next day: checked again.
        assert!(matches!(
            c.update_ytdlp_if_due(false, 10 * day + 60).await,
            UpdateOutcome::Skipped { .. }
        ));
        assert!(matches!(
            c.update_ytdlp_if_due(false, 11 * day + 1).await,
            UpdateOutcome::UpToDate { .. }
        ));
        // A forced check (extraction failed) is allowed sooner, but not at once.
        assert!(matches!(
            c.update_ytdlp_if_due(true, 11 * day + 30).await,
            UpdateOutcome::Skipped { .. }
        ));
        assert!(matches!(
            c.update_ytdlp_if_due(true, 11 * day + FORCED_UPDATE_MIN_SECS + 5)
                .await,
            UpdateOutcome::UpToDate { .. }
        ));
        // Toggle off: no automatic checks; the setting persists.
        c.set_auto_update(false);
        assert!(matches!(
            c.update_ytdlp_if_due(false, 99 * day).await,
            UpdateOutcome::Skipped { .. }
        ));
        let reopened = manager(
            manifest_for("http://127.0.0.1:1", "x", &"0".repeat(64), 1),
            &root,
        );
        assert!(!reopened.auto_update());
        let _ = std::fs::remove_dir_all(root);
    }
}
