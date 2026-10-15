//! Browser bridge: a localhost-only HTTP API through which the browser
//! extension hands downloads to the app.
//!
//! * Binds **127.0.0.1 only** (never 0.0.0.0), on [`DEFAULT_PORT`] or — if
//!   that is taken — the next [`PORT_FALLBACKS`] ports.
//! * Every request except `GET /v1/ping` must carry [`TOKEN_HEADER`] equal to
//!   a random 32-byte token (hex) kept in the app data dir. A custom header
//!   also forces a CORS preflight, which is approved only for allowed origins.
//! * A request with an `Origin` header is refused unless it is
//!   `chrome-extension://<allowed id>`; CORS answers only that origin, never
//!   `*`. `Host` must be loopback, which defeats DNS rebinding.
//! * Bodies are capped at [`MAX_BODY`], every field is validated and only
//!   http(s) URLs are accepted. Tokens, cookies and header values are never
//!   logged, never echoed in errors and never persisted.
//! * No Tauri in here: the app is reached through [`BridgeHost`] (like the
//!   engine's `ProgressSink`), so the whole server is tested with a fake host.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Router};
use reqwest::header::{HeaderName, HeaderValue as ReqHeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tracing::{info, warn};
use uuid::Uuid;

use crate::downloader::{choose_file_name, safe_file_name};
use crate::history::write_json_atomic;
use crate::queue::now_secs;
use crate::stream_dl::Quality;
use crate::youtube::is_stream_site;

/// Preferred port; the extension probes it (and the next few) with `/v1/ping`.
pub const DEFAULT_PORT: u16 = 38917;
/// How many ports after [`DEFAULT_PORT`] are tried when it is taken.
pub const PORT_FALLBACKS: u16 = 10;
/// Maximum request body size.
pub const MAX_BODY: usize = 64 * 1024;
/// Version of this HTTP contract (reported by `/v1/ping`).
pub const PROTOCOL_VERSION: u32 = 1;
/// The auth header (HTTP header names are case-insensitive).
pub const TOKEN_HEADER: &str = "x-idm-token";
/// File name of the persisted bridge settings + token, in the app data dir.
pub const CONFIG_FILE: &str = "bridge.json";

const CONFIG_VERSION: u32 = 1;
/// All requests waiting for Accept / Dismiss.
const MAX_PENDING: usize = 200;
/// Of those, at most this many from non-take-over sources (a hostile page
/// pushing "page-scan" suggestions can't flood the UI).
const MAX_PENDING_NON_TAKEOVER: usize = 50;
/// Take-over downloads that may start without confirmation per window; the
/// excess waits as pending-confirm.
const TAKEOVER_BURST: usize = 20;
const TAKEOVER_WINDOW: Duration = Duration::from_secs(10);
/// The same URL arriving again this soon is answered with the first result.
const DEDUPE_WINDOW: Duration = Duration::from_secs(5);
/// Pairing prompts per minute (a hostile page can't nag the user).
const PAIR_ATTEMPTS: usize = 3;
const PAIR_WINDOW: Duration = Duration::from_secs(60);
/// How long the Allow / Deny prompt waits before counting as denied.
const PAIR_TIMEOUT: Duration = Duration::from_secs(120);

/// Extension IDs that ship allowed, so nobody has to type one: the
/// development / unpacked build (its ID is fixed by the `key` in the
/// extension's manifest) and, once published, the IDs the Chrome Web Store
/// and Edge Add-ons assign — set those in `src-tauri/extension-store.json`.
pub const DEV_EXTENSION_ID: &str = "pcmcmknffhfehgpkoedecnabbphgnabg";

/// The IDs allowed out of the box (settings can add more under Advanced).
pub fn shipped_extension_ids() -> Vec<&'static str> {
    std::iter::once(DEV_EXTENSION_ID)
        .chain(crate::browser_ext::StoreConfig::embedded().store_ids())
        .collect()
}
const MAX_BATCH: usize = 100;
const MAX_URL: usize = 8 * 1024;
const MAX_COOKIES: usize = 32 * 1024;
const MAX_HEADER_VALUE: usize = 8 * 1024;
const MAX_USER_AGENT: usize = 1024;
const MAX_HEADERS: usize = 32;
const MAX_FILE_NAME_INPUT: usize = 1024;
const MAX_MEDIA_TYPE: usize = 128;
const MAX_EXTENSION_IDS: usize = 8;
/// Display name of a stream download until yt-dlp resolves its title.
pub const RESOLVING_NAME: &str = "(resolving…)";

/// Request headers a browser may not set on a job: hop-by-hop / framing
/// headers, `Range` and `Accept-Encoding` (the engine owns them), and our own
/// token. `Referer` / `User-Agent` / `Cookie` have dedicated fields.
const FORBIDDEN_HEADERS: [&str; 12] = [
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "upgrade",
    "te",
    "trailer",
    "expect",
    "range",
    "accept-encoding",
    TOKEN_HEADER,
];

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------
// Machine-readable errors
// ---------------------------------------------------------------------------

/// Stable error codes in every error response: `{ok:false, code, message}`.
/// The extension maps them to its own wording; never rename one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// 401: token missing or wrong.
    BadToken,
    /// 403: `Origin` is not an allowed extension.
    OriginNotAllowed,
    /// 403: `Host` is not loopback (DNS rebinding).
    BadHost,
    /// 400: malformed JSON / invalid field.
    BadRequest,
    /// 400: a URL is missing, not http(s), or malformed.
    BadUrl,
    /// 400: a stream was requested for a site/URL yt-dlp isn't set up for.
    UnsupportedSite,
    /// 415: body isn't `application/json`.
    UnsupportedMediaType,
    /// 413: body over 64 KB.
    PayloadTooLarge,
    /// 429: too many requests waiting for confirmation.
    AppBusy,
    /// 503: yt-dlp is not installed (stream downloads).
    YtdlpMissing,
    /// 503: no JavaScript runtime for yt-dlp (QuickJS, deno or node).
    JsRuntimeMissing,
    /// 503: ffmpeg is not installed (stream downloads).
    FfmpegMissing,
    /// The stream is DRM-protected. Reported on the job (its error text),
    /// because it is only discovered after the request was accepted.
    #[allow(dead_code)] // part of the documented wire contract
    DrmProtected,
    /// 403: the user clicked Deny (or didn't answer) on the pairing prompt.
    PairingDenied,
    /// 409: another pairing prompt is already on screen.
    PairingPending,
    /// 429: too many pairing attempts; try again in a minute.
    PairingRateLimited,
    /// 404: unknown endpoint, or no such pending request.
    NotFound,
    /// 500: anything else.
    #[allow(dead_code)] // part of the documented wire contract
    Internal,
}

impl ErrorCode {
    pub fn status(self) -> StatusCode {
        match self {
            Self::BadToken => StatusCode::UNAUTHORIZED,
            Self::OriginNotAllowed | Self::BadHost => StatusCode::FORBIDDEN,
            Self::BadRequest | Self::BadUrl | Self::UnsupportedSite => StatusCode::BAD_REQUEST,
            Self::UnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::AppBusy => StatusCode::TOO_MANY_REQUESTS,
            Self::YtdlpMissing | Self::JsRuntimeMissing | Self::FfmpegMissing => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            Self::DrmProtected => StatusCode::UNPROCESSABLE_ENTITY,
            Self::PairingDenied => StatusCode::FORBIDDEN,
            Self::PairingPending => StatusCode::CONFLICT,
            Self::PairingRateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// A failure the app side reports when starting a job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostError {
    pub code: ErrorCode,
    pub message: String,
}

impl HostError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

type Invalid = (ErrorCode, String);

fn invalid(code: ErrorCode, message: impl Into<String>) -> Invalid {
    (code, message.into())
}

// ---------------------------------------------------------------------------
// Settings + token (persisted)
// ---------------------------------------------------------------------------

/// User-editable bridge settings (`get_bridge_info` / `set_bridge_settings`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BridgeSettings {
    /// Run the HTTP server at all.
    pub enabled: bool,
    /// Chrome extension IDs whose `chrome-extension://<id>` origin is allowed.
    /// Empty = every request carrying an `Origin` is refused (tools like curl
    /// send none and still work with the token).
    pub allowed_extension_ids: Vec<String>,
    /// Queue browser requests as "pending-confirm" until accepted in the UI
    /// (floating button, context menu, popup, page-scan — not take-over).
    pub ask_before_start: bool,
    /// Also confirm downloads Chrome handed over (`source:"takeover"`). Off by
    /// default: the user already clicked the link in the browser.
    pub confirm_takeover: bool,
}

impl Default for BridgeSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            allowed_extension_ids: Vec::new(),
            ask_before_start: true,
            confirm_takeover: false,
        }
    }
}

impl BridgeSettings {
    /// Trim, lower-case and de-duplicate the extension IDs, rejecting
    /// anything that is not a Chrome extension ID.
    pub fn normalized(mut self) -> std::result::Result<Self, String> {
        let mut ids: Vec<String> = Vec::new();
        for raw in &self.allowed_extension_ids {
            let id = raw.trim().to_ascii_lowercase();
            if id.is_empty() {
                continue;
            }
            if !is_extension_id(&id) {
                let shown: String = raw.trim().chars().take(40).collect();
                return Err(format!(
                    "\"{shown}\" is not a Chrome extension ID (32 letters a–p, see chrome://extensions)"
                ));
            }
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        if ids.len() > MAX_EXTENSION_IDS {
            return Err(format!(
                "at most {MAX_EXTENSION_IDS} extension IDs can be allowed"
            ));
        }
        self.allowed_extension_ids = ids;
        Ok(self)
    }
}

fn is_extension_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| (b'a'..=b'p').contains(&b))
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigFile {
    version: u32,
    token: String,
    #[serde(flatten)]
    settings: BridgeSettings,
}

/// A fresh random token: 32 bytes from the OS CSPRNG, hex-encoded.
pub fn new_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|e| anyhow!("no OS randomness: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn is_token(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Constant-time comparison, so response timing reveals nothing about the token.
fn token_matches(given: &[u8], expected: &[u8]) -> bool {
    given.len() == expected.len()
        && given
            .iter()
            .zip(expected)
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

/// Read the config file; anything missing or unreadable yields `None`.
fn load_config(path: &Path) -> Option<ConfigFile> {
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            warn!(error = %e, "could not read browser bridge settings; using defaults");
            return None;
        }
    };
    match serde_json::from_slice::<ConfigFile>(&data) {
        Ok(c) if c.version == CONFIG_VERSION => Some(c),
        Ok(c) => {
            warn!(
                version = c.version,
                "unsupported browser bridge settings version; using defaults"
            );
            None
        }
        Err(e) => {
            warn!(error = %e, "browser bridge settings unreadable; using defaults");
            None
        }
    }
}

/// Owner-only permissions for the file holding the token.
fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            warn!(error = %e, "could not restrict browser bridge settings permissions");
        }
    }
    #[cfg(windows)]
    {
        // The per-user app data dir is already private to the user.
        let _ = path;
    }
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/// What the browser asks for. Deliberately no `Debug`: it may hold cookies.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DownloadRequest {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub page_url: Option<String>,
    #[serde(default)]
    pub referer: Option<String>,
    #[serde(default)]
    pub user_agent: Option<String>,
    #[serde(default)]
    pub cookies: Option<String>,
    #[serde(default)]
    pub headers: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub kind: Option<RequestKind>,
    #[serde(default)]
    pub media_type: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    /// Stream quality: `"best"` (default) | `"audio"` | a height like `"1080"`.
    #[serde(default)]
    pub quality: Option<String>,
    /// Why the browser sent it (default `"user-action"`).
    #[serde(default)]
    pub source: Option<Source>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestKind {
    Auto,
    File,
    Stream,
}

