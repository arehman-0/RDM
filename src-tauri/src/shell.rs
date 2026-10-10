//! The app shell around the engine: component setup/update commands, app
//! settings (autostart), the system tray, "Pause all", and a safe Quit.
//! Everything here is Tauri glue; the logic it calls (`components.rs`,
//! `app_settings.rs`, `bridge.rs`) is Tauri-free and unit-tested.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, CustomMenuItem, Manager, State, SystemTray, SystemTrayEvent, SystemTrayMenu,
    SystemTrayMenuItem, Window,
};

use crate::app_settings::{apply_autostart, AppSettings, HkcuRunKey};
use crate::bridge::{Bridge, PairingPrompt};
use crate::components::{
    ComponentEvents, ComponentProgress, ComponentStatus, Components, UpdateOutcome,
};
use crate::queue::now_secs;
use crate::{lock, AppState, Ctx};

/// Shell state injected into the commands below.
pub struct ShellState {
    pub components: Arc<Components>,
    pub settings: Mutex<AppSettings>,
    pub settings_path: PathBuf,
    /// The first-run setup (or a retry) is downloading right now.
    pub setting_up: AtomicBool,
}

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

/// Forwards install/update progress to the UI as `component-progress`.
pub struct TauriComponentEvents {
    pub app: AppHandle,
}

impl ComponentEvents for TauriComponentEvents {
    fn progress(&self, p: &ComponentProgress) {
        let _ = self.app.emit_all("component-progress", p);
    }
}

/// `get_components_status` result and `components-status` event.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentsInfo {
    pub components: Vec<ComponentStatus>,
    /// Ids that are missing everywhere (the setup screen downloads these).
    pub missing: Vec<String>,
    /// Bytes the setup would download.
    pub missing_bytes: u64,
    pub setting_up: bool,
    pub auto_update: bool,
    /// Why the last setup attempt stopped, if it failed.
    pub error: Option<String>,
}

fn components_info(shell: &ShellState, error: Option<String>) -> ComponentsInfo {
    let missing = shell.components.missing();
    ComponentsInfo {
        components: shell.components.status(),
        missing_bytes: missing.iter().map(|c| c.size).sum(),
        missing: missing.into_iter().map(|c| c.id).collect(),
        setting_up: shell.setting_up.load(Ordering::SeqCst),
        auto_update: shell.components.auto_update(),
        error,
    }
}

#[tauri::command]
pub fn get_components_status(shell: State<'_, ShellState>) -> ComponentsInfo {
    components_info(&shell, None)
}

/// Download + verify + install whatever is missing. One run at a time; the
/// UI follows it through `component-progress` and `components-status`.
pub async fn run_setup(app: &AppHandle) {
    let shell = app.state::<ShellState>();
    if shell.setting_up.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = app.emit_all("components-status", components_info(&shell, None));
    let result = shell.components.install_missing().await;
    shell.setting_up.store(false, Ordering::SeqCst);
    let error = result.err().map(|e| e.to_string());
    let _ = app.emit_all("components-status", components_info(&shell, error));
}

/// The setup screen's Install / Retry button.
#[tauri::command]
pub async fn install_components(app: AppHandle) -> Result<(), String> {
    run_setup(&app).await;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YtdlpInfo {
    /// The version in use now (`None` when yt-dlp isn't installed).
    pub version: Option<String>,
    pub outcome: Option<UpdateOutcome>,
}

#[tauri::command]
pub async fn ytdlp_version(shell: State<'_, ShellState>) -> Result<YtdlpInfo, String> {
    Ok(YtdlpInfo {
        version: shell.components.ytdlp_version().await,
        outcome: None,
    })
}

/// "Check now".
#[tauri::command]
pub async fn check_ytdlp_update(shell: State<'_, ShellState>) -> Result<YtdlpInfo, String> {
    let outcome = shell.components.update_ytdlp(now_secs()).await;
    Ok(YtdlpInfo {
        version: shell.components.ytdlp_version().await,
        outcome: Some(outcome),
    })
}

/// "Keep yt-dlp up to date".
#[tauri::command]
pub fn set_ytdlp_auto_update(app: AppHandle, shell: State<'_, ShellState>, enabled: bool) {
    shell.components.set_auto_update(enabled);
    let _ = app.emit_all("components-status", components_info(&shell, None));
}

/// Background: the daily yt-dlp check (first one shortly after startup).
pub fn spawn_update_loop(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(20)).await;
        loop {
            let components = app.state::<ShellState>().components.clone();
            let outcome = components.update_ytdlp_if_due(false, now_secs()).await;
            if let UpdateOutcome::Updated { from, to } = &outcome {
                tracing::info!("yt-dlp updated {from} → {to}");
            }
            tokio::time::sleep(Duration::from_secs(60 * 60)).await;
        }
    });
}

/// A YouTube extraction failed: yt-dlp may be outdated, so check now
/// (rate-limited inside the manager).
pub fn update_after_extraction_failure(app: &AppHandle) {
    let components = app.state::<ShellState>().components.clone();
    tauri::async_runtime::spawn(async move {
        components.update_ytdlp_if_due(true, now_secs()).await;
    });
}

