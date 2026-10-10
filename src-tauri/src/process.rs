//! Spawning external tools (yt-dlp, ffmpeg) consistently.

use std::ffi::OsStr;
use std::process::Stdio;

use tokio::process::Command;

/// A `Command` for an external tool. On Windows it gets `CREATE_NO_WINDOW`:
/// the release build is a GUI-subsystem app, so every console tool would
/// otherwise flash its own console window. On Unix it is placed in its own
/// process group so [`kill_tree`] can take down any children it forks.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        cmd.process_group(0);
    }
    cmd
}

/// Forcefully kill process `pid` **and its descendants**. `kill_on_drop` only
/// reaches the direct child, but e.g. PyInstaller one-file builds of yt-dlp do
/// the real work in a child process. Best-effort: errors are ignored.
#[cfg(windows)]
pub async fn kill_tree(pid: u32) {
    let _ = command("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
}

/// Forcefully kill process `pid` **and its descendants** (its process group,
/// which [`command`] created). Best-effort: errors are ignored.
#[cfg(unix)]
pub async fn kill_tree(pid: u32) {
    let _ = Command::new("kill")
        .args(["-KILL", "--", &format!("-{pid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
}