/// Where a request came from — decides whether it needs confirmation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// Chrome started a download and the extension took it over: the user
    /// already clicked, so it starts at once (rate-limited; extension only).
    Takeover,
    /// Floating button, context menu, popup row, pasted link.
    #[default]
    UserAction,
    /// "Download all" / page suggestions.
    PageScan,
    /// A URL passed on the command line to a second app launch.
    CommandLine,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchRequest {
    items: Vec<DownloadRequest>,
}

/// A validated request, ready for the app's normal start path.
#[derive(Clone)]
pub struct BridgeJob {
    /// What to download: a file URL, a manifest, or the page for yt-dlp.
    pub url: String,
    /// Goes through yt-dlp (a known site's page, or a manifest).
    pub stream: bool,
    /// `url` is an HLS/DASH manifest downloaded directly by yt-dlp.
    pub manifest: bool,
    /// Normalised quality (`best`, `audio`, `720` …) for stream jobs.
    pub quality: Option<String>,
    /// Request headers for this job only (Referer / User-Agent / Cookie and
    /// validated extras). Values are secrets: never log or persist them.
    pub headers: Vec<(String, String)>,
    /// Sanitised browser file name (plain files only).
    pub name_hint: Option<String>,
    pub page_url: Option<String>,
    pub media_type: Option<String>,
    pub size: Option<u64>,
    pub source: Source,
}

impl fmt::Debug for BridgeJob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Header *names* only.
        let names: Vec<&str> = self.headers.iter().map(|(k, _)| k.as_str()).collect();
        f.debug_struct("BridgeJob")
            .field("url", &self.url)
            .field("stream", &self.stream)
            .field("manifest", &self.manifest)
            .field("headers", &names)
            .field("name_hint", &self.name_hint)
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

impl BridgeJob {
    /// Provisional display name (the engine may still refine a plain file's
    /// name from `Content-Disposition`; yt-dlp names a stream).
    pub fn display_name(&self) -> String {
        if self.stream {
            RESOLVING_NAME.to_string()
        } else {
            self.name_hint
                .clone()
                .unwrap_or_else(|| choose_file_name(&self.url, None))
        }
    }

    fn has_cookies(&self) -> bool {
        self.headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("cookie"))
    }
}

/// An absolute http(s) URL with a host; anything else (file:, javascript:,
/// data:, blob:, ftp:, …) is refused. Returns the normalised URL.
fn http_url(field: &str, raw: &str) -> Result<String, Invalid> {
    let bad = |m: String| invalid(ErrorCode::BadUrl, m);
    let s = raw.trim();
    if s.is_empty() {
        return Err(bad(format!("{field} is empty")));
    }
    if s.len() > MAX_URL {
        return Err(bad(format!("{field} is too long")));
    }
    let u = reqwest::Url::parse(s).map_err(|_| bad(format!("{field} is not a valid URL")))?;
    if !matches!(u.scheme(), "http" | "https") {
        return Err(bad(format!(
            "{field}: only http and https URLs are accepted"
        )));
    }
    if u.host_str().is_none_or(str::is_empty) {
        return Err(bad(format!("{field} has no host")));
    }
    Ok(u.to_string())
}

/// A header value that is safe to send (visible ASCII + tab/space, no CR/LF).
/// Error messages name the field, never the value.
fn header_value(field: &str, raw: &str, max: usize) -> Result<String, Invalid> {
    let v = raw.trim();
    if v.len() > max {
        return Err(invalid(
            ErrorCode::BadRequest,
            format!("{field} is too long"),
        ));
    }
    if ReqHeaderValue::from_str(v).is_err() {
        return Err(invalid(
            ErrorCode::BadRequest,
            format!("{field} contains characters not allowed in a header"),
        ));
    }
    Ok(v.to_string())
}

/// An HLS/DASH manifest: `.m3u8` / `.mpd` path, or the matching media type.
pub fn is_manifest(url: &str, media_type: Option<&str>) -> bool {
    let by_type = media_type.is_some_and(|m| {
        let m = m.to_ascii_lowercase();
        m.contains("mpegurl") || m.contains("dash+xml")
    });
    let by_path = reqwest::Url::parse(url).is_ok_and(|u| {
        let p = u.path().to_ascii_lowercase();
        p.ends_with(".m3u8") || p.ends_with(".mpd")
    });
    by_type || by_path
}

/// What to download and how: `(target, stream, manifest)`.
fn resolve_kind(
    kind: RequestKind,
    url: Option<String>,
    page_url: Option<&str>,
    media_type: Option<&str>,
) -> Result<(String, bool, bool), Invalid> {
    let unsupported = |m: &str| invalid(ErrorCode::UnsupportedSite, m);
    match kind {
        RequestKind::File => url
            .map(|u| (u, false, false))
            .ok_or_else(|| invalid(ErrorCode::BadUrl, "kind \"file\" needs url")),
        RequestKind::Stream => {
            if let Some(u) = url.as_deref() {
                if is_stream_site(u) {
                    return Ok((u.to_string(), true, false));
                }
                if is_manifest(u, media_type) {
                    return Ok((u.to_string(), true, true));
                }
            }
            match page_url {
                Some(p) if is_stream_site(p) => Ok((p.to_string(), true, false)),
                Some(_) | None if url.is_none() && page_url.is_none() => {
                    Err(invalid(ErrorCode::BadUrl, "url or pageUrl is required"))
                }
                _ => Err(unsupported(
                    "kind \"stream\" needs a supported video site or an .m3u8/.mpd manifest URL",
                )),
            }
        }
        RequestKind::Auto => match (url, page_url) {
            (Some(u), _) if is_stream_site(&u) => Ok((u, true, false)),
            // A manifest must never be saved as the playlist text.
            (Some(u), _) if is_manifest(&u, media_type) => Ok((u, true, true)),
            (Some(u), _) => Ok((u, false, false)),
            (None, Some(p)) if is_stream_site(p) => Ok((p.to_string(), true, false)),
            (None, Some(_)) => Err(unsupported(
                "pageUrl alone is only accepted for supported video sites; send url",
            )),
            (None, None) => Err(invalid(ErrorCode::BadUrl, "url or pageUrl is required")),
        },
    }
}

impl DownloadRequest {
    /// Validate every field and build the job (headers, target, name hint).
    pub fn validate(self) -> Result<BridgeJob, Invalid> {
        let url = self
            .url
            .as_deref()
            .map(|u| http_url("url", u))
            .transpose()?;
        let page_url = self
            .page_url
            .as_deref()
            .map(|u| http_url("pageUrl", u))
            .transpose()?;
        let referer = self
            .referer
            .as_deref()
            .map(|u| http_url("referer", u))
            .transpose()?;
        let user_agent = self
            .user_agent
            .as_deref()
            .map(|v| header_value("userAgent", v, MAX_USER_AGENT))
            .transpose()?
            .filter(|v| !v.is_empty());
        let cookies = self
            .cookies
            .as_deref()
            .map(|v| header_value("cookies", v, MAX_COOKIES))
            .transpose()?
            .filter(|v| !v.is_empty());

        let custom = self.headers.unwrap_or_default();
        if custom.len() > MAX_HEADERS {
            return Err(invalid(
                ErrorCode::BadRequest,
                format!("at most {MAX_HEADERS} headers are accepted"),
            ));
        }
        let mut headers: Vec<(String, String)> = Vec::new();
        for (name, value) in &custom {
            let name = name.trim();
            if name.len() > 64 || HeaderName::from_bytes(name.as_bytes()).is_err() {
                return Err(invalid(
                    ErrorCode::BadRequest,
                    "headers: invalid header name",
                ));
            }
            let lower = name.to_ascii_lowercase();
            if FORBIDDEN_HEADERS.contains(&lower.as_str())
                || lower.starts_with("proxy-")
                || lower.starts_with("sec-")
            {
                return Err(invalid(
                    ErrorCode::BadRequest,
                    format!("headers: \"{name}\" cannot be set"),
                ));
            }
            // The dedicated fields win over a duplicate in the map.
            let overridden = match lower.as_str() {
                "referer" => referer.is_some(),
                "user-agent" => user_agent.is_some(),
                "cookie" => cookies.is_some(),
                _ => false,
            };
            if overridden || headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(name)) {
                continue;
            }
            let value = header_value(&format!("headers.{name}"), value, MAX_HEADER_VALUE)?;
            headers.push((name.to_string(), value));
        }
        if let Some(r) = referer {
            headers.push(("Referer".into(), r));
        }
        if let Some(ua) = user_agent {
            headers.push(("User-Agent".into(), ua));
        }
        if let Some(c) = cookies {
            headers.push(("Cookie".into(), c));
        }

        let file_name = match self.file_name.as_deref() {
            Some(n) if n.len() > MAX_FILE_NAME_INPUT => {
                return Err(invalid(ErrorCode::BadRequest, "fileName is too long"))
            }
            // Only a hint: it must survive `safe_file_name`, else it is ignored.
            Some(n) => safe_file_name(n),
            None => None,
        };
        let media_type = match self.media_type.as_deref().map(str::trim) {
            Some(m) if m.len() > MAX_MEDIA_TYPE || m.chars().any(char::is_control) => {
                return Err(invalid(ErrorCode::BadRequest, "mediaType is invalid"))
            }
            Some("") | None => None,
            Some(m) => Some(m.to_string()),
        };
        let quality = match self.quality.as_deref() {
            None => None,
            Some(q) => Some(
                Quality::parse(q)
                    .ok_or_else(|| {
                        invalid(
                            ErrorCode::BadRequest,
                            "quality must be \"best\", \"audio\" or a height like \"1080\"",
                        )
                    })?
                    .as_string(),
            ),
        };

        let (target, stream, manifest) = resolve_kind(
            self.kind.unwrap_or(RequestKind::Auto),
            url,
            page_url.as_deref(),
            media_type.as_deref(),
        )?;
        Ok(BridgeJob {
            url: target,
            stream,
            manifest,
            quality: if stream { quality } else { None },
            headers,
            name_hint: if stream { None } else { file_name },
            page_url,
            media_type,
            size: self.size,
            source: self.source.unwrap_or_default(),
        })
    }
}

// ---------------------------------------------------------------------------
// App side (implemented in main.rs; faked in tests)
// ---------------------------------------------------------------------------

/// A browser request waiting for Accept / Dismiss (`bridge-request` event,
/// `list_bridge_requests`). Holds no header values.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingRequest {
    pub id: String,
    /// Always `"pending-confirm"`.
    pub status: String,
    pub url: String,
    pub page_url: Option<String>,
    pub file_name: String,
    /// `"file"` | `"stream"`
    pub kind: String,
    pub media_type: Option<String>,
    pub size: Option<u64>,
    /// Header names only (e.g. `Referer`, `Cookie`).
    pub header_names: Vec<String>,
    pub has_cookies: bool,
    /// Epoch seconds.
    pub received_at: u64,
    /// `takeover` | `user-action` | `page-scan` | `command-line`
    pub source: Source,
    pub quality: Option<String>,
}

impl PendingRequest {
    fn new(id: &str, job: &BridgeJob, now: u64) -> Self {
        Self {
            id: id.to_string(),
            status: "pending-confirm".into(),
            url: job.url.clone(),
            page_url: job.page_url.clone(),
            file_name: job.display_name(),
            kind: if job.stream { "stream" } else { "file" }.into(),
            media_type: job.media_type.clone(),
            size: job.size,
            header_names: job.headers.iter().map(|(k, _)| k.clone()).collect(),
            has_cookies: job.has_cookies(),
            received_at: now,
            source: job.source,
            quality: job.quality.clone(),
        }
    }
}