// ---------------------------------------------------------------------------
// App settings (autostart)
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn get_app_settings(shell: State<'_, ShellState>) -> AppSettings {
    lock(&shell.settings).clone()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettingsInput {
    start_with_windows: bool,
    start_minimized: bool,
}

/// Save the two user-facing settings; the HKCU Run value is added or removed
/// to match "Start with Windows".
#[tauri::command]
pub fn set_app_settings(
    shell: State<'_, ShellState>,
    settings: AppSettingsInput,
) -> Result<AppSettings, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    apply_autostart(&HkcuRunKey, settings.start_with_windows, &exe)?;
    let updated = {
        let mut s = lock(&shell.settings);
        s.start_with_windows = settings.start_with_windows;
        s.start_minimized = settings.start_minimized;
        s.clone()
    };
    updated.save(&shell.settings_path)?;
    Ok(updated)
}

// ---------------------------------------------------------------------------
// Pairing prompt
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn get_pending_pairing(bridge: State<'_, Arc<Bridge>>) -> Option<PairingPrompt> {
    bridge.pending_pairing()
}

#[tauri::command]
pub fn answer_pairing(
    bridge: State<'_, Arc<Bridge>>,
    id: String,
    allow: bool,
) -> Result<(), String> {
    bridge.answer_pairing(&id, allow).map_err(|e| e.message)
}

// ---------------------------------------------------------------------------
// Tray, window, quit
// ---------------------------------------------------------------------------

pub fn tray() -> SystemTray {
    let menu = SystemTrayMenu::new()
        .add_item(CustomMenuItem::new("open", "Open IDM Replica"))
        .add_native_item(SystemTrayMenuItem::Separator)
        .add_item(CustomMenuItem::new("pause_all", "Pause all"))
        .add_item(CustomMenuItem::new("resume_all", "Resume all"))
        .add_native_item(SystemTrayMenuItem::Separator)
        .add_item(CustomMenuItem::new("quit", "Quit"));
    SystemTray::new()
        .with_menu(menu)
        .with_tooltip("IDM Replica")
}

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

pub fn on_tray_event(app: &AppHandle, event: SystemTrayEvent) {
    match event {
        SystemTrayEvent::LeftClick { .. } | SystemTrayEvent::DoubleClick { .. } => show_main(app),
        SystemTrayEvent::MenuItemClick { id, .. } => match id.as_str() {
            "open" => show_main(app),
            "pause_all" | "resume_all" => {
                let (app, pause) = (app.clone(), id == "pause_all");
                tauri::async_runtime::spawn(async move { pause_all(&app, pause).await });
            }
            "quit" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move { quit(app).await });
            }
            _ => {}
        },
        _ => {}
    }
}

/// The window's X: keep running in the tray (browser take-over only works
/// while the app runs). The first time, say so.
pub fn hide_to_tray(window: &Window) {
    let _ = window.hide();
    let app = window.app_handle();
    let shell = app.state::<ShellState>();
    let first_time = {
        let mut s = lock(&shell.settings);
        !std::mem::replace(&mut s.tray_notice_shown, true)
    };
    if first_time {
        let snapshot = lock(&shell.settings).clone();
        let _ = snapshot.save(&shell.settings_path);
        tauri::api::dialog::message(
            None::<&Window>,
            "IDM Replica is still running",
            "IDM Replica keeps running in the system tray so it can take over downloads \
             from your browser.\n\nUse the tray icon to open it again, or choose Quit there \
             to close it completely.",
        );
    }
}

/// Tray "Pause all" / "Resume all": running engine jobs and waiting entries.
pub async fn pause_all(app: &AppHandle, pause: bool) {
    let ctx = {
        let state = app.state::<AppState>();
        Ctx::new(app, &state)
    };
    for id in ctx.queue.pending_ids() {
        ctx.queue.set_held(&ctx, &id, pause, now_secs());
    }
    for job in ctx.manager.all().await {
        if pause {
            job.pause();
        } else {
            job.resume();
        }
    }
}

/// Really exit — safely. Engine jobs are paused (workers stop at the next
/// frame, the flusher drains), we wait for the coordinator's next ticks to
/// write each resume sidecar, yt-dlp processes are killed, the history is
/// flushed, and only then does the process end. Nothing is lost: on the next
/// launch these downloads show as "Interrupted — resumable".
pub async fn quit(app: AppHandle) {
    let (manager, history, streams) = {
        let state = app.state::<AppState>();
        (
            state.manager.clone(),
            state.history.clone(),
            state.streams.clone(),
        )
    };
    for token in lock(&streams).values() {
        token.cancel();
    }
    for job in manager.all().await {
        job.pause();
    }
    // Two progress ticks (~400 ms each): sidecars now cover every flushed byte.
    tokio::time::sleep(Duration::from_millis(900)).await;
    if let Err(e) = history.flush() {
        tracing::warn!(error = %e, "could not save download history on quit");
    }
    tracing::info!("quit from the tray");
    app.exit(0);
}
