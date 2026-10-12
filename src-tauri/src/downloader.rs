//! Core download engine implementing IDM-style **Dynamic File Segmentation**.
//!
//! Design overview
//! ===============
//! A single download (`DownloadJob`) is split into `N` byte ranges (`Chunk`).
//! Each chunk is driven by an async worker that streams its `Range:` request
//! straight to disk at the correct byte offset. When a worker finishes its
//! range early, a central *coordinator* splits the **largest remaining** chunk
//! in half and spawns a fresh worker on the new tail — keeping `N` connections
//! saturated until the file is too small to subdivide. This is the algorithm
//! that lets IDM keep every connection busy until the very last byte.
//!
//! Concurrency & safety
//! ====================
//! * `current` / `end` of every chunk are `AtomicU64`, so workers stream
//!   lock-free and a split is just an atomic `store` on the victim's `end`.
//! * Disk writes go through ONE dedicated blocking flusher thread using OS
//!   *positioned writes* (`seek_write` on Windows, `write_at` on Unix). These
//!   do not touch a shared file cursor, so there is zero chance of two writers
//!   clobbering each other's offset — and we avoid the throughput cliff of a
//!   global `Mutex<File>`.
//! * Workers never hold a lock while doing network I/O.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use futures_util::StreamExt;
use reqwest::header::{ACCEPT_ENCODING, ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_RANGE, RANGE};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex, Notify};
use tracing::{debug, info, instrument, warn};

// ---------------------------------------------------------------------------
// Tunables
// ---------------------------------------------------------------------------

/// Don't split a chunk whose remaining bytes are below this — the HTTP/setup
/// overhead of a new connection would outweigh the gain.
const MIN_SPLIT_BYTES: u64 = 1024 * 1024; // 1 MiB
/// Max transient failures (timeout / dropped connection) before a chunk is
/// declared permanently failed.
const MAX_RETRIES: u32 = 6;
/// Channel depth for the disk flusher — provides natural back-pressure so a
/// slow disk cannot let RAM usage balloon.
const WRITE_CHANNEL_DEPTH: usize = 512;
/// How often progress is sampled and emitted to the UI.
const TICK: Duration = Duration::from_millis(400);
/// Sent on every request. Product token + version, no placeholder URL.
const USER_AGENT: &str = concat!("IDM-Replica/", env!("CARGO_PKG_VERSION"));

// ---------------------------------------------------------------------------
// IPC payloads (Rust -> React)
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize)]
pub struct ThreadProgress {
    pub id: usize,
    pub start: u64,
    pub current: u64,
    pub end: u64,
    /// 0.0 – 100.0 for this individual segment.
    pub progress: f64,
    pub done: bool,
    pub failed: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressPayload {
    pub id: String,
    pub file_name: String,
    pub url: String,
    pub total: u64,
    pub downloaded: u64,
    /// Bytes per second (instantaneous, smoothed).
    pub speed: f64,
    /// Seconds remaining; `-1` if unknown.
    pub eta: f64,
    pub progress: f64,
    /// downloading | paused | muxing | complete | error | canceled
    pub status: String,
    pub connections: usize,
    pub active_threads: usize,
    /// Current per-download speed cap in bytes/sec; `0` = unlimited.
    pub speed_limit: u64,
    pub threads: Vec<ThreadProgress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// SHA-256 of the finished file (only when integrity check ran).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// `Some(true/false)` when an expected checksum was compared; else `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified: Option<bool>,
    /// Non-fatal warnings (e.g. a mirror was rejected). Omitted when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl ProgressPayload {
    /// Terminal `error` payload for a download that failed before the engine
    /// could report anything itself (probe or pre-allocation failed), so the
    /// UI never sits on "Connecting…" forever.
    pub fn failed_early(id: &str, file_name: &str, url: &str, error: String) -> Self {
        Self {
            error: Some(error),
            ..Self::simple(id, file_name, url, "error")
        }
    }

    /// A payload without per-thread data, for jobs the segmented engine
    /// doesn't run (e.g. yt-dlp manifest downloads). Callers fill in numbers.
    pub fn simple(id: &str, file_name: &str, url: &str, status: &str) -> Self {
        Self {
            id: id.to_string(),
            file_name: file_name.to_string(),
            url: url.to_string(),
            total: 0,
            downloaded: 0,
            speed: 0.0,
            eta: -1.0,
            progress: 0.0,
            status: status.into(),
            connections: 0,
            active_threads: 0,
            speed_limit: 0,
            threads: Vec::new(),
            error: None,
            sha256: None,
            verified: None,
            warnings: Vec::new(),
        }
    }
}

/// How the engine reports progress to the outside world. Implemented by a
/// Tauri-backed sink in `main.rs` and by an in-memory sink in tests — this is
/// what keeps the engine free of any GUI dependency.
pub trait ProgressSink: Send + Sync {
    fn emit(&self, payload: ProgressPayload);
}

/// Where [`download_to_file`] writes.
#[derive(Clone, Debug)]
pub enum Destination {
    /// An exact path (Save-As, or an internal temp file).
    File(PathBuf),
    /// A directory; the file name is chosen after probing, from
    /// `Content-Disposition` or the URL (see [`choose_file_name`]).
    Dir(PathBuf),
}

impl From<PathBuf> for Destination {
    fn from(p: PathBuf) -> Self {
        Destination::File(p)
    }
}

/// Per-download knobs passed into [`download_to_file`].
#[derive(Clone, Default)]
pub struct DownloadOptions {
    /// Desired parallel connections (clamped to 1..=16).
    pub connections: usize,
    /// Speed cap in bytes/sec; `0` = unlimited.
    pub speed_limit: u64,
    /// If true, hash the finished file (SHA-256 + MD5) after completion.
    pub verify: bool,
    /// Optional expected checksum (hex, SHA-256 or MD5) to compare against.
    pub expected_checksum: Option<String>,
    /// Extra request headers applied to every request (auth, cookies, etc.).
    pub headers: Vec<(String, String)>,
    /// Additional mirror URLs serving the *same* file. Workers spread their
    /// ranges across the primary URL + mirrors to combine their bandwidth.
    pub mirrors: Vec<String>,
    /// A limiter shared with other jobs so their *combined* throughput obeys
    /// one cap (YouTube video + audio). When set, `speed_limit` is ignored and
    /// `set_speed_limit` on any sharing job changes the cap for all of them.
    pub shared_limiter: Option<Arc<RateLimiter>>,
    /// Suggested file name (e.g. from a browser) used with [`Destination::Dir`]
    /// when the server sends no `Content-Disposition` name. Untrusted: it goes
    /// through [`safe_file_name`]; an unusable hint falls back to the URL.
    pub name_hint: Option<String>,
}

// ---------------------------------------------------------------------------
// Rate limiter (token bucket) — gates the network-read step only.
// ---------------------------------------------------------------------------

struct RateState {
    allowance: f64,
    last: Instant,
}

/// A shared token bucket. All workers of one job await it before consuming a
/// network frame, so the **sum** of their throughput respects the cap. When the
/// rate is `0` (unlimited) the fast path returns immediately with no locking.
pub struct RateLimiter {
    /// Bytes/sec; `0` = unlimited.
    rate: AtomicU64,
    state: Mutex<RateState>,
}

impl RateLimiter {
    /// A limiter capped at `rate` bytes/sec (`0` = unlimited).
    pub fn new(rate: u64) -> Self {
        Self {
            rate: AtomicU64::new(rate),
            state: Mutex::new(RateState {
                allowance: 0.0,
                last: Instant::now(),
            }),
        }
    }

    fn set_rate(&self, rate: u64) {
        self.rate.store(rate, Ordering::Release);
    }

    fn rate(&self) -> u64 {
        self.rate.load(Ordering::Acquire)
    }

