//! Updates from GitHub Releases. The built app checks shortly after launch and then every few
//! hours; a newer version is downloaded, checked against the public key in tauri.conf.json, and put
//! in place of the app on disk straight away. The running app carries on as it is: the new version
//! opens at the next launch, or now via "Restart to update". Development builds only report.

use serde::Serialize;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

use crate::Relock;

const EVERY: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub current: String,
    /// A newer version on GitHub.
    pub available: Option<String>,
    pub notes: Option<String>,
    /// The version installed on disk, waiting for a restart.
    pub installed: Option<String>,
    pub busy: bool,
    pub last_check: Option<String>,
    pub error: Option<String>,
    /// Downloads and installs by itself (Settings), otherwise only says one is available.
    pub automatic: bool,
    /// A built app that can replace itself (not `tauri dev`).
    pub can_install: bool,
}

#[derive(Default)]
pub struct Updates {
    status: Mutex<Status>,
}

fn status(app: &AppHandle) -> Status {
    let mut s = app.state::<Updates>().status.lock().unwrap_or_else(|e| e.into_inner()).clone();
    s.current = app.package_info().version.to_string();
    s.automatic = automatic(app);
    s.can_install = crate::notifications::bundled();
    s
}

fn automatic(app: &AppHandle) -> bool {
    app.state::<crate::AppState>().setting("auto_update").as_deref() != Some("0")
}

fn change(app: &AppHandle, f: impl FnOnce(&mut Status)) {
    f(&mut app.state::<Updates>().status.lock().unwrap_or_else(|e| e.into_inner()));
    let _ = app.emit("update-status", status(app));
}

/// Look for a newer version; download and install it if `install` (and this is a built app).
async fn check(app: &AppHandle, install: bool) -> Result<(), String> {
    change(app, |s| {
        s.busy = true;
        s.error = None;
    });
    let result = async {
        let mut builder = app.updater_builder().timeout(Duration::from_secs(60));
        // Test aid: point at a local latest.json instead of GitHub.
        if let Ok(url) = std::env::var("DEV_INBOX_UPDATE_URL") {
            builder = builder.endpoints(vec![url.parse().map_err(|e| format!("{e}"))?]).map_err(|e| e.to_string())?;
        }
        let updater = builder.build().map_err(|e| e.to_string())?;
        let Some(update) = updater.check().await.map_err(|e| e.to_string())? else {
            change(app, |s| s.available = None);
            return Ok(());
        };
        let version = update.version.clone();
        change(app, |s| {
            s.available = Some(version.clone());
            s.notes = update.body.clone();
        });
        if !install || !crate::notifications::bundled() || status(app).installed.as_deref() == Some(&version) {
            return Ok(());
        }
        let bytes = update.download(|_, _| {}, || {}).await.map_err(|e| format!("Download failed: {e}"))?;
        // Swapping the bundle can ask for an admin password (an app in a folder you can't write);
        // the plugin does that through the main thread, so this mustn't run on it.
        tauri::async_runtime::spawn_blocking(move || update.install(bytes))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("Install failed: {e}"))?;
        change(app, |s| s.installed = Some(version));
        Ok(())
    }
    .await;
    change(app, |s| {
        s.busy = false;
        s.last_check = Some(crate::now());
        s.error = result.clone().err();
    });
    result
}

/// Background checks for the built app: a minute after launch, then every few hours.
pub fn start(app: AppHandle) {
    let testing = std::env::var("DEV_INBOX_UPDATE_URL").is_ok();
    if !crate::notifications::bundled() || app.state::<crate::AppState>().demo && !testing {
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(if testing { 3 } else { 60 })).await;
        loop {
            let install = automatic(&app);
            if let Err(e) = check(&app, install).await {
                eprintln!("update check: {e}");
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}

#[tauri::command(async)]
pub fn update_status(app: AppHandle) -> Status {
    status(&app)
}

/// "Check now", or "Download and install" when automatic updates are off.
#[tauri::command]
pub async fn check_for_updates(app: AppHandle, install: bool) -> Result<Status, String> {
    if status(&app).busy {
        return Ok(status(&app));
    }
    check(&app, install || automatic(&app)).await?;
    Ok(status(&app))
}

#[tauri::command(async)]
pub fn set_auto_update(app: AppHandle, on: bool) -> Result<(), String> {
    let state = app.state::<crate::AppState>();
    crate::db::set_setting(&state.db.relock(), "auto_update", if on { "1" } else { "0" })
        .map_err(|e| e.to_string())?;
    let _ = app.emit("update-status", status(&app));
    Ok(())
}

/// Open the installed new version now.
#[tauri::command]
pub fn restart_to_update(app: AppHandle) -> Result<(), String> {
    if status(&app).installed.is_none() {
        return Err("No update is installed yet".into());
    }
    if crate::recording_now(&app) {
        return Err("Stop the recording first — restarting would end it".into());
    }
    app.restart();
}
