//! "Open file" / "Show in folder" for finished downloads.
//!
//! Deliberately narrow instead of widening the Tauri `shell` allowlist: the UI
//! passes only a download **id**; the path comes from our own history record,
//! must sit inside that record's directory, and must exist. Executables and
//! scripts are never opened directly (that would run them) — only revealed.

use std::path::{Path, PathBuf};

use crate::history::{JobRecord, JobStatus};

/// What the user asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenTarget {
    File,
    Folder,
}

impl OpenTarget {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "file" => Ok(Self::File),
            "folder" => Ok(Self::Folder),
            other => Err(format!("unknown open target `{other}`")),
        }
    }
}

/// The OS action to perform.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenAction {
    /// Open the file with its default application.
    OpenFile(PathBuf),
    /// Open the containing folder with the file selected.
    Reveal(PathBuf),
    /// Open a folder (the file isn't there, e.g. not started yet).
    OpenDir(PathBuf),
}

/// Extensions that execute code when "opened".
const RUNNABLE: &[&str] = &[
    "exe",
    "msi",
    "msix",
    "msixbundle",
    "appx",
    "appxbundle",
    "bat",
    "cmd",
    "com",
    "scr",
    "pif",
    "cpl",
    "ps1",
    "psm1",
    "vbs",
    "vbe",
    "js",
    "jse",
    "wsf",
    "wsh",
    "hta",
    "lnk",
    "reg",
    "jar",
    "sh",
    "command",
    "app",
    "appimage",
    "run",
];

fn is_runnable(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| RUNNABLE.contains(&e.to_ascii_lowercase().as_str()))
}

/// Decide what to open for `rec`, validating everything on the way.
pub fn resolve(rec: &JobRecord, target: OpenTarget) -> Result<OpenAction, String> {
    let dest = rec
        .dest
        .as_ref()
        .ok_or_else(|| "The file's location isn't known yet.".to_string())?;
    // The path must be exactly "<record dir>/<one component>" — never
    // somewhere else, even if a record were tampered with.
    if dest.parent() != Some(rec.dir.as_path()) || dest.file_name().is_none() {
        return Err("Refusing to open a path outside the download's folder.".into());
    }
    match target {
        OpenTarget::File => {
            if rec.status != JobStatus::Complete {
                return Err("The download hasn't finished yet.".into());
            }
            if !dest.is_file() {
                return Err(format!(
                    "{} no longer exists (moved or deleted?).",
                    dest.display()
                ));
            }
            if is_runnable(dest) {
                return Err(
                    "Programs and scripts aren't opened directly — use “Show in folder”.".into(),
                );
            }
            Ok(OpenAction::OpenFile(dest.clone()))
        }
        OpenTarget::Folder => {
            if dest.is_file() {
                Ok(OpenAction::Reveal(dest.clone()))
            } else if rec.dir.is_dir() {
                Ok(OpenAction::OpenDir(rec.dir.clone()))
            } else {
                Err(format!("{} no longer exists.", rec.dir.display()))
            }
        }
    }
}

/// Hand the action to the OS file manager / default-app launcher. Fire and
/// forget: these launchers return immediately (Explorer's exit code is
/// meaningless), so only a failure to spawn is reported.
pub fn perform(action: &OpenAction) -> Result<(), String> {
    let mut cmd = launcher(action);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd.spawn()
        .map(drop)
        .map_err(|e| format!("couldn't open: {e}"))
}

#[cfg(windows)]
fn launcher(action: &OpenAction) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new("explorer.exe");
    match action {
        OpenAction::OpenFile(p) | OpenAction::OpenDir(p) => {
            cmd.arg(p);
        }
        // Explorer parses `/select,"<path>"` itself; std's quoting of the
        // whole argument would break it. Windows paths can't contain `"`.
        OpenAction::Reveal(p) => {
            cmd.raw_arg(format!("/select,\"{}\"", p.display()));
        }
    }
    cmd
}