    /// Block (cooperatively) until `bytes` worth of tokens are available.
    async fn acquire(&self, bytes: u64) {
        // Fast path: unlimited => zero overhead, no lock taken.
        if self.rate.load(Ordering::Acquire) == 0 {
            return;
        }
        loop {
            let rate = self.rate.load(Ordering::Acquire);
            if rate == 0 {
                return;
            }
            let wait = {
                let mut st = self.state.lock().await;
                let now = Instant::now();
                let elapsed = now.duration_since(st.last).as_secs_f64();
                st.last = now;
                st.allowance += elapsed * rate as f64;
                // Cap the burst to ~1 second of budget.
                let cap = rate as f64;
                if st.allowance > cap {
                    st.allowance = cap;
                }
                if st.allowance >= bytes as f64 {
                    st.allowance -= bytes as f64;
                    None
                } else {
                    let deficit = bytes as f64 - st.allowance;
                    Some(Duration::from_secs_f64(deficit / rate as f64))
                }
            };
            match wait {
                None => return,
                Some(d) => tokio::time::sleep(d).await,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

/// One contiguous byte range being downloaded.
struct Chunk {
    id: usize,
    start: u64,
    /// Next absolute byte offset we still need. Advances as we write.
    current: AtomicU64,
    /// Exclusive end. A split *shrinks* this atomically.
    end: AtomicU64,
    /// End of the prefix the flusher has actually written to the file.
    /// `current` advances when a frame is *queued* (up to the channel depth
    /// ahead), so only this is safe to persist in the resume sidecar.
    written: AtomicU64,
    done: AtomicBool,
    failed: AtomicBool,
}

impl Chunk {
    fn new(id: usize, start: u64, end: u64) -> Self {
        Self {
            id,
            start,
            current: AtomicU64::new(start),
            end: AtomicU64::new(end),
            written: AtomicU64::new(start),
            done: AtomicBool::new(false),
            failed: AtomicBool::new(false),
        }
    }

    /// Bytes still outstanding for this segment.
    fn remaining(&self) -> u64 {
        self.end
            .load(Ordering::Acquire)
            .saturating_sub(self.current.load(Ordering::Acquire))
    }

    fn snapshot(&self) -> ThreadProgress {
        let start = self.start;
        let cur = self.current.load(Ordering::Acquire);
        let end = self.end.load(Ordering::Acquire);
        let span = end.saturating_sub(start).max(1);
        let progress = ((cur.saturating_sub(start)) as f64 / span as f64) * 100.0;
        ThreadProgress {
            id: self.id,
            start,
            current: cur,
            end,
            progress: progress.clamp(0.0, 100.0),
            done: self.done.load(Ordering::Acquire),
            failed: self.failed.load(Ordering::Acquire),
        }
    }
}

/// A positioned write request handed to the flusher thread.
struct WriteCmd {
    offset: u64,
    data: Bytes,
    /// The chunk this frame belongs to; the flusher advances its `written`.
    chunk: Arc<Chunk>,
}

/// Resume records for the sidecar: `current` is capped at what the flusher
/// has really written, so a crash/kill can never leave the sidecar claiming
/// bytes that were still queued in the write channel.
fn sidecar_chunks(chunks: &[Arc<Chunk>]) -> Vec<ChunkRec> {
    chunks
        .iter()
        .map(|c| ChunkRec {
            start: c.start,
            current: c
                .current
                .load(Ordering::Acquire)
                .min(c.written.load(Ordering::Acquire)),
            end: c.end.load(Ordering::Acquire),
        })
        .collect()
}

/// Shared, controllable handle for a single download.
pub struct DownloadJob {
    pub id: String,
    pub url: String,
    pub file_name: String,
    pub total_size: u64,
    /// Whether the server supports `Range` (false => single stream fallback).
    pub resumable: bool,
    pub max_connections: usize,

    /// URL pool: primary first, then any validated mirrors. Workers pick by chunk id.
    urls: Vec<String>,
    /// Non-fatal problems to show the user (e.g. a rejected mirror).
    warnings: Vec<String>,
    /// Extra headers applied to every request.
    headers: Vec<(String, String)>,

    chunks: Mutex<Vec<Arc<Chunk>>>,
    next_chunk_id: AtomicUsize,
    active: AtomicUsize,

    paused: AtomicBool,
    cancelled: AtomicBool,
    resume: Notify,

    /// Shared token bucket; may be shared with sibling jobs (YouTube video+audio).
    limiter: Arc<RateLimiter>,
    client: reqwest::Client,
}

impl DownloadJob {
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    /// Change the per-download speed cap live (bytes/sec; `0` = unlimited).
    pub fn set_speed_limit(&self, bytes_per_sec: u64) {
        self.limiter.set_rate(bytes_per_sec);
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
        self.resume.notify_waiters();
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        // Wake anything parked on a pause so it can observe cancellation.
        self.resume.notify_waiters();
    }

    /// Bytes actually covered. `current` is clamped to `end` per chunk: after a
    /// split the victim may have streamed past its shrunk `end`, and those
    /// overlap bytes belong to the tail chunk — counting them twice would
    /// over-report progress.
    fn downloaded(&self, chunks: &[Arc<Chunk>]) -> u64 {
        chunks
            .iter()
            .map(|c| {
                let cur = c.current.load(Ordering::Acquire);
                let end = c.end.load(Ordering::Acquire);
                cur.min(end).saturating_sub(c.start)
            })
            .sum()
    }
}

/// A known-size download is complete only when *every* chunk has reached its
/// `end`. Chunks tile `[0, total)`, so this is the one predicate that cannot be
/// fooled by a victim overshooting its split point.
fn all_chunks_reached_end(chunks: &[Arc<Chunk>]) -> bool {
    !chunks.is_empty()
        && chunks
            .iter()
            .all(|c| c.current.load(Ordering::Acquire) >= c.end.load(Ordering::Acquire))
}

// ---------------------------------------------------------------------------
// The Manager — registry of live jobs for pause/resume/cancel from the UI.
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct DownloadManager {
    jobs: Mutex<HashMap<String, Arc<DownloadJob>>>,
}

impl DownloadManager {
    pub fn new() -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
        }
    }

    async fn register(&self, job: Arc<DownloadJob>) {
        self.jobs.lock().await.insert(job.id.clone(), job);
    }

    async fn unregister(&self, id: &str) {
        self.jobs.lock().await.remove(id);
    }

    pub async fn get(&self, id: &str) -> Option<Arc<DownloadJob>> {
        self.jobs.lock().await.get(id).cloned()
    }

    /// Every job that is currently registered (tray "Pause all", quit).
    pub async fn all(&self) -> Vec<Arc<DownloadJob>> {
        self.jobs.lock().await.values().cloned().collect()
    }
}

// ---------------------------------------------------------------------------
// Phase 1 — Initialization (HEAD probe)
// ---------------------------------------------------------------------------

struct ProbeResult {
    total_size: u64,
    resumable: bool,
    file_name: String,
    /// `file_name` came from the server's `Content-Disposition` (so it beats
    /// a caller's [`DownloadOptions::name_hint`]).
    server_named: bool,
    /// Cache validators (`ETag`, `Last-Modified`) — used to decide whether a
    /// resume sidecar still describes the file the server is serving now.
    etag: Option<String>,
    last_modified: Option<String>,
}

/// Response headers the probe cares about beyond size/ranges.
#[derive(Default)]
struct ProbeMeta {
    content_disposition: Option<String>,
    etag: Option<String>,
    last_modified: Option<String>,
}

impl ProbeMeta {
    /// Fill any field still missing from `resp` (HEAD first, then the GET probe).
    fn fill_from(&mut self, resp: &reqwest::Response) {
        let get = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        self.content_disposition = self
            .content_disposition
            .take()
            .or_else(|| get("content-disposition"));
        self.etag = self.etag.take().or_else(|| get("etag"));
        self.last_modified = self.last_modified.take().or_else(|| get("last-modified"));
    }
}

/// Apply a slice of `(name, value)` headers to a request builder, and pin
/// `Accept-Encoding: identity`. With a content-encoded response, Range offsets
/// and lengths refer to the *encoded* bytes, which would corrupt a segmented
/// download — so this cannot be overridden by user headers.
fn apply_headers(
    mut rb: reqwest::RequestBuilder,
    headers: &[(String, String)],
) -> reqwest::RequestBuilder {
    for (k, v) in headers {
        if k.trim().eq_ignore_ascii_case("accept-encoding") {
            continue;
        }
        rb = rb.header(k.as_str(), v.as_str());
    }
    rb.header(ACCEPT_ENCODING, "identity")
}

async fn probe(
    client: &reqwest::Client,
    url: &str,
    headers: &[(String, String)],
) -> Result<ProbeResult> {
    // Try HEAD first; many CDNs answer it cheaply.
    let head = apply_headers(client.head(url), headers).send().await;

    let mut meta = ProbeMeta::default();
    let (len, accept_ranges) = match head {
        Ok(resp) if resp.status().is_success() => {
            let len = resp
                .headers()
                .get(CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
            let ar = resp
                .headers()
                .get(ACCEPT_RANGES)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.to_ascii_lowercase().contains("bytes"))
                .unwrap_or(false);
            meta.fill_from(&resp);
            (len, ar)
        }
        _ => (None, false),
    };

    // Some servers lie or refuse HEAD. Confirm range support with a tiny GET
    // and read Content-Range to recover the real length.
    let (total_size, resumable) = match (len, accept_ranges) {
        (Some(l), true) if l > 0 => (l, true),
        _ => {
            let resp = apply_headers(client.get(url), headers)
                .header(RANGE, "bytes=0-0")
                .send()
                .await
                .context("range probe GET failed")?;
            let status = resp.status();
            // An error status means there is no file to fetch; don't mistake
            // it for "no range support" and download the error page. 416 is
            // the exception: an empty file legitimately rejects `bytes=0-0`.
            if (status.is_client_error() || status.is_server_error())
                && status != reqwest::StatusCode::RANGE_NOT_SATISFIABLE
            {
                return Err(anyhow!("server returned HTTP {status}"));
            }
            meta.fill_from(&resp);
            if resp.status().as_u16() == 206 {
                let total = resp
                    .headers()
                    .get(CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.rsplit('/').next().map(|s| s.to_string()))
                    .and_then(|s| s.parse::<u64>().ok());
                match total {
                    Some(t) if t > 0 => (t, true),
                    _ => (len.unwrap_or(0), false),
                }
            } else {
                // No range support — fall back to a single sequential stream.
                let l = resp
                    .headers()
                    .get(CONTENT_LENGTH)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0);
                (l, false)
            }
        }
    };

    let file_name = choose_file_name(url, meta.content_disposition.as_deref());
    let server_named = meta
        .content_disposition
        .as_deref()
        .and_then(disposition_file_name)
        .and_then(|n| safe_file_name(&n))
        .is_some();
    Ok(ProbeResult {
        total_size,
        resumable,
        file_name,
        server_named,
        etag: meta.etag,
        last_modified: meta.last_modified,
    })
}

/// Probe every mirror (concurrently) and keep only those that serve a file of
/// the same size with range support. A mismatched mirror would silently
/// write a different file's bytes into our ranges, so it is dropped and a
/// warning is returned for the UI. Same size is necessary, not sufficient:
/// a same-size different file still needs ETag/checksum (not done here).
async fn validate_mirrors(
    client: &reqwest::Client,
    primary: &ProbeResult,
    opts: &DownloadOptions,
) -> (Vec<String>, Vec<String>) {
    let mut warnings = Vec::new();
    if opts.mirrors.is_empty() {
        return (Vec::new(), warnings);
    }
    if !primary.resumable || primary.total_size == 0 {
        let msg = "mirrors ignored: primary server does not support ranged downloads".to_string();
        warn!("{msg}");
        warnings.push(msg);
        return (Vec::new(), warnings);
    }
    let probes = futures_util::future::join_all(
        opts.mirrors
            .iter()
            .map(|m| async move { (m, probe(client, m, &opts.headers).await) }),
    )
    .await;
    let mut kept = Vec::new();
    for (mirror, result) in probes {
        let reason = match result {
            Ok(p) if !p.resumable => "does not support ranged downloads".to_string(),
            Ok(p) if p.total_size != primary.total_size => format!(
                "size {} differs from primary {}",
                p.total_size, primary.total_size
            ),
            Ok(_) => {
                kept.push(mirror.clone());
                continue;
            }
            Err(e) => format!("probe failed: {e:#}"),
        };
        let msg = format!("mirror dropped ({reason}): {mirror}");
        warn!("{msg}");
        warnings.push(msg);
    }
    (kept, warnings)
}

// ---------------------------------------------------------------------------
// File names — every name from a URL or a server header is hostile input.
// ---------------------------------------------------------------------------

/// Last-resort name when neither Content-Disposition nor the URL yields one.
const FALLBACK_FILE_NAME: &str = "download";
/// Keep well under every filesystem's 255-byte component limit.
const MAX_FILE_NAME_CHARS: usize = 200;

/// Pick the local file name for a download: `Content-Disposition` first, then
/// the URL's last path segment, then [`FALLBACK_FILE_NAME`]. Every candidate
/// goes through [`safe_file_name`], so the result is always a single, plain
/// path component that cannot escape the target directory.
pub fn choose_file_name(url: &str, content_disposition: Option<&str>) -> String {
    content_disposition
        .and_then(disposition_file_name)
        .and_then(|n| safe_file_name(&n))
        .or_else(|| url_last_segment(url).and_then(safe_file_name))
        .unwrap_or_else(|| FALLBACK_FILE_NAME.to_string())
}

/// Reduce untrusted text to a safe single file-name component, or `None` if
/// nothing usable remains. Percent-decodes, keeps only the part after the last
/// `/` or `\` (so `..\..\x` → `x`), replaces characters Windows forbids,
/// strips trailing dots/spaces, and rejects `.`/`..` and Windows reserved
/// device names (`CON`, `NUL`, `COM1`, … with or without an extension).
pub fn safe_file_name(raw: &str) -> Option<String> {
    let decoded = percent_decode(raw);
    let base = decoded.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    // Windows silently drops trailing dots/spaces, which would also let
    // "CON." alias a device — strip them before the reserved-name check.
    let name = cleaned.trim().trim_end_matches(['.', ' ']);
    if name.is_empty() || name.chars().all(|c| c == '.') || is_windows_reserved(name) {
        return None;
    }
    Some(truncate_file_name(name))
}

/// `dir/name`, refusing anything but a single plain component — a last guard
/// (names are already sanitised) against writing outside the chosen folder.
pub fn join_inside(dir: &Path, name: &str) -> Result<PathBuf> {
    let mut parts = Path::new(name).components();
    match (parts.next(), parts.next()) {
        (Some(std::path::Component::Normal(_)), None) => Ok(dir.join(name)),
        _ => Err(anyhow!(
            "refusing to write outside the download folder ({name:?})"
        )),
    }
}

fn is_windows_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    let upper = stem.to_ascii_uppercase();
    match upper.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" => true,
        _ => {
            let b = upper.as_bytes();
            b.len() == 4
                && (upper.starts_with("COM") || upper.starts_with("LPT"))
                && b[3].is_ascii_digit()
        }
    }
}

/// Cap the length, keeping a short extension intact.
fn truncate_file_name(name: &str) -> String {
    if name.chars().count() <= MAX_FILE_NAME_CHARS {
        return name.to_string();
    }
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && ext.chars().count() <= 16 => {
            let keep = MAX_FILE_NAME_CHARS - ext.chars().count() - 1;
            format!("{}.{ext}", stem.chars().take(keep).collect::<String>())
        }
        _ => name.chars().take(MAX_FILE_NAME_CHARS).collect(),
    }
}

