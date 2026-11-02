//! Getting the browser extension installed: the store listing config, which
//! Chromium browsers are installed, and opening the right page.
//!
//! Chrome only installs extensions with the user's consent — from a store
//! page, or "Load unpacked" in developer mode. Nothing here bypasses that: no
//! enterprise policies, no registry installs. We only open pages and folders.
//!
//! `src-tauri/extension-store.json` is the single place for the store IDs and
//! URLs (the installer reads it too, via `scripts/build-installer.ps1`).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// The embedded store config. `null` = not published yet.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoreConfig {
    /// Free-text reminder in the file; ignored.
    #[serde(default, rename = "TODO", skip_serializing)]
    pub todo: Option<String>,
    pub chrome_web_store_id: Option<String>,
    pub chrome_web_store_url: Option<String>,
    pub edge_addons_id: Option<String>,
    pub edge_addons_url: Option<String>,
}

pub const STORE_JSON: &str = include_str!("../extension-store.json");

impl StoreConfig {
    pub fn parse(json: &str) -> Result<Self, String> {
        let c: Self = serde_json::from_str(json).map_err(|e| e.to_string())?;
        for id in [&c.chrome_web_store_id, &c.edge_addons_id]
            .into_iter()
            .flatten()
        {
            if !is_extension_id(id) {
                return Err(format!("`{id}` is not an extension ID (32 letters a-p)"));
            }
        }
        for url in [&c.chrome_web_store_url, &c.edge_addons_url]
            .into_iter()
            .flatten()
        {
            if !url.starts_with("https://") {
                return Err(format!("store URL must be https: {url}"));
            }
        }
        Ok(c)
    }

    /// The config built into this exe (checked by a unit test).
    pub fn embedded() -> &'static Self {
        static CONFIG: OnceLock<StoreConfig> = OnceLock::new();
        CONFIG.get_or_init(|| Self::parse(STORE_JSON).unwrap_or_default())
    }

    /// Store-assigned extension IDs (Chrome and Edge assign different ones).
    pub fn store_ids(&self) -> impl Iterator<Item = &str> {
        [&self.chrome_web_store_id, &self.edge_addons_id]
            .into_iter()
            .flatten()
            .map(String::as_str)
    }
}

pub fn is_extension_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| (b'a'..=b'p').contains(&b))
}

// ---------------------------------------------------------------------------
// Browsers
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Browser {
    Chrome,
    Edge,
    Brave,
}

impl Browser {
    pub const ALL: [Browser; 3] = [Browser::Chrome, Browser::Edge, Browser::Brave];

    pub fn name(self) -> &'static str {
        match self {
            Browser::Chrome => "Google Chrome",
            Browser::Edge => "Microsoft Edge",
            Browser::Brave => "Brave",
        }
    }

    /// The exe name registered under "App Paths".
    pub fn exe(self) -> &'static str {
        match self {
            Browser::Chrome => "chrome.exe",
            Browser::Edge => "msedge.exe",
            Browser::Brave => "brave.exe",
        }
    }

    /// Its extensions page (only the browser itself can open this).
    pub fn extensions_url(self) -> &'static str {
        match self {
            Browser::Chrome => "chrome://extensions",
            Browser::Edge => "edge://extensions",
            Browser::Brave => "brave://extensions",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledBrowser {
    pub id: Browser,
    pub name: String,
    pub path: PathBuf,
}

/// What the "Install the browser extension" screen needs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupInfo {
    pub browsers: Vec<InstalledBrowser>,
    pub chrome_web_store_url: Option<String>,
    pub edge_addons_url: Option<String>,
    /// The unpacked extension shipped with the app, if this build has it.
    pub unpacked_dir: Option<PathBuf>,
}

pub fn setup_info() -> SetupInfo {
    let c = StoreConfig::embedded();
    SetupInfo {
        browsers: installed_browsers(),
        chrome_web_store_url: c.chrome_web_store_url.clone(),
        edge_addons_url: c.edge_addons_url.clone(),
        unpacked_dir: unpacked_dir(),
    }
}

/// `<exe dir>/resources/extension` — where the installer puts the unpacked
/// extension (it has a `manifest.json`).
pub fn unpacked_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.join("resources").join("extension");
    dir.join("manifest.json").is_file().then_some(dir)
}

/// The value of `(Default)` in `reg query <key> /ve` output.
#[cfg(windows)]
pub fn parse_reg_default(out: &str) -> Option<PathBuf> {
    out.lines().find_map(|l| {
        let (_, v) = l
            .split_once("REG_SZ")
            .or_else(|| l.split_once("REG_EXPAND_SZ"))?;
        let v = v.trim().trim_matches('"');
        (!v.is_empty()).then(|| PathBuf::from(v))
    })
}