/// The "Allow this extension?" prompt (`pairing-request` event).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingPrompt {
    pub id: String,
    pub extension_id: String,
    /// "Chrome", "Microsoft Edge" or "browser" (from the User-Agent; display only).
    pub browser: String,
}

struct PendingPair {
    prompt: PairingPrompt,
    answer: tokio::sync::oneshot::Sender<bool>,
}

/// Clears the prompt when the waiting request ends for any reason.
struct PairGuard {
    bridge: Arc<Bridge>,
    id: String,
}

impl Drop for PairGuard {
    fn drop(&mut self) {
        let still_ours = {
            let mut slot = lock(&self.bridge.pairing);
            if slot.as_ref().is_some_and(|p| p.prompt.id == self.id) {
                slot.take();
                true
            } else {
                false
            }
        };
        if still_ours {
            self.bridge.host.pairing_closed(&self.id);
        }
    }
}

/// `get_bridge_info` result and `bridge-status` event payload.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeInfo {
    pub settings: BridgeSettings,
    pub listening: bool,
    pub port: Option<u16>,
    /// `"127.0.0.1:PORT"` while listening.
    pub address: Option<String>,
    /// Why the server is not listening (e.g. every port taken).
    pub error: Option<String>,
    /// For the UI's Copy-token button only (local IPC, never logged).
    pub token: String,
    pub default_port: u16,
    /// IDs allowed without any setup (shown read-only in the UI).
    pub shipped_extension_ids: Vec<String>,
    pub protocol: u32,
}

/// What the bridge needs from the app.
// See youtube.rs: `async_trait` output trips this lint, not our code.
#[allow(clippy::double_must_use)]
#[async_trait]
pub trait BridgeHost: Send + Sync {
    /// Enqueue `job` under `id` through the app's normal start path. Returns
    /// the provisional file name.
    async fn start(&self, id: String, job: BridgeJob) -> Result<String, HostError>;
    fn pending_added(&self, req: &PendingRequest);
    fn pending_removed(&self, id: &str);
    fn status_changed(&self, info: &BridgeInfo);
    /// Show the "Allow this extension?" prompt (one at a time).
    fn pairing_requested(&self, prompt: &PairingPrompt);
    /// The prompt was answered, timed out, or the extension went away.
    fn pairing_closed(&self, id: &str);
}

// ---------------------------------------------------------------------------
// The bridge (settings, server lifecycle, pending requests)
// ---------------------------------------------------------------------------

struct Config {
    settings: BridgeSettings,
    token: String,
}

#[derive(Default)]
struct Status {
    port: Option<u16>,
    error: Option<String>,
}

struct Running {
    task: JoinHandle<()>,
}

pub struct Bridge {
    host: Arc<dyn BridgeHost>,
    /// Where settings + token are saved (`None` in tests: memory only).
    path: Option<PathBuf>,
    base_port: u16,
    config: Mutex<Config>,
    status: Mutex<Status>,
    /// Requests awaiting confirmation, oldest first; in memory only.
    pending: Mutex<Vec<(PendingRequest, BridgeJob)>>,
    /// Recent results by URL, to answer duplicates within [`DEDUPE_WINDOW`].
    recent: Mutex<HashMap<String, (Instant, Accepted)>>,
    /// Start times of recent auto-started take-overs (rate limit).
    takeover_starts: Mutex<VecDeque<Instant>>,
    /// The pairing prompt currently on screen, if any.
    pairing: Mutex<Option<PendingPair>>,
    pair_attempts: Mutex<VecDeque<Instant>>,
    /// Prompt timeout in ms (shortened by tests).
    pair_timeout_ms: std::sync::atomic::AtomicU64,
    server: tokio::sync::Mutex<Option<Running>>,
}

impl Bridge {
    fn new(
        host: Arc<dyn BridgeHost>,
        path: Option<PathBuf>,
        base_port: u16,
        settings: BridgeSettings,
        token: String,
        error: Option<String>,
    ) -> Arc<Self> {
        Arc::new(Self {
            host,
            path,
            base_port,
            config: Mutex::new(Config { settings, token }),
            status: Mutex::new(Status { port: None, error }),
            pending: Mutex::new(Vec::new()),
            recent: Mutex::new(HashMap::new()),
            takeover_starts: Mutex::new(VecDeque::new()),
            pairing: Mutex::new(None),
            pair_attempts: Mutex::new(VecDeque::new()),
            pair_timeout_ms: std::sync::atomic::AtomicU64::new(PAIR_TIMEOUT.as_millis() as u64),
            server: tokio::sync::Mutex::new(None),
        })
    }

    /// Load settings + token from `path` (creating a token on first run).
    /// Never fails: problems are logged and shown in the status line. Call
    /// [`Bridge::ensure_running`] afterwards to start listening.
    pub fn open(host: Arc<dyn BridgeHost>, path: PathBuf, base_port: u16) -> Arc<Self> {
        let loaded = load_config(&path);
        let settings = loaded
            .as_ref()
            .map(|c| c.settings.clone())
            .and_then(|s| s.normalized().ok())
            .unwrap_or_default();
        let existing = loaded.map(|c| c.token).filter(|t| is_token(t));
        let fresh = existing.is_none();
        let (token, error) = match existing.map(Ok).unwrap_or_else(new_token) {
            Ok(t) => (t, None),
            Err(e) => (
                String::new(),
                Some(format!("could not create a token: {e:#}")),
            ),
        };
        let bridge = Self::new(host, Some(path), base_port, settings, token, error);
        if fresh {
            if let Err(e) = bridge.save() {
                warn!(error = %e, "could not save the browser bridge token");
            }
        }
        bridge
    }

