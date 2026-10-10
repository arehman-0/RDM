//! Direct HLS / DASH manifest downloads (`.m3u8`, `.mpd`) through yt-dlp.
//!
//! The segmented engine only handles single files, so a manifest URL from the
//! browser bridge is handed to yt-dlp, which downloads the fragments and (with
//! ffmpeg) merges separate video/audio tracks. Progress comes from a machine-
//! readable `--progress-template`; cancellation kills the whole yt-dlp process
//! tree (including any ffmpeg it spawned). Intermediate files go to a private
//! temp dir that is removed afterwards, so nothing partial is left behind.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

use crate::process;
use crate::youtube::CancelToken;

/// Quality requested by the caller (`quality` in the bridge request).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Best,
    /// Best variant at most this tall (e.g. 1080).
    MaxHeight(u32),
    /// Best audio only.
    Audio,
}

impl Quality {
    /// `"best"` | `"audio"` | a height like `"1080"` / `"720p"`; `None` = invalid.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        match s.as_str() {
            "" | "best" => Some(Self::Best),
            "audio" => Some(Self::Audio),
            _ => {
                let h: u32 = s.strip_suffix('p').unwrap_or(&s).parse().ok()?;
                (144..=4320).contains(&h).then_some(Self::MaxHeight(h))
            }
        }
    }

    pub fn as_string(self) -> String {
        match self {
            Self::Best => "best".into(),
            Self::Audio => "audio".into(),
            Self::MaxHeight(h) => h.to_string(),
        }
    }

    /// yt-dlp `-f` selector. A height cap falls back to the best available
    /// rather than failing when no variant is small enough.
    pub fn format(self) -> String {
        match self {
            Self::Best => "bv*+ba/b".into(),
            Self::Audio => "ba/b".into(),
            Self::MaxHeight(h) => format!("bv*[height<={h}]+ba/b[height<={h}]/bv*+ba/b"),
        }
    }
}

/// One progress sample parsed from yt-dlp.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StreamProgress {
    pub downloaded: u64,
    /// Exact or estimated total (HLS only knows an estimate).
    pub total: Option<u64>,
    pub speed: Option<f64>,
    pub eta: Option<f64>,
}

#[derive(Debug)]
pub enum StreamError {
    Canceled,
    /// The stream uses DRM; yt-dlp refuses it (it never hangs on it).
    Drm(String),
    /// yt-dlp couldn't be started.
    Spawn(String),
    /// yt-dlp failed; carries its last error lines.
    Failed(String),
    /// yt-dlp reported a file outside the download directory.
    Outside(PathBuf),
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Canceled => write!(f, "canceled"),
            Self::Drm(detail) => write!(
                f,
                "This stream is DRM-protected, so it can't be downloaded. ({detail})"
            ),
            Self::Spawn(e) => write!(f, "could not start yt-dlp: {e}"),
            Self::Failed(e) => write!(f, "yt-dlp failed: {e}"),
            Self::Outside(p) => write!(
                f,
                "refused: yt-dlp wrote outside the download folder ({})",
                p.display()
            ),
        }
    }
}

const PROGRESS_TAG: &str = "idmp";
const FILE_TAG: &str = "idmf";

/// Parse one `idmp …` line (fields are `NA` when unknown).
pub fn parse_progress(line: &str) -> Option<StreamProgress> {
    let mut it = line.split_whitespace();
    if it.next()? != PROGRESS_TAG {
        return None;
    }
    let num = |s: Option<&str>| {
        s.and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 0.0)
    };
    let downloaded = num(it.next())? as u64;
    let total = num(it.next());
    let estimate = num(it.next());
    let speed = num(it.next());
    let eta = num(it.next());
    Some(StreamProgress {
        downloaded,
        total: total.or(estimate).map(|t| t as u64),
        speed,
        eta,
    })
}

/// DRM wording yt-dlp uses ("This video is DRM protected", "DRM-protected").
pub fn is_drm_error(stderr: &str) -> bool {
    stderr.to_ascii_lowercase().contains("drm")
}

