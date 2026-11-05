// Prevents an extra console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_settings;
mod bridge;
mod browser_ext;
mod components;
mod downloader;
mod history;
mod instance_lock;
mod logs;
mod muxer;
mod opener;
mod process;
mod queue;
mod shell;
mod stream_dl;
#[cfg(test)]
mod testutil;
mod youtube;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

use bridge::{ErrorCode, HostError};
use downloader::{
    build_client, choose_file_name, download_to_file, safe_file_name, Destination, DownloadManager,
    DownloadOptions, ProgressPayload, ProgressSink, RateLimiter,
};
use history::{
    entry_from_record, history_file, History, JobKind, JobRecord, JobStatus, JsonFileStore,
    SAVE_DEBOUNCE,
};
use queue::{now_secs, Priority, QueueEntry, QueueHost, QueueManager, QueueStatus};

/// Cancel handles for running stream-site pipelines (by parent id), so
/// `cancel_download` can stop yt-dlp before any download job exists.
type StreamTokens = Arc<StdMutex<HashMap<String, youtube::CancelToken>>>;

/// Shared application state injected into every Tauri command.
struct AppState {
    manager: Arc<DownloadManager>,
    queue: Arc<QueueManager>,
    client: reqwest::Client,
    history: Arc<History>,
    streams: StreamTokens,
    logs: Arc<logs::LogBuffer>,
}

/// Everything a running download needs: app state plus the app handle.
/// Implements [`QueueHost`], so the queue's side effects land here.
#[derive(Clone)]
struct Ctx {
    app: AppHandle,
    manager: Arc<DownloadManager>,
    queue: Arc<QueueManager>,
    client: reqwest::Client,
    history: Arc<History>,
    streams: StreamTokens,
}

impl Ctx {
    fn new(app: &AppHandle, s: &AppState) -> Self {
        Self {
            app: app.clone(),
            manager: s.manager.clone(),
            queue: s.queue.clone(),
            client: s.client.clone(),
            history: s.history.clone(),
            streams: s.streams.clone(),
        }
    }

    fn sink(&self) -> Arc<dyn ProgressSink> {
        Arc::new(TauriSink {
            app: self.app.clone(),
            history: self.history.clone(),
        })
    }
}

fn lock<T>(m: &StdMutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------
// Progress sink (engine -> IPC + history)
// ---------------------------------------------------------------------------

/// Bridges the GUI-agnostic engine to Tauri's IPC by broadcasting each progress
/// payload as a `download-progress` event, and records it in the history
/// (the engine emits every ~400 ms per job — never per network frame).
struct TauriSink {
    app: AppHandle,
    history: Arc<History>,
}

impl ProgressSink for TauriSink {
    fn emit(&self, payload: ProgressPayload) {
        self.history.on_progress(&payload, now_secs());
        let _ = self.app.emit_all("download-progress", payload);
    }
}

// ---------------------------------------------------------------------------
// Queue host: launching, status events, schedule timers
// ---------------------------------------------------------------------------

impl QueueHost for Ctx {
    fn launch(&self, entry: QueueEntry) {
        // Live from now on: if the app dies mid-run this becomes "interrupted".
        self.history
            .set_status(&entry.id, JobStatus::Downloading, None, now_secs());
        // Tell the UI right away (the engine's first progress tick comes
        // only after probing), e.g. so a resumed card leaves "interrupted".
        let _ = self
            .app
            .emit_all("queue-status", QueueStatus::left(&entry, "starting"));
        let ctx = self.clone();
        tauri::async_runtime::spawn(async move {
            run_entry(&ctx, entry).await;
            // A slot freed — pull in the next waiting download.
            ctx.queue.finished(&ctx, now_secs());
        });
    }

    fn queue_status(&self, status: QueueStatus) {
        let now = now_secs();
        let s = match status.status.as_str() {
            "scheduled" => Some(JobStatus::Scheduled),
            "canceled" => Some(JobStatus::Canceled),
            // A held entry is still waiting; `held` is recorded separately.
            "queued" | "held" => Some(JobStatus::Queued),
            _ => None,
        };
        if let Some(s) = s {
            self.history.set_status(&status.id, s, None, now);
            if status.position > 0 {
                self.history
                    .set_queue_meta(&status.id, status.priority, status.held, now);
            }
        }
        let _ = self.app.emit_all("queue-status", status);
    }

    fn arm_timer(&self, at: u64) {
        let ctx = self.clone();
        let delay = at.saturating_sub(now_secs());
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(delay)).await;
            ctx.queue.promote(&ctx, now_secs());
        });
    }
}

