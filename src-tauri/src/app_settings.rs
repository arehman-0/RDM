//! App-level settings (`settings.json` in the app data dir) and the optional
//! "Start with Windows" entry. No Tauri here.
//!
//! Autostart is a per-user value under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` â€” never HKLM, so it
//! needs no elevation and affects only this user. Turning it off deletes the
//! value. The registry is reached through [`RunKey`], so the logic is tested
//! with an in-memory fake.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::history::write_json_atomic;

pub const SETTINGS_FILE: &str = "settings.json";
/// Value name under the Run key.
pub const RUN_VALUE_NAME: &str = "IDM Replica";
/// Passed to the app when Windows starts it at login.
pub const AUTOSTART_FLAG: &str = "--autostart";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    /// Launch at login (HKCU Run key). Default off.
    pub start_with_windows: bool,
    /// When launched at login, stay in the tray instead of showing the window.
    pub start_minimized: bool,
    /// The "still running in the tray" explanation was shown once.
    pub tray_notice_shown: bool,
    /// The "Install the browser extension" step was finished or skipped.
    pub extension_setup_done: bool,
}

impl AppSettings {
    /// Missing or unreadable file = defaults.
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        write_json_atomic(path, self).map_err(|e| format!("{e:#}"))
    }
}

/// The command line stored in the Run key: the quoted exe + the flag.
pub fn run_command(exe: &Path) -> String {
    format!("\"{}\" {AUTOSTART_FLAG}", exe.display())
}

/// Was this process started by the Run key entry?
pub fn launched_at_login(args: &[String]) -> bool {
    args.iter().skip(1).any(|a| a == AUTOSTART_FLAG)
}

/// The per-user "run at login" registry key.
pub trait RunKey {
    fn get(&self, name: &str) -> Option<String>;
    fn set(&self, name: &str, command: &str) -> Result<(), String>;
    /// Removing a value that isn't there is not an error.
    fn remove(&self, name: &str) -> Result<(), String>;
}

/// Make the Run key match `enabled` (idempotent).
pub fn apply_autostart(key: &dyn RunKey, enabled: bool, exe: &Path) -> Result<(), String> {
    if enabled {
        let wanted = run_command(exe);
        if key.get(RUN_VALUE_NAME).as_deref() != Some(wanted.as_str()) {
            key.set(RUN_VALUE_NAME, &wanted)?;
        }
        Ok(())
    } else {
        key.remove(RUN_VALUE_NAME)
    }
}

/// The real key, driven through `reg.exe` (no console window, no extra crate).
pub struct HkcuRunKey;

#[cfg(windows)]
mod imp {
    use super::{HkcuRunKey, RunKey};
    use std::process::Stdio;

    const KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn reg(args: &[&str]) -> std::io::Result<std::process::Output> {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("reg")
            .args(args)
            .stdin(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
    }

    impl RunKey for HkcuRunKey {
        fn get(&self, name: &str) -> Option<String> {
            let out = reg(&["query", KEY, "/v", name]).ok()?;
            if !out.status.success() {
                return None;
            }
            // "    IDM Replica    REG_SZ    "C:\...\app.exe" --autostart"
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .find_map(|l| l.split_once("REG_SZ").map(|(_, v)| v.trim().to_string()))
        }

        fn set(&self, name: &str, command: &str) -> Result<(), String> {
            let out = reg(&["add", KEY, "/v", name, "/t", "REG_SZ", "/d", command, "/f"])
                .map_err(|e| format!("could not run reg.exe: {e}"))?;
            if out.status.success() {
                Ok(())
            } else {
                Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
            }
        }

        fn remove(&self, name: &str) -> Result<(), String> {
            if self.get(name).is_none() {
                return Ok(());
            }
            let out = reg(&["delete", KEY, "/v", name, "/f"])
                .map_err(|e| format!("could not run reg.exe: {e}"))?;
            if out.status.success() {
                Ok(())
            } else {
                Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
            }
        }
    }
}

#[cfg(unix)]
mod imp {
    use super::{HkcuRunKey, RunKey};

    // Shipping target is Windows; elsewhere the setting is simply unavailable.
    impl RunKey for HkcuRunKey {
        fn get(&self, _name: &str) -> Option<String> {
            None
        }
        fn set(&self, _name: &str, _command: &str) -> Result<(), String> {
            Err("Start with Windows is only available on Windows".into())
        }
        fn remove(&self, _name: &str) -> Result<(), String> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeKey {
        values: Mutex<HashMap<String, String>>,
        writes: Mutex<u32>,
    }

    impl RunKey for FakeKey {
        fn get(&self, name: &str) -> Option<String> {
            self.values.lock().unwrap().get(name).cloned()
        }
        fn set(&self, name: &str, command: &str) -> Result<(), String> {
            *self.writes.lock().unwrap() += 1;
            self.values
                .lock()
                .unwrap()
                .insert(name.into(), command.into());
            Ok(())
        }
        fn remove(&self, name: &str) -> Result<(), String> {
            self.values.lock().unwrap().remove(name);
            Ok(())
        }
    }

    #[test]
    fn autostart_adds_and_cleanly_removes_the_run_value() {
        let key = FakeKey::default();
        let exe = PathBuf::from(r"C:\Program Files\IDM Replica\IDM Replica.exe");
        apply_autostart(&key, true, &exe).unwrap();
        assert_eq!(
            key.get(RUN_VALUE_NAME).as_deref(),
            Some(r#""C:\Program Files\IDM Replica\IDM Replica.exe" --autostart"#)
        );
        // Idempotent: no second write for the same command.
        apply_autostart(&key, true, &exe).unwrap();
        assert_eq!(*key.writes.lock().unwrap(), 1);
        // A moved exe is corrected.
        apply_autostart(&key, true, Path::new(r"D:\Apps\idm.exe")).unwrap();
        assert!(key
            .get(RUN_VALUE_NAME)
            .unwrap()
            .starts_with(r#""D:\Apps\idm.exe""#));
        // Off: the value is gone (and removing twice is fine).
        apply_autostart(&key, false, &exe).unwrap();
        assert_eq!(key.get(RUN_VALUE_NAME), None);
        apply_autostart(&key, false, &exe).unwrap();
        assert!(key.values.lock().unwrap().is_empty());
    }

    #[test]
    fn login_flag_detection() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(launched_at_login(&args(&["app.exe", "--autostart"])));
        assert!(!launched_at_login(&args(&["app.exe"])));
        assert!(!launched_at_login(&args(&["--autostart"])));
        assert!(!launched_at_login(&args(&[
            "app.exe",
            "https://x.example/a.zip"
        ])));
    }

    #[test]
    fn settings_default_off_and_round_trip() {
        let dir = temp_dir("app_settings");
        let path = dir.join(SETTINGS_FILE);
        let s = AppSettings::load(&path);
        assert_eq!(s, AppSettings::default());
        assert!(!s.start_with_windows && !s.start_minimized && !s.tray_notice_shown);
        let changed = AppSettings {
            start_with_windows: true,
            start_minimized: true,
            tray_notice_shown: true,
            extension_setup_done: true,
        };
        changed.save(&path).unwrap();
        assert_eq!(AppSettings::load(&path), changed);
        std::fs::write(&path, b"{ broken").unwrap();
        assert_eq!(AppSettings::load(&path), AppSettings::default());
        let _ = std::fs::remove_dir_all(dir);
    }
}