/// Decode `%XX` escapes (invalid escapes are kept literally); the bytes are
/// interpreted as UTF-8, lossily.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The raw (still-encoded) last path segment of a URL, if it has a path.
fn url_last_segment(url: &str) -> Option<&str> {
    let no_frag = url.split(['?', '#']).next().unwrap_or(url);
    let path = match no_frag.split_once("://") {
        Some((_, rest)) => &rest[rest.find('/')?..],
        None => no_frag,
    };
    path.rsplit('/').next().filter(|s| !s.trim().is_empty())
}

/// Extract the file name from a `Content-Disposition` value. RFC 6266:
/// `filename*` (RFC 5987, `charset'lang'pct-encoded`) wins over `filename`.
fn disposition_file_name(cd: &str) -> Option<String> {
    let mut plain = None;
    let mut extended = None;
    for param in split_params(cd) {
        let Some((k, v)) = param.split_once('=') else {
            continue;
        };
        let v = v.trim().trim_matches('"');
        match k.trim().to_ascii_lowercase().as_str() {
            // Value is already percent-encoded; `safe_file_name` decodes it.
            "filename*" => extended = v.splitn(3, '\'').nth(2).map(str::to_string),
            "filename" => plain = Some(v.to_string()),
            _ => {}
        }
    }
    extended.or(plain).filter(|s| !s.trim().is_empty())
}

/// Split a header value on `;`, ignoring semicolons inside quoted strings.
fn split_params(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut start, mut quoted) = (0, false);
    for (i, c) in s.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ';' if !quoted => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

// ---------------------------------------------------------------------------
// Phase 2 — Pre-allocation
// ---------------------------------------------------------------------------

fn preallocate(path: &Path, size: u64) -> Result<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let file = OpenOptions::new()
        .create(true)
        // Deliberately do NOT truncate: we manage the file's size via
        // `set_len` and write at explicit offsets. Preserving existing bytes is
        // also what the cross-restart resume path will rely on.
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    if size > 0 {
        // Reserve the full extent up-front to avoid on-disk fragmentation as
        // many threads write to scattered offsets.
        file.set_len(size).context("pre-allocating file length")?;
    }
    Ok(file)
}

// ---------------------------------------------------------------------------
// Cross-platform positioned write (no shared cursor => concurrency-safe)
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn pwrite_all(file: &File, mut offset: u64, mut buf: &[u8]) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = file.seek_write(buf, offset)?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "seek_write wrote 0 bytes",
            ));
        }
        offset += n as u64;
        buf = &buf[n..];
    }
    Ok(())
}

#[cfg(unix)]
fn pwrite_all(file: &File, mut offset: u64, mut buf: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    while !buf.is_empty() {
        let n = file.write_at(buf, offset)?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "write_at wrote 0 bytes",
            ));
        }
        offset += n as u64;
        buf = &buf[n..];
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Phase 4 — Dynamic split of the largest live chunk
// ---------------------------------------------------------------------------

/// Find the chunk with the most outstanding bytes and split it in half.
/// Returns the freshly created tail chunk (already inserted) or `None` if
/// nothing is worth splitting.
fn split_largest(job: &DownloadJob, chunks: &mut Vec<Arc<Chunk>>) -> Option<Arc<Chunk>> {
    // Select by index so the immutable borrow ends before we `push`.
    let mut victim_idx: Option<usize> = None;
    let mut best = 0u64;
    for (i, c) in chunks.iter().enumerate() {
        if c.done.load(Ordering::Acquire) || c.failed.load(Ordering::Acquire) {
            continue;
        }
        let rem = c.remaining();
        if rem > best {
            best = rem;
            victim_idx = Some(i);
        }
    }
    let idx = victim_idx?;
    if best < MIN_SPLIT_BYTES {
        return None;
    }

    let (mid, end) = {
        let victim = &chunks[idx];
        let cur = victim.current.load(Ordering::Acquire);
        let end = victim.end.load(Ordering::Acquire);
        if end.saturating_sub(cur) < MIN_SPLIT_BYTES {
            return None;
        }
        let mid = cur + (end - cur) / 2;
        if mid <= cur || mid >= end {
            return None;
        }
        // Shrink the victim. Its worker reloads `end` every block and stops
        // cleanly at `mid`. If it has already streamed slightly past `mid` the
        // new worker simply re-fetches that small overlap — positioned writes
        // are idempotent, so the file stays correct.
        victim.end.store(mid, Ordering::Release);
        (mid, end)
    };

    let id = job.next_chunk_id.fetch_add(1, Ordering::SeqCst);
    let tail = Arc::new(Chunk::new(id, mid, end));
    chunks.push(tail.clone());
    Some(tail)
}

// ---------------------------------------------------------------------------
// The per-chunk worker (Phase 3 streaming loop + retry/pause handling)
// ---------------------------------------------------------------------------

async fn run_worker(
    job: Arc<DownloadJob>,
    chunk: Arc<Chunk>,
    write_tx: mpsc::Sender<WriteCmd>,
    finished_tx: mpsc::Sender<()>,
) {
    let mut retries = 0u32;

    'outer: loop {
        if job.cancelled.load(Ordering::Acquire) {
            break;
        }

        // Honor pause: park until resumed or cancelled.
        while job.paused.load(Ordering::Acquire) && !job.cancelled.load(Ordering::Acquire) {
            job.resume.notified().await;
        }
        if job.cancelled.load(Ordering::Acquire) {
            break;
        }

        let cur = chunk.current.load(Ordering::Acquire);
        let end = chunk.end.load(Ordering::Acquire);
        if cur >= end {
            chunk.done.store(true, Ordering::Release);
            break;
        }

        // Pick a source from the URL pool (primary + mirrors) by chunk id, so
        // ranges spread across all mirrors and combine their bandwidth.
        let src = &job.urls[chunk.id % job.urls.len()];

        // Build the request. For non-resumable servers we stream the whole body.
        let mut req = apply_headers(job.client.get(src), &job.headers);
        if job.resumable {
            req = req.header(RANGE, format!("bytes={}-{}", cur, end - 1));
        }

        let resp = match req.send().await {
            Ok(r) if r.status().is_success() => r,
            Ok(r) if !is_retryable_status(r.status()) => {
                // Permanent (404, 403, …): retrying only delays the error.
                warn!(status = %r.status(), "chunk: non-retryable HTTP status");
                chunk.failed.store(true, Ordering::Release);
                break;
            }
            Ok(r) => {
                // 408/429/5xx — back off and retry, then give up on this chunk.
                if !should_retry(&mut retries, format!("HTTP {}", r.status())).await {
                    chunk.failed.store(true, Ordering::Release);
                    break;
                }
                continue;
            }
            Err(e) => {
                if !should_retry(&mut retries, e.to_string()).await {
                    chunk.failed.store(true, Ordering::Release);
                    break;
                }
                continue;
            }
        };

        // Pin the response body stream so `.next()` (which needs `Unpin`) works
        // regardless of the concrete stream type reqwest hands back.
        let mut stream = std::pin::pin!(resp.bytes_stream());
        loop {
            // React quickly to control signals between network frames.
            if job.cancelled.load(Ordering::Acquire) {
                break 'outer;
            }
            if job.paused.load(Ordering::Acquire) {
                // Drop this connection; the outer loop resumes from `current`.
                continue 'outer;
            }

            match stream.next().await {
                Some(Ok(bytes)) => {
                    let cur = chunk.current.load(Ordering::Acquire);
                    let end = chunk.end.load(Ordering::Acquire);
                    if cur >= end {
                        // We were split out from under; this range is finished.
                        break;
                    }
                    // Clamp to our (possibly newly shrunk) end.
                    let take = std::cmp::min(bytes.len() as u64, end - cur) as usize;
                    if take == 0 {
                        break;
                    }
                    let data = bytes.slice(0..take);

                    // Throttle: gate at the network-read step (not the flusher).
                    // No-op / zero overhead when the cap is 0 (unlimited).
                    job.limiter.acquire(take as u64).await;
                    if job.cancelled.load(Ordering::Acquire) {
                        break 'outer;
                    }

                    if write_tx
                        .send(WriteCmd {
                            offset: cur,
                            data,
                            chunk: chunk.clone(),
                        })
                        .await
                        .is_err()
                    {
                        // Flusher gone — abort whole job.
                        break 'outer;
                    }
                    chunk.current.fetch_add(take as u64, Ordering::AcqRel);
                    retries = 0; // real progress resets the failure budget

                    if take < bytes.len() {
                        // We hit our shrunk end mid-frame — done with this range.
                        break;
                    }
                }
                Some(Err(e)) => {
                    // Dropped connection / read timeout — reconnect from `current`.
                    if !should_retry(&mut retries, e.to_string()).await {
                        chunk.failed.store(true, Ordering::Release);
                        break 'outer;
                    }
                    continue 'outer;
                }
                None => {
                    // Body ended (clean EOF). For a non-resumable single
                    // stream this means the whole file has arrived — we cannot
                    // re-request a tail, so this chunk is complete.
                    if !job.resumable {
                        chunk.done.store(true, Ordering::Release);
                        break 'outer;
                    }
                    break;
                }
            }
        }

        if chunk.current.load(Ordering::Acquire) >= chunk.end.load(Ordering::Acquire) {
            chunk.done.store(true, Ordering::Release);
            break;
        }
        // Body ended early without an error (rare) — loop to re-request the rest.
    }

    job.active.fetch_sub(1, Ordering::SeqCst);
    // Tell the coordinator a slot freed up so it can top concurrency back up.
    let _ = finished_tx.send(()).await;
}

/// Is an HTTP error status worth retrying? Only timeouts, rate limiting and
/// server-side errors are transient; every other 4xx is permanent.
fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

/// Exponential backoff for the `n`th retry (1-based): 250 ms · 2^(n-1),
/// exponent capped at 5.
fn backoff_base(n: u32) -> Duration {
    Duration::from_millis(250u64 * (1u64 << n.saturating_sub(1).min(5)))
}

/// "Equal jitter": a uniformly random delay in `[base/2, base]`, so many
/// workers failing together don't reconnect in lock-step.
fn jittered(base: Duration) -> Duration {
    use std::hash::{BuildHasher, Hasher};
    // A fresh `RandomState` is randomly keyed — enough entropy for jitter
    // without pulling in a RNG crate.
    let r = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    let frac = (r >> 11) as f64 / (1u64 << 53) as f64; // [0, 1)
    base / 2 + base.mul_f64(0.5 * frac)
}

/// Sleep with jittered exponential backoff. Returns `false` once the retry
/// budget is spent.
async fn should_retry(retries: &mut u32, reason: String) -> bool {
    *retries += 1;
    if *retries > MAX_RETRIES {
        warn!(retries = *retries - 1, %reason, "chunk: giving up after retries");
        return false;
    }
    let backoff = jittered(backoff_base(*retries));
    warn!(
        %reason,
        retry = *retries,
        max = MAX_RETRIES,
        ?backoff,
        "chunk: transient error, retrying"
    );
    tokio::time::sleep(backoff).await;
    true
}

// ---------------------------------------------------------------------------
// Integrity: stream the finished file through SHA-256 + MD5.
// ---------------------------------------------------------------------------

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Returns `(sha256_hex, md5_hex)`. Blocking — call via `spawn_blocking`.
fn hash_file(path: &Path) -> std::io::Result<(String, String)> {
    use md5::Md5;
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let mut file = File::open(path)?;
    let mut sha = Sha256::new();
    let mut md5 = Md5::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        sha.update(&buf[..n]);
        md5.update(&buf[..n]);
    }
    Ok((to_hex(&sha.finalize()), to_hex(&md5.finalize())))
}