/// Execute one queued entry to completion (the queue slot is held for its whole
/// lifetime, including the FFmpeg mux for stream downloads).
async fn run_entry(ctx: &Ctx, entry: QueueEntry) {
    if entry.is_stream {
        let id = entry.id.clone();
        let cancel = youtube::CancelToken::default();
        lock(&ctx.streams).insert(id.clone(), cancel.clone());
        if entry.manifest {
            run_manifest_pipeline(ctx, entry, cancel).await;
        } else {
            run_youtube_pipeline(ctx, entry, cancel).await;
        }
        lock(&ctx.streams).remove(&id);
        return;
    }

    // Save-As (or a resumed download's recorded path) wins; otherwise let the
    // engine name the file after probing (Content-Disposition, then URL).
    let dest = match entry.save_path.clone() {
        Some(p) => Destination::File(p),
        None => Destination::Dir(entry.dir.clone()),
    };
    let opts = DownloadOptions {
        connections: entry.connections,
        speed_limit: entry.speed_limit,
        verify: entry.verify,
        expected_checksum: entry.expected_checksum.clone(),
        headers: entry.headers.clone(),
        mirrors: entry.mirrors.clone(),
        shared_limiter: None,
        name_hint: entry.name_hint.clone(),
    };
    let result = download_to_file(
        ctx.sink(),
        ctx.manager.clone(),
        ctx.client.clone(),
        entry.id.clone(),
        entry.url,
        dest,
        opts,
    )
    .await;
    if let Err(e) = result {
        // A user cancel isn't a problem; don't count it as a warning.
        if e.to_string() == "download canceled" {
            tracing::info!("download canceled");
        } else {
            tracing::warn!("download finished with error: {e:#}");
        }
        // Still "live" means the engine failed before it could report anything
        // itself (probe / pre-allocation). Report it through the normal sink so
        // both the history and the UI card see the error.
        if let Some(rec) = ctx.history.get(&entry.id) {
            if rec.status.is_live() {
                ctx.sink().emit(ProgressPayload::failed_early(
                    &entry.id,
                    &rec.file_name,
                    &rec.url,
                    // `{:#}` keeps the cause (e.g. "connection refused"), not just
                    // the outermost context ("range probe GET failed").
                    format!("{e:#}"),
                ));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Lightweight command return value
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartResult {
    id: String,
    kind: String, // "file" | "youtube"
    file_name: String,
}

fn default_download_dir(_app: &AppHandle) -> PathBuf {
    // Tauri 1.x exposes the OS Downloads dir via the api::path helper.
    tauri::api::path::download_dir()
        .or_else(dirs_download_fallback)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn dirs_download_fallback() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .map(|p| PathBuf::from(p).join("Downloads"))
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join("Downloads")))
}

// ---------------------------------------------------------------------------
// Command: enqueue a download (file or auto-detected stream site).
// ---------------------------------------------------------------------------

#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn start_download(
    app: AppHandle,
    state: State<'_, AppState>,
    url: String,
    connections: usize,
    save_dir: Option<String>,
    save_path: Option<String>,
    speed_limit: Option<u64>,
    verify: Option<bool>,
    expected_checksum: Option<String>,
    headers: Option<std::collections::HashMap<String, String>>,
    mirrors: Option<Vec<String>>,
    start_at: Option<u64>,
) -> Result<StartResult, String> {
    let ctx = Ctx::new(&app, &state);
    enqueue_download(
        &ctx,
        NewDownload {
            id: Uuid::new_v4().to_string(),
            url,
            connections,
            save_dir,
            save_path,
            speed_limit: speed_limit.unwrap_or(0),
            verify: verify.unwrap_or(false),
            expected_checksum,
            headers: headers.unwrap_or_default().into_iter().collect(),
            mirrors: mirrors.unwrap_or_default(),
            start_at,
            force_stream: None,
            name_hint: None,
            manifest: false,
            quality: None,
        },
    )
    .await
    .map_err(|e| e.message)
}

/// Everything needed to add a download, from the UI or the browser bridge.
struct NewDownload {
    id: String,
    url: String,
    connections: usize,
    save_dir: Option<String>,
    save_path: Option<String>,
    speed_limit: u64,
    verify: bool,
    expected_checksum: Option<String>,
    headers: Vec<(String, String)>,
    mirrors: Vec<String>,
    start_at: Option<u64>,
    /// `Some` = the caller already decided file vs. stream pipeline.
    force_stream: Option<bool>,
    /// Untrusted file-name suggestion (goes through `safe_file_name`).
    name_hint: Option<String>,
    /// `url` is an HLS/DASH manifest for yt-dlp (implies the stream path).
    manifest: bool,
    /// Stream quality (`best` / `audio` / a height).
    quality: Option<String>,
}

/// The single path that records + enqueues a new download.
async fn enqueue_download(ctx: &Ctx, d: NewDownload) -> Result<StartResult, HostError> {
    let url = d.url.trim().to_string();
    if url.is_empty() {
        return Err(HostError::new(ErrorCode::BadUrl, "URL is empty"));
    }

    let is_stream = d.manifest
        || d.force_stream
            .unwrap_or_else(|| youtube::is_stream_site(&url));

    // An explicit Save-As path wins; otherwise use the chosen/save directory.
    let save_path = d
        .save_path
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from);
    let dir = save_path
        .as_ref()
        .and_then(|p| p.parent().map(PathBuf::from))
        .or_else(|| {
            d.save_dir
                .filter(|s| !s.trim().is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| default_download_dir(&ctx.app));

    // Stream sites need yt-dlp + ffmpeg, and a JavaScript runtime to extract
    // a video page (not for a plain manifest); check now for immediate feedback.
    if is_stream {
        youtube::preflight_check(!d.manifest)
            .await
            .map_err(|(tool, msg)| {
                let code = match tool {
                    youtube::MissingTool::YtDlp => ErrorCode::YtdlpMissing,
                    youtube::MissingTool::JsRuntime => ErrorCode::JsRuntimeMissing,
                    youtube::MissingTool::Ffmpeg => ErrorCode::FfmpegMissing,
                };
                HostError::new(code, msg)
            })?;
    }

    // A site video is named by yt-dlp; a playlist from a page's player by the
    // page title the extension sent (a playlist has no title of its own).
    let name_hint = if is_stream && !d.manifest {
        None
    } else {
        d.name_hint.as_deref().and_then(safe_file_name)
    };
    let file_name = if is_stream && name_hint.is_none() {
        bridge::RESOLVING_NAME.to_string()
    } else if is_stream {
        name_hint.clone().unwrap_or_default()
    } else {
        save_path
            .as_ref()
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string()))
            .or_else(|| name_hint.clone())
            // Provisional display name; the engine may refine it from
            // Content-Disposition and reports the final one in progress events.
            .unwrap_or_else(|| choose_file_name(&url, None))
    };

    let headers: Vec<(String, String)> = d
        .headers
        .into_iter()
        .filter(|(k, _)| !k.trim().is_empty())
        .collect();
    let entry = QueueEntry {
        id: d.id.clone(),
        url,
        save_path,
        dir,
        file_name: file_name.clone(),
        connections: d.connections,
        speed_limit: d.speed_limit,
        verify: d.verify,
        expected_checksum: d.expected_checksum,
        headers: headers.clone(),
        mirrors: d
            .mirrors
            .into_iter()
            .filter(|m| !m.trim().is_empty())
            .collect(),
        is_stream,
        start_at: d.start_at,
        priority: Priority::Normal,
        held: false,
        name_hint,
        manifest: d.manifest,
        quality: d.quality,
    };

    let now = now_secs();
    ctx.history.insert(record_for(&entry, now), headers);
    ctx.queue.enqueue(ctx, entry, now);

    Ok(StartResult {
        id: d.id,
        // A manifest job reports as one engine-style card.
        kind: if is_stream && !d.manifest {
            "youtube"
        } else {
            "file"
        }
        .into(),
        file_name,
    })
}