#[cfg(unix)]
fn launcher(action: &OpenAction) -> std::process::Command {
    let mac = cfg!(target_os = "macos");
    let mut cmd = std::process::Command::new(if mac { "open" } else { "xdg-open" });
    match action {
        OpenAction::OpenFile(p) | OpenAction::OpenDir(p) => {
            cmd.arg(p);
        }
        OpenAction::Reveal(p) if mac => {
            cmd.arg("-R").arg(p);
        }
        // xdg-open can't select a file; open its folder instead.
        OpenAction::Reveal(p) => {
            cmd.arg(p.parent().unwrap_or(p));
        }
    }
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::JobKind;
    use crate::queue::Priority;
    use crate::testutil::temp_dir;

    fn rec(dir: &Path, name: &str, status: JobStatus) -> JobRecord {
        JobRecord {
            id: "x".into(),
            kind: JobKind::File,
            url: "https://h/x".into(),
            mirrors: vec![],
            header_names: vec![],
            dir: dir.to_path_buf(),
            dest: Some(dir.join(name)),
            file_name: name.into(),
            status,
            total: 1,
            downloaded: 1,
            connections: 1,
            speed_limit: 0,
            verify: false,
            expected_checksum: None,
            start_at: None,
            priority: Priority::Normal,
            held: false,
            quality: None,
            created_at: 0,
            updated_at: 0,
            error: None,
        }
    }

    #[test]
    fn opens_only_finished_existing_non_runnable_files_in_their_folder() {
        let dir = temp_dir("open");
        std::fs::write(dir.join("movie.mp4"), b"x").unwrap();
        std::fs::write(dir.join("setup.EXE"), b"x").unwrap();
        let ok = rec(&dir, "movie.mp4", JobStatus::Complete);

        assert_eq!(
            resolve(&ok, OpenTarget::File),
            Ok(OpenAction::OpenFile(dir.join("movie.mp4")))
        );
        assert_eq!(
            resolve(&ok, OpenTarget::Folder),
            Ok(OpenAction::Reveal(dir.join("movie.mp4")))
        );

        let running = rec(&dir, "movie.mp4", JobStatus::Downloading);
        assert!(resolve(&running, OpenTarget::File)
            .unwrap_err()
            .contains("finished"));
        assert!(
            resolve(&running, OpenTarget::Folder).is_ok(),
            "folder is fine mid-download"
        );

        let exe = rec(&dir, "setup.EXE", JobStatus::Complete);
        assert!(resolve(&exe, OpenTarget::File)
            .unwrap_err()
            .contains("Show in folder"));
        assert_eq!(
            resolve(&exe, OpenTarget::Folder),
            Ok(OpenAction::Reveal(dir.join("setup.EXE")))
        );

        let gone = rec(&dir, "deleted.zip", JobStatus::Complete);
        assert!(resolve(&gone, OpenTarget::File)
            .unwrap_err()
            .contains("no longer exists"));
        assert_eq!(
            resolve(&gone, OpenTarget::Folder),
            Ok(OpenAction::OpenDir(dir.clone()))
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rejects_paths_outside_the_record_folder_and_unknown_targets() {
        let dir = temp_dir("open_esc");
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut r = rec(&dir, "a.txt", JobStatus::Complete);
        r.dest = Some(dir.join("sub").join("a.txt"));
        assert!(resolve(&r, OpenTarget::Folder)
            .unwrap_err()
            .contains("outside"));
        r.dest = Some(dir.join("..").join("a.txt"));
        assert!(resolve(&r, OpenTarget::File)
            .unwrap_err()
            .contains("outside"));
        r.dest = None;
        assert!(resolve(&r, OpenTarget::File)
            .unwrap_err()
            .contains("isn't known"));
        assert!(OpenTarget::parse("exec").is_err());
        assert_eq!(OpenTarget::parse("folder"), Ok(OpenTarget::Folder));
        std::fs::remove_dir_all(&dir).ok();
    }
}
