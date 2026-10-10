//! Extraction layer for adaptive-streaming platforms (YouTube et al.).
//!
//! Stream sites are handled behind an [`Extractor`] provider trait so new
//! platforms can be added without touching the download engine or the muxer —
//! a provider's only job is to turn a *page* URL into two *direct* stream URLs.
//! The first (and currently only) provider, [`YtDlpExtractor`], shells out to
//! `yt-dlp` (the maintained `youtube-dl` fork), which is the only robust way to
//! resolve YouTube's frequently-changing signature-ciphered DASH manifests.
//!
//! ## Adding a second provider
//! Implement [`Extractor`] (a `detect` host check + an async `extract`) and add
//! an instance to [`providers`]. Detection runs in order, first match wins. A
//! native, no-subprocess provider for a specific site would slot in here.

use std::ffi::OsString;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::process;
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use tokio::sync::Notify;
use tracing::info;

/// yt-dlp format selector: best video-only (prefer mp4 for a fast copy-mux)
/// **plus** best audio-only (prefer m4a). `+` binds tighter than `/`, hence the
/// parentheses. One selector => one yt-dlp run returns both streams.
/// `[protocol^=http]` keeps only plain HTTP(S) files: HLS (`m3u8_native`) URLs
/// point at a playlist, not media — downloading one yields a text file that
/// FFmpeg can't mux.
const YT_DLP_FORMAT: &str = "(bv*[ext=mp4][protocol^=http]/bv*[protocol^=http])+(ba[ext=m4a][protocol^=http]/ba[protocol^=http])";

/// The resolved, directly-downloadable streams for a watch URL.
#[derive(Debug)]
pub struct ExtractedStreams {
    pub title: String,
    pub video_url: String,
    /// Container extension for the video-only stream (e.g. `mp4`, `webm`).
    pub video_ext: String,
    pub audio_url: String,
    pub audio_ext: String,
}

/// Cooperative cancellation for work that runs before a download job exists
/// (extraction). Cheap to clone; `cancel` is sticky and wakes every waiter.
#[derive(Clone, Default)]
pub struct CancelToken(Arc<CancelInner>);

#[derive(Default)]
struct CancelInner {
    flag: AtomicBool,
    notify: Notify,
}

impl CancelToken {
    pub fn cancel(&self) {
        self.0.flag.store(true, Ordering::SeqCst);
        self.0.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.flag.load(Ordering::SeqCst)
    }