/// The initial history record for a freshly added download.
fn record_for(e: &QueueEntry, now: u64) -> JobRecord {
    JobRecord {
        id: e.id.clone(),
        kind: if e.manifest {
            JobKind::Manifest
        } else if e.is_stream {
            JobKind::Youtube
        } else {
            JobKind::File
        },
        url: e.url.clone(),
        mirrors: e.mirrors.clone(),
        header_names: Vec::new(), // filled by History::insert (names only)
        dir: e.dir.clone(),
        dest: e.save_path.clone(),
        file_name: e.file_name.clone(),
        status: if e.start_at.is_some_and(|t| t > now) {
            JobStatus::Scheduled
        } else {
            JobStatus::Queued
        },
        total: 0,
        downloaded: 0,
        connections: e.connections,
        speed_limit: e.speed_limit,
        verify: e.verify,
        expected_checksum: e.expected_checksum.clone(),
        start_at: e.start_at,
        priority: e.priority,
        held: e.held,
        quality: e.quality.clone(),
        created_at: now,
        updated_at: now,
        error: None,
    }
}

// ---------------------------------------------------------------------------
// YouTube / adaptive-stream pipeline (dual segmented download + mux).
// ---------------------------------------------------------------------------

async fn run_youtube_pipeline(ctx: &Ctx, entry: QueueEntry, cancel: youtube::CancelToken) {
    let parent_id = entry.id;
    let dir = entry.dir;
    let connections = entry.connections;
    let speed_limit = entry.speed_limit;
    let headers = entry.headers;

    let streams = match youtube::extract(&entry.url, &headers, &cancel).await {
        Ok(_) | Err(_) if cancel.is_cancelled() => {
            emit_youtube_status(ctx, &parent_id, "(canceled)", "canceled", None);
            return;
        }
        Ok(s) => s,
        Err(e) => {
            // Sites change often; an outdated yt-dlp is the usual cause.
            shell::update_after_extraction_failure(&ctx.app);
            emit_youtube_status(ctx, &parent_id, "(failed)", "error", Some(e.to_string()));
            return;
        }
    };

    let safe_title = sanitize_title(&streams.title);
    let display_name = format!("{safe_title}.mp4");
    let video_tmp = dir.join(format!(".{parent_id}.video.{}", streams.video_ext));
    let audio_tmp = dir.join(format!(".{parent_id}.audio.{}", streams.audio_ext));
    let output = dir.join(&display_name);

    // One limiter for both streams so the user's cap is the *total* rate.
    let limiter = Arc::new(RateLimiter::new(speed_limit));
    let opts = || DownloadOptions {
        connections,
        headers: headers.clone(),
        shared_limiter: Some(limiter.clone()),
        ..Default::default()
    };
    let video_id = format!("{parent_id}::video");
    let audio_id = format!("{parent_id}::audio");

    // Forward a cancel to both jobs. It is re-sent until the pipeline ends,
    // because a job only becomes reachable once it registers (after probing).
    let watcher = {
        let (cancel, manager) = (cancel.clone(), ctx.manager.clone());
        let ids = [video_id.clone(), audio_id.clone()];
        tokio::spawn(async move {
            cancel.cancelled().await;
            loop {
                for id in &ids {
                    if let Some(job) = manager.get(id).await {
                        job.cancel();
                    }
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
    };

    let video_fut = download_to_file(
        ctx.sink(),
        ctx.manager.clone(),
        ctx.client.clone(),
        video_id,
        streams.video_url,
        video_tmp.clone(),
        opts(),
    );
    let audio_fut = download_to_file(
        ctx.sink(),
        ctx.manager.clone(),
        ctx.client.clone(),
        audio_id,
        streams.audio_url,
        audio_tmp.clone(),
        opts(),
    );

    let (rv, ra) = tokio::join!(video_fut, audio_fut);
    watcher.abort();
    if cancel.is_cancelled() {
        let _ = tokio::fs::remove_file(&video_tmp).await;
        let _ = tokio::fs::remove_file(&audio_tmp).await;
        emit_youtube_status(ctx, &parent_id, &display_name, "canceled", None);
        return;
    }
    match (rv, ra) {
        (Ok(v), Ok(a)) => {
            emit_youtube_status(ctx, &parent_id, &display_name, "muxing", None);
            match muxer::mux(&v, &a, &output).await {
                Ok(_) => emit_youtube_status(ctx, &parent_id, &display_name, "complete", None),
                Err(e) => emit_youtube_status(
                    ctx,
                    &parent_id,
                    &display_name,
                    "error",
                    Some(e.to_string()),
                ),
            }
        }
        (rv, ra) => {
            let msg = rv
                .err()
                .map(|e| e.to_string())
                .or_else(|| ra.err().map(|e| e.to_string()));
            let _ = tokio::fs::remove_file(&video_tmp).await;
            let _ = tokio::fs::remove_file(&audio_tmp).await;
            emit_youtube_status(ctx, &parent_id, &display_name, "error", msg);
        }
    }
}

/// An HLS/DASH manifest from the browser: yt-dlp downloads (and, for DASH,
/// merges) it as ONE job; progress goes through the normal sink so the card,
/// history and cancel behave like any download. Cancel kills yt-dlp's tree.
async fn run_manifest_pipeline(ctx: &Ctx, entry: QueueEntry, cancel: youtube::CancelToken) {
    let sink = ctx.sink();
    let quality = entry
        .quality
        .as_deref()
        .and_then(stream_dl::Quality::parse)
        .unwrap_or(stream_dl::Quality::Best);
    let temp_dir = entry.dir.join(format!(".idm-{}.parts", entry.id));
    let payload =
        |status: &str| ProgressPayload::simple(&entry.id, &entry.file_name, &entry.url, status);
    sink.emit(payload("downloading"));

    let mut last_emit = std::time::Instant::now();
    let result = stream_dl::download(
        stream_dl::StreamJob {
            program: &components::tool_program(components::Tool::YtDlp),
            extra_args: &components::yt_dlp_args(),
            url: &entry.url,
            headers: &entry.headers,
            dir: &entry.dir,
            temp_dir: &temp_dir,
            quality,
            name: entry.name_hint.as_deref(),
        },
        &cancel,
        |p| {
            // yt-dlp prints several lines a second; the UI needs ~2.5/s.
            if last_emit.elapsed() < Duration::from_millis(400) {
                return;
            }
            last_emit = std::time::Instant::now();
            let mut pl = payload("downloading");
            pl.downloaded = p.downloaded;
            pl.total = p.total.unwrap_or(0).max(p.downloaded);
            pl.speed = p.speed.unwrap_or(0.0);
            pl.eta = p.eta.unwrap_or(-1.0);
            pl.progress = if pl.total > 0 {
                (pl.downloaded as f64 / pl.total as f64 * 100.0).min(99.9)
            } else {
                0.0
            };
            pl.active_threads = 1;
            sink.emit(pl);
        },
    )
    .await;

    match result {
        Ok(path) => {
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| entry.file_name.clone());
            let mut pl = ProgressPayload::simple(&entry.id, &name, &entry.url, "complete");
            pl.total = size;
            pl.downloaded = size;
            pl.progress = 100.0;
            sink.emit(pl);
            tracing::info!("stream download complete");
        }
        Err(stream_dl::StreamError::Canceled) => {
            tracing::info!("stream download canceled");
            sink.emit(payload("canceled"));
        }
        Err(e) => {
            tracing::warn!("stream download failed: {e}");
            let mut pl = payload("error");
            pl.error = Some(e.to_string());
            sink.emit(pl);
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct YoutubeStatus {
    id: String,
    file_name: String,
    status: String,
    error: Option<String>,
}

fn emit_youtube_status(ctx: &Ctx, id: &str, file_name: &str, status: &str, error: Option<String>) {
    ctx.history
        .on_youtube_status(id, file_name, status, error.clone(), now_secs());
    let _ = ctx.app.emit_all(
        "youtube-status",
        YoutubeStatus {
            id: id.to_string(),
            file_name: file_name.to_string(),
            status: status.to_string(),
            error,
        },
    );
}

fn sanitize_title(t: &str) -> String {
    let cleaned: String = t
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').to_string();
    if trimmed.is_empty() {
        "video".into()
    } else {
        trimmed.chars().take(120).collect()
    }
}

// ---------------------------------------------------------------------------
// Control commands
// ---------------------------------------------------------------------------

/// Pause a running download, or hold a waiting one in its queue position.
#[tauri::command]
async fn pause_download(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let ctx = Ctx::new(&app, &state);
    if ctx.queue.set_held(&ctx, &id, true, now_secs()) {
        return Ok(());
    }
    for variant in id_variants(&id) {
        if let Some(job) = state.manager.get(&variant).await {
            job.pause();
        }
    }
    Ok(())
}

/// Resume a paused download, or release a held waiting one.
#[tauri::command]
async fn resume_download(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let ctx = Ctx::new(&app, &state);
    if ctx.queue.set_held(&ctx, &id, false, now_secs()) {
        return Ok(());
    }
    for variant in id_variants(&id) {
        if let Some(job) = state.manager.get(&variant).await {
            job.resume();
        }
    }
    Ok(())
}

#[tauri::command]
async fn cancel_download(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    // If it's still waiting in the queue, just drop it from the list.
    let ctx = Ctx::new(&app, &state);
    if ctx.queue.cancel_pending(&ctx, &id, now_secs()).is_some() {
        return Ok(());
    }

    // Otherwise it's running. A stream-site pipeline may still be extracting
    // (no jobs yet): its token kills yt-dlp and is forwarded to the jobs.
    let stream = lock(&state.streams).get(&id).cloned();
    if let Some(token) = stream {
        token.cancel();
    }
    // Signal the live job(s).
    for variant in id_variants(&id) {
        if let Some(job) = state.manager.get(&variant).await {
            job.cancel();
        }
    }
    Ok(())
}

/// A YouTube parent id controls both its `::video` and `::audio` jobs.
fn id_variants(id: &str) -> Vec<String> {
    vec![
        id.to_string(),
        format!("{id}::video"),
        format!("{id}::audio"),
    ]
}

/// Change a download's speed cap live (bytes/sec; 0 = unlimited).
#[tauri::command]
async fn set_speed_limit(
    state: State<'_, AppState>,
    id: String,
    bytes_per_sec: u64,
) -> Result<(), String> {
    for variant in id_variants(&id) {
        if let Some(job) = state.manager.get(&variant).await {
            job.set_speed_limit(bytes_per_sec);
        }
    }
    Ok(())
}

/// Set how many downloads may run at once, then promote any that now fit.
#[tauri::command]
async fn set_max_concurrent(
    app: AppHandle,
    state: State<'_, AppState>,
    max: usize,
) -> Result<(), String> {
    let ctx = Ctx::new(&app, &state);
    ctx.queue.set_max_concurrent(max);
    ctx.queue.promote(&ctx, now_secs());
    Ok(())
}

// ---------------------------------------------------------------------------
// History commands (persistence)
// ---------------------------------------------------------------------------

/// `history-removed` event payload.
#[derive(Clone, Serialize)]
struct HistoryRemoved {
    ids: Vec<String>,
}

/// Every known download, oldest first. The UI calls this once on mount to
/// rebuild its cards; live changes then arrive as events.
#[tauri::command]
fn list_history(state: State<'_, AppState>) -> Vec<JobRecord> {
    state.history.list()
}

/// Forget one finished / failed / interrupted download. Files on disk
/// (including a partial file and its `.idm` sidecar) are left untouched.
#[tauri::command]
fn remove_history_entry(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    state.history.remove(&id)?;
    let _ = app.emit_all("history-removed", HistoryRemoved { ids: vec![id] });
    Ok(())
}

/// Forget every completed or canceled download.
#[tauri::command]
fn clear_completed(app: AppHandle, state: State<'_, AppState>) -> Vec<String> {
    let ids = state.history.clear_completed();
    if !ids.is_empty() {
        let _ = app.emit_all("history-removed", HistoryRemoved { ids: ids.clone() });
    }
    ids
}

/// Restart an interrupted, failed or canceled download with its saved
/// settings. A plain file reuses its recorded path, so the engine picks up the
/// `.idm` sidecar and continues from where it stopped.
#[tauri::command]
fn retry_download(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    let ctx = Ctx::new(&app, &state);
    let rec = ctx
        .history
        .get(&id)
        .ok_or_else(|| "no such download".to_string())?;
    if !rec.status.is_retryable() {
        return Err(format!("cannot retry a download that is {:?}", rec.status).to_lowercase());
    }
    let entry = entry_from_record(&rec, ctx.history.headers(&id), None)?;
    let now = now_secs();
    ctx.history.set_start_at(&id, None, now);
    ctx.history.set_status(&id, JobStatus::Queued, None, now);
    ctx.queue.enqueue(&ctx, entry, now);
    Ok(())
}

/// Move a waiting download `delta` places in the queue (negative = earlier).
/// It stays within its priority band; the backend re-emits `queue-status`.
#[tauri::command]
fn move_in_queue(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    delta: i32,
) -> Result<(), String> {
    let ctx = Ctx::new(&app, &state);
    if ctx.queue.move_by(&ctx, &id, delta, now_secs()) {
        Ok(())
    } else {
        Err("only waiting downloads can be reordered".into())
    }
}

/// Set a waiting download's priority (`high` | `normal` | `low`).
#[tauri::command]
fn set_queue_priority(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    priority: Priority,
) -> Result<(), String> {
    let ctx = Ctx::new(&app, &state);
    if ctx.queue.set_priority(&ctx, &id, priority, now_secs()) {
        Ok(())
    } else {
        Err("only waiting downloads have a queue priority".into())
    }
}

/// Open a download's file (`target: "file"`) or reveal it in its folder
/// (`"folder"`). Takes only an id: the path comes from the history record
/// and is validated (see `opener.rs`).
#[tauri::command]
fn open_download(state: State<'_, AppState>, id: String, target: String) -> Result<(), String> {
    let target = opener::OpenTarget::parse(&target)?;
    let rec = state
        .history
        .get(&id)
        .ok_or_else(|| "no such download".to_string())?;
    let action = opener::resolve(&rec, target)?;
    opener::perform(&action)
}

/// The log panel's backlog (newest last); new lines arrive as `log-line`.
#[tauri::command]
fn recent_logs(state: State<'_, AppState>) -> Vec<logs::LogLine> {
    state.logs.recent()
}

/// Open a native Save-As dialog; returns the chosen absolute path (or `None`).
#[tauri::command]
async fn pick_save_path(default_name: String, default_dir: Option<String>) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut builder = tauri::api::dialog::FileDialogBuilder::new();
    if !default_name.trim().is_empty() {
        builder = builder.set_file_name(&default_name);
    }
    if let Some(d) = default_dir.filter(|s| !s.trim().is_empty()) {
        builder = builder.set_directory(d);
    }
    builder.save_file(move |path| {
        let _ = tx.send(path);
    });
    rx.await
        .ok()
        .flatten()
        .map(|p| p.to_string_lossy().to_string())
}

/// Probe for the external binaries the YouTube pipeline needs.
#[tauri::command]
async fn check_prerequisites() -> Result<(), String> {
    youtube::preflight().await.map_err(|e| e.to_string())
}

#[tauri::command]
fn default_save_dir(app: AppHandle) -> String {
    default_download_dir(&app).to_string_lossy().to_string()
}

#[tauri::command]
fn suggested_connections() -> usize {
    // A sensible default: scale with cores, capped at the slider max.
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    (cores * 2).clamp(4, 16)
}

// ---------------------------------------------------------------------------
// Browser bridge (localhost HTTP API for the extension; see bridge.rs)
// ---------------------------------------------------------------------------

/// The app side of the bridge: starts jobs through the normal enqueue path
/// and mirrors bridge state to the UI as events.
struct TauriBridgeHost {
    app: AppHandle,
}

/// `bridge-request-removed` event payload.
#[derive(Clone, Serialize)]
struct BridgeRequestRemoved {
    id: String,
}

#[async_trait::async_trait]
impl bridge::BridgeHost for TauriBridgeHost {
    async fn start(&self, id: String, job: bridge::BridgeJob) -> Result<String, HostError> {
        let ctx = {
            let state = self.app.state::<AppState>();
            Ctx::new(&self.app, &state)
        };
        // The UI's per-download options live in the webview, so browser
        // downloads use the defaults: suggested connections, OS Downloads
        // folder, no speed cap (the queue's concurrency cap still applies).
        let started = enqueue_download(
            &ctx,
            NewDownload {
                id,
                url: job.url,
                connections: suggested_connections(),
                save_dir: None,
                save_path: None,
                speed_limit: 0,
                verify: false,
                expected_checksum: None,
                headers: job.headers,
                mirrors: Vec::new(),
                start_at: None,
                force_stream: Some(job.stream),
                name_hint: job.name_hint,
                manifest: job.manifest,
                quality: job.quality,
            },
        )
        .await?;
        Ok(started.file_name)
    }

    fn pending_added(&self, req: &bridge::PendingRequest) {
        let _ = self.app.emit_all("bridge-request", req);
        // Ask right away, in a popup above every other window.
        shell::show_confirm_window(&self.app);
    }

    fn pending_removed(&self, id: &str) {
        let _ = self.app.emit_all(
            "bridge-request-removed",
            BridgeRequestRemoved { id: id.into() },
        );
    }

    fn status_changed(&self, info: &bridge::BridgeInfo) {
        let _ = self.app.emit_all("bridge-status", info);
    }

    fn pairing_requested(&self, prompt: &bridge::PairingPrompt) {
        // The prompt must be seen even if the app sits in the tray.
        shell::show_main(&self.app);
        let _ = self.app.emit_all("pairing-request", prompt);
    }

    fn pairing_closed(&self, id: &str) {
        let _ = self
            .app
            .emit_all("pairing-closed", BridgeRequestRemoved { id: id.into() });
    }
}

type BridgeState<'a> = State<'a, Arc<bridge::Bridge>>;

#[tauri::command]
fn get_bridge_info(bridge: BridgeState<'_>) -> bridge::BridgeInfo {
    bridge.info()
}

/// Apply + persist bridge settings (starts/stops the server on `enabled`).
#[tauri::command]
async fn set_bridge_settings(
    bridge: BridgeState<'_>,
    settings: bridge::BridgeSettings,
) -> Result<bridge::BridgeInfo, String> {
    bridge.set_settings(settings).await
}

/// New random token; the old one stops working immediately.
#[tauri::command]
fn regenerate_token(bridge: BridgeState<'_>) -> Result<bridge::BridgeInfo, String> {
    bridge.regenerate_token()
}

/// Browser requests waiting for Accept / Dismiss (once on mount; then events).
#[tauri::command]
fn list_bridge_requests(bridge: BridgeState<'_>) -> Vec<bridge::PendingRequest> {
    bridge.pending()
}

#[tauri::command]
async fn accept_bridge_request(bridge: BridgeState<'_>, id: String) -> Result<(), String> {
    bridge.accept(&id).await.map(|_| ()).map_err(|e| e.message)
}

#[tauri::command]
fn dismiss_bridge_request(bridge: BridgeState<'_>, id: String) -> Result<(), String> {
    bridge.dismiss(&id).map_err(|e| e.message)
}

// ---------------------------------------------------------------------------
// Entrypoint
// ---------------------------------------------------------------------------

/// Another launch was attempted: bring our window forward, and offer any
/// http(s) URL it was given as a download (as an Accept/Dismiss card).
fn on_second_launch(app: &AppHandle, argv: &[String]) {
    let hooks = std::env::var_os(shell::TEST_HOOKS_ENV).is_some();
    if let Some(id) = shell::test_tray_action(hooks, argv) {
        tracing::info!(id, "test hook: tray menu action");
        shell::menu_action(app, id);
        return;
    }
    tracing::info!("second launch: focusing the running window");
    shell::show_main(app);
    let urls: Vec<String> = argv
        .iter()
        .skip(1)
        .filter(|a| {
            let a = a.trim().to_ascii_lowercase();
            a.starts_with("http://") || a.starts_with("https://")
        })
        .cloned()
        .collect();
    if urls.is_empty() {
        return;
    }
    let Some(bridge) = app.try_state::<Arc<bridge::Bridge>>() else {
        return;
    };
    let bridge = bridge.inner().clone();
    tauri::async_runtime::spawn(async move {
        for url in urls {
            if let Err(e) = bridge.submit_command_line(&url).await {
                tracing::warn!(code = ?e.code, "command-line URL refused: {}", e.message);
            }
        }
    });
}

/// The global log level from `RUST_LOG` (default `info`).
fn log_level() -> tracing_subscriber::filter::LevelFilter {
    use tracing_subscriber::filter::LevelFilter;
    match std::env::var("RUST_LOG")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "trace" => LevelFilter::TRACE,
        "debug" => LevelFilter::DEBUG,
        "warn" => LevelFilter::WARN,
        "error" => LevelFilter::ERROR,
        "off" => LevelFilter::OFF,
        _ => LevelFilter::INFO,
    }
}

fn main() {
    // Structured logging; control verbosity with RUST_LOG=trace|debug|info|warn|error
    // (one global level: per-target filters would pull a regex engine into the exe).
    // Also feeds the in-app log panel (INFO+ from this app, WARN+ from deps).
    let logs = Arc::new(logs::LogBuffer::default());
    {
        use tracing_subscriber::prelude::*;
        tracing_subscriber::registry()
            .with(log_level())
            .with(tracing_subscriber::fmt::layer())
            .with(logs::LogLayer(logs.clone()))
            .init();
    }

    let context = tauri::generate_context!();

    // Restore the persisted history before the UI exists; a missing or corrupt
    // file just means an empty list.
    let data_dir =
        tauri::api::path::app_data_dir(context.config()).unwrap_or_else(|| PathBuf::from("."));

    // One instance per data dir, BEFORE touching any persisted file. The OS
    // drops this lock if the holder crashes, so it can't go stale.
    let _instance =
        match instance_lock::InstanceLock::acquire(&data_dir.join(instance_lock::LOCK_FILE)) {
            Ok(lock) => {
                tracing::info!(path = %lock.path().display(), "instance lock held");
                Some(lock)
            }
            Err(instance_lock::LockError::Held) => {
                tracing::info!("IDM Replica is already running; handing over to it");
                // The plugin passes our arguments to the running instance (which
                // focuses its window) and exits this process during `build`.
                let _ = tauri::Builder::default()
                    .plugin(tauri_plugin_single_instance::init(|_, _, _| {}))
                    .build(context);
                // Only reached if the running window couldn't be found. Never
                // open the other instance's files.
                std::process::exit(0);
            }
            Err(e) => {
                tracing::warn!(error = %e, "continuing without the instance lock");
                None
            }
        };

    let bridge_config = data_dir.join(bridge::CONFIG_FILE);
    let local_dir =
        tauri::api::path::app_local_data_dir(context.config()).unwrap_or_else(|| data_dir.clone());
    let settings_path = data_dir.join(app_settings::SETTINGS_FILE);
    let app_settings = app_settings::AppSettings::load(&settings_path);
    let args: Vec<String> = std::env::args().collect();
    // Started by the "Start with Windows" entry and told to stay in the tray?
    let start_hidden = app_settings::launched_at_login(&args) && app_settings.start_minimized;
    let store = Arc::new(JsonFileStore::new(history_file(&data_dir)));
    let (history, requeue) = History::open(store, now_secs());
    let history = Arc::new(history);
    tracing::info!(
        path = %history_file(&data_dir).display(),
        entries = history.list().len(),
        requeued = requeue.len(),
        "download history loaded"
    );

    let app = tauri::Builder::default()
        // A second launch lands here (in THIS process) and then exits.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            on_second_launch(app, &argv);
        }))
        .system_tray(shell::tray())
        .on_system_tray_event(shell::on_tray_event)
        .on_window_event(|event| {
            // Closing the main window keeps the app (and browser take-over)
            // alive. The download-request popup really closes; its requests
            // stay in the main window's list.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event.event() {
                if event.window().label() == "main" {
                    api.prevent_close();
                    shell::hide_to_tray(event.window());
                }
            }
        })
        .manage(AppState {
            manager: Arc::new(DownloadManager::new()),
            queue: Arc::new(QueueManager::default()),
            client: build_client(),
            history: history.clone(),
            streams: Arc::default(),
            logs: logs.clone(),
        })
        .invoke_handler(tauri::generate_handler![
            start_download,
            pause_download,
            resume_download,
            cancel_download,
            set_speed_limit,
            set_max_concurrent,
            list_history,
            remove_history_entry,
            clear_completed,
            retry_download,
            move_in_queue,
            set_queue_priority,
            open_download,
            recent_logs,
            get_bridge_info,
            set_bridge_settings,
            regenerate_token,
            list_bridge_requests,
            accept_bridge_request,
            dismiss_bridge_request,
            pick_save_path,
            check_prerequisites,
            default_save_dir,
            suggested_connections,
            shell::get_components_status,
            shell::install_components,
            shell::ytdlp_version,
            shell::check_ytdlp_update,
            shell::set_ytdlp_auto_update,
            shell::get_app_settings,
            shell::set_app_settings,
            shell::get_pending_pairing,
            shell::answer_pairing,
            shell::close_confirm_window,
            shell::extension_setup_info,
            shell::open_extension_store,
            shell::open_browser_extensions_page,
            shell::reveal_unpacked_extension,
            shell::set_extension_setup_done,
        ])
        .setup(move |app| {
            let handle = app.handle();
            logs.set_sink(Box::new(move |line| {
                let _ = handle.emit_all("log-line", line);
            }));
            // Browser bridge: load settings + token, then listen on 127.0.0.1.
            let bridge = bridge::Bridge::open(
                Arc::new(TauriBridgeHost { app: app.handle() }),
                bridge_config,
                bridge::DEFAULT_PORT,
            );
            app.manage(bridge.clone());
            tauri::async_runtime::spawn(async move { bridge.ensure_running().await });
            // External tools (yt-dlp, QuickJS, FFmpeg): one resolver for
            // the whole app; anything missing is downloaded on first run.
            let components = Arc::new(components::Components::new(
                components::Manifest::embedded(),
                components::Locations::system(&local_dir),
                app.state::<AppState>().client.clone(),
                Arc::new(shell::TauriComponentEvents { app: app.handle() }),
            ));
            components::set_global(components.clone());
            // FULL installer: move the shipped tools somewhere updatable.
            let adopt = components.clone();
            tauri::async_runtime::spawn_blocking(move || adopt.adopt_bundled());
            let needs_setup = !components.missing().is_empty();
            app.manage(shell::ShellState {
                components,
                settings: StdMutex::new(app_settings),
                settings_path,
                setting_up: std::sync::atomic::AtomicBool::new(false),
            });
            if needs_setup {
                let handle = app.handle();
                tauri::async_runtime::spawn(async move { shell::run_setup(&handle).await });
            }
            shell::spawn_update_loop(app.handle());
            if !start_hidden {
                shell::show_main(&app.handle());
            }
            let ctx = Ctx::new(&app.handle(), app.state::<AppState>().inner());
            tauri::async_runtime::spawn(ctx.history.clone().run_saver(SAVE_DEBOUNCE));
            // Scheduled downloads come back on their own: future ones re-arm
            // their timer, past-due ones start per the queue rules.
            for id in requeue {
                let Some(rec) = ctx.history.get(&id) else {
                    continue;
                };
                match entry_from_record(&rec, None, rec.start_at) {
                    Ok(entry) => ctx.queue.enqueue(&ctx, entry, now_secs()),
                    Err(e) => {
                        ctx.history
                            .set_status(&id, JobStatus::Interrupted, Some(e), now_secs())
                    }
                }
            }
            Ok(())
        })
        .build(context)
        .expect("error while building tauri application");

    app.run(move |_, event| {
        // Normal shutdown: write the final state now (the debounced saver
        // may not get another tick).
        if let tauri::RunEvent::Exit = event {
            if let Err(e) = history.flush() {
                tracing::warn!(error = %e, "could not save download history on exit");
            }
        }
    });
}
