//! Persistent download history: what survives closing the app.
//!
//! * [`StateStore`] abstracts where the state lives. [`JsonFileStore`] writes
//!   one JSON file atomically (tmp + fsync + rename); [`MemoryStore`] backs
//!   tests. A corrupt file is moved aside and the app starts empty — it never
//!   crashes on bad JSON.
//! * [`History`] is the in-memory registry. It is updated from progress
//!   events (every ~400 ms per job — never the per-frame path), queue events
//!   and commands, and saved by a debounced background task ([`History::run_saver`]).
//! * **Secrets:** custom header *values* (auth tokens, cookies) are never
//!   written to disk; only header *names* are, so the UI can tell the user why
//!   such a download needs its headers re-entered. Values are kept in memory
//!   for the current session only.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tracing::warn;

use crate::downloader::ProgressPayload;
use crate::queue::{Priority, QueueEntry};

/// On-disk schema version. A file with any other version is set aside.
pub const STATE_VERSION: u32 = 1;
/// How long state may stay dirty before it is written.
pub const SAVE_DEBOUNCE: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobKind {
    File,
    Youtube,
    /// An HLS/DASH manifest downloaded by yt-dlp.
    Manifest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Queued,
    Scheduled,
    Downloading,
    Paused,
    Muxing,
    /// Was running or waiting when the app closed; resumable via retry.
    Interrupted,
    Complete,
    Error,
    Canceled,
}

impl JobStatus {
    /// Actively owned by a running task.
    pub fn is_live(self) -> bool {
        matches!(self, Self::Downloading | Self::Paused | Self::Muxing)
    }

    /// Can be (re)started with `retry_download`.
    pub fn is_retryable(self) -> bool {
        matches!(self, Self::Interrupted | Self::Error | Self::Canceled)
    }

    fn from_progress(s: &str) -> Option<Self> {
        Some(match s {
            "downloading" => Self::Downloading,
            "paused" => Self::Paused,
            "muxing" => Self::Muxing,
            "complete" => Self::Complete,
            "error" => Self::Error,
            "canceled" => Self::Canceled,
            "queued" => Self::Queued,
            "scheduled" => Self::Scheduled,
            _ => return None,
        })
    }
}

/// One download as persisted (and as sent to the UI by `list_history`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRecord {
    pub id: String,
    pub kind: JobKind,
    pub url: String,
    #[serde(default)]
    pub mirrors: Vec<String>,
    /// Names of the custom headers used. Values are deliberately NOT stored.
    #[serde(default)]
    pub header_names: Vec<String>,
    /// Directory the file goes to.
    pub dir: PathBuf,
    /// Exact file path once known (Save-As, or resolved by the engine).
    pub dest: Option<PathBuf>,
    pub file_name: String,
    pub status: JobStatus,
    pub total: u64,
    pub downloaded: u64,
    pub connections: usize,
    pub speed_limit: u64,
    pub verify: bool,
    pub expected_checksum: Option<String>,
    pub start_at: Option<u64>,
    /// Queue priority (only meaningful while waiting).
    #[serde(default)]
    pub priority: Priority,
    /// Paused while waiting in the queue.
    #[serde(default)]
    pub held: bool,
    /// Stream quality for manifest jobs (`best` / `audio` / a height).
    #[serde(default)]
    pub quality: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PersistedState {
    pub version: u32,
    pub jobs: Vec<JobRecord>,
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

pub trait StateStore: Send + Sync {
    /// Load the saved state; a missing or unreadable file yields an empty one.
    fn load(&self) -> PersistedState;
    fn save(&self, state: &PersistedState) -> Result<()>;
}

/// One JSON file, replaced atomically on every save.
pub struct JsonFileStore {
    path: PathBuf,
}

impl JsonFileStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Move an unreadable file aside (kept for inspection, never deleted).
    fn quarantine(&self, why: &str) {
        let mut aside = self.path.as_os_str().to_owned();
        aside.push(format!(".corrupt-{}", crate::queue::now_secs()));
        let aside = PathBuf::from(aside);
        warn!(path = %self.path.display(), aside = %aside.display(), why, "download history unreadable; starting empty");
        if let Err(e) = std::fs::rename(&self.path, &aside) {
            warn!(error = %e, "could not move the unreadable history file aside");
        }
    }
}

