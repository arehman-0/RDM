//! Test-only helpers: fake external programs (yt-dlp, ffmpeg) as scripts.
//!
//! The production code takes the program path as a parameter, so tests point
//! it at a script in a private temp dir instead of mutating the process-wide
//! `PATH` (which would race with other tests). On Windows the fake is a
//! `.cmd` (std runs `.bat`/`.cmd` through `cmd.exe`); on Unix a `sh` script.

use std::path::{Path, PathBuf};

/// A unique, empty temp directory for one test.
pub fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("idm_{tag}_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Write an executable script named `name` into `dir` and return its path.
/// Pass the body for both platforms; only the matching one is used.
pub fn fake_program(dir: &Path, name: &str, unix_body: &str, windows_body: &str) -> PathBuf {
    #[cfg(windows)]
    {
        let _ = unix_body;
        let path = dir.join(format!("{name}.cmd"));
        let body = format!("@echo off\r\n{}\r\n", windows_body.replace('\n', "\r\n"));
        std::fs::write(&path, body).unwrap();
        path
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = windows_body;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{unix_body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
}
