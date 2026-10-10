//! Muxing layer — stream-copy merge of separate video + audio via FFmpeg.
//!
//! Because YouTube serves high-res video and audio as separate adaptive
//! streams, we download each independently (each with its own Dynamic File
//! Segmentation job) and then losslessly remux them into one container with
//! `-c copy`. No re-encode => near-instant, zero quality loss, negligible CPU.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::process;
use anyhow::{bail, Context, Result};

/// Is `program` runnable (`program -version` exits 0)?
async fn ffmpeg_available(program: &OsStr) -> bool {
    process::command(program)
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run `ffmpeg -i video -i audio -c:v copy -c:a copy output` and, on success,
/// delete the two temporary stream files.
pub async fn mux(video: &Path, audio: &Path, output: &Path) -> Result<PathBuf> {
    mux_with(OsStr::new("ffmpeg"), video, audio, output).await
}

/// [`mux`] with an explicit FFmpeg executable (tests pass a fake).
async fn mux_with(program: &OsStr, video: &Path, audio: &Path, output: &Path) -> Result<PathBuf> {
    if !ffmpeg_available(program).await {
        bail!("`ffmpeg` not found on PATH. Install it from https://ffmpeg.org/download.html");
    }

    let status = process::command(program)
        .args([
            "-y", // overwrite output if present
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
        ])
        .arg(video)
        .arg("-i")
        .arg(audio)
        .args([
            "-c:v",
            "copy", // stream-copy video (no transcode)
            "-c:a",
            "copy", // stream-copy audio
            // Some web audio is opus/aac; m4a in mp4 needs the +faststart and
            // a sane stream mapping. Map both inputs explicitly.
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-movflags",
            "+faststart",
        ])
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("failed to spawn ffmpeg")?;

    if !status.status.success() {
        let err = String::from_utf8_lossy(&status.stderr);
        bail!("ffmpeg mux failed: {}", err.trim());
    }

    // Clean up the separate temp streams now that the merge succeeded.
    let _ = tokio::fs::remove_file(video).await;
    let _ = tokio::fs::remove_file(audio).await;

    Ok(output.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{fake_program, temp_dir};

    /// Fake ffmpeg: `-version` succeeds; otherwise records its args and
    /// creates the output file (the last argument).
    fn ok_ffmpeg(dir: &Path) -> PathBuf {
        fake_program(
            dir,
            "ffmpeg",
            r#"[ "$1" = "-version" ] && exit 0
printf '%s\n' "$@" > "$(dirname "$0")/args.txt"
for a; do last=$a; done
: > "$last""#,
            r#"if "%~1"=="-version" exit /b 0
echo %* > "%~dp0args.txt"
set "last="
for %%a in (%*) do set "last=%%~a"
type nul > "%last%"
exit /b 0"#,
        )
    }

    fn inputs(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let (v, a) = (dir.join("in video.mp4"), dir.join("in audio.m4a"));
        std::fs::write(&v, b"v").unwrap();
        std::fs::write(&a, b"a").unwrap();
        (v, a, dir.join("out file.mp4"))
    }

    #[tokio::test]
    async fn mux_stream_copies_and_removes_temps() {
        let dir = temp_dir("mux_ok");
        let program = ok_ffmpeg(&dir);
        let (v, a, out) = inputs(&dir);

        let res = mux_with(program.as_os_str(), &v, &a, &out).await.unwrap();
        assert_eq!(res, out);
        assert!(out.exists(), "fake ffmpeg should have created the output");
        assert!(!v.exists() && !a.exists(), "temp streams should be deleted");

        let args = std::fs::read_to_string(dir.join("args.txt")).unwrap();
        for needle in [
            "-c:v",
            "copy",
            "-c:a",
            "0:v:0",
            "1:a:0",
            "+faststart",
            "in video.mp4",
        ] {
            assert!(args.contains(needle), "missing {needle:?} in {args}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn mux_failure_reports_stderr_and_keeps_temps() {
        let dir = temp_dir("mux_fail");
        let program = fake_program(
            &dir,
            "ffmpeg",
            r#"[ "$1" = "-version" ] && exit 0
echo 'boom: invalid data' >&2; exit 1"#,
            "if \"%~1\"==\"-version\" exit /b 0\necho boom: invalid data 1>&2\nexit /b 1",
        );
        let (v, a, out) = inputs(&dir);

        let e = mux_with(program.as_os_str(), &v, &a, &out)
            .await
            .unwrap_err();
        assert!(e.to_string().contains("boom: invalid data"), "{e}");
        assert!(v.exists() && a.exists(), "temps must survive a failed mux");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn missing_ffmpeg_is_a_clear_error() {
        let dir = temp_dir("mux_missing");
        let (v, a, out) = inputs(&dir);
        let e = mux_with(dir.join("no-such-ffmpeg").as_os_str(), &v, &a, &out)
            .await
            .unwrap_err();
        assert!(e.to_string().contains("not found"), "{e}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