#[cfg(windows)]
fn app_path(exe: &str) -> Option<PathBuf> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    for root in ["HKCU", "HKLM"] {
        let key = format!(r"{root}\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\{exe}");
        let out = std::process::Command::new("reg")
            .args(["query", &key, "/ve"])
            .stdin(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        if out.status.success() {
            if let Some(p) = parse_reg_default(&String::from_utf8_lossy(&out.stdout)) {
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    None
}

#[cfg(unix)]
fn app_path(_exe: &str) -> Option<PathBuf> {
    // Shipping target is Windows; elsewhere nothing is detected.
    None
}

/// Chrome, Edge and Brave, if installed (registered under "App Paths").
pub fn installed_browsers() -> Vec<InstalledBrowser> {
    Browser::ALL
        .iter()
        .filter_map(|&b| {
            app_path(b.exe()).map(|path| InstalledBrowser {
                id: b,
                name: b.name().into(),
                path,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Opening pages
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Store {
    Chrome,
    Edge,
}

/// What to launch for a store page: the store's own browser if installed
/// (`Some(exe)`), else the default browser (`None`).
pub fn store_launch(
    store: Store,
    config: &StoreConfig,
    browsers: &[InstalledBrowser],
) -> Result<(Option<PathBuf>, String), String> {
    let (url, wanted) = match store {
        Store::Chrome => (&config.chrome_web_store_url, Browser::Chrome),
        Store::Edge => (&config.edge_addons_url, Browser::Edge),
    };
    let url = url.clone().ok_or_else(|| {
        "The extension isn't published in this store yet — use Manual install.".to_string()
    })?;
    let exe = browsers
        .iter()
        .find(|b| b.id == wanted)
        .map(|b| b.path.clone());
    Ok((exe, url))
}

fn spawn(program: &Path, arg: &str) -> Result<(), String> {
    std::process::Command::new(program)
        .arg(arg)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(drop)
        .map_err(|e| format!("could not start {}: {e}", program.display()))
}

/// Open a store listing.
pub fn open_store(store: Store) -> Result<(), String> {
    let (exe, url) = store_launch(store, StoreConfig::embedded(), &installed_browsers())?;
    match exe {
        Some(exe) => spawn(&exe, &url),
        None => open_default(&url),
    }
}

/// Open `chrome://extensions` (or Edge's / Brave's) in that browser.
pub fn open_extensions_page(browser: Browser) -> Result<(), String> {
    let b = installed_browsers()
        .into_iter()
        .find(|b| b.id == browser)
        .ok_or_else(|| format!("{} is not installed", browser.name()))?;
    spawn(&b.path, browser.extensions_url())
}

/// Show the unpacked extension folder in Explorer.
pub fn reveal_unpacked() -> Result<(), String> {
    let dir = unpacked_dir().ok_or_else(|| {
        "This build has no unpacked extension (it ships with the installer).".to_string()
    })?;
    open_default(&dir.to_string_lossy())
}

#[cfg(windows)]
fn open_default(target: &str) -> Result<(), String> {
    spawn(Path::new("explorer.exe"), target)
}

#[cfg(unix)]
fn open_default(target: &str) -> Result<(), String> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    spawn(Path::new(opener), target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_store_config_is_valid() {
        let c = StoreConfig::parse(STORE_JSON).expect("extension-store.json must parse");
        assert_eq!(&c, StoreConfig::embedded());
    }

    #[test]
    fn store_config_validation() {
        let ok = r#"{"chromeWebStoreId":"abcdefghijklmnopabcdefghijklmnop",
            "chromeWebStoreUrl":"https://chromewebstore.google.com/detail/x",
            "edgeAddonsId":null,"edgeAddonsUrl":null}"#;
        let c = StoreConfig::parse(ok).unwrap();
        assert_eq!(
            c.store_ids().collect::<Vec<_>>(),
            ["abcdefghijklmnopabcdefghijklmnop"]
        );
        let bad_id = ok.replace("abcdefghijklmnopabcdefghijklmnop", "xyz");
        assert!(StoreConfig::parse(&bad_id).is_err());
        let http = ok.replace("https://", "http://");
        assert!(StoreConfig::parse(&http).is_err());
        assert!(StoreConfig::parse(r#"{"surprise":1}"#).is_err());
    }

    #[test]
    #[cfg(windows)]
    fn reg_default_value_parsing() {
        let out = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\...\\chrome.exe\r\n    (Default)    REG_SZ    C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe\r\n    Path    REG_SZ    C:\\x\r\n";
        assert_eq!(
            parse_reg_default(out),
            Some(PathBuf::from(
                r"C:\Program Files\Google\Chrome\Application\chrome.exe"
            ))
        );
        let quoted = "    (Default)    REG_SZ    \"C:\\b\\brave.exe\"";
        assert_eq!(
            parse_reg_default(quoted),
            Some(PathBuf::from(r"C:\b\brave.exe"))
        );
        assert_eq!(parse_reg_default("ERROR: not found"), None);
    }

    #[test]
    fn store_pages_open_in_their_own_browser_or_the_default() {
        let unpublished = StoreConfig::default();
        assert!(store_launch(Store::Chrome, &unpublished, &[]).is_err());
        let c = StoreConfig {
            chrome_web_store_url: Some("https://chrome.example/x".into()),
            edge_addons_url: Some("https://edge.example/x".into()),
            ..Default::default()
        };
        let chrome = InstalledBrowser {
            id: Browser::Chrome,
            name: "Google Chrome".into(),
            path: PathBuf::from("chrome.exe"),
        };
        assert_eq!(
            store_launch(Store::Chrome, &c, std::slice::from_ref(&chrome)).unwrap(),
            (
                Some(PathBuf::from("chrome.exe")),
                "https://chrome.example/x".into()
            )
        );
        // Edge isn't installed: the default browser opens the listing.
        assert_eq!(
            store_launch(Store::Edge, &c, &[chrome]).unwrap(),
            (None, "https://edge.example/x".into())
        );
    }
}