    fn save(&self) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let file = {
            let c = lock(&self.config);
            ConfigFile {
                version: CONFIG_VERSION,
                token: c.token.clone(),
                settings: c.settings.clone(),
            }
        };
        write_json_atomic(path, &file).context("saving browser bridge settings")?;
        restrict_permissions(path);
        Ok(())
    }

    pub fn info(&self) -> BridgeInfo {
        let (settings, token) = {
            let c = lock(&self.config);
            (c.settings.clone(), c.token.clone())
        };
        let status = lock(&self.status);
        BridgeInfo {
            settings,
            listening: status.port.is_some(),
            port: status.port,
            address: status.port.map(|p| format!("127.0.0.1:{p}")),
            error: status.error.clone(),
            token,
            default_port: DEFAULT_PORT,
            shipped_extension_ids: shipped_extension_ids()
                .into_iter()
                .map(str::to_string)
                .collect(),
            protocol: PROTOCOL_VERSION,
        }
    }

    /// Start or stop the server to match the `enabled` setting, then report
    /// the status to the host.
    pub async fn ensure_running(self: &Arc<Self>) {
        let enabled = lock(&self.config).settings.enabled;
        {
            let mut server = self.server.lock().await;
            if enabled && server.is_none() {
                match bind_with_fallback(self.base_port, PORT_FALLBACKS).await {
                    Ok((listener, port)) => {
                        let app = router(self.clone());
                        let task = tokio::spawn(async move {
                            if let Err(e) = axum::serve(listener, app).await {
                                warn!(error = %e, "browser bridge server stopped");
                            }
                        });
                        *server = Some(Running { task });
                        *lock(&self.status) = Status {
                            port: Some(port),
                            error: None,
                        };
                        info!(port, "browser bridge listening on 127.0.0.1");
                    }
                    Err(e) => {
                        warn!(error = %e, "browser bridge could not start");
                        *lock(&self.status) = Status {
                            port: None,
                            error: Some(format!("{e:#}")),
                        };
                    }
                }
            } else if !enabled {
                if let Some(r) = server.take() {
                    r.task.abort();
                    let _ = r.task.await;
                    info!("browser bridge stopped");
                }
                let mut status = lock(&self.status);
                status.port = None;
                // Keep a token error visible; a bind error no longer applies.
                if lock(&self.config).token.is_empty() {
                    status.error = Some("no token: the bridge cannot accept requests".into());
                } else {
                    status.error = None;
                }
            }
        }
        self.host.status_changed(&self.info());
    }

    /// Validate, apply and persist new settings.
    pub async fn set_settings(
        self: &Arc<Self>,
        settings: BridgeSettings,
    ) -> std::result::Result<BridgeInfo, String> {
        let settings = settings.normalized()?;
        lock(&self.config).settings = settings;
        let saved = self.save();
        self.ensure_running().await;
        saved.map_err(|e| format!("settings applied but not saved: {e:#}"))?;
        Ok(self.info())
    }

    /// Replace the token (the old one stops working at once) and persist it.
    pub fn regenerate_token(&self) -> std::result::Result<BridgeInfo, String> {
        let token = new_token().map_err(|e| format!("{e:#}"))?;
        lock(&self.config).token = token;
        let saved = self.save();
        let info = self.info();
        self.host.status_changed(&info);
        saved.map_err(|e| format!("new token in use but not saved: {e:#}"))?;
        Ok(info)
    }

    pub fn pending(&self) -> Vec<PendingRequest> {
        lock(&self.pending).iter().map(|(r, _)| r.clone()).collect()
    }

    fn take_pending(&self, id: &str) -> Result<BridgeJob, HostError> {
        let job = {
            let mut pending = lock(&self.pending);
            let i = pending
                .iter()
                .position(|(r, _)| r.id == id)
                .ok_or_else(|| HostError::new(ErrorCode::NotFound, "no such browser request"))?;
            pending.remove(i).1
        };
        self.host.pending_removed(id);
        Ok(job)
    }

    /// Start a pending request (same id, so its card appears under it).
    pub async fn accept(&self, id: &str) -> Result<String, HostError> {
        let job = self.take_pending(id)?;
        self.host.start(id.to_string(), job).await
    }

    pub fn dismiss(&self, id: &str) -> Result<(), HostError> {
        self.take_pending(id).map(|_| ())
    }

    fn token_ok(&self, given: Option<&HeaderValue>) -> bool {
        let c = lock(&self.config);
        !c.token.is_empty()
            && given.is_some_and(|g| token_matches(g.as_bytes(), c.token.as_bytes()))
    }

    /// The `Origin` value if it is an allowed `chrome-extension://<id>`.
    fn allowed_origin(&self, origin: &HeaderValue) -> Option<HeaderValue> {
        let id = origin.to_str().ok()?.strip_prefix("chrome-extension://")?;
        let allowed = shipped_extension_ids().contains(&id)
            || lock(&self.config)
                .settings
                .allowed_extension_ids
                .iter()
                .any(|a| a == id);
        allowed.then(|| origin.clone())
    }

    /// The pairing prompt on screen (the UI asks once on mount).
    pub fn pending_pairing(&self) -> Option<PairingPrompt> {
        lock(&self.pairing).as_ref().map(|p| p.prompt.clone())
    }

    /// The user's answer to prompt `id`.
    pub fn answer_pairing(&self, id: &str, allow: bool) -> Result<(), HostError> {
        let pending = {
            let mut slot = lock(&self.pairing);
            match slot.as_ref() {
                Some(p) if p.prompt.id == id => slot.take(),
                _ => None,
            }
        };
        let Some(p) = pending else {
            return Err(HostError::new(
                ErrorCode::NotFound,
                "no such pairing request",
            ));
        };
        self.host.pairing_closed(id);
        // The waiting HTTP request may already be gone; that's fine.
        let _ = p.answer.send(allow);
        Ok(())
    }

    /// `POST /v1/pair`: ask the user, then hand the token to the extension.
    /// `extension_id` comes from the (already allow-listed) Origin header.
    async fn pair(
        self: &Arc<Self>,
        extension_id: String,
        browser: &str,
    ) -> Result<String, HostError> {
        let now = Instant::now();
        {
            let mut attempts = lock(&self.pair_attempts);
            while attempts
                .front()
                .is_some_and(|t| now.duration_since(*t) >= PAIR_WINDOW)
            {
                attempts.pop_front();
            }
            if attempts.len() >= PAIR_ATTEMPTS {
                return Err(HostError::new(
                    ErrorCode::PairingRateLimited,
                    "too many pairing attempts; try again in a minute",
                ));
            }
            attempts.push_back(now);
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let prompt = PairingPrompt {
            id: Uuid::new_v4().to_string(),
            extension_id,
            browser: browser.to_string(),
        };
        {
            let mut slot = lock(&self.pairing);
            if slot.is_some() {
                // Never stack prompts.
                return Err(HostError::new(
                    ErrorCode::PairingPending,
                    "a pairing request is already waiting in IDM Replica",
                ));
            }
            *slot = Some(PendingPair {
                prompt: prompt.clone(),
                answer: tx,
            });
        }
        info!(browser, "pairing requested by the browser extension");
        self.host.pairing_requested(&prompt);
        // If the extension disconnects, the prompt goes away with it.
        let _cleanup = PairGuard {
            bridge: self.clone(),
            id: prompt.id.clone(),
        };
        let wait = Duration::from_millis(
            self.pair_timeout_ms
                .load(std::sync::atomic::Ordering::Relaxed),
        );
        match tokio::time::timeout(wait, rx).await {
            Ok(Ok(true)) => {
                info!("pairing allowed");
                Ok(lock(&self.config).token.clone())
            }
            Ok(_) => Err(HostError::new(
                ErrorCode::PairingDenied,
                "pairing was denied in IDM Replica",
            )),
            Err(_) => Err(HostError::new(
                ErrorCode::PairingDenied,
                "nobody answered the pairing prompt in IDM Replica",
            )),
        }
    }

    /// Reserve an auto-start slot for a take-over, or `false` when more than
    /// [`TAKEOVER_BURST`] started within [`TAKEOVER_WINDOW`].
    fn takeover_slot(&self, now: Instant) -> bool {
        let mut starts = lock(&self.takeover_starts);
        while starts
            .front()
            .is_some_and(|t| now.duration_since(*t) >= TAKEOVER_WINDOW)
        {
            starts.pop_front();
        }
        if starts.len() >= TAKEOVER_BURST {
            return false;
        }
        starts.push_back(now);
        true
    }

    fn queue_pending(&self, id: String, job: BridgeJob) -> Result<Accepted, HostError> {
        let req = PendingRequest::new(&id, &job, now_secs());
        {
            let mut pending = lock(&self.pending);
            let busy = || {
                HostError::new(
                    ErrorCode::AppBusy,
                    "too many downloads are waiting for confirmation in IDM Replica",
                )
            };
            if pending.len() >= MAX_PENDING {
                return Err(busy());
            }
            let non_takeover = pending
                .iter()
                .filter(|(r, _)| r.source != Source::Takeover)
                .count();
            if job.source != Source::Takeover && non_takeover >= MAX_PENDING_NON_TAKEOVER {
                return Err(busy());
            }
            pending.push((req.clone(), job));
        }
        self.host.pending_added(&req);
        Ok(Accepted {
            id,
            file_name: req.file_name,
            queued: false,
            duplicate: false,
        })
    }

    /// Hand one validated job to the app: started at once, or pending until
    /// the user accepts it. `from_extension` = sent from the allowed
    /// extension's origin (only then is `source:"takeover"` honoured).
    async fn submit(
        &self,
        mut job: BridgeJob,
        from_extension: bool,
    ) -> Result<Accepted, HostError> {
        if job.source == Source::Takeover && !from_extension {
            job.source = Source::UserAction;
        }
        let now = Instant::now();
        let key = format!(
            "{}|{}|{}",
            job.url,
            job.manifest,
            job.quality.as_deref().unwrap_or("")
        );
        {
            let mut recent = lock(&self.recent);
            recent.retain(|_, (t, _)| now.duration_since(*t) < DEDUPE_WINDOW);
            if let Some((_, first)) = recent.get(&key) {
                return Ok(Accepted {
                    duplicate: true,
                    ..first.clone()
                });
            }
        }
        let (ask, confirm_takeover) = {
            let c = lock(&self.config);
            (c.settings.ask_before_start, c.settings.confirm_takeover)
        };
        let confirm = match job.source {
            // Rate limit: the excess waits for the user instead of starting.
            Source::Takeover => confirm_takeover || !self.takeover_slot(now),
            Source::UserAction | Source::PageScan => ask,
            // A URL handed to a second app launch always asks.
            Source::CommandLine => true,
        };
        // Host + kind only: query strings and headers may carry secrets.
        info!(
            host = %url_host(&job.url),
            stream = job.stream,
            manifest = job.manifest,
            source = ?job.source,
            confirm,
            "download request from the browser"
        );
        let id = Uuid::new_v4().to_string();
        let accepted = if confirm {
            self.queue_pending(id, job)?
        } else {
            let file_name = self.host.start(id.clone(), job).await?;
            Accepted {
                id,
                file_name,
                queued: true,
                duplicate: false,
            }
        };
        lock(&self.recent).insert(key, (now, accepted.clone()));
        Ok(accepted)
    }

    /// A URL from the command line of a second launch: always asks first.
    pub async fn submit_command_line(&self, url: &str) -> Result<(), HostError> {
        let job = DownloadRequest {
            url: Some(url.to_string()),
            page_url: None,
            referer: None,
            user_agent: None,
            cookies: None,
            headers: None,
            file_name: None,
            kind: None,
            media_type: None,
            size: None,
            quality: None,
            source: Some(Source::CommandLine),
        }
        .validate()
        .map_err(|(code, message)| HostError::new(code, message))?;
        self.submit(job, false).await.map(|_| ())
    }
}

fn url_host(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_default()
}

/// Bind 127.0.0.1 on `base`, else on each of the next `extra` ports.
pub async fn bind_with_fallback(base: u16, extra: u16) -> Result<(TcpListener, u16)> {
    let last = base.saturating_add(extra);
    let mut error = None;
    for port in base..=last {
        match TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await {
            Ok(l) => {
                let bound = l.local_addr()?.port();
                return Ok((l, bound));
            }
            Err(e) => error = Some(e),
        }
    }
    Err(anyhow!(
        "ports {base}–{last} on 127.0.0.1 are all in use ({})",
        error.map(|e| e.to_string()).unwrap_or_default()
    ))
}

// ---------------------------------------------------------------------------
// HTTP layer
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Accepted {
    id: String,
    file_name: String,
    /// `true` = started (or waiting for a queue slot); `false` = waiting for
    /// the user to accept it in the app.
    queued: bool,
    /// The same URL arrived again within a few seconds: this is the first
    /// request's result and nothing new was started.
    duplicate: bool,
}

impl Accepted {
    fn json(&self) -> serde_json::Value {
        json!({
            "ok": true,
            "id": self.id,
            "fileName": self.file_name,
            "queued": self.queued,
            "status": if self.queued { "queued" } else { "pending-confirm" },
            "duplicate": self.duplicate,
        })
    }
}

/// `{ok:false, code, message}` (+ `error`, the same text, for older clients).
struct ApiError {
    code: ErrorCode,
    message: String,
}

impl ApiError {
    fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn body(&self) -> serde_json::Value {
        json!({ "ok": false, "code": self.code, "message": self.message, "error": self.message })
    }
}

impl From<HostError> for ApiError {
    fn from(e: HostError) -> Self {
        Self::new(e.code, e.message)
    }
}

impl From<Invalid> for ApiError {
    fn from((code, message): Invalid) -> Self {
        Self::new(code, message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        json_response(self.code.status(), &self.body())
    }
}

fn json_response(status: StatusCode, body: &serde_json::Value) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body.to_string(),
    )
        .into_response()
}

fn router(bridge: Arc<Bridge>) -> Router {
    Router::new()
        .route("/v1/ping", get(ping))
        .route("/v1/auth", get(auth))
        .route("/v1/pair", post(pair))
        .route("/v1/download", post(download))
        .route("/v1/batch", post(batch))
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .layer(middleware::from_fn_with_state(bridge.clone(), guard))
        .with_state(bridge)
}

/// Set by the guard: the request came from the allowed extension's origin.
#[derive(Clone, Copy)]
struct FromExtension(bool);

/// Set by the guard: the allowed extension's ID, taken from `Origin`.
#[derive(Clone)]
struct ExtensionId(Option<String>);

/// `Host` is a loopback name — so a DNS-rebinding page (Host: evil.example)
/// cannot talk to us even before the Origin and token checks.
fn loopback_host(headers: &HeaderMap) -> bool {
    let Some(host) = headers.get(header::HOST).and_then(|h| h.to_str().ok()) else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    let name = if host.starts_with('[') {
        host.split_inclusive(']').next().unwrap_or("")
    } else {
        host.split(':').next().unwrap_or("")
    };
    matches!(name, "127.0.0.1" | "localhost" | "[::1]")
}

fn with_cors(mut resp: Response, origin: &Option<HeaderValue>) -> Response {
    if let Some(o) = origin {
        let h = resp.headers_mut();
        h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, o.clone());
        h.insert(header::VARY, HeaderValue::from_static("Origin"));
    }
    resp
}

/// Host check → Origin check → CORS preflight → token (all but ping).
async fn guard(State(bridge): State<Arc<Bridge>>, mut req: Request, next: Next) -> Response {
    let headers = req.headers();
    if !loopback_host(headers) {
        return ApiError::new(ErrorCode::BadHost, "Host must be 127.0.0.1").into_response();
    }
    let origin = match headers.get(header::ORIGIN) {
        None => None,
        Some(o) => match bridge.allowed_origin(o) {
            Some(o) => Some(o),
            None => {
                return ApiError::new(ErrorCode::OriginNotAllowed, "origin not allowed")
                    .into_response()
            }
        },
    };
    if req.method() == Method::OPTIONS {
        let Some(o) = origin else {
            return ApiError::new(
                ErrorCode::OriginNotAllowed,
                "CORS preflight needs an allowed Origin",
            )
            .into_response();
        };
        let mut resp = StatusCode::NO_CONTENT.into_response();
        let h = resp.headers_mut();
        h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, o);
        h.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST"),
        );
        h.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("content-type, x-idm-token"),
        );
        h.insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static("600"),
        );
        h.insert(header::VARY, HeaderValue::from_static("Origin"));
        if req
            .headers()
            .get("access-control-request-private-network")
            .is_some_and(|v| v == "true")
        {
            h.insert(
                "access-control-allow-private-network",
                HeaderValue::from_static("true"),
            );
        }
        return resp;
    }
    let is_ping = req.method() == Method::GET && req.uri().path() == "/v1/ping";
    // Pairing is how the extension GETS the token, so it can't require one;
    // instead it requires the allowed extension Origin and the user's OK.
    let is_pair = req.method() == Method::POST && req.uri().path() == "/v1/pair";
    if is_pair && origin.is_none() {
        return ApiError::new(
            ErrorCode::OriginNotAllowed,
            "pairing is only available to the IDM Replica browser extension",
        )
        .into_response();
    }
    if !is_ping && !is_pair && !bridge.token_ok(req.headers().get(TOKEN_HEADER)) {
        let resp =
            ApiError::new(ErrorCode::BadToken, "missing or invalid X-IDM-Token").into_response();
        return with_cors(resp, &origin);
    }
    req.extensions_mut().insert(FromExtension(origin.is_some()));
    let extension_id = origin
        .as_ref()
        .and_then(|o| o.to_str().ok())
        .and_then(|o| o.strip_prefix("chrome-extension://"))
        .map(str::to_string);
    req.extensions_mut().insert(ExtensionId(extension_id));
    let resp = next.run(req).await;
    with_cors(resp, &origin)
}