impl StateStore for JsonFileStore {
    fn load(&self) -> PersistedState {
        let empty = PersistedState {
            version: STATE_VERSION,
            jobs: Vec::new(),
        };
        let data = match std::fs::read(&self.path) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return empty,
            Err(e) => {
                warn!(error = %e, "could not read download history; starting empty");
                return empty;
            }
        };
        match serde_json::from_slice::<PersistedState>(&data) {
            Ok(s) if s.version == STATE_VERSION => s,
            Ok(s) => {
                self.quarantine(&format!("unsupported version {}", s.version));
                empty
            }
            Err(e) => {
                self.quarantine(&e.to_string());
                empty
            }
        }
    }

    fn save(&self, state: &PersistedState) -> Result<()> {
        write_json_atomic(&self.path, state)
    }
}

/// Write `value` as JSON to `path` atomically (tmp + fsync + rename), creating
/// the parent directory. Shared by every file the app persists.
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).context("creating app data dir")?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let data = serde_json::to_vec_pretty(value)?;
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp).context("creating tmp file")?;
        f.write_all(&data)?;
        // Durable before the rename makes it visible.
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

/// In-memory store for tests.
#[cfg(test)]
#[derive(Default)]
pub struct MemoryStore {
    pub state: Mutex<Option<PersistedState>>,
    pub saves: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
impl StateStore for MemoryStore {
    fn load(&self) -> PersistedState {
        lock(&self.state).clone().unwrap_or(PersistedState {
            version: STATE_VERSION,
            jobs: Vec::new(),
        })
    }

    fn save(&self, state: &PersistedState) -> Result<()> {
        *lock(&self.state) = Some(state.clone());
        self.saves.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// Recover from a poisoned lock: every mutation here is a single
/// assignment/insert, so the data stays consistent.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------
// Restore-on-startup rules
// ---------------------------------------------------------------------------

/// Message shown when a download can't be restarted because its header
/// values were (deliberately) not saved.
pub fn headers_not_saved_msg(names: &[String]) -> String {
    format!(
        "This download used custom headers ({}), which are not saved to disk. \
         Add it again with the headers to continue.",
        names.join(", ")
    )
}

/// Apply the startup rules to freshly loaded records. Returns the ids to
/// re-enqueue automatically (scheduled entries).
///
/// * downloading / paused / muxing → `interrupted` (resume is manual).
/// * queued without a schedule → `interrupted`.
/// * queued/scheduled **with** `start_at` → re-enqueued (future: timer;
///   past-due: starts per the queue rules) — unless it needs header values
///   we did not save, in which case it becomes `interrupted` with a reason.
pub fn restore(jobs: &mut [JobRecord], now: u64) -> Vec<String> {
    let mut requeue = Vec::new();
    for j in jobs.iter_mut() {
        let waiting = matches!(j.status, JobStatus::Queued | JobStatus::Scheduled);
        if j.status.is_live() || (waiting && j.start_at.is_none()) {
            j.status = JobStatus::Interrupted;
            j.updated_at = now;
        } else if waiting {
            if j.header_names.is_empty() {
                j.status = if j.start_at.is_some_and(|t| t > now) {
                    JobStatus::Scheduled
                } else {
                    JobStatus::Queued
                };
                requeue.push(j.id.clone());
            } else {
                j.status = JobStatus::Interrupted;
                j.error = Some(headers_not_saved_msg(&j.header_names));
                j.updated_at = now;
            }
        }
    }
    requeue
}

/// Build the queue entry that (re)starts `rec`. A plain file reuses its
/// recorded destination, so the engine finds the `.idm` sidecar and resumes
/// from `current` instead of byte 0.
pub fn entry_from_record(
    rec: &JobRecord,
    headers: Option<Vec<(String, String)>>,
    start_at: Option<u64>,
) -> std::result::Result<QueueEntry, String> {
    let headers = match headers {
        Some(h) => h,
        None if rec.header_names.is_empty() => Vec::new(),
        None => return Err(headers_not_saved_msg(&rec.header_names)),
    };
    let is_stream = matches!(rec.kind, JobKind::Youtube | JobKind::Manifest);
    // A restored scheduled entry keeps its hold; a manual retry starts released.
    let held = start_at.is_some() && rec.held;
    Ok(QueueEntry {
        id: rec.id.clone(),
        url: rec.url.clone(),
        save_path: if is_stream { None } else { rec.dest.clone() },
        dir: rec.dir.clone(),
        file_name: rec.file_name.clone(),
        connections: rec.connections,
        speed_limit: rec.speed_limit,
        verify: rec.verify,
        expected_checksum: rec.expected_checksum.clone(),
        headers,
        mirrors: rec.mirrors.clone(),
        is_stream,
        start_at,
        priority: rec.priority,
        held,
        // A retry targets the recorded path (or re-probes); no browser hint.
        name_hint: None,
        manifest: rec.kind == JobKind::Manifest,
        quality: rec.quality.clone(),
    })
}

// ---------------------------------------------------------------------------
// The registry
// ---------------------------------------------------------------------------

pub struct History {
    store: Arc<dyn StateStore>,
    /// Insertion-ordered records.
    jobs: Mutex<Vec<JobRecord>>,
    /// Header values for this session only (never persisted).
    secrets: Mutex<HashMap<String, Vec<(String, String)>>>,
    /// Latest (total, downloaded) per YouTube sub-job, to sum per parent.
    parts: Mutex<HashMap<String, (u64, u64)>>,
    dirty: AtomicBool,
    wake: Notify,
}

impl History {
    /// Load from `store` and apply the startup [`restore`] rules. Returns the
    /// history plus the ids that should be re-enqueued now.
    pub fn open(store: Arc<dyn StateStore>, now: u64) -> (Self, Vec<String>) {
        let mut state = store.load();
        let requeue = restore(&mut state.jobs, now);
        let h = Self {
            store,
            jobs: Mutex::new(state.jobs),
            secrets: Mutex::new(HashMap::new()),
            parts: Mutex::new(HashMap::new()),
            dirty: AtomicBool::new(false),
            wake: Notify::new(),
        };
        // Persist the restored statuses (e.g. interrupted) promptly.
        h.mark_dirty();
        (h, requeue)
    }

    fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Release);
        self.wake.notify_one();
    }