    /// Resolves once [`cancel`](Self::cancel) has been called (immediately if
    /// it already was).
    pub async fn cancelled(&self) {
        loop {
            // Register before checking the flag so a concurrent `cancel` can't
            // slip between the check and the wait.
            let notified = self.0.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

/// A pluggable stream-site resolver: detect a supported URL, then resolve it to
/// the best video-only + audio-only direct URLs.
// `async_trait`'s expansion adds `#[must_use]` to a boxed-future return type,
// which newer clippy flags; the lint is about macro output, not our code.
#[allow(clippy::double_must_use)]
#[async_trait]
pub trait Extractor: Send + Sync {
    /// Human-readable provider name (for logs / diagnostics).
    fn name(&self) -> &'static str;
    /// Does this provider handle `url`?
    fn detect(&self, url: &str) -> bool;
    /// Resolve `url` into directly-downloadable streams, sending the user's
    /// custom `headers` (auth, cookies). Must stop promptly once `cancel` fires.
    async fn extract(
        &self,
        url: &str,
        headers: &[(String, String)],
        cancel: &CancelToken,
    ) -> Result<ExtractedStreams>;
}

/// The registry of available providers, in detection-priority order.
pub fn providers() -> Vec<Box<dyn Extractor>> {
    vec![Box::new(YtDlpExtractor::default())]
}

/// Find the first provider that claims `url`.
pub fn detect(url: &str) -> Option<Box<dyn Extractor>> {
    providers().into_iter().find(|p| p.detect(url))
}

/// Heuristic: does any provider handle this URL (so we route it to extraction
/// rather than treating it as a direct file link)?
pub fn is_stream_site(url: &str) -> bool {
    detect(url).is_some()
}

/// Resolve a stream-site URL via whichever provider matches.
pub async fn extract(
    url: &str,
    headers: &[(String, String)],
    cancel: &CancelToken,
) -> Result<ExtractedStreams> {
    match detect(url) {
        Some(p) => {
            info!(provider = p.name(), "resolving stream URL");
            p.extract(url, headers, cancel).await
        }
        None => bail!("no extractor provider matched this URL"),
    }
}

/// Lower-cased `(host, path)` of an absolute http(s) URL, without port/userinfo.
fn host_and_path(url: &str) -> Option<(String, String)> {
    let lower = url.trim().to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))?;
    let (authority, path) = match rest.find(['/', '?', '#']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let host = authority.rsplit('@').next()?.split(':').next()?;
    (!host.is_empty()).then(|| (host.to_string(), path.to_string()))
}

/// `host` is `domain` or a subdomain of it (never a mere substring match).
fn host_matches(host: &str, domain: &str) -> bool {
    host == domain
        || host
            .strip_suffix(domain)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

/// Pages yt-dlp resolves to one video. Keep in sync with the extension's
/// `extension/src/lib/sites.ts`. Hosts are lower-cased, paths too.
fn is_video_page(host: &str, path: &str) -> bool {
    let on = |d: &str| host_matches(host, d);
    let starts = |prefixes: &[&str]| prefixes.iter().any(|p| path.starts_with(p));
    // Whole sites (every page is a video or yt-dlp handles the listing).
    if [
        "youtube.com",
        "youtu.be",
        "youtube-nocookie.com",
        "vimeo.com",
        "twitter.com",
        "x.com",
    ]
    .iter()
    .any(|d| on(d))
    {
        return true;
    }
    // Short-link hosts: every path is a video.
    if [
        "fb.watch",
        "v.redd.it",
        "redd.it",
        "dai.ly",
        "clips.twitch.tv",
        "vm.tiktok.com",
        "vt.tiktok.com",
    ]
    .iter()
    .any(|d| on(d))
    {
        return path.len() > 1;
    }
    if on("facebook.com") {
        // /watch, /reel/…, /reels/…, /share/v|r/…, /<page>/videos/<id>, /videos/<id>
        return starts(&[
            "/watch",
            "/reel/",
            "/reels/",
            "/share/v/",
            "/share/r/",
            "/videos/",
        ]) || path.contains("/videos/");
    }
    if on("instagram.com") {
        return starts(&["/p/", "/reel/", "/reels/", "/tv/"]);
    }
    if on("tiktok.com") {
        return path.contains("/video/") || starts(&["/t/"]);
    }
    if on("reddit.com") {
        return path.contains("/comments/");
    }
    if on("twitch.tv") {
        // VODs and clips; a live channel would never finish downloading.
        return starts(&["/videos/"]) || path.contains("/clip/");
    }
    if on("dailymotion.com") {
        return starts(&["/video/"]);
    }
    false
}

/// The default provider: anything `yt-dlp` can handle.
pub struct YtDlpExtractor {
    /// The yt-dlp executable (a bare name is resolved via `PATH`).
    program: OsString,
}

impl Default for YtDlpExtractor {
    fn default() -> Self {
        Self {
            program: "yt-dlp".into(),
        }
    }
}

#[async_trait]
impl Extractor for YtDlpExtractor {
    fn name(&self) -> &'static str {
        "yt-dlp"
    }

    fn detect(&self, url: &str) -> bool {
        let Some((host, path)) = host_and_path(url) else {
            return false;
        };
        is_video_page(&host, &path)
    }

    async fn extract(
        &self,
        url: &str,
        headers: &[(String, String)],
        cancel: &CancelToken,
    ) -> Result<ExtractedStreams> {
        let json = run_yt_dlp(&self.program, url, headers, cancel).await?;
        parse_yt_dlp_json(&json)
    }
}

/// Verify both external binaries the pipeline needs are installed, returning a
/// single actionable, OS-aware message listing whatever is missing.
pub async fn preflight() -> Result<()> {
    preflight_check().await.map_err(|(_, msg)| anyhow!(msg))
}

/// Which external tool is missing (for the bridge's machine-readable codes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissingTool {
    YtDlp,
    Ffmpeg,
}

/// Like [`preflight`], but says which tool is missing first (yt-dlp wins).
pub async fn preflight_check() -> std::result::Result<(), (MissingTool, String)> {
    let have_ytdlp = binary_ok("yt-dlp", &["--version"]).await;
    let have_ffmpeg = binary_ok("ffmpeg", &["-version"]).await;
    let first = if !have_ytdlp {
        MissingTool::YtDlp
    } else if !have_ffmpeg {
        MissingTool::Ffmpeg
    } else {
        return Ok(());
    };
    Err((first, missing_message(have_ytdlp, have_ffmpeg)))
}

fn missing_message(have_ytdlp: bool, have_ffmpeg: bool) -> String {
    let mut missing = Vec::new();
    if !have_ytdlp {
        missing.push("yt-dlp");
    }
    if !have_ffmpeg {
        missing.push("ffmpeg");
    }

    let hint = if cfg!(windows) {
        "Install with: winget install yt-dlp.yt-dlp  and  winget install Gyan.FFmpeg \
         (then reopen the app so PATH refreshes)."
    } else if cfg!(target_os = "macos") {
        "Install with: brew install yt-dlp ffmpeg."
    } else {
        "Install via your package manager, e.g. on Debian/Ubuntu: \
         sudo apt install ffmpeg  and grab yt-dlp from https://github.com/yt-dlp/yt-dlp."
    };

    format!(
        "Missing required tool(s) for video downloads: {}. {}",
        missing.join(" and "),
        hint
    )
}

/// True if `bin args...` runs and exits 0.
async fn binary_ok(bin: &str, args: &[&str]) -> bool {
    process::command(bin)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run yt-dlp **once** (`-J`: metadata only, no download) and return its JSON.
/// The child is killed if `cancel` fires (or if this future is dropped).
async fn run_yt_dlp(
    program: &OsString,
    url: &str,
    headers: &[(String, String)],
    cancel: &CancelToken,
) -> Result<String> {
    let mut cmd = process::command(program);
    cmd.args(["--no-warnings", "--no-playlist", "-J", "-f", YT_DLP_FORMAT]);
    for (k, v) in headers {
        cmd.arg("--add-header")
            .arg(format!("{}:{}", k.trim(), v.trim()));
    }
    // `--` so a URL starting with `-` can never be parsed as an option.
    cmd.arg("--").arg(url);
    let child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Backstop if this future is dropped without a cancel.
        .kill_on_drop(true)
        .spawn()
        .context("could not start yt-dlp (is it installed and on PATH?)")?;
    let pid = child.id();

    // Pinned outside `select!` so the child is still alive (and its process
    // tree discoverable) when we kill it on cancel.
    let wait = child.wait_with_output();
    tokio::pin!(wait);
    let out = tokio::select! {
        out = &mut wait => out.context("yt-dlp did not complete")?,
        _ = cancel.cancelled() => {
            if let Some(pid) = pid {
                process::kill_tree(pid).await;
            }
            bail!("canceled");
        }
    };
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("yt-dlp failed: {}", err.trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[derive(Deserialize)]
struct YtInfo {
    title: Option<String>,
    requested_formats: Option<Vec<YtFormat>>,
}

#[derive(Deserialize)]
struct YtFormat {
    url: String,
    ext: Option<String>,
    vcodec: Option<String>,
    acodec: Option<String>,
    /// yt-dlp download protocol: `https`/`http` are single files; anything
    /// else (`m3u8_native`, `http_dash_segments`, …) is a manifest.
    protocol: Option<String>,
}

/// Only a plain HTTP(S) file can be fetched with ranged requests.
fn is_direct(f: &YtFormat) -> bool {
    f.protocol
        .as_deref()
        .is_none_or(|p| p == "https" || p == "http")
}

/// Pick the video-only and audio-only streams out of `yt-dlp -J` output.
fn parse_yt_dlp_json(json: &str) -> Result<ExtractedStreams> {
    let info: YtInfo = serde_json::from_str(json).context("yt-dlp returned invalid JSON")?;
    let formats = info.requested_formats.unwrap_or_default();
    let has = |c: &Option<String>| c.as_deref().is_some_and(|c| c != "none");
    let video = formats
        .iter()
        .find(|f| has(&f.vcodec))
        .ok_or_else(|| anyhow!("yt-dlp did not return a separate video stream"))?;
    let audio = formats
        .iter()
        .find(|f| has(&f.acodec) && !has(&f.vcodec))
        .ok_or_else(|| anyhow!("yt-dlp did not return a separate audio stream"))?;
    for f in [video, audio] {
        if !is_direct(f) {
            bail!(
                "this video is only offered as a streaming playlist ({}), which isn't supported yet",
                f.protocol.as_deref().unwrap_or("?")
            );
        }
    }
    let ext = |f: &YtFormat| f.ext.clone().unwrap_or_else(|| "bin".into());
    let title = info
        .title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "video".into());
    Ok(ExtractedStreams {
        title,
        video_url: video.url.clone(),
        video_ext: ext(video),
        audio_url: audio.url.clone(),
        audio_ext: ext(audio),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{fake_program, temp_dir};
    use std::time::{Duration, Instant};

    #[test]
    fn detects_supported_hosts_only() {
        for url in [
            "https://www.youtube.com/watch?v=abc",
            "https://m.youtube.com/watch?v=abc",
            "https://music.youtube.com/watch?v=abc",
            "https://youtu.be/abc",
            "HTTPS://WWW.YOUTUBE.COM/watch?v=abc",
            "https://vimeo.com/123",
            "https://x.com/user/status/1",
            "https://twitter.com/user/status/1",
            // Facebook: every video URL shape.
            "https://www.facebook.com/watch/?v=1",
            "https://www.facebook.com/watch?v=1",
            "https://www.facebook.com/reel/123456",
            "https://www.facebook.com/reels/123456",
            "https://www.facebook.com/videos/123456",
            "https://www.facebook.com/SomePage/videos/123456/",
            "https://m.facebook.com/share/v/abc/",
            "https://fb.watch/abcDEF/",
            // Instagram posts / reels.
            "https://www.instagram.com/p/Cabc123/",
            "https://www.instagram.com/reel/Cabc123/",
            "https://www.instagram.com/tv/Cabc123/",
            // Reddit posts and its video host.
            "https://www.reddit.com/r/videos/comments/abc123/title/",
            "https://old.reddit.com/r/videos/comments/abc123/",
            "https://v.redd.it/abc123",
            // Twitch VODs and clips.
            "https://www.twitch.tv/videos/123456789",
            "https://www.twitch.tv/somechannel/clip/FunnyClip",
            "https://clips.twitch.tv/FunnyClip",
            // Dailymotion.
            "https://www.dailymotion.com/video/x8abc",
            "https://dai.ly/x8abc",
            // TikTok.
            "https://www.tiktok.com/@user/video/7234567890",
            "https://vm.tiktok.com/ZMabc/",
        ] {
            assert!(is_stream_site(url), "should detect {url}");
        }
        for url in [
            "https://example.com/?ref=youtube.com",
            "https://example.com/youtube.com/file.zip",
            "https://notyoutube.com/watch?v=abc",
            "https://youtube.com.evil.example/file.bin",
            "https://www.facebook.com/photo.php",
            "https://www.facebook.com/",
            "https://www.facebook.com/groups/123",
            "https://www.instagram.com/someuser/",
            "https://www.instagram.com/",
            "https://www.reddit.com/r/videos/",
            "https://www.reddit.com/",
            // A live channel would never finish; only VODs/clips.
            "https://www.twitch.tv/somechannel",
            "https://www.twitch.tv/",
            "https://www.dailymotion.com/",
            "https://www.tiktok.com/@user",
            "https://fb.watch/",
            "https://evilfacebook.com/watch?v=1",
            "https://facebook.com.evil.example/reel/1",
            "https://cdn.example.com/video.mp4",
            "not a url",
        ] {
            assert!(!is_stream_site(url), "should NOT detect {url}");
        }
    }

    const SAMPLE_JSON: &str = r#"{
        "title": "  My Clip  ",
        "ext": "mp4",
        "requested_formats": [
            {"url": "https://v.example/video", "ext": "mp4", "vcodec": "avc1.640028", "acodec": "none"},
            {"url": "https://a.example/audio", "ext": "m4a", "vcodec": "none", "acodec": "mp4a.40.2"}
        ],
        "formats": [{"url": "ignored"}]
    }"#;

    #[test]
    fn parses_video_and_audio_from_json() {
        let s = parse_yt_dlp_json(SAMPLE_JSON).unwrap();
        assert_eq!(s.title, "My Clip");
        assert_eq!(s.video_url, "https://v.example/video");
        assert_eq!(s.video_ext, "mp4");
        assert_eq!(s.audio_url, "https://a.example/audio");
        assert_eq!(s.audio_ext, "m4a");
    }

    #[test]
    fn json_edge_cases() {
        // Order-independent; missing ext/title get defaults.
        let s = parse_yt_dlp_json(
            r#"{"requested_formats": [
                {"url": "A", "vcodec": "none", "acodec": "opus"},
                {"url": "V", "vcodec": "vp9", "acodec": "none"}]}"#,
        )
        .unwrap();
        assert_eq!((s.video_url.as_str(), s.audio_url.as_str()), ("V", "A"));
        assert_eq!((s.video_ext.as_str(), s.title.as_str()), ("bin", "video"));
        // A single muxed format (no requested_formats) is not supported.
        let e = parse_yt_dlp_json(r#"{"title": "t", "url": "x", "ext": "mp4"}"#).unwrap_err();
        assert!(e.to_string().contains("video stream"), "{e}");
        // Video-only result: no audio stream.
        let e = parse_yt_dlp_json(r#"{"requested_formats": [{"url": "V", "vcodec": "vp9"}]}"#)
            .unwrap_err();
        assert!(e.to_string().contains("audio stream"), "{e}");
        assert!(parse_yt_dlp_json("not json").is_err());
        // An HLS playlist must be rejected, not downloaded as if it were video.
        let e = parse_yt_dlp_json(
            r#"{"requested_formats": [
                {"url": "V.m3u8", "vcodec": "avc1", "acodec": "none", "protocol": "m3u8_native"},
                {"url": "A", "vcodec": "none", "acodec": "mp4a", "protocol": "https"}]}"#,
        )
        .unwrap_err();
        assert!(e.to_string().contains("m3u8_native"), "{e}");
    }

    #[tokio::test]
    async fn single_invocation_with_headers_and_url_guard() {
        let dir = temp_dir("ytdlp_ok");
        std::fs::write(dir.join("canned.json"), SAMPLE_JSON).unwrap();
        let program = fake_program(
            &dir,
            "yt-dlp",
            r#"d=$(dirname "$0"); printf '%s\n' "$@" >> "$d/args.txt"; cat "$d/canned.json""#,
            "echo %* >> \"%~dp0args.txt\"\ntype \"%~dp0canned.json\"",
        );
        let ex = YtDlpExtractor {
            program: program.into_os_string(),
        };
        let headers = vec![("X-Test".to_string(), "abc".to_string())];
        let s = ex
            .extract("https://youtu.be/abc", &headers, &CancelToken::default())
            .await
            .unwrap();
        assert_eq!(s.video_url, "https://v.example/video");

        let args = std::fs::read_to_string(dir.join("args.txt")).unwrap();
        // Exactly one yt-dlp run: every run appends its args, `-J` once each.
        assert_eq!(args.matches("-J").count(), 1, "args: {args}");
        assert!(
            args.contains("--add-header") && args.contains("X-Test:abc"),
            "args: {args}"
        );
        assert!(
            args.contains("--") && args.contains("https://youtu.be/abc"),
            "args: {args}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn yt_dlp_failure_surfaces_stderr() {
        let dir = temp_dir("ytdlp_fail");
        let program = fake_program(
            &dir,
            "yt-dlp",
            "echo 'ERROR: Unsupported URL' >&2; exit 1",
            "echo ERROR: Unsupported URL 1>&2\nexit /b 1",
        );
        let ex = YtDlpExtractor {
            program: program.into_os_string(),
        };
        let e = ex
            .extract("https://youtu.be/x", &[], &CancelToken::default())
            .await
            .unwrap_err();
        assert!(e.to_string().contains("Unsupported URL"), "{e}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn extraction_is_cancellable() {
        let dir = temp_dir("ytdlp_hang");
        // Hangs ~20 s unless killed.
        let program = fake_program(&dir, "yt-dlp", "exec sleep 20", "ping -n 20 127.0.0.1 >nul");
        let ex = YtDlpExtractor {
            program: program.into_os_string(),
        };
        let cancel = CancelToken::default();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            c2.cancel();
        });
        let t0 = Instant::now();
        let e = ex
            .extract("https://youtu.be/x", &[], &cancel)
            .await
            .unwrap_err();
        assert_eq!(e.to_string(), "canceled");
        assert!(
            t0.elapsed() < Duration::from_secs(3),
            "took {:?}",
            t0.elapsed()
        );
        // An already-cancelled token resolves immediately.
        tokio::time::timeout(Duration::from_millis(100), cancel.cancelled())
            .await
            .expect("cancelled() should resolve at once");
        std::fs::remove_dir_all(&dir).ok();
    }
}