async fn ping() -> Response {
    json_response(
        StatusCode::OK,
        &json!({
            "app": "idm-replica",
            "version": env!("CARGO_PKG_VERSION"),
            "bridge": PROTOCOL_VERSION,
        }),
    )
}

/// One-click pairing. The ID is the Origin's, never anything in the body.
async fn pair(
    State(bridge): State<Arc<Bridge>>,
    Extension(ExtensionId(id)): Extension<ExtensionId>,
    headers: HeaderMap,
) -> Response {
    let Some(id) = id else {
        return ApiError::new(ErrorCode::OriginNotAllowed, "origin not allowed").into_response();
    };
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let browser = if ua.contains("Edg/") {
        "Microsoft Edge"
    } else if ua.contains("Chrome/") {
        "Chrome"
    } else {
        "browser"
    };
    match bridge.pair(id, browser).await {
        Ok(token) => json_response(StatusCode::OK, &json!({ "ok": true, "token": token })),
        Err(e) => ApiError::from(e).into_response(),
    }
}

/// Token check without side effects: 204 when the guard let it through.
async fn auth() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

async fn not_found() -> Response {
    ApiError::new(ErrorCode::NotFound, "no such endpoint").into_response()
}

/// JSON content type + the body (size-limited by `DefaultBodyLimit`).
fn json_body(headers: &HeaderMap, body: Result<Bytes, BytesRejection>) -> Result<Bytes, ApiError> {
    let is_json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(';')
                .next()
                .is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json"))
        });
    if !is_json {
        return Err(ApiError::new(
            ErrorCode::UnsupportedMediaType,
            "Content-Type must be application/json",
        ));
    }
    body.map_err(|r| {
        if r.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::new(
                ErrorCode::PayloadTooLarge,
                format!("request body is larger than {} KB", MAX_BODY / 1024),
            )
        } else {
            ApiError::new(ErrorCode::BadRequest, "could not read the request body")
        }
    })
}

fn parse<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, ApiError> {
    // serde's message names the field / type problem; it is returned to the
    // caller only and never logged.
    serde_json::from_slice(body)
        .map_err(|e| ApiError::new(ErrorCode::BadRequest, format!("invalid request: {e}")))
}

async fn download(
    State(bridge): State<Arc<Bridge>>,
    Extension(FromExtension(from_extension)): Extension<FromExtension>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let result = async {
        let body = json_body(&headers, body)?;
        let job = parse::<DownloadRequest>(&body)?.validate()?;
        Ok::<_, ApiError>(bridge.submit(job, from_extension).await?)
    }
    .await;
    match result {
        Ok(a) => json_response(StatusCode::OK, &a.json()),
        Err(e) => e.into_response(),
    }
}