// ---------------------------------------------------------------------------
// Cross-restart resume — the `.idm` sidecar.
// ---------------------------------------------------------------------------
//
// While a download runs, the coordinator periodically writes a tiny JSON
// sidecar next to the destination recording every chunk's [start, current, end).
// On a fresh `download_to_file` for the same (file + sidecar) pair, we re-open
// the already pre-allocated file and restart each worker from its saved
// `current` instead of byte 0. Only the coordinator touches the sidecar — never
// the per-frame workers — so the lock-free hot path is untouched.

/// v2 added `etag` / `last_modified`. Any other version is rejected (the
/// download then restarts from 0), never migrated.
const SIDECAR_VERSION: u32 = 2;

#[derive(Serialize, Deserialize)]
struct ChunkRec {
    start: u64,
    current: u64,
    end: u64,
}

#[derive(Serialize, Deserialize)]
struct Sidecar {
    version: u32,
    url: String,
    total_size: u64,
    resumable: bool,
    /// Validators from the probe that created the partial file. `None` = the
    /// server didn't send it.
    etag: Option<String>,
    last_modified: Option<String>,
    chunks: Vec<ChunkRec>,
}

/// `<dest>.idm`
fn sidecar_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".idm");
    PathBuf::from(s)
}

/// Atomically persist the sidecar (temp file + rename) so a crash mid-write
/// can never leave a torn file.
fn write_sidecar(path: &Path, sc: &Sidecar) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let data = serde_json::to_vec(sc)?;
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

/// Return the saved chunk layout iff the sidecar is valid, the server still
/// serves the same file (same size **and** same `ETag`/`Last-Modified` —
/// absent on both sides counts as equal), AND the destination file still
/// exists at the expected size. Anything else would splice old bytes into a
/// changed file.
fn load_sidecar(
    dest: &Path,
    scp: &Path,
    url: &str,
    probe: &ProbeResult,
) -> Option<Vec<(u64, u64, u64)>> {
    let data = std::fs::read(scp).ok()?;
    let sc: Sidecar = serde_json::from_slice(&data).ok()?;
    if sc.version != SIDECAR_VERSION
        || sc.url != url
        || sc.total_size != probe.total_size
        || !sc.resumable
    {
        return None;
    }
    if sc.etag != probe.etag || sc.last_modified != probe.last_modified {
        info!("resume sidecar ignored: server validators changed");
        return None;
    }
    // The bytes already on disk must match what the sidecar claims.
    let meta = std::fs::metadata(dest).ok()?;
    if meta.len() != probe.total_size || sc.chunks.is_empty() {
        return None;
    }
    Some(
        sc.chunks
            .into_iter()
            .map(|c| (c.start, c.current, c.end))
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Public entry point — fully drives one download to completion.
// ---------------------------------------------------------------------------

/// Build the shared HTTP client. One client => one connection pool, reused
/// across every chunk and download for low overhead.
pub fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .pool_max_idle_per_host(32)
        .connect_timeout(Duration::from_secs(15))
        // A *read* timeout (not a total timeout!) so a stalled socket is
        // surfaced as a stream error and retried, while a slow-but-alive
        // multi-minute chunk transfer is never aborted mid-flight.
        .read_timeout(Duration::from_secs(30))
        .user_agent(USER_AGENT)
        .build()
        .expect("failed to build reqwest client")
}

/// Download `url` to `dest`, reporting progress through `sink`.
/// Awaits until the file is fully written (or failed / cancelled).
#[instrument(skip_all, fields(id = %id, url = %url))]
pub async fn download_to_file(
    sink: Arc<dyn ProgressSink>,
    manager: Arc<DownloadManager>,
    client: reqwest::Client,
    id: String,
    url: String,
    dest: impl Into<Destination>,
    opts: DownloadOptions,
) -> Result<PathBuf> {
    let connections = opts.connections.clamp(1, 16);

    // ---- Phase 1: probe -------------------------------------------------
    let info = probe(&client, &url, &opts.headers).await?;

    // URL pool: primary first, then only the mirrors that verifiably serve
    // the same file.
    let (mirrors, warnings) = validate_mirrors(&client, &info, &opts).await;
    let urls: Vec<String> = std::iter::once(url.clone()).chain(mirrors).collect();
    info!(
        total = info.total_size,
        resumable = info.resumable,
        connections,
        mirrors = urls.len() - 1,
        "probe complete"
    );
    let dest = match dest.into() {
        Destination::File(p) => p,
        // `info.file_name` is already a sanitized single component; so is a
        // hint after `safe_file_name`.
        Destination::Dir(dir) => {
            let hinted = (!info.server_named)
                .then(|| opts.name_hint.as_deref().and_then(safe_file_name))
                .flatten();
            join_inside(&dir, hinted.as_deref().unwrap_or(&info.file_name))?
        }
    };
    let file_name = dest
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| info.file_name.clone());

    // ---- Cross-restart resume: inspect the sidecar BEFORE pre-allocation
    // (which would otherwise create the file and invalidate the size check).
    let scp = sidecar_path(&dest);
    let restored = if info.resumable && info.total_size > 0 {
        load_sidecar(&dest, &scp, &url, &info)
    } else {
        None
    };
    if restored.is_none() {
        // Stale / mismatched sidecar from an unrelated download — discard it.
        let _ = std::fs::remove_file(&scp);
    }

    // ---- Phase 2: pre-allocate -----------------------------------------
    let file = preallocate(&dest, info.total_size).context("pre-allocation failed")?;
    let file = Arc::new(file);

    let job = Arc::new(DownloadJob {
        id: id.clone(),
        url: url.clone(),
        file_name: file_name.clone(),
        total_size: info.total_size,
        resumable: info.resumable,
        max_connections: connections,
        urls,
        warnings,
        headers: opts.headers.clone(),
        chunks: Mutex::new(Vec::new()),
        next_chunk_id: AtomicUsize::new(0),
        active: AtomicUsize::new(0),
        paused: AtomicBool::new(false),
        cancelled: AtomicBool::new(false),
        resume: Notify::new(),
        limiter: opts
            .shared_limiter
            .clone()
            .unwrap_or_else(|| Arc::new(RateLimiter::new(opts.speed_limit))),
        client: client.clone(),
    });
    manager.register(job.clone()).await;

    // ---- Dedicated flusher thread (positioned writes) -------------------
    let (write_tx, mut write_rx) = mpsc::channel::<WriteCmd>(WRITE_CHANNEL_DEPTH);
    let flusher_file = file.clone();
    let flusher = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        // `blocking_recv` parks this dedicated OS thread; positioned writes
        // never contend on a shared cursor, so this is the only writer.
        while let Some(cmd) = write_rx.blocking_recv() {
            pwrite_all(&flusher_file, cmd.offset, &cmd.data)?;
            // One atomic per write; workers write a chunk in increasing order.
            cmd.chunk
                .written
                .fetch_max(cmd.offset + cmd.data.len() as u64, Ordering::AcqRel);
        }
        flusher_file.sync_all()?;
        Ok(())
    });

    // ---- Phase 3: static spawning of N initial chunks -------------------
    let (finished_tx, mut finished_rx) = mpsc::channel::<()>(connections * 4 + 8);

    {
        let mut chunks = job.chunks.lock().await;
        if let Some(recs) = restored {
            // ---- Resume: rebuild the saved chunk layout -----------------
            let mut to_spawn = Vec::new();
            for (i, (start, current, end)) in recs.into_iter().enumerate() {
                let c = Arc::new(Chunk::new(i, start, end));
                c.current.store(current, Ordering::Release);
                c.written.store(current, Ordering::Release);
                if current >= end {
                    c.done.store(true, Ordering::Release);
                } else {
                    to_spawn.push(c.clone());
                }
                chunks.push(c);
                job.next_chunk_id.store(i + 1, Ordering::SeqCst);
            }
            drop(chunks);
            info!(live = to_spawn.len(), "resumed from .idm sidecar");
            for c in to_spawn {
                spawn_worker(&job, c, &write_tx, &finished_tx);
            }
            // If the saved layout left us below the connection budget, top up
            // by splitting the largest remaining ranges (same path the
            // coordinator uses), so resume is just as parallel as a fresh start.
            let mut chunks = job.chunks.lock().await;
            while job.active.load(Ordering::Acquire) < job.max_connections {
                match split_largest(&job, &mut chunks) {
                    Some(tail) => spawn_worker(&job, tail, &write_tx, &finished_tx),
                    None => break,
                }
            }
        } else if info.total_size == 0 || !info.resumable {
            // Unknown length or no range support: single sequential stream.
            // If the length is known we still cap `end` at it; otherwise use a
            // sentinel and let clean EOF mark completion.
            let end = if info.total_size > 0 {
                info.total_size
            } else {
                u64::MAX / 2
            };
            let c = Arc::new(Chunk::new(0, 0, end));
            job.next_chunk_id.store(1, Ordering::SeqCst);
            chunks.push(c.clone());
            spawn_worker(&job, c, &write_tx, &finished_tx);
        } else {
            let n = connections.min(((info.total_size / MIN_SPLIT_BYTES).max(1)) as usize);
            let base = info.total_size / n as u64;
            for i in 0..n {
                let start = i as u64 * base;
                let end = if i == n - 1 {
                    info.total_size
                } else {
                    start + base
                };
                let c = Arc::new(Chunk::new(i, start, end));
                chunks.push(c.clone());
            }
            job.next_chunk_id.store(n, Ordering::SeqCst);
            let snapshot: Vec<_> = chunks.iter().cloned().collect();
            drop(chunks);
            for c in snapshot {
                spawn_worker(&job, c, &write_tx, &finished_tx);
            }
        }
    }

    // ---- Coordinator loop: progress emit + dynamic top-up --------------
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut last_downloaded = 0u64;
    let mut last_instant = Instant::now();
    let mut smoothed_speed = 0.0f64;

    let final_status: String;
    let mut final_error: Option<String> = None;

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let chunks = job.chunks.lock().await;
                let downloaded = job.downloaded(&chunks);
                let reached_end = all_chunks_reached_end(&chunks);
                let threads: Vec<ThreadProgress> = chunks.iter().map(|c| c.snapshot()).collect();
                let durable = sidecar_chunks(&chunks);
                drop(chunks);

                let now = Instant::now();
                let dt = now.duration_since(last_instant).as_secs_f64().max(1e-6);
                let inst = (downloaded.saturating_sub(last_downloaded)) as f64 / dt;
                // Exponential moving average keeps the readout from flickering.
                smoothed_speed = if smoothed_speed == 0.0 { inst } else { smoothed_speed * 0.7 + inst * 0.3 };
                last_downloaded = downloaded;
                last_instant = now;

                let status = if job.cancelled.load(Ordering::Acquire) {
                    "canceled"
                } else if job.paused.load(Ordering::Acquire) {
                    "paused"
                } else {
                    "downloading"
                };

                // Persist the resume sidecar (cheap, off the hot path). Only
                // for ranged downloads where resuming is actually possible.
                if info.resumable && info.total_size > 0 {
                    let sc = Sidecar {
                        version: SIDECAR_VERSION,
                        url: url.clone(),
                        total_size: info.total_size,
                        resumable: true,
                        etag: info.etag.clone(),
                        last_modified: info.last_modified.clone(),
                        chunks: durable,
                    };
                    let _ = write_sidecar(&scp, &sc);
                }

                emit_progress(
                    sink.as_ref(),
                    &job,
                    downloaded,
                    smoothed_speed,
                    status,
                    threads,
                    None,
                    None,
                    None,
                );

                // Completion / failure detection.
                if job.cancelled.load(Ordering::Acquire) {
                    final_status = "canceled".into();
                    break;
                }
                if info.total_size > 0 && reached_end {
                    final_status = "complete".into();
                    break;
                }
            }

            // A worker exited -> a connection slot opened up.
            maybe = finished_rx.recv() => {
                if maybe.is_none() {
                    // All finished-senders dropped unexpectedly (defensive: the
                    // coordinator normally holds one until it breaks out itself).
                    final_status = "error".into();
                    final_error = Some("internal: worker channel closed early".into());
                    break;
                }

                if job.cancelled.load(Ordering::Acquire) {
                    final_status = "canceled".into();
                    break;
                }

                let mut chunks = job.chunks.lock().await;

                if info.total_size > 0 && all_chunks_reached_end(&chunks) {
                    drop(chunks);
                    final_status = "complete".into();
                    break;
                }

                // Phase 4: keep connections saturated by splitting the largest
                // remaining chunk for each free slot.
                while job.active.load(Ordering::Acquire) < job.max_connections {
                    match split_largest(&job, &mut chunks) {
                        Some(tail) => {
                            debug!(chunk = tail.id, start = tail.start, "dynamic split");
                            spawn_worker(&job, tail, &write_tx, &finished_tx);
                        }
                        None => break,
                    }
                }

                // Nothing left running: decide complete vs failed. For unknown
                // size (total == 0) we rely on every chunk reaching clean EOF
                // (`done`) with no failures, rather than a byte count.
                if job.active.load(Ordering::Acquire) == 0 {
                    let any_failed = chunks.iter().any(|c| c.failed.load(Ordering::Acquire));
                    let all_done = chunks.iter().all(|c| c.done.load(Ordering::Acquire));
                    let size_complete = info.total_size > 0 && all_chunks_reached_end(&chunks);
                    drop(chunks);
                    if !any_failed && (size_complete || all_done) {
                        final_status = "complete".into();
                    } else {
                        final_status = "error".into();
                        final_error = Some("all download segments failed after retries".into());
                    }
                    break;
                }
            }
        }
    }

    // ---- Teardown: close the writer & flush ----------------------------
    drop(write_tx); // last sender held by coordinator; workers' clones already dropped
                    // Wait for the flusher to drain the queue and fsync.
    let flush_result = flusher.await;
    manager.unregister(&id).await;

    // Emit a terminal event.
    let chunks = job.chunks.lock().await;
    let downloaded = job.downloaded(&chunks);
    let threads: Vec<ThreadProgress> = chunks.iter().map(|c| c.snapshot()).collect();
    drop(chunks);

    if let Err(e) = flush_result {
        let msg = format!("disk flush failed: {e}");
        emit_progress(
            sink.as_ref(),
            &job,
            downloaded,
            0.0,
            "error",
            threads,
            Some(msg.clone()),
            None,
            None,
        );
        return Err(anyhow!(msg));
    }

    match final_status.as_str() {
        "complete" => {
            // ---- Optional integrity verification (opt-in) ---------------
            let (sha256, verified) = if opts.verify {
                let path = dest.clone();
                match tokio::task::spawn_blocking(move || hash_file(&path)).await {
                    Ok(Ok((sha, md5))) => {
                        let verified = opts.expected_checksum.as_ref().map(|exp| {
                            let e = exp.trim().to_ascii_lowercase();
                            e == sha || e == md5
                        });
                        info!(sha256 = %sha, ?verified, "integrity check complete");
                        (Some(sha), verified)
                    }
                    other => {
                        warn!(?other, "integrity hashing failed");
                        (None, Some(false))
                    }
                }
            } else {
                (None, None)
            };

            // The download succeeded — the sidecar is no longer needed.
            let _ = std::fs::remove_file(&scp);
            info!("download complete");
            emit_progress(
                sink.as_ref(),
                &job,
                info.total_size.max(downloaded),
                0.0,
                "complete",
                threads,
                None,
                sha256,
                verified,
            );
            Ok(dest)
        }
        "canceled" => {
            // Cancel is an explicit abort: drop both the partial file and its
            // resume sidecar.
            let _ = std::fs::remove_file(&dest);
            let _ = std::fs::remove_file(&scp);
            info!("download canceled");
            emit_progress(
                sink.as_ref(),
                &job,
                downloaded,
                0.0,
                "canceled",
                threads,
                None,
                None,
                None,
            );
            Err(anyhow!("download canceled"))
        }
        _ => {
            let msg = final_error.unwrap_or_else(|| "download failed".into());
            warn!(error = %msg, "download failed");
            emit_progress(
                sink.as_ref(),
                &job,
                downloaded,
                0.0,
                "error",
                threads,
                Some(msg.clone()),
                None,
                None,
            );
            Err(anyhow!(msg))
        }
    }
}