    fn update<R>(&self, id: &str, now: u64, f: impl FnOnce(&mut JobRecord) -> R) -> Option<R> {
        let r = {
            let mut jobs = lock(&self.jobs);
            let j = jobs.iter_mut().find(|j| j.id == id)?;
            let r = f(j);
            j.updated_at = now;
            r
        };
        self.mark_dirty();
        Some(r)
    }

    pub fn list(&self) -> Vec<JobRecord> {
        lock(&self.jobs).clone()
    }

    pub fn get(&self, id: &str) -> Option<JobRecord> {
        lock(&self.jobs).iter().find(|j| j.id == id).cloned()
    }

    /// Session-only header values for `id`, if this session still has them.
    pub fn headers(&self, id: &str) -> Option<Vec<(String, String)>> {
        lock(&self.secrets).get(id).cloned()
    }

    /// Record a new download (replacing any record with the same id).
    pub fn insert(&self, mut rec: JobRecord, headers: Vec<(String, String)>) {
        rec.header_names = headers.iter().map(|(k, _)| k.clone()).collect();
        if !headers.is_empty() {
            lock(&self.secrets).insert(rec.id.clone(), headers);
        }
        {
            let mut jobs = lock(&self.jobs);
            jobs.retain(|j| j.id != rec.id);
            jobs.push(rec);
        }
        self.mark_dirty();
    }

    pub fn set_status(&self, id: &str, status: JobStatus, error: Option<String>, now: u64) {
        self.update(id, now, |j| {
            j.status = status;
            if error.is_some() || !matches!(status, JobStatus::Error) {
                j.error = error;
            }
        });
    }

    pub fn set_start_at(&self, id: &str, start_at: Option<u64>, now: u64) {
        self.update(id, now, |j| j.start_at = start_at);
    }

    /// Record a waiting entry's queue settings (from a `queue-status`).
    pub fn set_queue_meta(&self, id: &str, priority: Priority, held: bool, now: u64) {
        self.update(id, now, |j| {
            j.priority = priority;
            j.held = held;
        });
    }