async fn batch(
    State(bridge): State<Arc<Bridge>>,
    Extension(FromExtension(from_extension)): Extension<FromExtension>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let jobs = (|| {
        let body = json_body(&headers, body)?;
        let req = parse::<BatchRequest>(&body)?;
        if req.items.is_empty() || req.items.len() > MAX_BATCH {
            return Err(ApiError::new(
                ErrorCode::BadRequest,
                format!("items must hold 1–{MAX_BATCH} downloads"),
            ));
        }
        // All-or-nothing validation: a bad item rejects the whole batch.
        req.items
            .into_iter()
            .enumerate()
            .map(|(i, item)| {
                item.validate()
                    .map_err(|(code, m)| ApiError::new(code, format!("items[{i}]: {m}")))
            })
            .collect::<Result<Vec<_>, _>>()
    })();
    let jobs = match jobs {
        Ok(j) => j,
        Err(e) => return e.into_response(),
    };
    let mut results = Vec::with_capacity(jobs.len());
    let mut all_ok = true;
    for job in jobs {
        match bridge.submit(job, from_extension).await {
            Ok(a) => results.push(a.json()),
            Err(e) => {
                all_ok = false;
                let mut v = ApiError::from(e).body();
                v["queued"] = json!(false);
                results.push(v);
            }
        }
    }
    json_response(StatusCode::OK, &json!({ "ok": all_ok, "results": results }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{History, JobKind, JobRecord, JobStatus, MemoryStore};
    use crate::queue::Priority;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const EXT: &str = "abcdefghijklmnopabcdefghijklmnop";
    const SECRET_COOKIE: &str = "sid=TOP-SECRET-COOKIE-VALUE";

    #[derive(Default)]
    struct FakeHost {
        started: Mutex<Vec<(String, BridgeJob)>>,
        pending: Mutex<Vec<PendingRequest>>,
        removed: Mutex<Vec<String>>,
        statuses: Mutex<Vec<BridgeInfo>>,
        fail: AtomicBool,
    }

    #[async_trait]
    impl BridgeHost for FakeHost {
        async fn start(&self, id: String, job: BridgeJob) -> Result<String, HostError> {
            if self.fail.load(Ordering::SeqCst) {
                return Err(HostError::new(
                    ErrorCode::YtdlpMissing,
                    "yt-dlp is not installed",
                ));
            }
            let name = job.display_name();
            lock(&self.started).push((id, job));
            Ok(name)
        }
        fn pending_added(&self, req: &PendingRequest) {
            lock(&self.pending).push(req.clone());
        }
        fn pending_removed(&self, id: &str) {
            lock(&self.removed).push(id.to_string());
        }
        fn status_changed(&self, info: &BridgeInfo) {
            lock(&self.statuses).push(info.clone());
        }
        fn pairing_requested(&self, _prompt: &PairingPrompt) {}
        fn pairing_closed(&self, _id: &str) {}
    }

    fn settings(ids: &[&str], ask: bool) -> BridgeSettings {
        BridgeSettings {
            enabled: true,
            allowed_extension_ids: ids.iter().map(|s| s.to_string()).collect(),
            ask_before_start: ask,
            confirm_takeover: false,
        }
    }

    /// A running bridge on an OS-chosen port, with a fake host.
    async fn serve(s: BridgeSettings) -> (Arc<Bridge>, Arc<FakeHost>, String) {
        let host = Arc::new(FakeHost::default());
        let bridge = Bridge::new(host.clone(), None, 0, s, TOKEN.into(), None);
        bridge.ensure_running().await;
        let port = bridge.info().port.expect("bridge should listen");
        (bridge, host, format!("http://127.0.0.1:{port}"))
    }

    fn client() -> reqwest::Client {
        reqwest::Client::new()
    }

    fn post(url: &str, body: &serde_json::Value) -> reqwest::RequestBuilder {
        client()
            .post(url)
            .header("content-type", "application/json")
            .body(body.to_string())
    }

    async fn status_and_json(r: reqwest::RequestBuilder) -> (u16, serde_json::Value) {
        let resp = r.send().await.unwrap();
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(json!(text)))
    }

    #[tokio::test]
    async fn ping_needs_no_token() {
        let (_b, _h, base) = serve(settings(&[], false)).await;
        let (status, body) = status_and_json(client().get(format!("{base}/v1/ping"))).await;
        assert_eq!(status, 200);
        assert_eq!(body["app"], "idm-replica");
        assert_eq!(body["bridge"], 1);
        assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn missing_or_wrong_token_is_401() {
        let (_b, host, base) = serve(settings(&[], false)).await;
        let body = json!({ "url": "https://example.com/a.zip" });
        let url = format!("{base}/v1/download");
        let (s, _) = status_and_json(post(&url, &body)).await;
        assert_eq!(s, 401);
        let wrong = TOKEN.replace('0', "1");
        let (s, _) = status_and_json(post(&url, &body).header(TOKEN_HEADER, wrong)).await;
        assert_eq!(s, 401);
        let (s, _) = status_and_json(post(&url, &body).header(TOKEN_HEADER, "short")).await;
        assert_eq!(s, 401);
        assert!(lock(&host.started).is_empty());
    }

    #[tokio::test]
    async fn bad_origin_is_403_and_cors_answers_only_the_extension() {
        let (_b, host, base) = serve(settings(&[EXT], false)).await;
        let url = format!("{base}/v1/download");
        let body = json!({ "url": "https://example.com/a.zip" });
        for origin in [
            "https://evil.example",
            "null",
            "chrome-extension://bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        ] {
            let (s, _) = status_and_json(
                post(&url, &body)
                    .header(TOKEN_HEADER, TOKEN)
                    .header("origin", origin),
            )
            .await;
            assert_eq!(s, 403, "origin {origin}");
            // Even ping refuses foreign origins (no app fingerprinting).
            let (s, _) = status_and_json(
                client()
                    .get(format!("{base}/v1/ping"))
                    .header("origin", origin),
            )
            .await;
            assert_eq!(s, 403, "ping from {origin}");
        }
        assert!(lock(&host.started).is_empty());

        // Allowed extension: request goes through, CORS echoes that origin.
        let ext_origin = format!("chrome-extension://{EXT}");
        let resp = post(&url, &body)
            .header(TOKEN_HEADER, TOKEN)
            .header("origin", &ext_origin)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        assert_eq!(
            resp.headers()["access-control-allow-origin"]
                .to_str()
                .unwrap(),
            ext_origin
        );

        // Preflight: allowed origin → 204 with exact origin; others → 403.
        let pre = |origin: &str| {
            client()
                .request(reqwest::Method::OPTIONS, &url)
                .header("origin", origin)
                .header("access-control-request-method", "POST")
                .header("access-control-request-headers", "content-type,x-idm-token")
        };
        let ok = pre(&ext_origin).send().await.unwrap();
        assert_eq!(ok.status().as_u16(), 204);
        let acao = ok.headers()["access-control-allow-origin"]
            .to_str()
            .unwrap();
        assert_eq!(acao, ext_origin);
        assert_ne!(acao, "*");
        let bad = pre("https://evil.example").send().await.unwrap();
        assert_eq!(bad.status().as_u16(), 403);
        assert!(bad.headers().get("access-control-allow-origin").is_none());
    }

    #[tokio::test]
    async fn empty_allow_list_rejects_every_origin_but_allows_none() {
        let (_b, _h, base) = serve(settings(&[], false)).await;
        let url = format!("{base}/v1/download");
        let body = json!({ "url": "https://example.com/a.zip" });
        let (s, _) = status_and_json(
            post(&url, &body)
                .header(TOKEN_HEADER, TOKEN)
                .header("origin", format!("chrome-extension://{EXT}")),
        )
        .await;
        assert_eq!(s, 403);
        let (s, _) = status_and_json(post(&url, &body).header(TOKEN_HEADER, TOKEN)).await;
        assert_eq!(s, 200);
    }

    #[tokio::test]
    async fn non_http_urls_are_400() {
        let (_b, host, base) = serve(settings(&[], false)).await;
        let url = format!("{base}/v1/download");
        for bad in [
            "file:///C:/Windows/win.ini",
            "javascript:alert(1)",
            "data:text/plain,hi",
            "ftp://example.com/a",
            "blob:https://example.com/123",
            "not a url",
            "",
        ] {
            let (s, body) =
                status_and_json(post(&url, &json!({ "url": bad })).header(TOKEN_HEADER, TOKEN))
                    .await;
            assert_eq!(s, 400, "{bad}: {body}");
            assert_eq!(body["ok"], false);
        }
        let (s, _) = status_and_json(
            post(
                &url,
                &json!({ "url": "https://example.com/a", "referer": "javascript:x" }),
            )
            .header(TOKEN_HEADER, TOKEN),
        )
        .await;
        assert_eq!(s, 400);
        assert!(lock(&host.started).is_empty());
    }

    #[tokio::test]
    async fn oversize_body_is_413() {
        let (_b, host, base) = serve(settings(&[], false)).await;
        let big = "a".repeat(MAX_BODY + 1);
        let (s, _) = status_and_json(
            post(
                &format!("{base}/v1/download"),
                &json!({ "url": format!("https://example.com/{big}") }),
            )
            .header(TOKEN_HEADER, TOKEN),
        )
        .await;
        assert_eq!(s, 413);
        assert!(lock(&host.started).is_empty());
    }

    #[tokio::test]
    async fn wrong_content_type_unknown_fields_and_bad_host_are_refused() {
        let (_b, _h, base) = serve(settings(&[], false)).await;
        let url = format!("{base}/v1/download");
        let (s, _) = status_and_json(
            client()
                .post(&url)
                .header(TOKEN_HEADER, TOKEN)
                .header("content-type", "text/plain")
                .body(json!({ "url": "https://example.com/a" }).to_string()),
        )
        .await;
        assert_eq!(s, 415);
        let (s, _) = status_and_json(
            post(&url, &json!({ "url": "https://example.com/a", "evil": 1 }))
                .header(TOKEN_HEADER, TOKEN),
        )
        .await;
        assert_eq!(s, 400);

        // DNS rebinding: right socket, foreign Host header.
        let addr = base.trim_start_matches("http://");
        let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
        sock.write_all(b"GET /v1/ping HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut out = String::new();
        sock.read_to_string(&mut out).await.unwrap();
        assert!(out.starts_with("HTTP/1.1 403"), "{out}");
    }

    #[tokio::test]
    async fn valid_request_reaches_host_with_headers_and_safe_name() {
        let (_b, host, base) = serve(settings(&[], false)).await;
        let (s, body) = status_and_json(
            post(
                &format!("{base}/v1/download"),
                &json!({
                    "url": "https://cdn.example.com/files/get?id=7",
                    "pageUrl": "https://example.com/page",
                    "referer": "https://example.com/page",
                    "userAgent": "Mozilla/5.0 Test",
                    "cookies": SECRET_COOKIE,
                    "headers": { "X-Custom": "1", "Cookie": "ignored=1" },
                    "fileName": "..\\..\\Windows\\evil<>.exe",
                    "kind": "auto",
                    "mediaType": "application/zip",
                    "size": 1234
                }),
            )
            .header(TOKEN_HEADER, TOKEN),
        )
        .await;
        assert_eq!(s, 200, "{body}");
        assert_eq!(body["ok"], true);
        assert_eq!(body["queued"], true);
        assert_eq!(body["fileName"], "evil__.exe");

        let started = lock(&host.started);
        assert_eq!(started.len(), 1);
        let (id, job) = &started[0];
        assert_eq!(body["id"], id.as_str());
        assert!(!job.stream);
        assert_eq!(job.url, "https://cdn.example.com/files/get?id=7");
        assert_eq!(job.name_hint.as_deref(), Some("evil__.exe"));
        assert_eq!(
            job.headers,
            vec![
                ("X-Custom".to_string(), "1".to_string()),
                (
                    "Referer".to_string(),
                    "https://example.com/page".to_string()
                ),
                ("User-Agent".to_string(), "Mozilla/5.0 Test".to_string()),
                ("Cookie".to_string(), SECRET_COOKIE.to_string()),
            ]
        );
        assert_eq!(job.size, Some(1234));
        // Debug output never shows header values.
        assert!(!format!("{job:?}").contains("TOP-SECRET"));
    }

    #[test]
    fn kind_resolution() {
        let req = |v: serde_json::Value| serde_json::from_value::<DownloadRequest>(v).unwrap();
        let yt = "https://www.youtube.com/watch?v=abc";
        let j = req(json!({ "url": yt })).validate().unwrap();
        assert!(j.stream);
        let j = req(json!({ "pageUrl": yt })).validate().unwrap();
        assert!(j.stream);
        assert_eq!(j.url, yt);
        let j = req(json!({ "url": "https://x.example/f.bin", "pageUrl": yt, "kind": "stream" }))
            .validate()
            .unwrap();
        assert!(j.stream);
        assert_eq!(j.url, yt);
        let j = req(json!({ "url": yt, "kind": "file" }))
            .validate()
            .unwrap();
        assert!(!j.stream);
        assert!(req(json!({ "pageUrl": "https://plain.example/" }))
            .validate()
            .is_err());
        // A direct manifest is a stream (yt-dlp downloads it), even on a plain site.
        let j = req(json!({ "url": "https://plain.example/v.m3u8", "kind": "stream" }))
            .validate()
            .unwrap();
        assert!(j.stream && j.manifest);
        // ...but a plain page is not.
        assert_eq!(
            req(json!({ "url": "https://plain.example/page", "kind": "stream" }))
                .validate()
                .err()
                .map(|e| e.0),
            Some(ErrorCode::UnsupportedSite)
        );
        assert!(req(json!({})).validate().is_err());
        // Unusable hint ⇒ ignored (the engine names the file).
        let j = req(json!({ "url": "https://x.example/f.bin", "fileName": "CON" }))
            .validate()
            .unwrap();
        assert_eq!(j.name_hint, None);
        // Forbidden / malformed headers.
        for h in [
            json!({ "Host": "x" }),
            json!({ "Range": "bytes=0-" }),
            json!({ "X-IDM-Token": "x" }),
            json!({ "Sec-Fetch-Site": "x" }),
            json!({ "Bad Name": "x" }),
            json!({ "X-Ok": "line\r\nInjected: 1" }),
        ] {
            assert!(
                req(json!({ "url": "https://x.example/", "headers": h }))
                    .validate()
                    .is_err(),
                "{h}"
            );
        }
        assert!(
            req(json!({ "url": "https://x.example/", "cookies": "a=1\r\nX: y" }))
                .validate()
                .is_err()
        );
    }

    #[tokio::test]
    async fn ask_before_start_queues_pending_until_accepted() {
        let (bridge, host, base) = serve(settings(&[], true)).await;
        let url = format!("{base}/v1/download");
        let (s, body) = status_and_json(
            post(
                &url,
                &json!({ "url": "https://example.com/a.zip", "cookies": SECRET_COOKIE }),
            )
            .header(TOKEN_HEADER, TOKEN),
        )
        .await;
        assert_eq!(s, 200);
        assert_eq!(body["queued"], false);
        let id = body["id"].as_str().unwrap().to_string();
        assert!(lock(&host.started).is_empty());
        let pending = bridge.pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].status, "pending-confirm");
        assert!(pending[0].has_cookies);
        // The UI-facing record holds no cookie value.
        assert!(!serde_json::to_string(&pending)
            .unwrap()
            .contains("TOP-SECRET"));
        assert_eq!(lock(&host.pending).len(), 1);

        bridge.accept(&id).await.unwrap();
        assert!(bridge.pending().is_empty());
        assert_eq!(*lock(&host.removed), vec![id.clone()]);
        {
            let started = lock(&host.started);
            assert_eq!(started[0].0, id);
            assert!(started[0].1.has_cookies());
        }
        assert!(bridge.accept(&id).await.is_err());

        // Dismiss drops it without starting.
        let (_, body) = status_and_json(
            post(&url, &json!({ "url": "https://example.com/b.zip" })).header(TOKEN_HEADER, TOKEN),
        )
        .await;
        let id2 = body["id"].as_str().unwrap();
        bridge.dismiss(id2).unwrap();
        assert!(bridge.pending().is_empty());
        assert_eq!(lock(&host.started).len(), 1);
    }

    #[tokio::test]
    async fn batch_validates_all_then_submits_each() {
        let (_b, host, base) = serve(settings(&[], false)).await;
        let url = format!("{base}/v1/batch");
        let (s, body) = status_and_json(
            post(
                &url,
                &json!({ "items": [
                    { "url": "https://example.com/1.bin" },
                    { "url": "file:///etc/passwd" }
                ]}),
            )
            .header(TOKEN_HEADER, TOKEN),
        )
        .await;
        assert_eq!(s, 400);
        assert!(body["error"].as_str().unwrap().starts_with("items[1]"));
        assert!(lock(&host.started).is_empty());

        let (s, body) = status_and_json(
            post(
                &url,
                &json!({ "items": [
                    { "url": "https://example.com/1.bin" },
                    { "url": "https://example.com/2.bin", "fileName": "two.bin" }
                ]}),
            )
            .header(TOKEN_HEADER, TOKEN),
        )
        .await;
        assert_eq!(s, 200, "{body}");
        assert_eq!(body["ok"], true);
        assert_eq!(body["results"][1]["fileName"], "two.bin");
        assert_eq!(lock(&host.started).len(), 2);

        let (s, _) =
            status_and_json(post(&url, &json!({ "items": [] })).header(TOKEN_HEADER, TOKEN)).await;
        assert_eq!(s, 400);

        // A host failure is reported per item.
        host.fail.store(true, Ordering::SeqCst);
        let (s, body) = status_and_json(
            post(
                &url,
                &json!({ "items": [{ "url": "https://example.com/3.bin" }] }),
            )
            .header(TOKEN_HEADER, TOKEN),
        )
        .await;
        assert_eq!(s, 200);
        assert_eq!(body["ok"], false);
        assert_eq!(body["results"][0]["error"], "yt-dlp is not installed");
    }

    #[tokio::test]
    async fn port_fallback_skips_a_taken_port() {
        // Below the OS ephemeral range, where parallel tests' sockets live.
        let seed = Uuid::new_v4().as_u128();
        let (_blocker, taken) = (0..50u16)
            .find_map(|i| {
                let port = 20_000 + ((seed as u16).wrapping_add(i * 97) % 20_000);
                std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port))
                    .ok()
                    .map(|l| (l, port))
            })
            .expect("a free port below 40000");
        let (_l, got) = bind_with_fallback(taken, 10).await.unwrap();
        assert_ne!(got, taken);
        assert!(got > taken && got <= taken.saturating_add(10));
        // No fallback room ⇒ a clear error, not a panic.
        let err = bind_with_fallback(taken, 0).await.unwrap_err().to_string();
        assert!(err.contains("in use"), "{err}");
    }

    #[tokio::test]
    async fn disabling_stops_listening() {
        let (bridge, host, base) = serve(settings(&[], false)).await;
        let info = bridge
            .set_settings(BridgeSettings {
                enabled: false,
                ..bridge.info().settings
            })
            .await
            .unwrap();
        assert!(!info.listening);
        assert!(client()
            .get(format!("{base}/v1/ping"))
            .send()
            .await
            .is_err());
        assert!(!lock(&host.statuses).last().unwrap().listening);
    }

    #[tokio::test]
    async fn settings_and_token_persist_and_validate() {
        let dir = crate::testutil::temp_dir("bridge_cfg");
        let path = dir.join(CONFIG_FILE);
        let host: Arc<dyn BridgeHost> = Arc::new(FakeHost::default());

        let b = Bridge::open(host.clone(), path.clone(), 0);
        let token = b.info().token;
        assert!(is_token(&token));
        let disabled = BridgeSettings {
            enabled: false,
            allowed_extension_ids: vec![format!(" {} ", EXT.to_uppercase())],
            ask_before_start: false,
            confirm_takeover: false,
        };
        let info = b.set_settings(disabled).await.unwrap();
        assert_eq!(info.settings.allowed_extension_ids, vec![EXT.to_string()]);
        assert!(b
            .set_settings(settings(&["not-an-id"], true))
            .await
            .is_err());

        let again = Bridge::open(host.clone(), path.clone(), 0);
        assert_eq!(again.info().token, token);
        assert_eq!(
            again.info().settings.allowed_extension_ids,
            vec![EXT.to_string()]
        );
        assert!(!again.info().settings.ask_before_start);

        let regenerated = again.regenerate_token().unwrap().token;
        assert_ne!(regenerated, token);
        assert_eq!(
            Bridge::open(host.clone(), path.clone(), 0).info().token,
            regenerated
        );

        // Corrupt file ⇒ defaults and a fresh token, no panic.
        std::fs::write(&path, b"{ nope").unwrap();
        let fresh = Bridge::open(host, path.clone(), 0).info();
        assert!(is_token(&fresh.token));
        assert_ne!(fresh.token, regenerated);
        assert_eq!(fresh.settings, BridgeSettings::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            lock(&self.0).extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
        type Writer = Capture;
        fn make_writer(&'a self) -> Capture {
            self.clone()
        }
    }

    /// Runs on a current-thread runtime, so the server's tasks log through
    /// this thread's subscriber.
    #[tokio::test]
    async fn cookies_and_token_never_reach_logs_or_persisted_records() {
        let cap = Capture::default();
        let sub = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new("idm_replica=trace"))
            .with_writer(cap.clone())
            .with_ansi(false)
            .finish();
        let _guard = tracing::subscriber::set_default(sub);

        let (_b, host, base) = serve(settings(&[], false)).await;
        // tracing caches per-callsite interest process-wide; a parallel test
        // without a subscriber can race our `set_default` and cache "never".
        // Rebuild the cache and retry until our line is captured.
        let mut logs = String::new();
        for attempt in 0..3 {
            tracing::callsite::rebuild_interest_cache();
            let (s, _) = status_and_json(
                post(
                    &format!("{base}/v1/download"),
                    &json!({
                        "url": format!("https://example.com/a.zip?sig=QUERY-SECRET&try={attempt}"),
                        "cookies": SECRET_COOKIE,
                        "headers": { "Authorization": "Bearer AUTH-SECRET" }
                    }),
                )
                .header(TOKEN_HEADER, TOKEN),
            )
            .await;
            assert_eq!(s, 200);
            logs = String::from_utf8(lock(&cap.0).clone()).unwrap();
            if logs.contains("download request from the browser") {
                break;
            }
        }
        assert!(logs.contains("download request from the browser"), "{logs}");
        for secret in ["TOP-SECRET", "AUTH-SECRET", "QUERY-SECRET", TOKEN] {
            assert!(!logs.contains(secret), "{secret} leaked into logs: {logs}");
        }

        // The persisted history record keeps header names only.
        let (id, job) = lock(&host.started)[0].clone();
        let store = Arc::new(MemoryStore::default());
        let (history, _) = History::open(store.clone(), 1);
        let rec = JobRecord {
            id: id.clone(),
            kind: JobKind::File,
            url: job.url.clone(),
            mirrors: vec![],
            header_names: vec![],
            dir: PathBuf::from("."),
            dest: None,
            file_name: job.display_name(),
            status: JobStatus::Queued,
            total: 0,
            downloaded: 0,
            connections: 4,
            speed_limit: 0,
            verify: false,
            expected_checksum: None,
            start_at: None,
            priority: Priority::Normal,
            held: false,
            quality: None,
            created_at: 1,
            updated_at: 1,
            error: None,
        };
        history.insert(rec, job.headers.clone());
        history.flush().unwrap();
        let saved = serde_json::to_string(&*lock(&store.state)).unwrap();
        assert!(saved.contains("Cookie"), "{saved}");
        assert!(!saved.contains("TOP-SECRET"), "{saved}");
        assert!(!saved.contains("AUTH-SECRET"), "{saved}");
        // ...while the session still has them for this job.
        assert_eq!(history.headers(&id).unwrap(), job.headers);
    }
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const EXT: &str = "abcdefghijklmnopabcdefghijklmnop";

    #[derive(Default)]
    struct Host {
        started: Mutex<Vec<BridgeJob>>,
        pending: Mutex<Vec<PendingRequest>>,
        fail: Mutex<Option<HostError>>,
        prompts: Mutex<Vec<PairingPrompt>>,
        closed: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl BridgeHost for Host {
        async fn start(&self, _id: String, job: BridgeJob) -> Result<String, HostError> {
            if let Some(e) = lock(&self.fail).clone() {
                return Err(e);
            }
            let name = job.display_name();
            lock(&self.started).push(job);
            Ok(name)
        }
        fn pending_added(&self, req: &PendingRequest) {
            lock(&self.pending).push(req.clone());
        }
        fn pending_removed(&self, _id: &str) {}
        fn status_changed(&self, _info: &BridgeInfo) {}
        fn pairing_requested(&self, prompt: &PairingPrompt) {
            lock(&self.prompts).push(prompt.clone());
        }
        fn pairing_closed(&self, id: &str) {
            lock(&self.closed).push(id.to_string());
        }
    }

    async fn serve(ask: bool, confirm_takeover: bool) -> (Arc<Bridge>, Arc<Host>, String) {
        let host = Arc::new(Host::default());
        let settings = BridgeSettings {
            enabled: true,
            allowed_extension_ids: vec![EXT.into()],
            ask_before_start: ask,
            confirm_takeover,
        };
        let bridge = Bridge::new(host.clone(), None, 0, settings, TOKEN.into(), None);
        bridge.ensure_running().await;
        let port = bridge.info().port.expect("listening");
        (bridge, host, format!("http://127.0.0.1:{port}"))
    }

    /// POST /v1/download; `ext` = send the allowed extension Origin.
    async fn send(base: &str, body: serde_json::Value, ext: bool) -> (u16, serde_json::Value) {
        let mut r = reqwest::Client::new()
            .post(format!("{base}/v1/download"))
            .header("content-type", "application/json")
            .header(TOKEN_HEADER, TOKEN)
            .body(body.to_string());
        if ext {
            r = r.header("origin", format!("chrome-extension://{EXT}"));
        }
        let resp = r.send().await.unwrap();
        let status = resp.status().as_u16();
        (
            status,
            serde_json::from_str(&resp.text().await.unwrap()).unwrap_or(json!(null)),
        )
    }

    async fn code_of(r: reqwest::Response) -> serde_json::Value {
        serde_json::from_str::<serde_json::Value>(&r.text().await.unwrap()).unwrap()["code"].clone()
    }

    #[tokio::test]
    async fn every_error_has_a_stable_code_and_message() {
        let (_b, _h, base) = serve(false, false).await;
        let client = reqwest::Client::new();
        // bad_token
        let r = client.get(format!("{base}/v1/auth")).send().await.unwrap();
        assert_eq!(r.status().as_u16(), 401);
        let v: serde_json::Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
        assert_eq!(v["ok"], json!(false));
        assert_eq!(v["code"], json!("bad_token"));
        assert!(v["message"].as_str().is_some_and(|m| !m.is_empty()));
        // origin_not_allowed
        let r = client
            .get(format!("{base}/v1/ping"))
            .header("origin", "https://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(code_of(r).await, "origin_not_allowed");
        // bad_url, unsupported_site, bad_request
        for (body, code) in [
            (json!({ "url": "file:///etc/passwd" }), "bad_url"),
            (json!({}), "bad_url"),
            (
                json!({ "url": "https://plain.example/page", "kind": "stream" }),
                "unsupported_site",
            ),
            (
                json!({ "url": "https://x.example/a", "quality": "8k" }),
                "bad_request",
            ),
            (
                json!({ "url": "https://x.example/a", "source": "hacker" }),
                "bad_request",
            ),
            (
                json!({ "url": "https://x.example/a", "nope": 1 }),
                "bad_request",
            ),
        ] {
            let (s, v) = send(&base, body.clone(), false).await;
            assert_eq!(v["code"], code, "{body} -> {s} {v}");
            assert_eq!(s, 400);
        }
        // unsupported_media_type, payload_too_large, not_found
        let r = client
            .post(format!("{base}/v1/download"))
            .header(TOKEN_HEADER, TOKEN)
            .header("content-type", "text/plain")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(code_of(r).await, "unsupported_media_type");
        let big = json!({ "url": format!("https://x.example/{}", "a".repeat(MAX_BODY)) });
        let (s, v) = send(&base, big, false).await;
        assert_eq!(s, 413);
        assert_eq!(v["code"], "payload_too_large");
        let r = client
            .get(format!("{base}/v1/nope"))
            .header(TOKEN_HEADER, TOKEN)
            .send()
            .await
            .unwrap();
        assert_eq!(code_of(r).await, "not_found");
    }

    #[tokio::test]
    async fn host_errors_keep_their_code() {
        let (_b, host, base) = serve(false, false).await;
        *lock(&host.fail) = Some(HostError::new(ErrorCode::YtdlpMissing, "install yt-dlp"));
        let yt = json!({ "url": "https://www.youtube.com/watch?v=abcdefghijk" });
        let (s, v) = send(&base, yt, false).await;
        assert_eq!((s, v["code"].clone()), (503, json!("ytdlp_missing")));
        *lock(&host.fail) = Some(HostError::new(ErrorCode::FfmpegMissing, "install ffmpeg"));
        let (s, v) = send(&base, json!({ "url": "https://vimeo.com/123" }), false).await;
        assert_eq!((s, v["code"].clone()), (503, json!("ffmpeg_missing")));
        *lock(&host.fail) = Some(HostError::new(ErrorCode::JsRuntimeMissing, "no runtime"));
        let yt = json!({ "url": "https://www.youtube.com/watch?v=lmnopqrstuv" });
        let (s, v) = send(&base, yt, false).await;
        assert_eq!((s, v["code"].clone()), (503, json!("js_runtime_missing")));
    }

    #[tokio::test]
    async fn auth_endpoint_checks_the_token_without_side_effects() {
        let (_b, host, base) = serve(true, false).await;
        let ok = reqwest::Client::new()
            .get(format!("{base}/v1/auth"))
            .header(TOKEN_HEADER, TOKEN)
            .send()
            .await
            .unwrap();
        assert_eq!(ok.status().as_u16(), 204);
        assert!(lock(&host.started).is_empty() && lock(&host.pending).is_empty());
    }

    #[tokio::test]
    async fn takeover_from_the_extension_starts_at_once_even_when_asking() {
        let (_b, host, base) = serve(true, false).await;
        let body = json!({ "url": "https://x.example/a.zip", "source": "takeover" });
        let (s, v) = send(&base, body, true).await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["queued"], json!(true));
        assert_eq!(v["status"], json!("queued"));
        assert_eq!(lock(&host.started).len(), 1);
        assert_eq!(lock(&host.started)[0].source, Source::Takeover);
        // A user action still asks.
        let body = json!({ "url": "https://x.example/b.zip", "source": "user-action" });
        let (_, v) = send(&base, body, true).await;
        assert_eq!(v["status"], "pending-confirm");
        assert_eq!(lock(&host.pending)[0].source, Source::UserAction);
    }

    #[tokio::test]
    async fn takeover_without_the_extension_origin_is_not_honoured() {
        let (_b, host, base) = serve(true, false).await;
        // e.g. curl with the token but no extension Origin: a user action.
        let body = json!({ "url": "https://x.example/a.zip", "source": "takeover" });
        let (_, v) = send(&base, body, false).await;
        assert_eq!(v["status"], "pending-confirm");
        assert!(lock(&host.started).is_empty());
        assert_eq!(lock(&host.pending)[0].source, Source::UserAction);
    }

    #[tokio::test]
    async fn confirm_takeover_setting_makes_takeovers_wait() {
        let (_b, host, base) = serve(false, true).await;
        let body = json!({ "url": "https://x.example/a.zip", "source": "takeover" });
        let (_, v) = send(&base, body, true).await;
        assert_eq!(v["status"], "pending-confirm");
        assert!(lock(&host.started).is_empty());
    }

    #[tokio::test]
    async fn takeover_rate_limit_queues_the_excess_as_pending() {
        let (_b, host, base) = serve(true, false).await;
        for i in 0..(TAKEOVER_BURST + 3) {
            let body = json!({ "url": format!("https://x.example/{i}.bin"), "source": "takeover" });
            let (s, _) = send(&base, body, true).await;
            assert_eq!(s, 200);
        }
        assert_eq!(lock(&host.started).len(), TAKEOVER_BURST);
        assert_eq!(lock(&host.pending).len(), 3);
        assert!(lock(&host.pending)
            .iter()
            .all(|p| p.source == Source::Takeover));
    }

    #[tokio::test]
    async fn the_same_url_within_seconds_is_answered_once() {
        let (_b, host, base) = serve(false, false).await;
        let body = json!({ "url": "https://x.example/same.zip", "source": "takeover" });
        let (_, a) = send(&base, body.clone(), true).await;
        let (_, b) = send(&base, body, true).await;
        assert_eq!(a["id"], b["id"]);
        assert_eq!(a["duplicate"], json!(false));
        assert_eq!(b["duplicate"], json!(true));
        assert_eq!(lock(&host.started).len(), 1);
    }

    #[tokio::test]
    async fn non_takeover_pending_cards_are_capped() {
        let (_b, host, base) = serve(true, false).await;
        for i in 0..MAX_PENDING_NON_TAKEOVER {
            let body = json!({ "url": format!("https://x.example/{i}"), "source": "page-scan" });
            let (s, _) = send(&base, body, true).await;
            assert_eq!(s, 200);
        }
        let (s, v) = send(&base, json!({ "url": "https://x.example/one-more" }), true).await;
        assert_eq!((s, v["code"].clone()), (429, json!("app_busy")));
        // Take-overs still get through (they have their own limit).
        let body = json!({ "url": "https://x.example/t.zip", "source": "takeover" });
        let (s, _) = send(&base, body, true).await;
        assert_eq!(s, 200);
        assert_eq!(lock(&host.pending).len(), MAX_PENDING_NON_TAKEOVER);
    }

    #[tokio::test]
    async fn manifests_go_to_yt_dlp_with_their_quality() {
        let (_b, host, base) = serve(false, false).await;
        let (s, v) = send(
            &base,
            json!({
                "url": "https://cdn.example/live/master.m3u8",
                "pageUrl": "https://site.example/watch",
                "referer": "https://site.example/watch",
                "kind": "stream",
                "quality": "720p",
                "fileName": "ignored.mp4"
            }),
            true,
        )
        .await;
        assert_eq!(s, 200, "{v}");
        {
            let started = lock(&host.started);
            let job = &started[0];
            assert!(job.stream && job.manifest);
            assert_eq!(job.quality.as_deref(), Some("720"));
            assert_eq!(job.name_hint, None);
            assert!(job.headers.iter().any(|(k, _)| k == "Referer"));
        }
        // auto + .mpd / by media type: also a manifest, never saved as text.
        send(
            &base,
            json!({ "url": "https://cdn.example/a/manifest.mpd" }),
            true,
        )
        .await;
        let by_type = json!({
            "url": "https://cdn.example/playlist?id=1",
            "mediaType": "application/vnd.apple.mpegurl"
        });
        send(&base, by_type, true).await;
        let started = lock(&host.started);
        assert_eq!(started.len(), 3);
        assert!(started[1..].iter().all(|j| j.manifest && j.stream));
    }

    #[tokio::test]
    async fn command_line_urls_always_ask() {
        let (bridge, host, _base) = serve(false, false).await;
        bridge
            .submit_command_line("https://x.example/cli.zip")
            .await
            .unwrap();
        assert!(lock(&host.started).is_empty());
        assert_eq!(lock(&host.pending)[0].source, Source::CommandLine);
        let e = bridge
            .submit_command_line("file:///etc/passwd")
            .await
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::BadUrl);
    }

    // ---- One-click pairing (POST /v1/pair) --------------------------------

    fn pair_req(base: &str, origin: Option<&str>) -> reqwest::RequestBuilder {
        let mut r = reqwest::Client::new()
            .post(format!("{base}/v1/pair"))
            .header("user-agent", "Mozilla/5.0 Chrome/141.0 Safari/537.36")
            // A body claiming another ID must be irrelevant.
            .header("content-type", "application/json")
            .body(r#"{"extensionId":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#);
        if let Some(o) = origin {
            r = r.header("origin", o);
        }
        r
    }

    async fn body_of(r: reqwest::Response) -> (u16, String) {
        (r.status().as_u16(), r.text().await.unwrap())
    }

    /// Wait until the host was asked to show a prompt, then return it.
    async fn next_prompt(host: &Host, n: usize) -> PairingPrompt {
        for _ in 0..200 {
            if let Some(p) = lock(&host.prompts).get(n).cloned() {
                return p;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("no pairing prompt #{n}");
    }

    #[tokio::test]
    async fn pairing_needs_the_extension_origin() {
        let (bridge, host, base) = serve(true, false).await;
        // No Origin at all (curl, another local program).
        let (s, body) = body_of(pair_req(&base, None).send().await.unwrap()).await;
        assert_eq!(s, 403, "{body}");
        assert!(body.contains("origin_not_allowed") && !body.contains(TOKEN));
        // A web page, a spoofed scheme, an extension that isn't allowed.
        for origin in [
            "https://evil.example",
            "http://127.0.0.1:8080",
            "null",
            "chrome-extension://bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "chrome-extension://",
        ] {
            let (s, body) = body_of(pair_req(&base, Some(origin)).send().await.unwrap()).await;
            assert_eq!(s, 403, "{origin}: {body}");
            assert!(!body.contains(TOKEN), "{origin} leaked the token");
        }
        assert!(lock(&host.prompts).is_empty(), "no prompt for a bad origin");
        assert!(bridge.pending_pairing().is_none());
    }

    #[tokio::test]
    async fn allow_returns_the_token_and_the_id_comes_from_the_origin() {
        let (bridge, host, base) = serve(true, false).await;
        let origin = format!("chrome-extension://{EXT}");
        let req = tokio::spawn(pair_req(&base, Some(&origin)).send());
        let prompt = next_prompt(&host, 0).await;
        assert_eq!(
            prompt.extension_id, EXT,
            "ID from Origin, not from the body"
        );
        assert_eq!(prompt.browser, "Chrome");
        assert_eq!(bridge.pending_pairing(), Some(prompt.clone()));
        bridge.answer_pairing(&prompt.id, true).unwrap();
        let (s, body) = body_of(req.await.unwrap().unwrap()).await;
        assert_eq!(s, 200, "{body}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], json!(true));
        assert_eq!(v["token"], json!(TOKEN));
        assert!(bridge.pending_pairing().is_none());
        assert!(lock(&host.closed).contains(&prompt.id));
        assert!(bridge.answer_pairing(&prompt.id, true).is_err());
    }

    #[tokio::test]
    async fn the_shipped_extension_id_works_with_an_empty_allow_list() {
        let host = Arc::new(Host::default());
        let bridge = Bridge::new(
            host.clone(),
            None,
            0,
            BridgeSettings::default(),
            TOKEN.into(),
            None,
        );
        bridge.ensure_running().await;
        let base = format!("http://127.0.0.1:{}", bridge.info().port.unwrap());
        assert!(bridge
            .info()
            .shipped_extension_ids
            .contains(&DEV_EXTENSION_ID.to_string()));
        let origin = format!("chrome-extension://{DEV_EXTENSION_ID}");
        let req = tokio::spawn(pair_req(&base, Some(&origin)).send());
        let prompt = next_prompt(&host, 0).await;
        bridge.answer_pairing(&prompt.id, true).unwrap();
        assert_eq!(req.await.unwrap().unwrap().status().as_u16(), 200);
    }

    #[tokio::test]
    async fn deny_and_timeout_never_leak_the_token() {
        let (bridge, host, base) = serve(true, false).await;
        let origin = format!("chrome-extension://{EXT}");
        let req = tokio::spawn(pair_req(&base, Some(&origin)).send());
        let prompt = next_prompt(&host, 0).await;
        bridge.answer_pairing(&prompt.id, false).unwrap();
        let (s, body) = body_of(req.await.unwrap().unwrap()).await;
        assert_eq!(s, 403);
        assert!(body.contains("pairing_denied"), "{body}");
        assert!(!body.contains(TOKEN) && !body.contains("token\""), "{body}");

        // Nobody answers: denied after the timeout, prompt removed.
        bridge
            .pair_timeout_ms
            .store(150, std::sync::atomic::Ordering::Relaxed);
        let (s, body) = body_of(pair_req(&base, Some(&origin)).send().await.unwrap()).await;
        assert_eq!(s, 403);
        assert!(
            body.contains("pairing_denied") && !body.contains(TOKEN),
            "{body}"
        );
        assert!(bridge.pending_pairing().is_none());
        assert_eq!(lock(&host.closed).len(), 2);
    }

    #[tokio::test]
    async fn prompts_never_stack_and_attempts_are_rate_limited() {
        let (bridge, host, base) = serve(true, false).await;
        let origin = format!("chrome-extension://{EXT}");
        let first = tokio::spawn(pair_req(&base, Some(&origin)).send());
        let prompt = next_prompt(&host, 0).await;
        // A second request while the prompt is up: refused, no second prompt.
        let (s, body) = body_of(pair_req(&base, Some(&origin)).send().await.unwrap()).await;
        assert_eq!(s, 409);
        assert!(body.contains("pairing_pending"), "{body}");
        assert_eq!(lock(&host.prompts).len(), 1);
        bridge.answer_pairing(&prompt.id, false).unwrap();
        assert_eq!(first.await.unwrap().unwrap().status().as_u16(), 403);

        // Third attempt in the minute is still allowed to prompt…
        let third = tokio::spawn(pair_req(&base, Some(&origin)).send());
        let p3 = next_prompt(&host, 1).await;
        bridge.answer_pairing(&p3.id, false).unwrap();
        assert_eq!(third.await.unwrap().unwrap().status().as_u16(), 403);
        // …the fourth is rate limited without showing anything.
        let (s, body) = body_of(pair_req(&base, Some(&origin)).send().await.unwrap()).await;
        assert_eq!(s, 429);
        assert!(
            body.contains("pairing_rate_limited") && !body.contains(TOKEN),
            "{body}"
        );
        assert_eq!(lock(&host.prompts).len(), 2);
    }

    #[tokio::test]
    async fn a_disconnected_extension_takes_its_prompt_with_it() {
        let (bridge, host, base) = serve(true, false).await;
        let origin = format!("chrome-extension://{EXT}");
        let req = tokio::spawn(pair_req(&base, Some(&origin)).send());
        let prompt = next_prompt(&host, 0).await;
        req.abort();
        for _ in 0..200 {
            if bridge.pending_pairing().is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            bridge.pending_pairing().is_none(),
            "prompt must not get stuck"
        );
        assert!(lock(&host.closed).contains(&prompt.id));
    }
}