fn spawn_worker(
    job: &Arc<DownloadJob>,
    chunk: Arc<Chunk>,
    write_tx: &mpsc::Sender<WriteCmd>,
    finished_tx: &mpsc::Sender<()>,
) {
    job.active.fetch_add(1, Ordering::SeqCst);
    tokio::spawn(run_worker(
        job.clone(),
        chunk,
        write_tx.clone(),
        finished_tx.clone(),
    ));
}

#[allow(clippy::too_many_arguments)]
fn emit_progress(
    sink: &dyn ProgressSink,
    job: &DownloadJob,
    downloaded: u64,
    speed: f64,
    status: &str,
    threads: Vec<ThreadProgress>,
    error: Option<String>,
    sha256: Option<String>,
    verified: Option<bool>,
) {
    let total = job.total_size;
    let progress = if total > 0 {
        ((downloaded as f64 / total as f64) * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };
    let eta = if speed > 1.0 && total > downloaded {
        (total - downloaded) as f64 / speed
    } else {
        -1.0
    };

    let payload = ProgressPayload {
        id: job.id.clone(),
        file_name: job.file_name.clone(),
        url: job.url.clone(),
        total,
        downloaded,
        speed,
        eta,
        progress,
        status: status.to_string(),
        connections: job.max_connections,
        active_threads: job.active.load(Ordering::Acquire),
        speed_limit: job.limiter.rate(),
        threads,
        error,
        sha256,
        verified,
        warnings: job.warnings.clone(),
    };
    sink.emit(payload);
}

// ===========================================================================
// Tests — deterministic engine exercises against a local axum fixture server.
// ===========================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        extract::State,
        http::{header, HeaderMap, StatusCode},
        response::{IntoResponse, Response},
        routing::get,
        Router,
    };
    use std::sync::atomic::AtomicBool;
    use std::sync::Mutex as StdMutex;

    /// How the fixture server should behave, to drive each engine code path.
    #[derive(Clone, Copy, PartialEq)]
    enum Behavior {
        /// Normal: HEAD advertises ranges; GET serves 206 slices.
        Ranges,
        /// Refuse HEAD (405) -> forces the `bytes=0-0` GET probe fallback.
        NoHead,
        /// No `Accept-Ranges` -> forces single-stream download.
        NoRanges,
        /// First ranged GET is truncated -> forces reconnect-from-`current`.
        DropFirst,
        /// Byte `at` never arrives: a range covering it is cut off just before
        /// it, and any range starting at/after it gets 404.
        FailFrom(usize),
        /// Ranges starting at 0 trickle out in delayed frames; all others are
        /// instant. Makes chunk 0 the slow straggler that gets split.
        SlowFirstRange,
        /// If the client advertises gzip, the representation becomes the gzip
        /// stream and Range/Content-Length refer to *encoded* bytes.
        GzipIfAsked,
        /// Like `Ranges`, plus this `Content-Disposition` on HEAD.
        Disposition(&'static str),
        /// Every request (HEAD and GET) answers 404.
        NotFound,
        /// Like `Ranges`, with these `ETag` / `Last-Modified` on HEAD.
        Validators(&'static str, &'static str),
        /// The first ranged GET answers 503; everything after is normal.
        First503,
    }

    /// Minimal valid gzip (stored deflate blocks) — no extra dev-dependency.
    fn gzip_stored(data: &[u8]) -> Vec<u8> {
        let mut crc = !0u32;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];
        let blocks: Vec<&[u8]> = if data.is_empty() {
            vec![&[]]
        } else {
            data.chunks(65_535).collect()
        };
        for (i, blk) in blocks.iter().enumerate() {
            out.push(u8::from(i == blocks.len() - 1));
            let n = blk.len() as u16;
            out.extend_from_slice(&n.to_le_bytes());
            out.extend_from_slice(&(!n).to_le_bytes());
            out.extend_from_slice(blk);
        }
        out.extend_from_slice(&(!crc).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out
    }

    fn wants_gzip(headers: &HeaderMap) -> bool {
        headers
            .get(header::ACCEPT_ENCODING)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("gzip"))
    }

    fn gzip_response(s: &TestServer, headers: &HeaderMap, head: bool) -> Response {
        let enc = gzip_stored(&s.body);
        let len = enc.len();
        let (status, slice, range) = match parse_range(headers, len) {
            Some((a, b)) if !head => (
                StatusCode::PARTIAL_CONTENT,
                enc[a..=b].to_vec(),
                Some(format!("bytes {a}-{b}/{len}")),
            ),
            _ => (StatusCode::OK, enc, None),
        };
        let mut rb = Response::builder()
            .status(status)
            .header(header::CONTENT_ENCODING, "gzip")
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_LENGTH, slice.len().to_string());
        if let Some(r) = range {
            rb = rb.header(header::CONTENT_RANGE, r);
        }
        let body = if head {
            Body::empty()
        } else {
            Body::from(slice)
        };
        rb.body(body).unwrap()
    }

    struct TestServer {
        body: Vec<u8>,
        behavior: Behavior,
        dropped: AtomicBool,
        /// (start, end) of every ranged GET, for resume assertions.
        requests: StdMutex<Vec<(usize, usize)>>,
        /// Last value seen for the `x-idm-test` header (auth-passthrough test).
        seen_header: StdMutex<Option<String>>,
        /// Every `Accept-Encoding` value received (compression test).
        accept_encodings: StdMutex<Vec<String>>,
    }

    fn capture_header(s: &TestServer, headers: &HeaderMap) {
        let ae = headers
            .get(header::ACCEPT_ENCODING)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("<none>");
        s.accept_encodings.lock().unwrap().push(ae.to_string());
        if let Some(v) = headers.get("x-idm-test").and_then(|v| v.to_str().ok()) {
            *s.seen_header.lock().unwrap() = Some(v.to_string());
        }
    }

    fn parse_range(headers: &HeaderMap, len: usize) -> Option<(usize, usize)> {
        let raw = headers.get(header::RANGE)?.to_str().ok()?;
        let spec = raw.strip_prefix("bytes=")?;
        let mut parts = spec.splitn(2, '-');
        let start: usize = parts.next()?.parse().ok()?;
        let end_s = parts.next()?;
        let end = if end_s.is_empty() {
            len - 1
        } else {
            end_s.parse().ok()?
        };
        Some((start, end.min(len - 1)))
    }

    async fn head_handler(State(s): State<Arc<TestServer>>, headers: HeaderMap) -> Response {
        capture_header(&s, &headers);
        if s.behavior == Behavior::GzipIfAsked && wants_gzip(&headers) {
            return gzip_response(&s, &headers, true);
        }
        let len = s.body.len();
        match s.behavior {
            Behavior::NoHead => StatusCode::METHOD_NOT_ALLOWED.into_response(),
            Behavior::NotFound => StatusCode::NOT_FOUND.into_response(),
            Behavior::NoRanges => Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_LENGTH, len.to_string())
                .body(Body::empty())
                .unwrap(),
            Behavior::Disposition(cd) => Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_LENGTH, len.to_string())
                .header(header::ACCEPT_RANGES, "bytes")
                .header(header::CONTENT_DISPOSITION, cd)
                .body(Body::empty())
                .unwrap(),
            Behavior::Validators(etag, last_modified) => Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_LENGTH, len.to_string())
                .header(header::ACCEPT_RANGES, "bytes")
                .header(header::ETAG, etag)
                .header(header::LAST_MODIFIED, last_modified)
                .body(Body::empty())
                .unwrap(),
            _ => Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_LENGTH, len.to_string())
                .header(header::ACCEPT_RANGES, "bytes")
                .body(Body::empty())
                .unwrap(),
        }
    }

    async fn get_handler(State(s): State<Arc<TestServer>>, headers: HeaderMap) -> Response {
        capture_header(&s, &headers);
        if s.behavior == Behavior::GzipIfAsked && wants_gzip(&headers) {
            return gzip_response(&s, &headers, false);
        }
        if s.behavior == Behavior::NotFound {
            return StatusCode::NOT_FOUND.into_response();
        }
        if s.behavior == Behavior::First503
            && headers.contains_key(header::RANGE)
            && s.dropped
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        let len = s.body.len();
        if s.behavior == Behavior::NoRanges {
            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_LENGTH, len.to_string())
                .body(Body::from(s.body.clone()))
                .unwrap();
        }
        match parse_range(&headers, len) {
            Some((start, end)) => {
                s.requests.lock().unwrap().push((start, end));
                if let Behavior::FailFrom(at) = s.behavior {
                    if start >= at {
                        return StatusCode::NOT_FOUND.into_response();
                    }
                }
                let slice = s.body[start..=end].to_vec();
                let full_len = slice.len();
                if let Behavior::FailFrom(at) = s.behavior {
                    if at <= end {
                        // Deliver every byte before `at`, then abort the body
                        // (a plain short `Body` would make hyper send nothing).
                        // The pause lets hyper flush the data before the abort.
                        let items: Vec<std::io::Result<Vec<u8>>> = slice[..at - start]
                            .chunks(64 * 1024)
                            .map(|c| Ok(c.to_vec()))
                            .collect();
                        let cut = futures_util::stream::once(async {
                            tokio::time::sleep(Duration::from_millis(200)).await;
                            Err(std::io::Error::other("fixture: connection cut"))
                        });
                        let stream = futures_util::stream::iter(items).chain(cut);
                        return Response::builder()
                            .status(StatusCode::PARTIAL_CONTENT)
                            .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"))
                            .header(header::CONTENT_LENGTH, full_len.to_string())
                            .body(Body::from_stream(stream))
                            .unwrap();
                    }
                }
                if s.behavior == Behavior::SlowFirstRange && start == 0 {
                    let frames: Vec<Vec<u8>> =
                        slice.chunks(64 * 1024).map(|c| c.to_vec()).collect();
                    let stream = futures_util::stream::iter(frames).then(|f| async move {
                        tokio::time::sleep(Duration::from_millis(15)).await;
                        Ok::<_, std::io::Error>(f)
                    });
                    return Response::builder()
                        .status(StatusCode::PARTIAL_CONTENT)
                        .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"))
                        .header(header::CONTENT_LENGTH, full_len.to_string())
                        .body(Body::from_stream(stream))
                        .unwrap();
                }
                // Truncate exactly the first ranged GET when in DropFirst mode.
                let truncate = s.behavior == Behavior::DropFirst
                    && s.dropped
                        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok();
                let sent = if truncate {
                    slice[..full_len / 2].to_vec()
                } else {
                    slice
                };
                Response::builder()
                    .status(StatusCode::PARTIAL_CONTENT)
                    .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"))
                    .header(header::CONTENT_LENGTH, full_len.to_string())
                    .header(header::ACCEPT_RANGES, "bytes")
                    .body(Body::from(sent))
                    .unwrap()
            }
            None => Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_LENGTH, len.to_string())
                .body(Body::from(s.body.clone()))
                .unwrap(),
        }
    }

    async fn spawn_server(body: Vec<u8>, behavior: Behavior) -> (String, Arc<TestServer>) {
        let state = Arc::new(TestServer {
            body,
            behavior,
            dropped: AtomicBool::new(false),
            requests: StdMutex::new(Vec::new()),
            seen_header: StdMutex::new(None),
            accept_encodings: StdMutex::new(Vec::new()),
        });
        let app = Router::new()
            .route("/file", get(get_handler).head(head_handler))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}/file"), state)
    }

    #[derive(Default)]
    struct TestSink {
        last: StdMutex<Option<ProgressPayload>>,
    }
    impl ProgressSink for TestSink {
        fn emit(&self, payload: ProgressPayload) {
            *self.last.lock().unwrap() = Some(payload);
        }
    }

    fn tmp_dest(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("idm_test_{}_{}.bin", tag, uuid::Uuid::new_v4()));
        p
    }

    /// Deterministic pseudo-random body.
    fn make_body(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i % 251) as u8).collect()
    }

    fn opts(connections: usize) -> DownloadOptions {
        DownloadOptions {
            connections,
            ..Default::default()
        }
    }

    async fn run(
        url: String,
        dest: PathBuf,
        opts: DownloadOptions,
    ) -> (Result<PathBuf>, Arc<TestSink>) {
        let sink = Arc::new(TestSink::default());
        let dyn_sink: Arc<dyn ProgressSink> = sink.clone();
        let manager = Arc::new(DownloadManager::new());
        let client = build_client();
        let res = download_to_file(
            dyn_sink,
            manager,
            client,
            uuid::Uuid::new_v4().to_string(),
            url,
            dest,
            opts,
        )
        .await;
        (res, sink)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn multi_connection_split_reassembles() {
        // ~8 MiB triggers the dynamic-split path (chunks > MIN_SPLIT_BYTES).
        let body = make_body(8 * 1024 * 1024 + 12345);
        let (url, _) = spawn_server(body.clone(), Behavior::Ranges).await;
        let dest = tmp_dest("split");
        let (res, _) = run(url, dest.clone(), opts(8)).await;
        assert!(res.is_ok(), "download failed: {:?}", res.err());
        let written = std::fs::read(&dest).unwrap();
        assert_eq!(written.len(), body.len(), "size mismatch");
        assert_eq!(written, body, "reassembled bytes differ");
        std::fs::remove_file(&dest).ok();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn single_stream_fallback_no_ranges() {
        let body = make_body(512 * 1024);
        let (url, _) = spawn_server(body.clone(), Behavior::NoRanges).await;
        let dest = tmp_dest("noranges");
        let (res, _) = run(url, dest.clone(), opts(8)).await;
        assert!(res.is_ok(), "download failed: {:?}", res.err());
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        std::fs::remove_file(&dest).ok();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn head_refused_uses_get_probe_fallback() {
        let body = make_body(2 * 1024 * 1024);
        let (url, _) = spawn_server(body.clone(), Behavior::NoHead).await;
        let dest = tmp_dest("nohead");
        let (res, _) = run(url, dest.clone(), opts(4)).await;
        assert!(res.is_ok(), "download failed: {:?}", res.err());
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        std::fs::remove_file(&dest).ok();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn recovers_after_truncated_first_response() {
        // < 1 MiB => a single chunk, so the first (only) GET is the truncated one.
        let body = make_body(200 * 1024);
        let (url, _) = spawn_server(body.clone(), Behavior::DropFirst).await;
        let dest = tmp_dest("retry");
        let (res, _) = run(url, dest.clone(), opts(1)).await;
        assert!(res.is_ok(), "download failed: {:?}", res.err());
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            body,
            "bytes after reconnect differ"
        );
        std::fs::remove_file(&dest).ok();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn integrity_sha_matches_served_body() {
        use sha2::{Digest, Sha256};
        let body = make_body(1024 * 1024 + 99);
        let mut hasher = Sha256::new();
        hasher.update(&body);
        let expected = to_hex(&hasher.finalize());

        let (url, _) = spawn_server(body.clone(), Behavior::Ranges).await;
        let dest = tmp_dest("verify");
        let mut o = opts(4);
        o.verify = true;
        o.expected_checksum = Some(expected.clone());
        let (res, sink) = run(url, dest.clone(), o).await;
        assert!(res.is_ok());
        let last = sink.last.lock().unwrap().clone().unwrap();
        assert_eq!(last.sha256.as_deref(), Some(expected.as_str()));
        assert_eq!(last.verified, Some(true));
        std::fs::remove_file(&dest).ok();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancel_removes_partial_file() {
        let body = make_body(8 * 1024 * 1024);
        let (url, _) = spawn_server(body.clone(), Behavior::Ranges).await;
        let dest = tmp_dest("cancel");

        let sink = Arc::new(TestSink::default());
        let dyn_sink: Arc<dyn ProgressSink> = sink.clone();
        let manager = Arc::new(DownloadManager::new());
        let client = build_client();
        let id = uuid::Uuid::new_v4().to_string();

        let mut o = opts(4);
        o.speed_limit = 64 * 1024; // throttle so it cannot finish before we cancel

        let (id2, dest2, mgr2) = (id.clone(), dest.clone(), manager.clone());
        let handle = tokio::spawn(async move {
            download_to_file(dyn_sink, mgr2, client, id2, url, dest2, o).await
        });

        // Let it register + start, then cancel mid-flight.
        tokio::time::sleep(Duration::from_millis(400)).await;
        if let Some(job) = manager.get(&id).await {
            job.cancel();
        }

        let res = handle.await.unwrap();
        assert!(res.is_err(), "expected a cancellation error");
        assert!(!dest.exists(), "partial file should be removed on cancel");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn resumes_from_sidecar_without_refetching_prefix() {
        let total = 4 * 1024 * 1024usize;
        let half = total / 2;
        let body = make_body(total);
        let (url, server) = spawn_server(body.clone(), Behavior::Ranges).await;
        let dest = tmp_dest("resume");

        // Simulate an interrupted prior run: the first half is already on disk
        // (correct bytes), the rest is zeros, at full size.
        {
            let mut pre = body.clone();
            for b in &mut pre[half..] {
                *b = 0;
            }
            std::fs::write(&dest, &pre).unwrap();
        }
        // And a sidecar saying a single chunk [0,total) is half done.
        let scp = sidecar_path(&dest);
        write_sidecar(
            &scp,
            &Sidecar {
                version: SIDECAR_VERSION,
                url: url.clone(),
                total_size: total as u64,
                resumable: true,
                etag: None,
                last_modified: None,
                chunks: vec![ChunkRec {
                    start: 0,
                    current: half as u64,
                    end: total as u64,
                }],
            },
        )
        .unwrap();

        let (res, _) = run(url, dest.clone(), opts(4)).await;
        assert!(res.is_ok(), "resume download failed: {:?}", res.err());
        assert_eq!(std::fs::read(&dest).unwrap(), body, "final bytes wrong");

        // Prove we resumed from `current`: no ranged GET touched the prefix.
        let reqs = server.requests.lock().unwrap().clone();
        assert!(!reqs.is_empty(), "expected at least one ranged request");
        assert!(
            reqs.iter().all(|(start, _)| *start >= half),
            "a request re-fetched already-downloaded bytes: {reqs:?}"
        );
        // Sidecar is cleaned up on successful completion.
        assert!(!scp.exists(), "sidecar should be deleted after completion");
        std::fs::remove_file(&dest).ok();
    }

    /// Persistence: "kill the app mid-download, relaunch, resume". A kill
    /// leaves the last sidecar + the file; we snapshot exactly that (sidecar
    /// first, then the file) from a live throttled download, then resume from
    /// the snapshot. Bytes must be identical and no chunk may restart from 0.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn resume_after_simulated_kill_is_byte_identical() {
        let total = 3 * 1024 * 1024usize;
        let body = make_body(total);
        let (url, server) = spawn_server(body.clone(), Behavior::Ranges).await;
        let dest = tmp_dest("kill");

        let sink = Arc::new(TestSink::default());
        let manager = Arc::new(DownloadManager::new());
        let id = uuid::Uuid::new_v4().to_string();
        let mut o = opts(4);
        o.speed_limit = 1024 * 1024; // ~3 s total: plenty of time to "kill"
        let handle = {
            let (s, m, id, url, dest) = (
                sink.clone(),
                manager.clone(),
                id.clone(),
                url.clone(),
                dest.clone(),
            );
            tokio::spawn(
                async move { download_to_file(s, m, build_client(), id, url, dest, o).await },
            )
        };
        tokio::time::sleep(Duration::from_millis(1300)).await;

        // The state a killed process leaves behind.
        let snap = tmp_dest("kill_snap");
        let sidecar = std::fs::read(sidecar_path(&dest)).expect("sidecar exists mid-download");
        std::fs::write(sidecar_path(&snap), &sidecar).unwrap();
        std::fs::copy(&dest, &snap).unwrap();
        if let Some(job) = manager.get(&id).await {
            job.cancel(); // stop the original (it deletes its own files)
        }
        let _ = handle.await;

        let saved: Sidecar = serde_json::from_slice(&sidecar).unwrap();
        let resumed_from: Vec<u64> = saved.chunks.iter().map(|c| c.current).collect();
        assert!(
            saved.chunks.iter().any(|c| c.current > c.start),
            "test needs real progress before the kill: {resumed_from:?}"
        );
        server.requests.lock().unwrap().clear();

        let (res, _) = run(url, snap.clone(), opts(4)).await;
        assert!(res.is_ok(), "resume failed: {:?}", res.err());
        assert_eq!(std::fs::read(&snap).unwrap(), body, "resumed file differs");
        let reqs = server.requests.lock().unwrap().clone();
        for c in &saved.chunks {
            if c.current > c.start {
                assert!(
                    !reqs.iter().any(|(s, _)| (*s as u64) >= c.start && (*s as u64) < c.current),
                    "re-fetched bytes the sidecar said were done: chunk {}..{} current {}, requests {reqs:?}",
                    c.start,
                    c.end,
                    c.current
                );
            }
        }
        std::fs::remove_file(&snap).ok();
    }

    /// §8.2 #12: a sidecar is only trusted if the server's ETag/Last-Modified
    /// still match what was saved; otherwise the file changed upstream and the
    /// download must restart from 0. Old-format (v1) sidecars are rejected.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn sidecar_restore_requires_matching_validators() {
        const LM_A: &str = "Wed, 01 Oct 2026 10:00:00 GMT";
        const LM_B: &str = "Thu, 02 Oct 2026 10:00:00 GMT";
        let total = 2 * 1024 * 1024usize;
        let half = total / 2;
        let body = make_body(total);
        // (name, server behavior, sidecar version, saved etag, saved LM, restore?)
        type Case = (
            &'static str,
            Behavior,
            u32,
            Option<&'static str>,
            Option<&'static str>,
            bool,
        );
        let cases: [Case; 6] = [
            (
                "same validators",
                Behavior::Validators("\"v1\"", LM_A),
                2,
                Some("\"v1\""),
                Some(LM_A),
                true,
            ),
            (
                "etag changed",
                Behavior::Validators("\"v2\"", LM_A),
                2,
                Some("\"v1\""),
                Some(LM_A),
                false,
            ),
            (
                "last-modified changed",
                Behavior::Validators("\"v1\"", LM_B),
                2,
                Some("\"v1\""),
                Some(LM_A),
                false,
            ),
            (
                "server stopped sending them",
                Behavior::Ranges,
                2,
                Some("\"v1\""),
                Some(LM_A),
                false,
            ),
            (
                "neither side has any",
                Behavior::Ranges,
                2,
                None,
                None,
                true,
            ),
            (
                "old v1 sidecar",
                Behavior::Validators("\"v1\"", LM_A),
                1,
                None,
                None,
                false,
            ),
        ];
        for (name, behavior, version, etag, lm, restore) in cases {
            let (url, server) = spawn_server(body.clone(), behavior).await;
            let dest = tmp_dest("validators");
            let mut pre = body.clone();
            pre[half..].fill(0);
            std::fs::write(&dest, &pre).unwrap();
            let mut sc = serde_json::json!({
                "version": version,
                "url": url,
                "total_size": total,
                "resumable": true,
                "chunks": [{"start": 0, "current": half, "end": total}],
            });
            if version >= 2 {
                sc["etag"] = serde_json::json!(etag);
                sc["last_modified"] = serde_json::json!(lm);
            }
            std::fs::write(sidecar_path(&dest), serde_json::to_vec(&sc).unwrap()).unwrap();

            let (res, _) = run(url, dest.clone(), opts(2)).await;
            assert!(res.is_ok(), "{name}: download failed: {:?}", res.err());
            assert_eq!(std::fs::read(&dest).unwrap(), body, "{name}: bytes wrong");
            let reqs = server.requests.lock().unwrap().clone();
            let restored = reqs.iter().all(|(start, _)| *start >= half);
            assert_eq!(
                restored, restore,
                "{name}: restored={restored}, requests={reqs:?}"
            );
            std::fs::remove_file(&dest).ok();
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn custom_headers_are_sent() {
        let body = make_body(256 * 1024);
        let (url, server) = spawn_server(body.clone(), Behavior::Ranges).await;
        let dest = tmp_dest("headers");
        let mut o = opts(2);
        o.headers = vec![("x-idm-test".into(), "secret-token".into())];
        let (res, _) = run(url, dest.clone(), o).await;
        assert!(res.is_ok(), "download failed: {:?}", res.err());
        let seen = server.seen_header.lock().unwrap().clone();
        assert_eq!(
            seen.as_deref(),
            Some("secret-token"),
            "custom header was not forwarded to the server"
        );
        std::fs::remove_file(&dest).ok();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn mirrors_spread_load_and_reassemble() {
        let body = make_body(8 * 1024 * 1024);
        let (primary, s1) = spawn_server(body.clone(), Behavior::Ranges).await;
        let (mirror, s2) = spawn_server(body.clone(), Behavior::Ranges).await;
        let dest = tmp_dest("mirror");
        let mut o = opts(4);
        o.mirrors = vec![mirror];
        let (res, _) = run(primary, dest.clone(), o).await;
        assert!(res.is_ok(), "download failed: {:?}", res.err());
        assert_eq!(std::fs::read(&dest).unwrap(), body, "reassembly differs");
        assert!(
            !s1.requests.lock().unwrap().is_empty(),
            "primary served nothing"
        );
        assert!(
            !s2.requests.lock().unwrap().is_empty(),
            "mirror served nothing"
        );
        std::fs::remove_file(&dest).ok();
    }

    /// §8.1 #1: after a split, the victim can overshoot its new `end` (it
    /// streamed a frame past the midpoint before reloading `end`). Summing
    /// `current - start` then over-counts, so the job looked "complete" while
    /// the tail was still missing bytes. Here the tail's last byte never
    /// arrives, so the only correct outcome is an error — not `Ok` with a hole.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn split_overshoot_does_not_report_premature_complete() {
        let total = 2 * 1024 * 1024usize;
        let mid = total / 2;
        let overshoot = 64 * 1024u64;
        let body = make_body(total);
        let (url, _) = spawn_server(body.clone(), Behavior::FailFrom(total - 1)).await;
        let dest = tmp_dest("overshoot");

        // Seed the exact state a split race leaves behind: victim [0, mid)
        // finished with `current` past its shrunk `end`; tail [mid, total)
        // not started (zeros on disk).
        {
            let mut pre = body.clone();
            for b in &mut pre[mid..] {
                *b = 0;
            }
            std::fs::write(&dest, &pre).unwrap();
        }
        let scp = sidecar_path(&dest);
        write_sidecar(
            &scp,
            &Sidecar {
                version: SIDECAR_VERSION,
                url: url.clone(),
                total_size: total as u64,
                resumable: true,
                etag: None,
                last_modified: None,
                chunks: vec![
                    ChunkRec {
                        start: 0,
                        current: mid as u64 + overshoot,
                        end: mid as u64,
                    },
                    ChunkRec {
                        start: mid as u64,
                        current: mid as u64,
                        end: total as u64,
                    },
                ],
            },
        )
        .unwrap();

        let (res, _) = run(url, dest.clone(), opts(1)).await;
        let written = std::fs::read(&dest).unwrap();
        assert!(
            res.is_err(),
            "reported complete with a missing tail byte (byte {} = {}, expected {})",
            total - 1,
            written[total - 1],
            body[total - 1]
        );
        std::fs::remove_file(&dest).ok();
        std::fs::remove_file(&scp).ok();
    }

    /// §8.1 #4/#5: names from URLs / Content-Disposition are hostile input.
    #[test]
    fn file_names_are_sanitized() {
        let cases: &[(&str, Option<&str>, &str)] = &[
            // (url, content-disposition, expected)
            ("http://h/..\\..\\x", None, "x"),
            ("http://h/a%2Fb", None, "b"),
            ("http://h/a%5Cb.zip", None, "b.zip"),
            ("http://h/CON", None, "download"),
            ("http://h/con.txt", None, "download"),
            ("http://h/LPT9", None, "download"),
            ("http://h/%2e%2e%2f", None, "download"),
            ("http://h/..", None, "download"),
            ("http://h/", None, "download"),
            ("", None, "download"),
            ("http://h/my%20file.zip?x=1#f", None, "my file.zip"),
            ("http://h/a%3Cb%3E.txt", None, "a_b_.txt"),
            ("http://h/name...", None, "name"),
            // Content-Disposition wins over the URL…
            (
                "http://h/dl?id=5",
                Some("attachment; filename=\"report.pdf\""),
                "report.pdf",
            ),
            // …RFC 5987 `filename*` wins over `filename`…
            (
                "http://h/dl",
                Some("attachment; filename=\"fallback.txt\"; filename*=UTF-8''na%C3%AFve.txt"),
                "naïve.txt",
            ),
            // …but a hostile/unusable one falls back to the URL, then "download".
            (
                "http://h/ok.bin",
                Some("attachment; filename=\"..\\..\\x\""),
                "x",
            ),
            (
                "http://h/ok.bin",
                Some("attachment; filename=\"CON\""),
                "ok.bin",
            ),
            (
                "http://h/ok.bin",
                Some("attachment; filename=\"\""),
                "ok.bin",
            ),
            ("http://h/NUL", Some("inline"), "download"),
        ];
        for (url, cd, want) in cases {
            assert_eq!(choose_file_name(url, *cd), *want, "url={url:?} cd={cd:?}");
        }
        assert_eq!(safe_file_name(""), None);
        assert_eq!(safe_file_name("%2e%2e%2f"), None);
        assert_eq!(safe_file_name("a/b").as_deref(), Some("b"));
        assert_eq!(safe_file_name("CON"), None);
    }

    /// Kill-safety: bytes still queued for the flusher must not be recorded
    /// as downloaded in the sidecar, or a resume would leave zero-filled holes.
    #[test]
    fn sidecar_records_only_flushed_bytes() {
        let c = Arc::new(Chunk::new(0, 100, 1000));
        c.current.store(900, Ordering::Release); // queued up to 900
        c.written.store(600, Ordering::Release); // flushed up to 600
        let done = Arc::new(Chunk::new(1, 1000, 2000));
        done.current.store(2000, Ordering::Release);
        done.written.store(2000, Ordering::Release);
        let recs = sidecar_chunks(&[c.clone(), done]);
        assert_eq!(
            (recs[0].start, recs[0].current, recs[0].end),
            (100, 600, 1000)
        );
        assert_eq!(recs[1].current, 2000);
        // Never ahead of `current` either (e.g. right after a restore).
        c.written.store(950, Ordering::Release);
        assert_eq!(sidecar_chunks(&[c])[0].current, 900);
    }

    #[test]
    fn completion_predicate_ignores_split_overshoot() {
        // Victim overshot its shrunk end by 10 bytes; tail is 1 byte short.
        let victim = Arc::new(Chunk::new(0, 0, 100));
        victim.current.store(110, Ordering::Release);
        let tail = Arc::new(Chunk::new(1, 100, 200));
        tail.current.store(199, Ordering::Release);
        let chunks = vec![victim, tail.clone()];
        assert!(!all_chunks_reached_end(&chunks));
        tail.current.store(200, Ordering::Release);
        assert!(all_chunks_reached_end(&chunks));
        assert!(!all_chunks_reached_end(&[]));
    }

    /// §8.1 #4/#5: with no Save-As path the engine names the file from
    /// Content-Disposition — sanitized, so it cannot escape the directory.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dir_destination_uses_sanitized_content_disposition() {
        let body = make_body(300 * 1024);
        let (url, _) = spawn_server(
            body.clone(),
            Behavior::Disposition("attachment; filename=\"..\\..\\report v2.pdf\""),
        )
        .await;
        let dir = std::env::temp_dir().join(format!("idm_test_dir_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let sink = Arc::new(TestSink::default());
        let res = download_to_file(
            sink.clone(),
            Arc::new(DownloadManager::new()),
            build_client(),
            uuid::Uuid::new_v4().to_string(),
            url,
            Destination::Dir(dir.clone()),
            opts(2),
        )
        .await;
        let path = res.expect("download failed");
        assert_eq!(path, dir.join("report v2.pdf"));
        assert_eq!(std::fs::read(&path).unwrap(), body);
        let last = sink.last.lock().unwrap().clone().unwrap();
        assert_eq!(last.file_name, "report v2.pdf");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A browser's name hint beats the URL but never Content-Disposition,
    /// and is sanitized like any other untrusted name.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn name_hint_is_sanitized_and_yields_to_content_disposition() {
        // (see also join_inside_refuses_paths below)
        let body = make_body(64 * 1024);
        let dir = std::env::temp_dir().join(format!("idm_test_hint_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let hinted = |hint: &str| DownloadOptions {
            name_hint: Some(hint.into()),
            ..opts(2)
        };
        let get = |url: String, o: DownloadOptions| {
            download_to_file(
                Arc::new(TestSink::default()),
                Arc::new(DownloadManager::new()),
                build_client(),
                uuid::Uuid::new_v4().to_string(),
                url,
                Destination::Dir(dir.clone()),
                o,
            )
        };

        let (plain, _) = spawn_server(body.clone(), Behavior::Ranges).await;
        let path = get(plain.clone(), hinted("..\\..\\evil<1>.bin"))
            .await
            .unwrap();
        assert_eq!(path, dir.join("evil_1_.bin"));
        // An unusable hint falls back to the URL-derived name.
        let path = get(plain, hinted("NUL")).await.unwrap();
        assert_eq!(path.parent(), Some(dir.as_path()));
        assert_ne!(path.file_name().unwrap(), "NUL");

        let (named, _) = spawn_server(
            body,
            Behavior::Disposition("attachment; filename=\"server.pdf\""),
        )
        .await;
        let path = get(named, hinted("browser.pdf")).await.unwrap();
        assert_eq!(path, dir.join("server.pdf"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// §8.1 #3: a mirror serving a different file must be dropped (never
    /// used for a single range), and the user warned.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn mismatched_mirror_is_dropped() {
        let total = 6 * 1024 * 1024;
        let body = make_body(total);
        let (primary, _) = spawn_server(body.clone(), Behavior::Ranges).await;
        let (good, s_good) = spawn_server(body.clone(), Behavior::Ranges).await;
        // A different file of a different size.
        let (bad, s_bad) = spawn_server(vec![0xAB; total + 1000], Behavior::Ranges).await;
        let dest = tmp_dest("badmirror");
        let mut o = opts(6);
        o.mirrors = vec![good, bad.clone()];
        let (res, sink) = run(primary, dest.clone(), o).await;
        assert!(res.is_ok(), "download failed: {:?}", res.err());
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            body,
            "bad mirror corrupted output"
        );
        assert!(
            s_bad.requests.lock().unwrap().is_empty(),
            "a ranged GET went to the mismatched mirror"
        );
        assert!(
            !s_good.requests.lock().unwrap().is_empty(),
            "good mirror unused"
        );
        let last = sink.last.lock().unwrap().clone().unwrap();
        assert_eq!(last.warnings.len(), 1, "warnings: {:?}", last.warnings);
        assert!(
            last.warnings[0].contains("mirror dropped") && last.warnings[0].contains(&bad),
            "unexpected warning: {:?}",
            last.warnings
        );
        std::fs::remove_file(&dest).ok();
    }

    /// §8.2 #10: a 404 is permanent — fail in well under a second instead of
    /// burning ~16 s of backoff. Covers both the probe and the worker path.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn http_404_fails_fast() {
        for behavior in [Behavior::NotFound, Behavior::FailFrom(0)] {
            let (url, _) = spawn_server(make_body(256 * 1024), behavior).await;
            let dest = tmp_dest("404");
            let t0 = Instant::now();
            let (res, _) = run(url, dest.clone(), opts(4)).await;
            let took = t0.elapsed();
            assert!(res.is_err(), "a 404 download must fail");
            assert!(took < Duration::from_secs(1), "404 took {took:?} to fail");
            std::fs::remove_file(&dest).ok();
            std::fs::remove_file(sidecar_path(&dest)).ok();
        }
    }

    /// §8.2 #10: transient server errors keep their backoff-and-retry.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn http_503_is_retried_then_succeeds() {
        let body = make_body(200 * 1024);
        let (url, server) = spawn_server(body.clone(), Behavior::First503).await;
        let dest = tmp_dest("503");
        let (res, _) = run(url, dest.clone(), opts(1)).await;
        assert!(res.is_ok(), "503 should be retried: {:?}", res.err());
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert!(
            server.dropped.load(Ordering::SeqCst),
            "503 was never served"
        );
        std::fs::remove_file(&dest).ok();
    }

    /// Status classification and jitter bounds.
    #[test]
    fn retry_classification() {
        use reqwest::StatusCode as S;
        for s in [
            S::REQUEST_TIMEOUT,
            S::TOO_MANY_REQUESTS,
            S::INTERNAL_SERVER_ERROR,
            S::BAD_GATEWAY,
            S::SERVICE_UNAVAILABLE,
        ] {
            assert!(is_retryable_status(s), "{s} should retry");
        }
        for s in [
            S::BAD_REQUEST,
            S::UNAUTHORIZED,
            S::FORBIDDEN,
            S::NOT_FOUND,
            S::GONE,
            S::RANGE_NOT_SATISFIABLE,
        ] {
            assert!(!is_retryable_status(s), "{s} should fail fast");
        }
        // Jitter stays within [base/2, base].
        for n in 1..=8 {
            let base = backoff_base(n);
            for _ in 0..50 {
                let d = jittered(base);
                assert!(
                    d >= base / 2 && d <= base,
                    "{d:?} outside jitter range of {base:?}"
                );
            }
        }
    }

    /// §8.2 #7: two jobs (YouTube video + audio) sharing one limiter must
    /// respect the cap *in total*, not each.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn shared_limiter_caps_combined_throughput() {
        let rate = 512 * 1024u64; // 512 KiB/s total
        let size = 256 * 1024; // two files => 512 KiB => >= ~1 s when shared
        let limiter = Arc::new(RateLimiter::new(rate));
        let job = |tag: &'static str| {
            let limiter = limiter.clone();
            async move {
                let (url, _) = spawn_server(make_body(size), Behavior::Ranges).await;
                let mut o = opts(2);
                o.shared_limiter = Some(limiter);
                run(url, tmp_dest(tag), o).await
            }
        };
        let t0 = Instant::now();
        let ((ra, sa), (rb, sb)) = tokio::join!(job("shared_a"), job("shared_b"));
        let took = t0.elapsed();
        let (pa, pb) = (ra.expect("a failed"), rb.expect("b failed"));
        // Unshared, each would need only ~0.5 s. Allow a little burst slack.
        assert!(
            took >= Duration::from_millis(850),
            "combined 512 KiB at 512 KiB/s finished in {took:?}: cap not shared"
        );
        // Both report the shared cap to the UI.
        for s in [sa, sb] {
            assert_eq!(s.last.lock().unwrap().as_ref().unwrap().speed_limit, rate);
        }
        std::fs::remove_file(pa).ok();
        std::fs::remove_file(pb).ok();
    }

    /// §8.1 #2: a server that gzips the representation makes Range offsets
    /// refer to encoded bytes. The engine must ask for `identity` everywhere
    /// so offsets are raw file bytes and the output is byte-identical.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn gzip_capable_server_still_yields_identical_bytes() {
        let body = make_body(2 * 1024 * 1024 + 4321);
        let (url, server) = spawn_server(body.clone(), Behavior::GzipIfAsked).await;
        let dest = tmp_dest("gzip");
        let (res, _) = run(url, dest.clone(), opts(4)).await;
        let seen = server.accept_encodings.lock().unwrap().clone();
        assert!(
            res.is_ok(),
            "download failed: {:?} (accept-encodings: {seen:?})",
            res.err()
        );
        assert_eq!(std::fs::read(&dest).unwrap(), body, "bytes differ");
        assert!(!seen.is_empty());
        assert!(
            seen.iter().all(|v| v == "identity"),
            "a request did not ask for identity encoding: {seen:?}"
        );
        std::fs::remove_file(&dest).ok();
    }

    /// §8.1 #1 (live): chunk 0 is a slow straggler, so the coordinator splits
    /// it mid-flight. The output must be byte-identical and every chunk must
    /// have reached its `end` by the time "complete" is reported.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn live_split_with_latency_is_byte_identical() {
        let total = 4 * 1024 * 1024 + 777;
        let body = make_body(total);
        let (url, server) = spawn_server(body.clone(), Behavior::SlowFirstRange).await;
        let dest = tmp_dest("livesplit");
        let (res, sink) = run(url, dest.clone(), opts(2)).await;
        assert!(res.is_ok(), "download failed: {:?}", res.err());
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            body,
            "bytes differ after split"
        );

        // A ranged GET starting somewhere other than the two initial chunk
        // boundaries proves a dynamic split actually happened.
        let base = total / 2;
        let reqs = server.requests.lock().unwrap().clone();
        assert!(
            reqs.iter().any(|(s, _)| *s != 0 && *s != base),
            "no dynamic split occurred: {reqs:?}"
        );
        let last = sink.last.lock().unwrap().clone().unwrap();
        assert_eq!(last.status, "complete");
        assert!(
            last.threads.iter().all(|t| t.current >= t.end),
            "complete reported while a chunk was unfinished"
        );
        std::fs::remove_file(&dest).ok();
    }
}

#[cfg(test)]
mod join_tests {
    use super::join_inside;
    use std::path::Path;

    #[test]
    fn join_inside_refuses_paths() {
        let dir = Path::new("downloads");
        assert_eq!(join_inside(dir, "a.zip").unwrap(), dir.join("a.zip"));
        for bad in ["../a.zip", "..", "sub/a.zip", "", ".", "/etc/passwd"] {
            assert!(join_inside(dir, bad).is_err(), "{bad:?}");
        }
        #[cfg(windows)]
        for bad in [r"C:\x.zip", r"..\a.zip", r"sub\a.zip"] {
            assert!(join_inside(dir, bad).is_err(), "{bad:?}");
        }
    }
}