    /// Fold an engine progress payload in. YouTube sub-jobs (`<id>::video`,
    /// `<id>::audio`) are summed into their parent; their terminal states are
    /// ignored because the pipeline reports the parent's final state itself.
    pub fn on_progress(&self, p: &ProgressPayload, now: u64) {
        let Some(status) = JobStatus::from_progress(&p.status) else {
            return;
        };
        if let Some((parent, _)) = p.id.split_once("::") {
            let (total, downloaded) = {
                let mut parts = lock(&self.parts);
                parts.insert(p.id.clone(), (p.total, p.downloaded));
                parts
                    .iter()
                    .filter(|(k, _)| k.split_once("::").is_some_and(|(g, _)| g == parent))
                    .fold((0, 0), |(t, d), (_, (pt, pd))| (t + pt, d + pd))
            };
            self.update(parent, now, |j| {
                j.total = total;
                j.downloaded = downloaded;
                if matches!(status, JobStatus::Downloading | JobStatus::Paused) {
                    j.status = status;
                }
            });
            return;
        }
        self.update(&p.id, now, |j| {
            j.total = p.total;
            j.downloaded = p.downloaded;
            j.status = status;
            j.error = p.error.clone();
            if !p.file_name.is_empty() {
                j.file_name = p.file_name.clone();
                // The engine resolved the real name: pin the exact path so a
                // later resume targets the same file (and its sidecar).
                if j.dest.is_none() {
                    j.dest = Some(j.dir.join(&p.file_name));
                }
            }
        });
    }

    /// Fold a `youtube-status` event in (`muxing|complete|error|canceled`).
    pub fn on_youtube_status(
        &self,
        id: &str,
        file_name: &str,
        status: &str,
        error: Option<String>,
        now: u64,
    ) {
        let Some(status) = JobStatus::from_progress(status) else {
            return;
        };
        self.update(id, now, |j| {
            j.status = status;
            j.error = error;
            if !file_name.starts_with('(') {
                j.file_name = file_name.to_string();
                j.dest = Some(j.dir.join(file_name));
            }
            if status == JobStatus::Complete {
                j.downloaded = j.total;
            }
        });
    }

    /// Remove one record (not while it is live or waiting).
    pub fn remove(&self, id: &str) -> std::result::Result<(), String> {
        {
            let mut jobs = lock(&self.jobs);
            let i = jobs
                .iter()
                .position(|j| j.id == id)
                .ok_or_else(|| "no such download".to_string())?;
            let s = jobs[i].status;
            if s.is_live() || matches!(s, JobStatus::Queued | JobStatus::Scheduled) {
                return Err("cancel the download before removing it".into());
            }
            jobs.remove(i);
        }
        lock(&self.secrets).remove(id);
        self.mark_dirty();
        Ok(())
    }

    /// Drop every completed or canceled record; returns the removed ids.
    pub fn clear_completed(&self) -> Vec<String> {
        let removed: Vec<String> = {
            let mut jobs = lock(&self.jobs);
            let ids = jobs
                .iter()
                .filter(|j| matches!(j.status, JobStatus::Complete | JobStatus::Canceled))
                .map(|j| j.id.clone())
                .collect();
            jobs.retain(|j| !matches!(j.status, JobStatus::Complete | JobStatus::Canceled));
            ids
        };
        if !removed.is_empty() {
            let mut secrets = lock(&self.secrets);
            for id in &removed {
                secrets.remove(id);
            }
            drop(secrets);
            self.mark_dirty();
        }
        removed
    }

    fn snapshot(&self) -> PersistedState {
        PersistedState {
            version: STATE_VERSION,
            jobs: self.list(),
        }
    }

    /// Write now if anything changed (used on exit and by the saver).
    pub fn flush(&self) -> Result<()> {
        if self.dirty.swap(false, Ordering::AcqRel) {
            if let Err(e) = self.store.save(&self.snapshot()) {
                // Keep it dirty so the next attempt retries.
                self.dirty.store(true, Ordering::Release);
                return Err(e);
            }
        }
        Ok(())
    }