/// The last few `ERROR:` lines (or the tail) of yt-dlp's stderr.
fn error_summary(stderr: &str) -> String {
    let errors: Vec<&str> = stderr
        .lines()
        .filter(|l| l.trim_start().starts_with("ERROR"))
        .collect();
    let lines: Vec<&str> = if errors.is_empty() {
        stderr
            .lines()
            .rev()
            .take(3)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    } else {
        errors
            .into_iter()
            .rev()
            .take(3)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    };
    let s = lines.join(" ").trim().to_string();
    if s.is_empty() {
        "no error output".into()
    } else {
        s.chars().take(500).collect()
    }
}

pub struct StreamJob<'a> {
    /// yt-dlp executable (a bare name is resolved via PATH).
    pub program: &'a OsStr,
    pub url: &'a str,
    /// Referer / User-Agent / Cookie etc., passed with `--add-header`.
    pub headers: &'a [(String, String)],
    /// Final directory. yt-dlp may only write the finished file here.
    pub dir: &'a Path,
    /// Private scratch dir for fragments (removed afterwards).
    pub temp_dir: &'a Path,
    pub quality: Quality,
}

/// The yt-dlp argument list (separate for testing).
pub fn build_args(job: &StreamJob<'_>) -> Vec<String> {
    let mut a: Vec<String> = [
        "--newline",
        "--no-warnings",
        "--no-playlist",
        "--no-mtime",
        "--windows-filenames",
        "--progress",
        "--no-simulate",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    a.push("--progress-template".into());
    a.push(format!(
        "download:{PROGRESS_TAG} %(progress.downloaded_bytes)s %(progress.total_bytes)s \
         %(progress.total_bytes_estimate)s %(progress.speed)s %(progress.eta)s"
    ));
    a.push("--print".into());
    a.push(format!("after_move:{FILE_TAG} %(filepath)s"));
    a.push("-f".into());
    a.push(job.quality.format());
    a.push("-P".into());
    a.push(job.dir.to_string_lossy().into_owned());
    a.push("-P".into());
    a.push(format!("temp:{}", job.temp_dir.to_string_lossy()));
    a.push("-o".into());
    a.push("%(title).120B.%(ext)s".into());
    for (k, v) in job.headers {
        a.push("--add-header".into());
        a.push(format!("{}:{}", k.trim(), v.trim()));
    }
    // `--` so a URL starting with `-` can never be read as an option.
    a.push("--".into());
    a.push(job.url.to_string());
    a
}

/// Is `file` inside `dir` (after resolving `..` and links)?
pub fn is_inside(dir: &Path, file: &Path) -> bool {
    match (dir.canonicalize(), file.canonicalize()) {
        (Ok(d), Ok(f)) => f.starts_with(d),
        _ => false,
    }
}

/// Download `job.url` with yt-dlp, calling `on_progress` for every sample.
pub async fn download(
    job: StreamJob<'_>,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(StreamProgress),
) -> Result<PathBuf, StreamError> {
    let _ = std::fs::create_dir_all(job.dir);
    let _ = std::fs::create_dir_all(job.temp_dir);
    let result = run(&job, cancel, &mut on_progress).await;
    // Fragments / .part files never outlive the job.
    let _ = tokio::fs::remove_dir_all(job.temp_dir).await;
    result
}

async fn run(
    job: &StreamJob<'_>,
    cancel: &CancelToken,
    on_progress: &mut impl FnMut(StreamProgress),
) -> Result<PathBuf, StreamError> {
    let mut child = process::command(job.program)
        .args(build_args(job))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| StreamError::Spawn(e.to_string()))?;
    let pid = child.id();
    let kill = || async {
        if let Some(pid) = pid {
            process::kill_tree(pid).await;
        }
    };

    // Drain stderr concurrently (bounded) so the child never blocks on it.
    let stderr_task = child.stderr.take().map(|mut err| {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                match err.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        if buf.len() > 64 * 1024 {
                            let cut = buf.len() - 32 * 1024;
                            buf.drain(..cut);
                        }
                    }
                }
            }
            String::from_utf8_lossy(&buf).into_owned()
        })
    });

    let mut file: Option<PathBuf> = None;
    if let Some(out) = child.stdout.take() {
        let mut lines = BufReader::new(out).lines();
        loop {
            tokio::select! {
                line = lines.next_line() => match line {
                    Ok(Some(l)) => {
                        let l = l.trim();
                        if let Some(p) = parse_progress(l) {
                            on_progress(p);
                        } else if let Some(path) = l.strip_prefix(FILE_TAG) {
                            file = Some(PathBuf::from(path.trim()));
                        }
                    }
                    Ok(None) | Err(_) => break,
                },
                _ = cancel.cancelled() => {
                    kill().await;
                    return Err(StreamError::Canceled);
                }
            }
        }
    }
    let status = tokio::select! {
        s = child.wait() => s.map_err(|e| StreamError::Failed(e.to_string()))?,
        _ = cancel.cancelled() => {
            kill().await;
            return Err(StreamError::Canceled);
        }
    };
    let stderr = match stderr_task {
        Some(t) => t.await.unwrap_or_default(),
        None => String::new(),
    };
    if cancel.is_cancelled() {
        return Err(StreamError::Canceled);
    }
    if !status.success() {
        let summary = error_summary(&stderr);
        return Err(if is_drm_error(&stderr) {
            StreamError::Drm(summary)
        } else {
            StreamError::Failed(summary)
        });
    }
    let file = file.ok_or_else(|| StreamError::Failed("yt-dlp reported no output file".into()))?;
    if !is_inside(job.dir, &file) {
        return Err(StreamError::Outside(file));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{fake_program, temp_dir};
    use std::time::{Duration, Instant};

    #[test]
    fn quality_parsing_and_selectors() {
        assert_eq!(Quality::parse("best"), Some(Quality::Best));
        assert_eq!(Quality::parse(""), Some(Quality::Best));
        assert_eq!(Quality::parse("AUDIO"), Some(Quality::Audio));
        assert_eq!(Quality::parse("1080"), Some(Quality::MaxHeight(1080)));
        assert_eq!(Quality::parse("720p"), Some(Quality::MaxHeight(720)));
        for bad in ["8k", "99", "100000", "-1", "1080; rm -rf"] {
            assert_eq!(Quality::parse(bad), None, "{bad}");
        }
        assert_eq!(Quality::Best.format(), "bv*+ba/b");
        assert!(Quality::MaxHeight(720).format().contains("height<=720"));
        assert_eq!(Quality::MaxHeight(720).as_string(), "720");
    }

    #[test]
    fn progress_lines() {
        assert_eq!(
            parse_progress("idmp 1024 NA 4096 512.5 6"),
            Some(StreamProgress {
                downloaded: 1024,
                total: Some(4096),
                speed: Some(512.5),
                eta: Some(6.0)
            })
        );
        assert_eq!(
            parse_progress("idmp 10 20 NA NA NA").map(|p| (p.total, p.speed)),
            Some((Some(20), None))
        );
        assert_eq!(parse_progress("[download] 10%"), None);
        assert_eq!(parse_progress("idmp NA NA NA NA NA"), None);
    }

    #[test]
    fn drm_and_error_summaries() {
        assert!(is_drm_error("ERROR: [generic] This video is DRM protected"));
        assert!(!is_drm_error("ERROR: Unsupported URL"));
        let s = error_summary("noise\nERROR: first\nmore noise\nERROR: second\n");
        assert_eq!(s, "ERROR: first ERROR: second");
        assert_eq!(error_summary(""), "no error output");
    }

    #[test]
    fn args_forward_headers_quality_and_guard_the_url() {
        let dir = PathBuf::from("out");
        let tmp = PathBuf::from("tmp");
        let headers = vec![
            (
                "Referer".to_string(),
                "https://site.example/page".to_string(),
            ),
            ("Cookie".to_string(), "sid=1".to_string()),
        ];
        let a = build_args(&StreamJob {
            program: OsStr::new("yt-dlp"),
            url: "-https://x.example/master.m3u8",
            headers: &headers,
            dir: &dir,
            temp_dir: &tmp,
            quality: Quality::MaxHeight(720),
        });
        let joined = a.join("\n");
        assert!(joined.contains("--add-header\nReferer:https://site.example/page"));
        assert!(joined.contains("--add-header\nCookie:sid=1"));
        assert!(joined.contains("-f\nbv*[height<=720]"));
        assert!(joined.contains("-P\ntemp:tmp"));
        // The URL comes last, after `--`.
        assert_eq!(&a[a.len() - 2..], ["--", "-https://x.example/master.m3u8"]);
    }

    /// A job whose final dir is the fake script's own dir (the scripts write there).
    fn job<'a>(program: &'a OsStr, dir: &'a Path, tmp: &'a Path) -> StreamJob<'a> {
        StreamJob {
            program,
            url: "https://x.example/master.m3u8",
            headers: &[],
            dir,
            temp_dir: tmp,
            quality: Quality::Best,
        }
    }

    #[tokio::test]
    async fn success_reports_progress_and_the_file() {
        let dir = temp_dir("ytdl_ok");
        let program = fake_program(
            &dir,
            "yt-dlp",
            r#"d=$(dirname "$0"); echo "idmp 100 NA 1000 50 18"; echo "idmp 1000 1000 NA 50 0"; echo data > "$d/clip.mp4"; echo "idmf $d/clip.mp4""#,
            "echo idmp 100 NA 1000 50 18\necho idmp 1000 1000 NA 50 0\necho data> \"%~dp0clip.mp4\"\necho idmf %~dp0clip.mp4",
        );
        let tmp = dir.join(".tmp-job");
        let mut seen = Vec::new();
        let path = download(
            job(program.as_os_str(), &dir, &tmp),
            &CancelToken::default(),
            |p| seen.push(p),
        )
        .await
        .unwrap();
        assert_eq!(path.file_name().unwrap(), "clip.mp4");
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].total, Some(1000));
        assert_eq!(seen[1].downloaded, 1000);
        assert!(!tmp.exists(), "temp dir must be removed");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn non_zero_exit_surfaces_the_error() {
        let dir = temp_dir("ytdl_fail");
        let program = fake_program(
            &dir,
            "yt-dlp",
            "echo 'ERROR: Unable to download webpage: HTTP Error 404' >&2; exit 1",
            "echo ERROR: Unable to download webpage: HTTP Error 404 1>&2\nexit /b 1",
        );
        let tmp = dir.join(".tmp-job");
        let e = download(
            job(program.as_os_str(), &dir, &tmp),
            &CancelToken::default(),
            |_| {},
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&e, StreamError::Failed(m) if m.contains("HTTP Error 404")),
            "{e:?}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn drm_fails_fast_with_a_clear_message() {
        let dir = temp_dir("ytdl_drm");
        let program = fake_program(
            &dir,
            "yt-dlp",
            "echo 'ERROR: [generic] This video is DRM protected' >&2; exit 1",
            "echo ERROR: [generic] This video is DRM protected 1>&2\nexit /b 1",
        );
        let tmp = dir.join(".tmp-job");
        let t0 = Instant::now();
        let e = download(
            job(program.as_os_str(), &dir, &tmp),
            &CancelToken::default(),
            |_| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(e, StreamError::Drm(_)), "{e:?}");
        assert!(e.to_string().contains("DRM-protected"));
        assert!(t0.elapsed() < Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn cancel_kills_the_process_promptly() {
        let dir = temp_dir("ytdl_cancel");
        let program = fake_program(
            &dir,
            "yt-dlp",
            "echo 'idmp 1 NA NA NA NA'; exec sleep 30",
            "echo idmp 1 NA NA NA NA\nping -n 30 127.0.0.1 >nul",
        );
        let tmp = dir.join(".tmp-job");
        let cancel = CancelToken::default();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            c2.cancel();
        });
        let t0 = Instant::now();
        let e = download(job(program.as_os_str(), &dir, &tmp), &cancel, |_| {})
            .await
            .unwrap_err();
        assert!(matches!(e, StreamError::Canceled), "{e:?}");
        assert!(
            t0.elapsed() < Duration::from_secs(5),
            "took {:?}",
            t0.elapsed()
        );
        assert!(!tmp.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_file_outside_the_download_dir_is_refused() {
        let dir = temp_dir("ytdl_outside");
        let elsewhere = temp_dir("ytdl_elsewhere");
        std::fs::write(elsewhere.join("evil.mp4"), "x").unwrap();
        let target = elsewhere.join("evil.mp4");
        let program = fake_program(
            &dir,
            "yt-dlp",
            &format!("echo \"idmf {}\"", target.display()),
            &format!("echo idmf {}", target.display()),
        );
        let tmp = dir.join(".tmp-job");
        let e = download(
            job(program.as_os_str(), &dir, &tmp),
            &CancelToken::default(),
            |_| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(e, StreamError::Outside(_)), "{e:?}");
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(elsewhere);
    }
}