    /// Background saver: after any change, wait `debounce` (coalescing the
    /// burst of updates) and write once, off the async threads.
    pub async fn run_saver(self: Arc<Self>, debounce: Duration) {
        loop {
            self.wake.notified().await;
            tokio::time::sleep(debounce).await;
            let me = self.clone();
            match tokio::task::spawn_blocking(move || me.flush()).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => warn!(error = %e, "saving download history failed"),
                Err(e) => warn!(error = %e, "history saver task failed"),
            }
        }
    }
}

/// Small helper for tests and callers that need a path inside a dir.
pub fn history_file(dir: &Path) -> PathBuf {
    dir.join("downloads.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    const NOW: u64 = 1_700_000_000;

    fn rec(id: &str, status: JobStatus) -> JobRecord {
        JobRecord {
            id: id.into(),
            kind: JobKind::File,
            url: format!("https://h/{id}.bin"),
            mirrors: vec![],
            header_names: vec![],
            dir: PathBuf::from("dl"),
            dest: None,
            file_name: format!("{id}.bin"),
            status,
            total: 1000,
            downloaded: 400,
            connections: 8,
            speed_limit: 0,
            verify: true,
            expected_checksum: Some("abc".into()),
            start_at: None,
            priority: Priority::Normal,
            held: false,
            quality: None,
            created_at: NOW - 100,
            updated_at: NOW - 50,
            error: None,
        }
    }

    fn progress(
        id: &str,
        status: &str,
        total: u64,
        downloaded: u64,
        name: &str,
    ) -> ProgressPayload {
        ProgressPayload {
            id: id.into(),
            file_name: name.into(),
            url: String::new(),
            total,
            downloaded,
            speed: 0.0,
            eta: -1.0,
            progress: 0.0,
            status: status.into(),
            connections: 1,
            active_threads: 0,
            speed_limit: 0,
            threads: vec![],
            error: None,
            sha256: None,
            verified: None,
            warnings: vec![],
        }
    }

    #[test]
    fn json_round_trip_never_stores_header_values() {
        let dir = temp_dir("hist_rt");
        let path = history_file(&dir);
        let store: Arc<dyn StateStore> = Arc::new(JsonFileStore::new(path.clone()));
        let (h, _) = History::open(store.clone(), NOW);
        let mut r = rec("a", JobStatus::Downloading);
        r.mirrors = vec!["https://m/a.bin".into()];
        r.start_at = Some(NOW + 10);
        h.insert(
            r.clone(),
            vec![("Authorization".into(), "Bearer SECRET".into())],
        );
        h.flush().unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            !raw.contains("SECRET"),
            "header value leaked to disk: {raw}"
        );
        assert!(raw.contains("Authorization"), "header name should be kept");
        assert!(
            !path.with_extension("json.tmp").exists(),
            "tmp file left behind"
        );

        let loaded = JsonFileStore::new(path).load();
        assert_eq!(loaded.version, STATE_VERSION);
        let mut want = r;
        want.header_names = vec!["Authorization".into()];
        assert_eq!(loaded.jobs, vec![want]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_or_foreign_file_is_set_aside_not_fatal() {
        let dir = temp_dir("hist_bad");
        let path = history_file(&dir);
        for bad in ["{not json", r#"{"version": 99, "jobs": []}"#, ""] {
            std::fs::write(&path, bad).unwrap();
            let store = Arc::new(JsonFileStore::new(path.clone()));
            let (h, requeue) = History::open(store, NOW);
            assert!(h.list().is_empty() && requeue.is_empty(), "input {bad:?}");
            assert!(!path.exists(), "bad file should have been moved aside");
            // And the app can save fresh state afterwards.
            h.insert(rec("x", JobStatus::Queued), vec![]);
            h.flush().unwrap();
            assert_eq!(JsonFileStore::new(path.clone()).load().jobs.len(), 1);
            std::fs::remove_file(&path).unwrap();
        }
        let kept = std::fs::read_dir(&dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".corrupt-")
            })
            .count();
        assert!(kept >= 1, "corrupt copies are kept for inspection");
        // Missing file: empty, no error.
        assert!(JsonFileStore::new(dir.join("none.json"))
            .load()
            .jobs
            .is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn version_1_files_without_queue_fields_still_load() {
        let dir = temp_dir("hist_v1");
        let path = history_file(&dir);
        let mut v = serde_json::to_value(PersistedState {
            version: STATE_VERSION,
            jobs: vec![rec("old", JobStatus::Complete)],
        })
        .unwrap();
        let job = v["jobs"][0].as_object_mut().unwrap();
        job.remove("priority");
        job.remove("held");
        std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
        let loaded = JsonFileStore::new(path).load();
        assert_eq!(loaded.jobs.len(), 1, "older file must not be quarantined");
        assert_eq!(
            (loaded.jobs[0].priority, loaded.jobs[0].held),
            (Priority::Normal, false)
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn restore_rules() {
        let mut jobs = vec![
            rec("dl", JobStatus::Downloading),
            rec("paused", JobStatus::Paused),
            rec("mux", JobStatus::Muxing),
            rec("waiting", JobStatus::Queued),
            {
                let mut r = rec("future", JobStatus::Scheduled);
                r.start_at = Some(NOW + 3600);
                r
            },
            {
                let mut r = rec("overdue", JobStatus::Scheduled);
                r.start_at = Some(NOW - 3600);
                r
            },
            {
                let mut r = rec("secret", JobStatus::Scheduled);
                r.start_at = Some(NOW + 60);
                r.header_names = vec!["Cookie".into()];
                r
            },
            rec("done", JobStatus::Complete),
            rec("failed", JobStatus::Error),
        ];
        let requeue = restore(&mut jobs, NOW);
        let status = |id: &str| jobs.iter().find(|j| j.id == id).unwrap().status;
        for id in ["dl", "paused", "mux", "waiting", "secret"] {
            assert_eq!(status(id), JobStatus::Interrupted, "{id}");
        }
        assert_eq!(status("future"), JobStatus::Scheduled);
        assert_eq!(status("overdue"), JobStatus::Queued);
        assert_eq!(status("done"), JobStatus::Complete);
        assert_eq!(status("failed"), JobStatus::Error);
        assert_eq!(requeue, ["future", "overdue"]);
        let secret = jobs.iter().find(|j| j.id == "secret").unwrap();
        assert!(secret.error.as_deref().unwrap().contains("Cookie"));
    }

    #[test]
    fn resume_targets_the_recorded_file_so_the_sidecar_is_used() {
        let mut r = rec("a", JobStatus::Interrupted);
        r.dest = Some(PathBuf::from("dl").join("real-name.iso"));
        let e = entry_from_record(&r, None, None).unwrap();
        assert_eq!(
            e.save_path, r.dest,
            "must reuse the exact path (sidecar lives next to it)"
        );
        assert_eq!(e.id, "a", "same id so the UI card is reused");
        assert_eq!((e.connections, e.verify, e.is_stream), (8, true, false));
        assert_eq!(e.expected_checksum.as_deref(), Some("abc"));

        // Header values weren't saved: refuse rather than send an
        // unauthenticated request; in-session values are used if present.
        r.header_names = vec!["Authorization".into()];
        assert!(entry_from_record(&r, None, None)
            .unwrap_err()
            .contains("Authorization"));
        let h = vec![("Authorization".to_string(), "t".to_string())];
        assert_eq!(
            entry_from_record(&r, Some(h.clone()), None)
                .unwrap()
                .headers,
            h
        );

        // Priority carries over; a hold only survives a restore (start_at set).
        r.header_names.clear();
        r.priority = Priority::High;
        r.held = true;
        let e = entry_from_record(&r, None, None).unwrap();
        assert_eq!(
            (e.priority, e.held),
            (Priority::High, false),
            "manual retry is released"
        );
        let e = entry_from_record(&r, None, Some(NOW + 60)).unwrap();
        assert_eq!(
            (e.priority, e.held),
            (Priority::High, true),
            "restored schedule stays held"
        );

        // YouTube re-runs the pipeline in its directory.
        r.kind = JobKind::Youtube;
        r.header_names.clear();
        assert_eq!(entry_from_record(&r, None, None).unwrap().save_path, None);
    }

    #[test]
    fn progress_pins_dest_and_tracks_status() {
        let store = Arc::new(MemoryStore::default());
        let (h, _) = History::open(store.clone(), NOW);
        h.insert(rec("f", JobStatus::Queued), vec![]);
        h.on_progress(
            &progress("f", "downloading", 1000, 500, "server-name.iso"),
            NOW,
        );
        let j = h.get("f").unwrap();
        assert_eq!(j.status, JobStatus::Downloading);
        assert_eq!(j.downloaded, 500);
        assert_eq!(j.dest, Some(PathBuf::from("dl").join("server-name.iso")));

        // YouTube parts are summed into the parent.
        let mut y = rec("y", JobStatus::Queued);
        y.kind = JobKind::Youtube;
        h.insert(y, vec![]);
        h.on_progress(
            &progress("y::video", "downloading", 800, 100, ".y.video.mp4"),
            NOW,
        );
        h.on_progress(
            &progress("y::audio", "complete", 200, 200, ".y.audio.m4a"),
            NOW,
        );
        let j = h.get("y").unwrap();
        assert_eq!(
            (j.total, j.downloaded, j.status),
            (1000, 300, JobStatus::Downloading)
        );
        h.on_youtube_status("y", "Clip.mp4", "complete", None, NOW);
        let j = h.get("y").unwrap();
        assert_eq!(j.status, JobStatus::Complete);
        assert_eq!(j.dest, Some(PathBuf::from("dl").join("Clip.mp4")));
    }

    /// A probe that fails before the engine emits anything must still end the
    /// record (and the UI card, which receives the same payload).
    #[test]
    fn early_failure_payload_marks_the_job_failed() {
        let (h, _) = History::open(Arc::new(MemoryStore::default()), NOW);
        h.insert(rec("p", JobStatus::Queued), vec![]);
        h.set_status("p", JobStatus::Downloading, None, NOW); // launched
        let p = ProgressPayload::failed_early(
            "p",
            "p.bin",
            "https://h/p.bin",
            "connection refused".into(),
        );
        h.on_progress(&p, NOW);
        let j = h.get("p").unwrap();
        assert_eq!(j.status, JobStatus::Error);
        assert_eq!(j.error.as_deref(), Some("connection refused"));
        assert!(j.status.is_retryable());
    }

    #[test]
    fn remove_and_clear_completed() {
        let (h, _) = History::open(Arc::new(MemoryStore::default()), NOW);
        for (id, s) in [
            ("run", JobStatus::Downloading),
            ("ok", JobStatus::Complete),
            ("x", JobStatus::Canceled),
            ("bad", JobStatus::Error),
            ("int", JobStatus::Interrupted),
        ] {
            h.insert(rec(id, s), vec![]);
        }
        assert!(h.remove("run").is_err(), "live downloads can't be removed");
        assert!(h.remove("nope").is_err());
        h.remove("int").unwrap();
        let mut cleared = h.clear_completed();
        cleared.sort();
        assert_eq!(cleared, ["ok", "x"]);
        let left: Vec<String> = h.list().into_iter().map(|j| j.id).collect();
        assert_eq!(left, ["run", "bad"]);
    }

    #[tokio::test]
    async fn saver_debounces_bursts_into_one_write() {
        let debounce = Duration::from_millis(100);
        let store = Arc::new(MemoryStore::default());
        let (h, _) = History::open(store.clone(), NOW);
        let h = Arc::new(h);
        h.flush().unwrap(); // the post-restore save
        let base = store.saves.load(Ordering::SeqCst);
        tokio::spawn(h.clone().run_saver(debounce));

        // A burst of updates well inside one debounce window...
        h.insert(rec("a", JobStatus::Queued), vec![]);
        for i in 0..50 {
            h.on_progress(&progress("a", "downloading", 1000, i * 10, "a.bin"), NOW);
        }
        // ...is written once, with the latest values.
        let mut waited = Duration::ZERO;
        while store.saves.load(Ordering::SeqCst) == base && waited < Duration::from_secs(5) {
            tokio::time::sleep(Duration::from_millis(20)).await;
            waited += Duration::from_millis(20);
        }
        tokio::time::sleep(debounce * 3).await; // room for any extra saves
        let saves = store.saves.load(Ordering::SeqCst) - base;
        assert!(
            (1..=2).contains(&saves),
            "expected a coalesced save, got {saves}"
        );
        assert_eq!(
            store.load().jobs[0].downloaded,
            490,
            "latest state was written"
        );
        assert!(
            waited >= debounce / 2,
            "saved before the debounce elapsed ({waited:?})"
        );
    }
}
