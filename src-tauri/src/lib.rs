mod day;
mod db;
mod demo;
mod derive;
mod editor;
mod github;
mod intent;
mod jira;
mod ai;
mod ask;
mod ci;
mod meetings;
#[cfg(target_os = "macos")]
mod notifications;
mod reminders;
mod search;
mod suggest;
mod summary;
mod sync;
mod voices;

use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use std::sync::Mutex;
use std::time::Duration;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
#[cfg(not(target_os = "macos"))]
use tauri_plugin_notification::NotificationExt;

const KEYRING_SERVICE: &str = "dev-inbox";
const KEYRING_USER: &str = "github-token";
const DEFAULT_POLL_SECS: u64 = 120;

/// Locks that survive a panic elsewhere: a job that crashed while holding one mustn't take every
/// later database call (and so the whole app) down with it.
pub(crate) trait Relock<T> {
    fn relock(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> Relock<T> for Mutex<T> {
    fn relock(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

pub struct AppState {
    db: Mutex<Connection>,
    token: Mutex<Option<String>>,
    /// Jira API token, read from the keychain once per launch.
    jira_token: Mutex<Option<String>>,
    /// Anthropic API key for Claude meeting summaries, read from the keychain when first needed.
    anthropic_key: Mutex<Option<String>>,
    jira_lock: tokio::sync::Mutex<()>,
    /// The meeting being recorded, if any.
    recorder: Mutex<Option<meetings::Recording>>,
    /// Background jobs queued or running ("transcribe:<id>", "summary:<id>"), so none runs twice.
    jobs: Mutex<std::collections::HashSet<String>>,
    /// Held while a heavy job runs: one at a time.
    heavy_job: Mutex<()>,
    sync_lock: tokio::sync::Mutex<()>,
    wake: tokio::sync::Notify,
    /// `DEV_INBOX_DEMO=1`: fictional data in a separate database, never synced.
    demo: bool,
}

impl AppState {
    /// The GitHub token, read from the OS keychain once per launch.
    fn token(&self) -> Option<String> {
        let mut cached = self.token.relock();
        if cached.is_none() {
            *cached = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).ok().and_then(|e| e.get_password().ok());
        }
        cached.clone()
    }

    fn setting(&self, key: &str) -> Option<String> {
        db::get_setting(&self.db.relock(), key).ok().flatten()
    }
}

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn now() -> String {
    derive::fmt_ts(chrono::Utc::now())
}

// ---------- sync ----------

/// How many things need you: PRs and issues, Jira tickets, and your meeting action items.
fn needs_you_count(state: &AppState) -> usize {
    let prs = db::list_items(&state.db.relock())
        .map(|items| items.iter().filter(|i| i.reminder_due || derive::is_actionable(&i.state)).count())
        .unwrap_or(0);
    prs + ticket_attention(state).len() + open_actions(state).len()
}

/// The menu bar: a monochrome inbox icon (with a dot when something needs you, and the number too if
/// Settings asks for it) and "REC" while recording. Clicking it opens the panel.
fn refresh_tray(app: &AppHandle) {
    let state = app.state::<AppState>();
    let show_count = state.setting("menu_bar_count").as_deref() == Some("1");
    let recording = state.recorder.relock().is_some();
    let total = needs_you_count(&state);
    let Some(tray) = app.tray_by_id("main") else { return };
    #[cfg(target_os = "macos")]
    {
        let bytes: &[u8] = if total > 0 { include_bytes!("../icons/tray-dot.png") } else { include_bytes!("../icons/tray.png") };
        if let Ok(icon) = tauri::image::Image::from_bytes(bytes) {
            let _ = tray.set_icon(Some(icon));
            let _ = tray.set_icon_as_template(true);
        }
    }
    let title = if recording {
        Some("REC".to_string())
    } else {
        (show_count && total > 0).then(|| total.to_string())
    };
    let _ = tray.set_title(title);
    let tooltip = match total {
        0 => "Dev Inbox — all caught up".to_string(),
        1 => "Dev Inbox — 1 thing needs you".to_string(),
        n => format!("Dev Inbox — {n} things need you"),
    };
    let _ = tray.set_tooltip(Some(tooltip));
    // Let an open panel catch up.
    let _ = app.emit_to("tray", "inbox-updated", ());
}

/// Right-click on the icon: a short native menu.
fn tray_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::new(app)?;
    menu.append(&MenuItem::with_id(app, "open", "Open Dev Inbox", true, None::<&str>)?)?;
    menu.append(&MenuItem::with_id(app, "sync", "Sync now", true, None::<&str>)?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, "quit", "Quit Dev Inbox", true, None::<&str>)?)?;
    Ok(menu)
}

fn tray_action(app: &AppHandle, id: &str) {
    match id {
        "open" => show_main(app),
        "sync" => app.state::<AppState>().wake.notify_one(),
        "quit" => app.exit(0),
        _ => {}
    }
}

const PANEL_WIDTH: f64 = 380.0;

/// The menu bar panel: a borderless glass window of the app's own UI, under the icon.
fn create_panel(app: &AppHandle) -> tauri::Result<()> {
    use tauri::utils::config::{WindowEffectsConfig};
    use tauri::window::{Effect, EffectState};
    let panel = tauri::WebviewWindowBuilder::new(app, "tray", tauri::WebviewUrl::App("index.html".into()))
        .title("Dev Inbox")
        .inner_size(PANEL_WIDTH, 480.0)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        // The glass draws its own rounded edge; a window shadow would outline the square frame.
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible_on_all_workspaces(true)
        .visible(false)
        .focused(false)
        .effects(WindowEffectsConfig {
            // Liquid Glass on macOS 26+, the classic popover material before.
            effects: vec![Effect::LiquidGlassRegular, Effect::Popover],
            state: Some(EffectState::Active),
            radius: Some(16.0),
            color: None,
            interactive: false,
        })
        .build()?;
    let _ = panel;
    Ok(())
}

/// Show the panel under the tray icon (or hide it if it's showing).
fn toggle_panel(app: &AppHandle, icon: tauri::Rect) {
    let Some(panel) = app.get_webview_window("tray") else { return };
    if panel.is_visible().unwrap_or(false) {
        let _ = panel.hide();
        return;
    }
    let scale = panel.scale_factor().unwrap_or(2.0);
    let at = icon.position.to_physical::<f64>(scale);
    let size = icon.size.to_physical::<f64>(scale);
    let width = PANEL_WIDTH * scale;
    let mut x = at.x + size.width / 2.0 - width / 2.0;
    // Keep it on the screen the icon is on.
    if let Ok(Some(monitor)) = panel.monitor_from_point(at.x, at.y) {
        let left = monitor.position().x as f64;
        let right = left + monitor.size().width as f64;
        x = x.clamp(left + 8.0 * scale, right - width - 8.0 * scale);
    }
    let y = at.y + size.height + 6.0 * scale;
    let _ = panel.set_position(tauri::PhysicalPosition::new(x, y));
    let _ = panel.show();
    let _ = panel.set_focus();
    let _ = app.emit_to("tray", "panel-shown", ());
}

/// From the panel: open something in the main window ("view" with a view name, "ticket" with a key,
/// "meeting" with an id, "palette"), and put the panel away.
#[tauri::command(async)]
fn open_in_main(app: AppHandle, kind: String, target: Option<String>) {
    if let Some(panel) = app.get_webview_window("tray") {
        let _ = panel.hide();
    }
    show_main(&app);
    let target = target.unwrap_or_default();
    let _ = match kind.as_str() {
        "item" => app.emit_to("main", "open-item", target),
        "ticket" => app.emit_to("main", "open-ticket", target),
        "meeting" => app.emit_to("main", "open-meeting", target),
        "palette" => app.emit_to("main", "open-palette", ()),
        _ => app.emit_to("main", "navigate", if target.is_empty() { "today".to_string() } else { target }),
    };
}

#[tauri::command(async)]
fn hide_panel(app: AppHandle) {
    if let Some(panel) = app.get_webview_window("tray") {
        let _ = panel.hide();
    }
}

#[tauri::command(async)]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

fn refresh_count(app: &AppHandle) {
    refresh_tray(app);
    let _ = app.emit("inbox-updated", ());
}

async fn run_sync(app: &AppHandle) -> CmdResult<()> {
    let state = app.state::<AppState>();
    if state.demo {
        refresh_count(app);
        return Ok(());
    }
    let _guard = state.sync_lock.lock().await;
    let Some(login) = state.setting("login") else { return Ok(()) };
    let Some(token) = state.token() else { return Err("Not signed in".into()) };
    let _ = app.emit("sync-started", ());

    let client = github::Client::new(&token);
    let ctx = derive::Ctx::new(&login, &ignored_users(&state));
    let result = sync::run(&client, &state.db, &ctx).await;
    match result {
        Ok(out) => {
            if state.setting("notifications").as_deref() != Some("0") {
                for n in out.notices {
                    notify(app, n);
                }
            }
            let _ = out.needs_you;
            refresh_tray(app);
            let _ = app.emit("inbox-updated", ());
            read_intents(app);
            Ok(())
        }
        Err(e) => {
            let msg = format!("{e:#}");
            let _ = db::set_setting(&state.db.relock(), "last_error", &msg);
            let _ = app.emit("inbox-updated", ());
            Err(msg)
        }
    }
}

/// Show a desktop notification. On macOS, clicking it opens the item (and marks it seen).
fn notify(app: &AppHandle, n: sync::Notice) {
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        let (subtitle, message) = n.body.split_once('\n').unwrap_or(("", n.body.as_str()));
        notifications::send(&n.title, subtitle, message, n.url.as_deref(), n.item_id.as_deref());
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app.notification().builder().title(&n.title).body(&n.body).show();
    }
}

fn poll_secs(state: &AppState) -> u64 {
    state.setting("poll_seconds").and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_POLL_SECS).max(30)
}

fn start_poller(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            if run_sync(&app).await.is_ok() {
                maybe_digest(&app);
            }
            let _ = run_jira_sync(&app).await;
            let state = app.state::<AppState>();
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(poll_secs(&state))) => {}
                _ = state.wake.notified() => {}
            }
        }
    });
}

// ---------- auth ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    login: String,
    avatar_url: String,
}

#[tauri::command(async)]
fn get_session(state: State<AppState>) -> Option<Session> {
    Some(Session { login: state.setting("login")?, avatar_url: state.setting("avatar").unwrap_or_default() })
}

async fn sign_in_inner(app: &AppHandle, token: String) -> CmdResult<Session> {
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err("Token is empty".into());
    }
    let viewer = github::Client::new(&token).viewer().await.map_err(|e| format!("{e:#}"))?;
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .and_then(|e| e.set_password(&token))
        .map_err(|e| format!("Couldn't save token to the keychain: {e}"))?;
    let state = app.state::<AppState>();
    {
        let conn = state.db.relock();
        // A different account's memory must never bleed into this one.
        if db::get_setting(&conn, "login").map_err(err)?.is_some_and(|l| l != viewer.login) {
            db::reset(&conn).map_err(err)?;
        }
        db::set_setting(&conn, "login", &viewer.login).map_err(err)?;
        db::set_setting(&conn, "avatar", &viewer.avatar_url).map_err(err)?;
    }
    *state.token.relock() = Some(token);
    state.wake.notify_one();
    Ok(Session { login: viewer.login, avatar_url: viewer.avatar_url })
}

#[tauri::command]
async fn sign_in(app: AppHandle, token: String) -> CmdResult<Session> {
    sign_in_inner(&app, token).await
}

/// Reuse the GitHub CLI's login, if the user has `gh` installed and authenticated.
#[tauri::command]
async fn import_gh_token(app: AppHandle) -> CmdResult<Session> {
    // GUI apps on macOS don't inherit the shell PATH, so try the usual install locations too.
    let home = std::env::var("HOME").unwrap_or_default();
    let candidates = [
        "gh".to_string(),
        "/opt/homebrew/bin/gh".into(),
        "/usr/local/bin/gh".into(),
        format!("{home}/.local/bin/gh"),
        "/usr/bin/gh".into(),
    ];
    let mut last_err = "GitHub CLI (gh) not found".to_string();
    for bin in candidates {
        match std::process::Command::new(&bin).args(["auth", "token", "--hostname", "github.com"]).output() {
            Ok(out) if out.status.success() => {
                let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
                return sign_in_inner(&app, token).await;
            }
            Ok(out) => last_err = String::from_utf8_lossy(&out.stderr).trim().to_string(),
            Err(_) => continue,
        }
    }
    Err(last_err)
}

#[tauri::command(async)]
fn sign_out(state: State<AppState>, delete_data: bool) -> CmdResult<()> {
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER) {
        let _ = entry.delete_credential();
    }
    *state.token.relock() = None;
    let conn = state.db.relock();
    if delete_data {
        db::reset(&conn).map_err(err)?;
    } else {
        db::delete_setting(&conn, "login").map_err(err)?;
    }
    Ok(())
}

// ---------- inbox ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Inbox {
    items: Vec<db::Item>,
    last_sync: Option<String>,
    last_error: Option<String>,
    syncing: bool,
}

#[tauri::command(async)]
fn get_inbox(state: State<AppState>) -> CmdResult<Inbox> {
    let syncing = state.sync_lock.try_lock().is_err();
    let conn = state.db.relock();
    Ok(Inbox {
        items: db::list_items(&conn).map_err(err)?,
        last_sync: db::get_setting(&conn, "last_sync").map_err(err)?,
        last_error: db::get_setting(&conn, "last_error").map_err(err)?,
        syncing,
    })
}

#[tauri::command]
async fn sync_now(app: AppHandle) -> CmdResult<()> {
    run_sync(&app).await
}

fn local_time(iso: &str) -> String {
    derive::parse_ts(iso)
        .map(|t| t.with_timezone(&chrono::Local).format("%a %-d %b, %H:%M").to_string())
        .unwrap_or_else(|| iso.to_string())
}

/// Set (or clear, with `state: None`) the user's personal state for one or more PRs.
#[tauri::command(async)]
fn set_state(
    app: AppHandle,
    pr_ids: Vec<String>,
    state: Option<String>,
    until: Option<String>,
    note: Option<String>,
) -> CmdResult<()> {
    {
        let st = app.state::<AppState>();
        let conn = st.db.relock();
        let at = now();
        let note = note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
        // Normalise so snooze times compare correctly against our own timestamps.
        let until = until.as_deref().and_then(derive::parse_ts).map(derive::fmt_ts);
        let tx = conn.unchecked_transaction().map_err(err)?;
        let titles = db::open_titles(&tx).map_err(err)?;
        for pr_id in &pr_ids {
            let Some(pr) = db::PrRef::load(&tx, pr_id).map_err(err)? else { continue };
            // "after #392" or "after the API migration" (matched to a PR title) → come back when it merges.
            let waits_on = (state.as_deref() == Some(derive::LATER))
                .then(|| note.as_deref())
                .flatten()
                .and_then(|n| {
                    intent::parse_refs(n, &pr.repo).into_iter().next().or_else(|| {
                        intent::condition_of(n).and_then(|c| intent::match_condition(&c, &titles, pr_id))
                    })
                });
            let detail = match state.as_deref() {
                None => {
                    db::clear_personal(&tx, pr_id).map_err(err)?;
                    "Cleared personal state".to_string()
                }
                Some(s) => {
                    db::set_personal(
                        &tx,
                        pr_id,
                        &db::Personal {
                            state: s.to_string(),
                            note: note.clone(),
                            until: until.clone(),
                            set_at: at.clone(),
                            waits_on: waits_on.clone(),
                        },
                    )
                    .map_err(err)?;
                    let mut d = format!("Marked “{}”", sync::state_label(s));
                    if let Some(u) = &until {
                        d.push_str(&format!(" until {}", local_time(u)));
                    }
                    if let Some(w) = &waits_on {
                        d.push_str(&format!(" until {w} merges or closes"));
                    }
                    if let Some(n) = &note {
                        d.push_str(&format!(" — {n}"));
                    }
                    d
                }
            };
            db::mark_seen(&tx, pr_id, &at).map_err(err)?;
            db::record_activity(
                &tx,
                &db::ActivityRow { key: format!("marked:{pr_id}:{at}"), at: &at, kind: "marked", pr: Some(&pr), detail },
            )
            .map_err(err)?;
        }
        tx.commit().map_err(err)?;
    }
    refresh_count(&app);
    Ok(())
}

/// Sends a sample notification; clicking it opens GitHub notifications (checks permissions work).
#[tauri::command(async)]
fn test_notification(app: AppHandle) -> String {
    notify(
        &app,
        sync::Notice {
            title: "🔔 Dev Inbox notifications work".into(),
            body: "Click me\nThis opens Dev Inbox, the way a real one opens its PR or ticket".into(),
            url: Some("https://github.com/notifications".into()),
            item_id: None,
        },
    );
    #[cfg(target_os = "macos")]
    return match notifications::route() {
        notifications::Route::App => "Sent — it should appear as Dev Inbox.".into(),
        notifications::Route::Denied => {
            "Notifications are turned off for Dev Inbox: System Settings → Notifications → Dev Inbox → Allow notifications.".into()
        }
        notifications::Route::Dev => {
            "Sent the development way (shows as Script Editor; clicking it won't open the item). The built app sends them as Dev Inbox.".into()
        }
    };
    #[cfg(not(target_os = "macos"))]
    "Sent.".into()
}

/// A notification was clicked: bring Dev Inbox forward and open the item in it (a PR or issue in
/// Today with its details, a ticket in Tickets), marking it seen. "Open on GitHub" is one click away
/// from there. Notifications without an item (a burst summary, the morning digest) open Today.
fn notification_clicked(app: &AppHandle, _url: Option<String>, item: Option<String>) {
    show_main(app);
    match item.as_deref() {
        Some(id) if id.starts_with("jira:") => {
            let key = &id["jira:".len()..];
            seen_ticket(app, key);
            let _ = app.emit_to("main", "open-ticket", key);
        }
        Some(id) => {
            let state = app.state::<AppState>();
            let _ = db::mark_seen(&state.db.relock(), id, &now());
            let _ = app.emit("inbox-updated", ());
            let _ = app.emit_to("main", "open-item", id);
        }
        None => {
            let _ = app.emit_to("main", "navigate", "today");
        }
    }
}

#[tauri::command(async)]
fn get_following(state: State<AppState>) -> CmdResult<Vec<db::FollowItem>> {
    db::list_following(&state.db.relock()).map_err(err)
}

/// Follow or stop following items. Unfollowed items vanish from Today, counts and notifications.
#[tauri::command(async)]
fn set_followed(app: AppHandle, ids: Vec<String>, followed: bool) -> CmdResult<()> {
    {
        let st = app.state::<AppState>();
        let conn = st.db.relock();
        let at = now();
        let tx = conn.unchecked_transaction().map_err(err)?;
        for id in &ids {
            db::set_followed(&tx, id, followed, &at).map_err(err)?;
            // Keep the timeline readable when (un)following in bulk.
            if ids.len() <= 5 {
                if let Some(r) = db::PrRef::load(&tx, id).map_err(err)? {
                    let detail = if followed { "Following again" } else { "Stopped following" };
                    db::record_activity(
                        &tx,
                        &db::ActivityRow { key: format!("follow:{id}:{at}"), at: &at, kind: "marked", pr: Some(&r), detail: detail.into() },
                    )
                    .map_err(err)?;
                }
            }
        }
        tx.commit().map_err(err)?;
    }
    refresh_count(&app);
    Ok(())
}

/// Record that the user has looked at these PRs (clears their "new" dot).
#[tauri::command(async)]
fn mark_seen(app: AppHandle, pr_ids: Vec<String>) -> CmdResult<()> {
    {
        let st = app.state::<AppState>();
        let conn = st.db.relock();
        let at = now();
        for id in &pr_ids {
            db::mark_seen(&conn, id, &at).map_err(err)?;
        }
    }
    let _ = app.emit("inbox-updated", ());
    Ok(())
}

fn client(app: &AppHandle) -> CmdResult<github::Client> {
    let state = app.state::<AppState>();
    if state.demo {
        return Err("Not available in demo mode".into());
    }
    Ok(github::Client::new(&state.token().ok_or("Not signed in")?))
}

/// Approve, request changes, or leave a review comment. These act on GitHub as the user.
#[tauri::command]
async fn submit_review(app: AppHandle, pr_id: String, event: String, body: Option<String>) -> CmdResult<String> {
    if !matches!(event.as_str(), "APPROVE" | "REQUEST_CHANGES" | "COMMENT") {
        return Err(format!("Unknown review action {event}"));
    }
    let body = body.map(|b| b.trim().to_string()).filter(|b| !b.is_empty());
    if event != "APPROVE" && body.is_none() {
        return Err("Write a message first".into());
    }
    client(&app)?.submit_review(&pr_id, &event, body.as_deref()).await.map_err(|e| format!("{e:#}"))?;
    app.state::<AppState>().wake.notify_one();
    Ok(match event.as_str() {
        "APPROVE" => "Approved",
        "REQUEST_CHANGES" => "Requested changes",
        _ => "Review comment posted",
    }
    .into())
}

/// Comment on a PR/issue, or reply inside a review thread when `thread_id` is given.
#[tauri::command]
async fn send_comment(app: AppHandle, item_id: String, body: String, thread_id: Option<String>) -> CmdResult<String> {
    let body = body.trim();
    if body.is_empty() {
        return Err("Write a message first".into());
    }
    let c = client(&app)?;
    match thread_id.filter(|t| !t.is_empty()) {
        Some(t) => c.reply_in_thread(&t, body).await,
        None => c.add_comment(&item_id, body).await,
    }
    .map_err(|e| format!("{e:#}"))?;
    let state = app.state::<AppState>();
    let _ = db::mark_seen(&state.db.relock(), &item_id, &now());
    state.wake.notify_one();
    Ok("Reply posted".into())
}

#[tauri::command]
async fn merge_options(app: AppHandle, pr_id: String) -> CmdResult<github::MergeOptions> {
    client(&app)?.merge_options(&pr_id).await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn merge_pr(app: AppHandle, pr_id: String, method: String) -> CmdResult<String> {
    if !matches!(method.as_str(), "MERGE" | "SQUASH" | "REBASE") {
        return Err(format!("Unknown merge method {method}"));
    }
    client(&app)?.merge(&pr_id, &method).await.map_err(|e| format!("{e:#}"))?;
    app.state::<AppState>().wake.notify_one();
    Ok("Merged".into())
}

// ---------- local clones & editor ----------

#[derive(Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct EditorSettings {
    /// Installed editors we can launch.
    editors: Vec<String>,
    editor: Option<String>,
    /// Folders scanned for clones.
    roots: Vec<String>,
    /// "owner/repo" → path, discovered or set by hand.
    clones: std::collections::BTreeMap<String, String>,
    /// Where "clone and open" puts new clones.
    clone_dir: String,
}

fn json_setting<T: serde::de::DeserializeOwned>(state: &AppState, key: &str) -> Option<T> {
    state.setting(key).and_then(|v| serde_json::from_str(&v).ok())
}

fn roots(state: &AppState) -> Vec<String> {
    json_setting(state, "code_roots").unwrap_or_else(editor::default_roots)
}

/// Discovered clones merged with manual overrides (overrides win).
fn clones(state: &AppState) -> std::collections::BTreeMap<String, String> {
    let mut map: std::collections::BTreeMap<String, String> = json_setting(state, "clones").unwrap_or_default();
    let overrides: std::collections::BTreeMap<String, String> = json_setting(state, "repo_paths").unwrap_or_default();
    map.extend(overrides);
    map
}

async fn scan(app: &AppHandle) -> CmdResult<usize> {
    let roots = roots(&app.state::<AppState>());
    let found = tauri::async_runtime::spawn_blocking(move || editor::discover(&roots)).await.map_err(err)?;
    let n = found.len();
    let state = app.state::<AppState>();
    db::set_setting(&state.db.relock(), "clones", &serde_json::to_string(&found).map_err(err)?).map_err(err)?;
    Ok(n)
}

#[tauri::command]
async fn editor_settings(app: AppHandle) -> CmdResult<EditorSettings> {
    if app.state::<AppState>().setting("clones").is_none() {
        scan(&app).await?;
    }
    let state = app.state::<AppState>();
    let editors = editor::installed_editors();
    let editor = state.setting("editor").or_else(|| editors.first().cloned());
    Ok(EditorSettings {
        editors,
        editor,
        roots: roots(&state),
        clones: clones(&state),
        clone_dir: state.setting("clone_dir").unwrap_or_else(editor::default_clone_dir),
    })
}

#[tauri::command]
async fn update_editor_settings(
    app: AppHandle,
    editor: Option<String>,
    roots: Vec<String>,
    clone_dir: Option<String>,
) -> CmdResult<usize> {
    {
        let state = app.state::<AppState>();
        let conn = state.db.relock();
        if let Some(dir) = clone_dir.map(|d| d.trim().to_string()).filter(|d| !d.is_empty()) {
            db::set_setting(&conn, "clone_dir", &dir).map_err(err)?;
        }
        match editor {
            Some(e) => db::set_setting(&conn, "editor", &e).map_err(err)?,
            None => db::delete_setting(&conn, "editor").map_err(err)?,
        }
        let roots: Vec<String> = roots.into_iter().map(|r| r.trim().to_string()).filter(|r| !r.is_empty()).collect();
        db::set_setting(&conn, "code_roots", &serde_json::to_string(&roots).map_err(err)?).map_err(err)?;
    }
    scan(&app).await
}

/// Point a repo at a specific local folder (or clear the override with `None`).
#[tauri::command(async)]
fn set_repo_path(state: State<AppState>, repo: String, path: Option<String>) -> CmdResult<()> {
    let mut overrides: std::collections::BTreeMap<String, String> = json_setting(&state, "repo_paths").unwrap_or_default();
    match path.map(|p| p.trim().to_string()).filter(|p| !p.is_empty()) {
        Some(p) => {
            if !std::path::Path::new(&p).join(".git").exists() {
                return Err(format!("{p} isn't a git repository"));
            }
            overrides.insert(repo.to_lowercase(), p);
        }
        None => {
            overrides.remove(&repo.to_lowercase());
        }
    }
    db::set_setting(&state.db.relock(), "repo_paths", &serde_json::to_string(&overrides).map_err(err)?).map_err(err)
}

/// Open an item's repo in the editor, optionally switching to the PR's branch first.
#[tauri::command]
async fn open_locally(app: AppHandle, item_id: String, checkout: bool) -> CmdResult<String> {
    if app.state::<AppState>().setting("clones").is_none() {
        scan(&app).await?;
    }
    let state = app.state::<AppState>();
    let (kind, repo, number, branch, cross_repo) =
        db::checkout_info(&state.db.relock(), &item_id).map_err(err)?.ok_or("Unknown item")?;
    let path = clones(&state)
        .get(&repo.to_lowercase())
        .cloned()
        // The UI recognises this prefix and offers to clone.
        .ok_or_else(|| format!("NO_CLONE:{repo}"))?;
    let editor = state
        .setting("editor")
        .or_else(|| editor::installed_editors().into_iter().next())
        .ok_or("No editor found. Pick one in Settings → Code & editor.")?;
    let mut msg = format!("Opened {repo} in {editor}");
    if checkout && kind == "pr" {
        let p = path.clone();
        let result = tauri::async_runtime::spawn_blocking(move || editor::checkout_pr(&p, number, branch.as_deref(), cross_repo))
            .await
            .map_err(err)?;
        match result {
            Ok(b) => msg = format!("Checked out {b} and opened {repo} in {editor}"),
            // Still open the editor; just say why the branch didn't change.
            Err(e) => msg = format!("Opened {repo} in {editor} — {e:#}"),
        }
    }
    editor::open_in(&editor, &path).map_err(|e| format!("{e:#}"))?;
    let _ = db::mark_seen(&state.db.relock(), &item_id, &now());
    Ok(msg)
}

/// Clone a repo into the clone folder (default ~/code), remember it, then open the item.
#[tauri::command]
async fn clone_and_open(app: AppHandle, item_id: String, checkout: bool) -> CmdResult<String> {
    let (repo, parent) = {
        let state = app.state::<AppState>();
        let repo = db::checkout_info(&state.db.relock(), &item_id).map_err(err)?.ok_or("Unknown item")?.1;
        (repo, state.setting("clone_dir").unwrap_or_else(editor::default_clone_dir))
    };
    let r = repo.clone();
    let path = tauri::async_runtime::spawn_blocking(move || editor::clone_repo(&r, &parent))
        .await
        .map_err(err)?
        .map_err(|e| format!("{e:#}"))?;
    {
        let state = app.state::<AppState>();
        let mut found: std::collections::BTreeMap<String, String> = json_setting(&state, "clones").unwrap_or_default();
        found.insert(repo.to_lowercase(), path.clone());
        db::set_setting(&state.db.relock(), "clones", &serde_json::to_string(&found).map_err(err)?).map_err(err)?;
    }
    let msg = open_locally(app, item_id, checkout).await?;
    Ok(format!("Cloned to {path} · {msg}"))
}

/// Re-run failed GitHub Actions on a PR. Returns a human-readable summary.
#[tauri::command]
async fn rerun_checks(app: AppHandle, pr_id: String) -> CmdResult<String> {
    let state = app.state::<AppState>();
    if state.demo {
        return Err("Not available in demo mode".into());
    }
    let token = state.token().ok_or("Not signed in")?;
    let result = github::Client::new(&token).rerun_failed_checks(&pr_id).await.map_err(|e| format!("{e:#}"))?;
    // Load first so the lock is released before record_activity takes it again: a guard
    // created in an `if let` condition lives for the whole block, and re-locking deadlocks.
    let pr = db::PrRef::load(&state.db.relock(), &pr_id).map_err(err)?;
    if let Some(pr) = pr {
        let at = now();
        let _ = db::record_activity(
            &state.db.relock(),
            &db::ActivityRow {
                key: format!("rerun:{pr_id}:{at}"),
                at: &at,
                kind: "marked",
                pr: Some(&pr),
                detail: format!("Re-ran {} failed workflow{}", result.rerun, if result.rerun == 1 { "" } else { "s" }),
            },
        );
    }
    state.wake.notify_one();
    let mut msg = match result.rerun {
        0 => "No failed GitHub Actions workflows to re-run".to_string(),
        1 => "Re-running 1 failed workflow".to_string(),
        n => format!("Re-running {n} failed workflows"),
    };
    if !result.external.is_empty() {
        msg.push_str(&format!(" · re-run {} from its own dashboard", result.external.join(", ")));
    }
    Ok(msg)
}

#[tauri::command(async)]
fn get_activity(state: State<AppState>, days: i64) -> CmdResult<Vec<db::Activity>> {
    let since = derive::fmt_ts(chrono::Utc::now() - chrono::Duration::days(days));
    db::list_activity(&state.db.relock(), &since).map_err(err)
}

#[tauri::command(async)]
fn get_item_activity(state: State<AppState>, item_id: String) -> CmdResult<Vec<db::Activity>> {
    db::item_activity(&state.db.relock(), &item_id, 30).map_err(err)
}

#[tauri::command(async)]
fn get_repos(state: State<AppState>) -> CmdResult<Vec<db::RepoInfo>> {
    db::list_repos(&state.db.relock()).map_err(err)
}

#[tauri::command(async)]
fn set_repo_hidden(app: AppHandle, repo: String, hidden: bool) -> CmdResult<()> {
    db::set_repo_hidden(&app.state::<AppState>().db.relock(), &repo, hidden).map_err(err)?;
    refresh_count(&app);
    Ok(())
}

#[derive(Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    poll_seconds: u64,
    notifications: bool,
    /// Logins whose activity never counts (machine users, noisy bots).
    ignored_users: Vec<String>,
    /// A once-a-day "here's what needs you" notification.
    digest: bool,
    digest_hour: u32,
    /// Sample local clones for editing activity (feeds the Day view). Stays on this machine.
    #[serde(default = "yes")]
    track_local: bool,
    /// Days to keep meeting audio: 0 forever, -1 delete once transcribed.
    #[serde(default = "thirty")]
    audio_retention_days: i64,
    /// Who writes meeting summaries: apple (on this Mac) | claude | off.
    #[serde(default = "apple")]
    summary_provider: String,
    /// Which Claude model, when it's Claude.
    #[serde(default = "haiku")]
    claude_model: String,
    /// The other AI features (Ask, day and week summaries, suggestions, CI explanations, reminders):
    /// apple | claude | off, and the Claude model for them.
    #[serde(default = "apple")]
    ai_provider: String,
    #[serde(default = "haiku")]
    ai_model: String,
    /// Show what's being said while recording.
    #[serde(default = "yes")]
    live_captions: bool,
    /// Tell the voices on the call apart (Speaker 1, 2…) after recording, on this Mac.
    #[serde(default = "yes")]
    separate_speakers: bool,
    /// Remember voices you name, and name them automatically in later meetings.
    #[serde(default = "yes")]
    recognise_voices: bool,
    /// Show the number of things that need you next to the menu bar icon (otherwise just a dot).
    #[serde(default)]
    menu_bar_count: bool,
}

fn apple() -> String {
    "apple".into()
}

fn haiku() -> String {
    summary::DEFAULT_CLAUDE_MODEL.into()
}

fn thirty() -> i64 {
    30
}

fn yes() -> bool {
    true
}

fn ignored_users(state: &AppState) -> Vec<String> {
    state
        .setting("ignored_users")
        .unwrap_or_default()
        .split(|c: char| c == ',' || c.is_whitespace())
        .map(|s| s.trim().trim_start_matches('@').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn digest_hour(state: &AppState) -> u32 {
    state.setting("digest_hour").and_then(|h| h.parse().ok()).unwrap_or(9).min(23)
}

#[tauri::command(async)]
fn get_settings(state: State<AppState>) -> Settings {
    Settings {
        poll_seconds: poll_secs(&state),
        notifications: state.setting("notifications").as_deref() != Some("0"),
        ignored_users: ignored_users(&state),
        digest: state.setting("digest").as_deref() != Some("0"),
        digest_hour: digest_hour(&state),
        track_local: state.setting("track_local").as_deref() != Some("0"),
        audio_retention_days: audio_retention_days(&state),
        summary_provider: chosen_provider(&state).id().into(),
        claude_model: claude_model(&state).into(),
        ai_provider: chosen(&state, "ai_provider").id().into(),
        ai_model: summary::claude_model(state.setting("ai_model").as_deref()).into(),
        live_captions: state.setting("live_captions").as_deref() != Some("0"),
        separate_speakers: state.setting("separate_speakers").as_deref() != Some("0"),
        recognise_voices: state.setting("recognise_voices").as_deref() != Some("0"),
        menu_bar_count: state.setting("menu_bar_count").as_deref() == Some("1"),
    }
}

#[tauri::command(async)]
fn update_settings(app: AppHandle, settings: Settings) -> CmdResult<()> {
    let state = app.state::<AppState>();
    {
        let conn = state.db.relock();
        db::set_setting(&conn, "poll_seconds", &settings.poll_seconds.max(30).to_string()).map_err(err)?;
        db::set_setting(&conn, "notifications", if settings.notifications { "1" } else { "0" }).map_err(err)?;
        db::set_setting(&conn, "digest", if settings.digest { "1" } else { "0" }).map_err(err)?;
        db::set_setting(&conn, "digest_hour", &settings.digest_hour.min(23).to_string()).map_err(err)?;
        db::set_setting(&conn, "track_local", if settings.track_local { "1" } else { "0" }).map_err(err)?;
        db::set_setting(&conn, "audio_retention_days", &settings.audio_retention_days.to_string()).map_err(err)?;
        let provider = summary::Provider::from_setting(Some(&settings.summary_provider));
        db::set_setting(&conn, "summary_provider", provider.id()).map_err(err)?;
        db::set_setting(&conn, "claude_model", summary::claude_model(Some(&settings.claude_model))).map_err(err)?;
        let ai = summary::Provider::from_setting(Some(&settings.ai_provider));
        db::set_setting(&conn, "ai_provider", ai.id()).map_err(err)?;
        db::set_setting(&conn, "ai_model", summary::claude_model(Some(&settings.ai_model))).map_err(err)?;
        db::set_setting(&conn, "live_captions", if settings.live_captions { "1" } else { "0" }).map_err(err)?;
        db::set_setting(&conn, "separate_speakers", if settings.separate_speakers { "1" } else { "0" }).map_err(err)?;
        db::set_setting(&conn, "recognise_voices", if settings.recognise_voices { "1" } else { "0" }).map_err(err)?;
        db::set_setting(&conn, "menu_bar_count", if settings.menu_bar_count { "1" } else { "0" }).map_err(err)?;
        let ignored = settings.ignored_users.join(",");
        if db::get_setting(&conn, "ignored_users").map_err(err)?.unwrap_or_default() != ignored {
            db::set_setting(&conn, "ignored_users", &ignored).map_err(err)?;
            // Every PR's state may change: force the next sync to re-derive everything.
            db::delete_setting(&conn, "last_full_sync").map_err(err)?;
        }
    }
    state.wake.notify_one();
    refresh_tray(&app);
    Ok(())
}

/// Send the daily digest once, after the configured hour, on weekdays.
fn maybe_digest(app: &AppHandle) {
    use chrono::{Datelike, Timelike};
    let state = app.state::<AppState>();
    if state.demo || state.setting("digest").as_deref() == Some("0") || state.setting("notifications").as_deref() == Some("0") {
        return;
    }
    let now = chrono::Local::now();
    let today = now.format("%Y-%m-%d").to_string();
    let weekend = matches!(now.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun);
    if weekend || now.hour() < digest_hour(&state) || state.setting("last_digest").as_deref() == Some(today.as_str()) {
        return;
    }
    let conn = state.db.relock();
    let _ = db::set_setting(&conn, "last_digest", &today);
    if let Ok(Some(summary)) = sync::digest(&conn) {
        drop(conn);
        notify(app, sync::Notice { title: "☀️ Today in Dev Inbox".into(), body: summary, url: None, item_id: None });
    }
}

// ---------- Jira (read-only) ----------

const JIRA_KEYRING_USER: &str = "jira-token";

/// Jira, for changing a ticket; an error if it isn't connected (or this is the demo).
fn jira_for_edit(state: &AppState) -> CmdResult<jira::Client> {
    if state.demo {
        return Err("Jira isn't connected in demo mode".into());
    }
    jira_client(state).ok_or_else(|| "Connect Jira in Settings first".to_string())
}

/// After changing a ticket: fetch it again so the app shows the new status, assignee and history.
async fn refresh_ticket(app: &AppHandle, client: &jira::Client, key: &str) {
    let me = app.state::<AppState>().setting("jira_account").unwrap_or_default();
    if let Ok((issues, mut changes)) = client.issue_with_history(key).await {
        for i in &issues {
            changes.extend(jira::issue_changes(i, &me));
        }
        let _ = jira::store(&app.state::<AppState>().db.relock(), client, &me, &issues, &changes);
    }
    refresh_tray(app);
    let _ = app.emit("jira-updated", ());
}

/// The statuses a ticket can move to from where it is.
#[tauri::command]
async fn jira_transitions(app: AppHandle, key: String) -> CmdResult<Vec<jira::Transition>> {
    let client = jira_for_edit(&app.state::<AppState>())?;
    client.transitions(&key).await.map_err(|e| format!("{e:#}"))
}

/// Move a ticket to another status (in Jira).
#[tauri::command]
async fn jira_move(app: AppHandle, key: String, transition_id: String) -> CmdResult<()> {
    let client = jira_for_edit(&app.state::<AppState>())?;
    client.transition(&key, &transition_id).await.map_err(|e| format!("{e:#}"))?;
    refresh_ticket(&app, &client, &key).await;
    Ok(())
}

/// People a ticket can be assigned to, matching what's typed.
#[tauri::command]
async fn jira_assignable(app: AppHandle, key: String, query: String) -> CmdResult<Vec<jira::JiraUser>> {
    let client = jira_for_edit(&app.state::<AppState>())?;
    client.assignable(&key, query.trim()).await.map_err(|e| format!("{e:#}"))
}

/// Assign a ticket (in Jira); None unassigns it.
#[tauri::command]
async fn jira_assign(app: AppHandle, key: String, account_id: Option<String>) -> CmdResult<()> {
    let client = jira_for_edit(&app.state::<AppState>())?;
    // "me": the connected Jira account.
    let account_id = match account_id.as_deref() {
        Some("me") => Some(app.state::<AppState>().setting("jira_account").ok_or("Connect Jira in Settings first")?),
        _ => account_id,
    };
    client.assign(&key, account_id.as_deref()).await.map_err(|e| format!("{e:#}"))?;
    refresh_ticket(&app, &client, &key).await;
    Ok(())
}

fn jira_client(state: &AppState) -> Option<jira::Client> {
    let site = state.setting("jira_site")?;
    let email = state.setting("jira_email")?;
    let token = {
        let mut cached = state.jira_token.relock();
        if cached.is_none() {
            *cached = keyring::Entry::new(KEYRING_SERVICE, JIRA_KEYRING_USER).ok().and_then(|e| e.get_password().ok());
        }
        cached.clone()?
    };
    jira::Client::new(&site, &email, &token).ok()
}

/// Fetch tickets assigned to the user (now or before) that changed since the last sync,
/// with their movement history.
/// What a Jira sync fetches; bump to re-fetch the first-sync window after widening it
/// (tickets that involve you, then descriptions).
const JIRA_SCOPE: &str = "involved+description";

async fn run_jira_sync(app: &AppHandle) -> CmdResult<()> {
    let state = app.state::<AppState>();
    if state.demo {
        return Ok(());
    }
    let Some(client) = jira_client(&state) else { return Ok(()) };
    let Some(me) = state.setting("jira_account") else { return Ok(()) };
    let _guard = state.jira_lock.lock().await;
    let now = chrono::Utc::now();
    // Overlap a day so nothing slips between syncs; first sync looks back 30 days. Syncs from before
    // tickets you reported or worked on (not just assigned ones) were included look back 30 days once more.
    let rescan = state.setting("jira_scope").as_deref() != Some(JIRA_SCOPE);
    let since = state
        .setting("jira_last_sync")
        .filter(|_| !rescan)
        .and_then(|s| derive::parse_ts(&s))
        .map(|t| t - chrono::Duration::days(1))
        .unwrap_or(now - chrono::Duration::days(jira::FIRST_SYNC_DAYS));
    let result = async {
        let issues = client.involved_issues(since, &me).await?;
        let mut changes = client.changelogs(&issues).await?;
        for i in &issues {
            changes.extend(jira::issue_changes(i, &me));
        }
        jira::store(&state.db.relock(), &client, &me, &issues, &changes)?;
        anyhow::Ok(issues.len())
    }
    .await;
    let conn = state.db.relock();
    match result {
        Ok(_) => {
            let _ = db::set_setting(&conn, "jira_last_sync", &derive::fmt_ts(now));
            let _ = db::set_setting(&conn, "jira_scope", JIRA_SCOPE);
            let _ = db::delete_setting(&conn, "jira_error");
            // The first sync brings in 30 days of history: only the last day can need you, and none of it notifies.
            let since = match db::get_setting(&conn, "jira_attention_since") {
                Ok(Some(s)) => s,
                _ => {
                    let s = derive::fmt_ts(now - chrono::Duration::days(1));
                    let _ = db::set_setting(&conn, "jira_attention_since", &s);
                    let _ = jira::mark_all_notified(&conn);
                    s
                }
            };
            let fresh = jira::take_notifications(&conn, &me, &since).unwrap_or_default();
            let notifications = db::get_setting(&conn, "notifications").ok().flatten().as_deref() != Some("0");
            drop(conn);
            if notifications {
                notify_tickets(app, fresh);
            }
            refresh_count(app);
            let _ = app.emit("jira-updated", ());
            Ok(())
        }
        Err(e) => {
            let msg = format!("{e:#}");
            let _ = db::set_setting(&conn, "jira_error", &msg);
            Err(msg)
        }
    }
}

/// Tickets that need you (empty if Jira isn't connected). Reads settings before locking the database.
fn ticket_attention(state: &AppState) -> Vec<jira::Attention> {
    let (Some(me), Some(since)) = (state.setting("jira_account"), state.setting("jira_attention_since")) else {
        return Vec::new();
    };
    jira::attention(&state.db.relock(), &me, &since).unwrap_or_default()
}

/// Desktop notifications for tickets that newly need you; a burst becomes one summary.
fn notify_tickets(app: &AppHandle, fresh: Vec<jira::Attention>) {
    if fresh.len() > 3 {
        notify(
            app,
            sync::Notice {
                title: "🎫 Jira".into(),
                body: format!("{} ticket updates need you\n{}", fresh.len(), fresh.iter().map(|a| a.key.as_str()).collect::<Vec<_>>().join(", ")),
                url: None,
                item_id: None,
            },
        );
        return;
    }
    for a in fresh {
        let title = match a.kind.as_str() {
            "assignee" => "🎫 Assigned to you",
            "mention" => "💬 You were mentioned",
            "comment" => "💬 New comment",
            _ => "↪ Ticket moved",
        };
        notify(
            app,
            sync::Notice {
                title: title.into(),
                body: format!("{} {}\n{}", a.key, a.summary, a.reason),
                url: Some(a.url.clone()),
                item_id: Some(format!("jira:{}", a.key)),
            },
        );
    }
}

/// You've looked at a ticket: it leaves Today.
fn seen_ticket(app: &AppHandle, key: &str) {
    let state = app.state::<AppState>();
    let (Some(me), Some(since)) = (state.setting("jira_account"), state.setting("jira_attention_since")) else { return };
    let needed = jira::mark_seen(&state.db.relock(), key, &me, &since, &now()).unwrap_or(false);
    if needed {
        refresh_count(app);
        let _ = app.emit("jira-updated", ());
    }
}

/// Tickets that need you, for Today.
#[tauri::command(async)]
fn get_ticket_attention(state: State<AppState>) -> Vec<jira::Attention> {
    ticket_attention(&state)
}

#[tauri::command(async)]
fn mark_ticket_seen(app: AppHandle, key: String) {
    seen_ticket(&app, &key);
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JiraStatus {
    connected: bool,
    site: Option<String>,
    email: Option<String>,
    name: Option<String>,
    last_sync: Option<String>,
    error: Option<String>,
    tickets: usize,
}

fn jira_status_of(state: &AppState) -> JiraStatus {
    let tickets = state
        .db
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM jira_issues", [], |r| r.get::<_, i64>(0))
        .unwrap_or(0) as usize;
    JiraStatus {
        connected: state.setting("jira_account").is_some(),
        site: state.setting("jira_site"),
        email: state.setting("jira_email"),
        name: state.setting("jira_name"),
        last_sync: state.setting("jira_last_sync"),
        error: state.setting("jira_error"),
        tickets,
    }
}

#[tauri::command(async)]
fn jira_status(state: State<AppState>) -> JiraStatus {
    jira_status_of(&state)
}

/// Check the credentials against Jira, store them (token in the keychain), and start a first sync.
#[tauri::command]
async fn jira_connect(app: AppHandle, site: String, email: String, token: String) -> CmdResult<JiraStatus> {
    let client = jira::Client::new(&site, &email, &token).map_err(|e| format!("{e:#}"))?;
    let me = client.myself().await.map_err(|e| format!("{e:#}"))?;
    keyring::Entry::new(KEYRING_SERVICE, JIRA_KEYRING_USER)
        .and_then(|e| e.set_password(token.trim()))
        .map_err(|e| format!("Couldn't save the token to the keychain: {e}"))?;
    let state = app.state::<AppState>();
    {
        let conn = state.db.relock();
        // A different Jira account's history must not mix with this one.
        if db::get_setting(&conn, "jira_account").map_err(err)?.is_some_and(|a| a != me.account_id) {
            jira::forget(&conn).map_err(err)?;
        }
        db::set_setting(&conn, "jira_site", &jira::normalise_site(&site).map_err(err)?).map_err(err)?;
        db::set_setting(&conn, "jira_email", email.trim()).map_err(err)?;
        db::set_setting(&conn, "jira_account", &me.account_id).map_err(err)?;
        db::set_setting(&conn, "jira_name", &me.name).map_err(err)?;
    }
    *state.jira_token.relock() = Some(token.trim().to_string());
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = run_jira_sync(&handle).await;
    });
    Ok(jira_status_of(&state))
}

#[tauri::command(async)]
fn jira_disconnect(state: State<AppState>) -> CmdResult<()> {
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, JIRA_KEYRING_USER) {
        let _ = entry.delete_credential();
    }
    *state.jira_token.relock() = None;
    jira::forget(&state.db.relock()).map_err(err)
}

#[tauri::command]
async fn jira_sync_now(app: AppHandle) -> CmdResult<()> {
    run_jira_sync(&app).await
}

#[tauri::command(async)]
fn get_tickets(state: State<AppState>) -> CmdResult<Vec<jira::Ticket>> {
    let me = state.setting("jira_account").unwrap_or_default();
    jira::list_tickets(&state.db.relock(), &me).map_err(err)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PrLink {
    id: String,
    repo: String,
    number: i64,
    title: String,
    url: String,
    open: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TicketDetail {
    description: Option<String>,
    events: Vec<jira::TicketEvent>,
    prs: Vec<PrLink>,
    /// Meetings linked to this ticket in the app, newest first, and their action items.
    meetings: Vec<meetings::Meeting>,
    actions: Vec<summary::Action>,
    /// Recent meetings with no ticket, to link one.
    linkable: Vec<meetings::Meeting>,
}

/// A ticket's movement history, and the PRs that mention it (by branch or title).
#[tauri::command(async)]
fn get_ticket(state: State<AppState>, key: String) -> CmdResult<TicketDetail> {
    let me = state.setting("jira_account").unwrap_or_default();
    let conn = state.db.relock();
    let events = jira::ticket_events(&conn, &key, &me).map_err(err)?;
    let description = jira::description(&conn, &key).map_err(err)?;
    let like = format!("%{}%", key.to_lowercase());
    let mut stmt = conn
        .prepare(
            "SELECT id, repo, number, title, url, is_open, branch FROM prs
             WHERE kind = 'pr' AND (lower(COALESCE(branch, '')) LIKE ?1 OR lower(title) LIKE ?1)
             ORDER BY updated_at DESC",
        )
        .map_err(err)?;
    let prs = stmt
        .query_map([like], |r| {
            Ok((
                PrLink { id: r.get(0)?, repo: r.get(1)?, number: r.get(2)?, title: r.get(3)?, url: r.get(4)?, open: r.get(5)? },
                r.get::<_, Option<String>>(6)?,
            ))
        })
        .map_err(err)?
        .filter_map(Result::ok)
        // "IO-26" must not match "IO-2641": require the exact key.
        .filter(|(p, branch)| {
            branch.as_deref().and_then(day::ticket_in_branch).as_deref() == Some(key.as_str())
                || day::ticket_in_text(&p.title).as_deref() == Some(key.as_str())
        })
        .map(|(p, _)| p)
        .collect();
    let linked = meetings::for_ticket(&conn, &key).map_err(err)?;
    let mut actions = Vec::new();
    for m in &linked {
        actions.extend(summary::meeting_actions(&conn, &m.id).map_err(err)?);
    }
    let fortnight = derive::fmt_ts(chrono::Utc::now() - chrono::Duration::days(14));
    let linkable = meetings::recent_unlinked(&conn, &fortnight).map_err(err)?;
    Ok(TicketDetail { description, events, prs, meetings: linked, actions, linkable })
}

/// Link a meeting to a ticket in this app (nothing is written to Jira); None unlinks it.
#[tauri::command(async)]
fn link_meeting(app: AppHandle, id: String, ticket: Option<String>) -> CmdResult<()> {
    meetings::set_ticket(&app.state::<AppState>().db.relock(), &id, ticket.as_deref()).map_err(err)?;
    let _ = app.emit("meetings-updated", ());
    Ok(())
}

// ---------- Meetings ----------

/// Days to keep meeting audio (transcripts are always kept). 0 = forever, -1 = delete once transcribed.
fn audio_retention_days(state: &AppState) -> i64 {
    state.setting("audio_retention_days").and_then(|v| v.parse().ok()).unwrap_or(30)
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RecordingStatus {
    recording: bool,
    meeting_id: Option<String>,
    started_at: Option<String>,
}

fn recording_status_of(state: &AppState) -> RecordingStatus {
    let id = state.recorder.relock().as_ref().map(|r| r.meeting_id.clone());
    let started_at = id.as_ref().and_then(|id| {
        state
            .db
            .lock()
            .unwrap()
            .query_row("SELECT started_at FROM meetings WHERE id = ?1", [id], |r| r.get(0))
            .ok()
    });
    RecordingStatus { recording: id.is_some(), meeting_id: id, started_at }
}

#[tauri::command(async)]
fn recording_status(state: State<AppState>) -> RecordingStatus {
    recording_status_of(&state)
}

// ---------- background jobs ----------

/// Run background work on meetings (transcribing, summarising) safely: on its own thread, never the UI
/// thread; heavy jobs one at a time (they share the CPU, the speech model and Apple Intelligence); a
/// job that's already queued or running isn't started twice; and one that panics marks its meeting
/// failed (via `on_panic`) instead of leaving it spinning or taking the app down.
fn spawn_job(
    app: &AppHandle,
    key: String,
    on_panic: impl FnOnce(&AppHandle) + Send + 'static,
    job: impl FnOnce(&AppHandle) + Send + 'static,
) {
    let state = app.state::<AppState>();
    if !state.jobs.relock().insert(key.clone()) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let outcome = {
            let _one_at_a_time = state.heavy_job.relock();
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&app)))
        };
        state.jobs.relock().remove(&key);
        if outcome.is_err() {
            on_panic(&app);
            let _ = app.emit("meetings-updated", ());
        }
    });
}

/// Transcribe both tracks on-device, merge them, store; apply "delete audio after transcript"; then
/// write notes. Runs as a background job.
fn transcribe_meeting(app: AppHandle, id: String, dir: std::path::PathBuf) {
    let failed_id = id.clone();
    let on_panic = move |app: &AppHandle| {
        let state = app.state::<AppState>();
        let conn = state.db.relock();
        let _ = meetings::set_status(&conn, &failed_id, "failed", Some("Transcription stopped unexpectedly. Try again."));
    };
    spawn_job(&app, format!("transcribe:{id}"), on_panic, move |app| {
        let state = app.state::<AppState>();
        // Settings before taking the lock: `setting()` locks the database itself, and std's Mutex isn't
        // re-entrant, so calling it while holding `conn` deadlocks.
        let locale = state.setting("transcribe_locale").unwrap_or_default();
        let delete_audio = audio_retention_days(&state) < 0;
        let separate_speakers = state.setting("separate_speakers").as_deref() != Some("0");
        let recognise = state.setting("recognise_voices").as_deref() != Some("0");
        let models = app.path().app_data_dir().map(|d| d.join("models")).ok();
        let seconds = meetings::get(&state.db.relock(), &id).ok().flatten().and_then(|(m, _)| m.seconds).unwrap_or(0);
        // Generous, but never forever: a first run may download the speech model.
        let timeout = std::time::Duration::from_secs((seconds.max(0) as u64 * 3).max(10 * 60));
        let track = |name: &str, speaker: &str| -> anyhow::Result<Vec<meetings::Segment>> {
            let path = dir.join(name);
            if path.exists() && std::fs::metadata(&path).map(|m| m.len() > 1024).unwrap_or(false) {
                meetings::transcribe_file(&path, speaker, &locale, timeout)
            } else {
                Ok(Vec::new())
            }
        };
        // Lines, plus each voice's fingerprint (voice "1", "2"… → vector) when speakers were told apart.
        type Voices = std::collections::HashMap<String, Vec<f32>>;
        let result = (|| -> anyhow::Result<(Vec<meetings::Segment>, Option<Voices>)> {
            let you = track("mic.m4a", "you")?;
            let mut others = track("system.m4a", "others")?;
            let mut voices = None;
            // Tell the voices on the call apart. Best effort: without it they're all "Others".
            if let (true, false, Some(models)) = (separate_speakers, others.is_empty(), &models) {
                match meetings::diarize_file(&dir.join("system.m4a"), models, timeout) {
                    Ok(found) => {
                        let numbering = meetings::assign_voices(&mut others, &found.turns);
                        voices = Some(
                            found
                                .voices
                                .into_iter()
                                .filter_map(|(speaker, v)| Some((numbering.get(&speaker)?.clone(), v)))
                                .collect(),
                        );
                    }
                    Err(e) => eprintln!("speaker separation skipped: {e:#}"),
                }
            }
            Ok((meetings::merge(you, others), voices))
        })();
        let transcribed = result.is_ok();
        {
            let conn = state.db.relock();
            match result {
                Ok((segments, voices)) => {
                    let _ = meetings::save_transcript(&conn, &id, &segments);
                    if let Some(voices) = voices {
                        let _ = voices::save_meeting_voices(&conn, &id, &voices);
                        if recognise {
                            let _ = voices::recognise(&conn, &id);
                            let _ = search::reindex(&conn, &id);
                        }
                    }
                    if delete_audio {
                        let _ = std::fs::remove_dir_all(&dir);
                        let _ = conn.execute("UPDATE meetings SET audio_deleted = 1 WHERE id = ?1", [&id]);
                    }
                }
                Err(e) => {
                    let _ = meetings::set_status(&conn, &id, "failed", Some(&format!("{e:#}")));
                }
            }
        }
        let _ = app.emit("meetings-updated", ());
        if transcribed {
            summarise_meeting(app.clone(), id);
        }
    });
}

/// Pick up work an earlier run didn't finish (the app quit or crashed mid-way), so nothing is left
/// spinning forever.
fn resume_meeting_jobs(app: &AppHandle) {
    let state = app.state::<AppState>();
    let stuck = meetings::unfinished(&state.db.relock()).unwrap_or_default();
    for (id, status, summary_status, dir) in stuck {
        match status.as_str() {
            "recording" => {
                let conn = state.db.relock();
                let _ = meetings::interrupted_recording(&conn, &id, dir.as_deref().map(std::path::Path::new));
            }
            "transcribing" => {
                if let Some(dir) = dir {
                    transcribe_meeting(app.clone(), id.clone(), dir.into());
                }
            }
            _ => {}
        }
        if summary_status.as_deref() == Some("summarising") && status == "done" {
            summarise_meeting(app.clone(), id);
        }
    }
    let _ = app.emit("meetings-updated", ());
}

const ANTHROPIC_KEYRING_USER: &str = "anthropic-api-key";

/// An engine chosen in Settings (`setting`: "summary_provider" for meeting notes, "ai_provider" for
/// the other AI features). Until one is chosen: Claude if an Anthropic key is saved, otherwise Apple
/// Intelligence on this Mac.
fn chosen(state: &AppState, setting: &str) -> summary::Provider {
    match state.setting(setting) {
        Some(s) => summary::Provider::from_setting(Some(&s)),
        None if anthropic_key(state).is_some() => summary::Provider::Claude,
        None => summary::Provider::Apple,
    }
}

/// The engine that will actually run: Claude needs a key, so without one it's Apple Intelligence.
fn effective(state: &AppState, setting: &str) -> summary::Provider {
    match chosen(state, setting) {
        summary::Provider::Claude if anthropic_key(state).is_none() => summary::Provider::Apple,
        p => p,
    }
}

fn chosen_provider(state: &AppState) -> summary::Provider {
    chosen(state, "summary_provider")
}

fn summary_provider(state: &AppState) -> summary::Provider {
    effective(state, "summary_provider")
}

fn claude_model(state: &AppState) -> &'static str {
    summary::claude_model(state.setting("claude_model").as_deref())
}

/// The engine for the AI features other than meeting notes (Settings → AI); None when they're off.
fn ai_engine(state: &AppState) -> Option<ai::Engine> {
    match effective(state, "ai_provider") {
        summary::Provider::Off => None,
        summary::Provider::Apple => Some(ai::Engine::Apple),
        summary::Provider::Claude => Some(ai::Engine::Claude {
            key: anthropic_key(state)?,
            model: summary::claude_model(state.setting("ai_model").as_deref()),
        }),
    }
}

fn anthropic_key(state: &AppState) -> Option<String> {
    let mut cached = state.anthropic_key.relock();
    if cached.is_none() {
        *cached = keyring::Entry::new(KEYRING_SERVICE, ANTHROPIC_KEYRING_USER).ok().and_then(|e| e.get_password().ok());
    }
    cached.clone()
}

/// Write AI notes for a meeting with whichever summariser Settings chose (none if off), as a
/// background job. Claude's request has its own timeout; the on-device helper is killed if it hangs.
fn summarise_meeting(app: AppHandle, id: String) {
    let state = app.state::<AppState>();
    if summary_provider(&state) == summary::Provider::Off {
        return;
    }
    let has_transcript = meetings::get(&state.db.relock(), &id).ok().flatten().is_some_and(|(_, segments)| !segments.is_empty());
    if !has_transcript {
        return;
    }
    let _ = summary::set_status(&state.db.relock(), &id, Some("summarising"), None);
    let _ = app.emit("meetings-updated", ());
    let failed_id = id.clone();
    let on_panic = move |app: &AppHandle| {
        let state = app.state::<AppState>();
        let conn = state.db.relock();
        let _ = summary::set_status(&conn, &failed_id, Some("failed"), Some("Writing notes stopped unexpectedly. Try again."));
    };
    spawn_job(&app, format!("summary:{id}"), on_panic, move |app| {
        let state = app.state::<AppState>();
        // Settings first: setting() locks the database itself. Read again here: they may have changed
        // while this job waited its turn.
        let provider = summary_provider(&state);
        let key = (provider == summary::Provider::Claude).then(|| anthropic_key(&state)).flatten();
        let model = claude_model(&state);
        let found = meetings::get(&state.db.relock(), &id).ok().flatten();
        let Some((meeting, segments)) = found.filter(|(_, s)| !s.is_empty()) else {
            let _ = summary::set_status(&state.db.relock(), &id, None, None);
            return;
        };
        let names = meetings::speaker_names(&state.db.relock(), &id).unwrap_or_default();
        let transcript = summary::transcript_text(&segments, &names);
        let result = match provider {
            summary::Provider::Off => {
                let _ = summary::set_status(&state.db.relock(), &id, None, None);
                let _ = app.emit("meetings-updated", ());
                return;
            }
            summary::Provider::Claude => match key {
                Some(key) => tauri::async_runtime::block_on(summary::with_claude(&key, model, &meeting.title, &transcript)),
                None => Err(anyhow::anyhow!("Add your Anthropic API key in Settings → Meetings to summarise with Claude")),
            },
            summary::Provider::Apple => summary::on_device(&id, &transcript),
        };
        {
            let conn = state.db.relock();
            let _ = match &result {
                Ok(s) => summary::save(&conn, &id, s, if provider == summary::Provider::Claude { model } else { "apple" }),
                Err(e) => summary::set_status(&conn, &id, Some("failed"), Some(&format!("{e:#}"))),
            };
        }
        let _ = app.emit("meetings-updated", ());
        // Not linked to a ticket: suggest one from the notes (AI features' engine; never links by itself).
        if let (Ok(notes), None) = (&result, &meeting.ticket) {
            suggest_ticket(app, &id, &meeting.title, notes);
        }
    });
}

/// Suggest the ticket an unlinked meeting is about. Best effort: no engine, no candidates or no clear
/// answer leaves it unsuggested.
fn suggest_ticket(app: &AppHandle, id: &str, title: &str, notes: &summary::Summary) {
    let state = app.state::<AppState>();
    let Some(engine) = ai_engine(&state) else { return };
    let candidates = suggest::candidates(&state.db.relock()).unwrap_or_default();
    if candidates.is_empty() {
        return;
    }
    let prompt = ai::Prompt {
        instructions: suggest::INSTRUCTIONS.into(),
        prompt: ai::fit(&suggest::prompt(title, notes, &candidates), engine.budget()),
        max_tokens: 1_024,
    };
    match ai::complete(&engine, &prompt) {
        Ok(answer) => {
            if let Some((key, ticket_title)) = suggest::pick(&answer, &candidates) {
                let _ = meetings::set_suggestion(&state.db.relock(), id, Some((key, ticket_title)));
                let _ = app.emit("meetings-updated", ());
            }
        }
        Err(e) => eprintln!("ticket suggestion skipped: {e:#}"),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CiExplanation {
    text: String,
    /// "apple" or the Claude model.
    by: String,
    /// The failed checks it looked at: (name, link).
    checks: Vec<(String, Option<String>)>,
}

/// Explain why a PR's checks failed, from the failing step, annotations and log (Settings → AI).
#[tauri::command]
async fn explain_ci(app: AppHandle, pr_id: String) -> CmdResult<CiExplanation> {
    let (engine, token) = {
        let state = app.state::<AppState>();
        (ai_engine(&state).ok_or("AI features are off (Settings → AI)")?, state.token().ok_or("Not signed in")?)
    };
    let client = github::Client::new(&token);
    let failed = client.failed_checks(&pr_id).await.map_err(|e| format!("{e:#}"))?;
    if failed.is_empty() {
        return Err("No failed checks on the latest commit — they may have been re-run or fixed.".into());
    }
    let shown = &failed[..failed.len().min(3)];
    let per_check = engine.budget().saturating_sub(1_500) / shown.len();
    let mut facts = String::new();
    for c in shown {
        let mut block = format!("## Check: {}{}\n", c.name, if c.app.is_empty() { String::new() } else { format!(" ({})", c.app) });
        if let Some(step) = &c.failed_step {
            block.push_str(&format!("Failed step: {step}\n"));
        }
        for a in c.annotations.iter().take(10) {
            block.push_str(&format!("Annotation: {a}\n"));
        }
        if !c.details.trim().is_empty() {
            block.push_str(&format!("Check output: {}\n", ai::fit(&c.details, 800)));
        }
        if let Some(job) = c.job_id {
            match client.job_log(&c.repo, job).await {
                Ok(log) => {
                    let room = per_check.saturating_sub(block.chars().count() + 40);
                    block.push_str(&format!("Log excerpt:\n{}\n", ci::excerpt(&log, room)));
                }
                Err(e) => block.push_str(&format!("(Couldn't fetch the log: {e:#})\n")),
            }
        } else {
            block.push_str("(Not GitHub Actions: only the check's own output is available.)\n");
        }
        facts.push_str(&ai::fit(&block, per_check));
        facts.push('\n');
    }
    if failed.len() > shown.len() {
        facts.push_str(&format!("({} more checks also failed.)\n", failed.len() - shown.len()));
    }
    let prompt = ai::Prompt {
        instructions: ci::INSTRUCTIONS.into(),
        prompt: format!("Why did these checks fail?\n\n{facts}"),
        max_tokens: 2_000,
    };
    let by = engine.id().to_string();
    let text = tauri::async_runtime::spawn_blocking(move || ai::complete(&engine, &prompt))
        .await
        .map_err(err)?
        .map_err(|e| format!("{e:#}"))?;
    Ok(CiExplanation { text, by, checks: failed.into_iter().map(|c| (c.name, c.url)).collect() })
}

/// Smarter reminders: have the AI read the comments the reminder rules didn't match but that might
/// mean "I'll come back to this", and park those PRs. A background job; each comment is read once,
/// and if the AI can't be reached they wait for the next sync.
fn read_intents(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.demo {
        return;
    }
    let Some(engine) = ai_engine(&state) else { return };
    let pending = reminders::pending(&state.db.relock(), 10).unwrap_or_default();
    if pending.is_empty() {
        return;
    }
    spawn_job(app, "ai-intents".into(), |_| {}, move |app| {
        let state = app.state::<AppState>();
        let notifications = state.setting("notifications").as_deref() != Some("0");
        let mut changed = false;
        for (key, pr_id, at, body) in pending {
            let prompt = ai::Prompt {
                instructions: reminders::INSTRUCTIONS.into(),
                prompt: format!("Comment:\n{}", ai::fit(&body, engine.budget().saturating_sub(800))),
                max_tokens: 512,
            };
            let answer = match ai::complete(&engine, &prompt) {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("reading comments for reminders paused: {e:#}");
                    break;
                }
            };
            let verdict = reminders::parse(&answer);
            let applied = {
                let conn = state.db.relock();
                let r = reminders::apply(&conn, &key, &pr_id, &at, &verdict, chrono::Utc::now()).ok().flatten();
                let _ = reminders::mark_checked(&conn, &key);
                r
            };
            if let Some((back, pr)) = applied {
                changed = true;
                if notifications {
                    notify(
                        app,
                        sync::Notice {
                            title: "📌 Noted from your comment".into(),
                            body: format!("{}#{} {}\n{back}", pr.repo, pr.number, pr.title),
                            url: Some(pr.url.clone()),
                            item_id: Some(pr.id.clone()),
                        },
                    );
                }
            }
        }
        if changed {
            refresh_count(app);
        }
    });
}

/// "Not this": forget the suggested ticket for a meeting.
#[tauri::command(async)]
fn dismiss_suggestion(app: AppHandle, id: String) -> CmdResult<()> {
    meetings::set_suggestion(&app.state::<AppState>().db.relock(), &id, None).map_err(err)?;
    let _ = app.emit("meetings-updated", ());
    Ok(())
}

/// Your open action items from meeting notes, for Today.
fn open_actions(state: &AppState) -> Vec<summary::Action> {
    summary::open_actions(&state.db.relock(), &now()).unwrap_or_default()
}

#[tauri::command(async)]
fn get_meeting_actions(state: State<AppState>) -> Vec<summary::Action> {
    open_actions(&state)
}

#[tauri::command(async)]
fn set_action_done(app: AppHandle, id: i64, done: bool) -> CmdResult<()> {
    summary::set_done(&app.state::<AppState>().db.relock(), id, done, &now()).map_err(err)?;
    refresh_count(&app);
    let _ = app.emit("meetings-updated", ());
    Ok(())
}

/// Hide an action item until `until` (an ISO time; none shows it again now).
#[tauri::command(async)]
fn snooze_action(app: AppHandle, id: i64, until: Option<String>) -> CmdResult<()> {
    let until = match until {
        Some(u) => Some(derive::parse_ts(&u).map(derive::fmt_ts).ok_or("Bad snooze time")?),
        None => None,
    };
    summary::snooze(&app.state::<AppState>().db.relock(), id, until.as_deref()).map_err(err)?;
    refresh_count(&app);
    let _ = app.emit("meetings-updated", ());
    Ok(())
}

/// Make (or remake) a meeting's summary now.
#[tauri::command(async)]
fn summarise_meeting_now(app: AppHandle, id: String) -> CmdResult<()> {
    let state = app.state::<AppState>();
    if state.demo {
        return Err("Summaries aren't made in demo mode".into());
    }
    if summary_provider(&state) == summary::Provider::Off {
        return Err("Meeting summaries are off (Settings → Meetings)".into());
    }
    summarise_meeting(app.clone(), id);
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SummaryStatus {
    /// The summariser that will actually run (Claude falls back to Apple Intelligence without a key).
    provider: String,
    /// The engine the other AI features will actually use.
    ai_provider: String,
    /// Apple Intelligence can summarise on this Mac now.
    apple_ready: bool,
    /// Why not, in words.
    apple_reason: Option<String>,
    /// An Anthropic API key is saved.
    claude_key: bool,
    /// The Claude models on offer: (model id, name).
    claude_models: Vec<ClaudeModel>,
}

#[derive(Serialize)]
struct ClaudeModel {
    id: &'static str,
    name: &'static str,
}

/// What Settings shows about summaries. Asks the helper whether Apple Intelligence is ready.
#[tauri::command]
async fn summary_status(app: AppHandle) -> SummaryStatus {
    let check = tauri::async_runtime::spawn_blocking(|| {
        let out = meetings::run_helper(&[std::ffi::OsStr::new("check")], std::time::Duration::from_secs(20)).ok()?;
        out.lines().find_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
    })
    .await
    .ok()
    .flatten()
    .unwrap_or_default();
    let state = app.state::<AppState>();
    SummaryStatus {
        provider: summary_provider(&state).id().into(),
        ai_provider: effective(&state, "ai_provider").id().into(),
        apple_ready: check["summary"].as_bool().unwrap_or(false),
        apple_reason: check["summaryReason"].as_str().map(String::from).or_else(|| {
            (!check["summary"].as_bool().unwrap_or(false)).then(|| "On-device summaries need macOS 26 and Apple Intelligence".into())
        }),
        claude_key: anthropic_key(&state).is_some(),
        claude_models: summary::CLAUDE_MODELS.iter().map(|&(id, name)| ClaudeModel { id, name }).collect(),
    }
}

/// Save the Anthropic API key (keychain) for Claude summaries. The user types it; it's never shown again.
#[tauri::command(async)]
fn set_anthropic_key(state: State<AppState>, key: String) -> CmdResult<()> {
    let key = key.trim();
    if key.is_empty() {
        return Err("Paste an API key".into());
    }
    keyring::Entry::new(KEYRING_SERVICE, ANTHROPIC_KEYRING_USER)
        .and_then(|e| e.set_password(key))
        .map_err(|e| format!("Couldn't save the key to the keychain: {e}"))?;
    *state.anthropic_key.relock() = Some(key.to_string());
    Ok(())
}

#[tauri::command(async)]
fn clear_anthropic_key(state: State<AppState>) {
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, ANTHROPIC_KEYRING_USER) {
        let _ = entry.delete_credential();
    }
    *state.anthropic_key.relock() = None;
}

fn set_recording_tray(app: &AppHandle, _on: bool) {
    refresh_tray(app);
}

/// Start recording a meeting (mic + meeting audio). Only ever started by the user.
#[tauri::command(async)]
fn start_recording(app: AppHandle, title: Option<String>) -> CmdResult<RecordingStatus> {
    let state = app.state::<AppState>();
    if state.demo {
        return Err("Recording isn't available in demo mode".into());
    }
    if state.recorder.relock().is_some() {
        return Err("Already recording".into());
    }
    let started = chrono::Utc::now();
    let id = format!("m{}", started.timestamp_millis());
    let dir = app.path().app_data_dir().map_err(err)?.join("meetings").join(&id);
    let title = title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| format!("Meeting {}", started.with_timezone(&chrono::Local).format("%H:%M")));
    meetings::create(&state.db.relock(), &id, &title, &derive::fmt_ts(started), &dir).map_err(err)?;

    let handle = app.clone();
    let meeting_id = id.clone();
    let rec_dir = dir.clone();
    let locale = state.setting("transcribe_locale").unwrap_or_default();
    let captions = state.setting("live_captions").as_deref() != Some("0");
    let recording = meetings::start(&id, &dir, &locale, captions, move |ev| {
        let state = handle.state::<AppState>();
        match ev["event"].as_str() {
            Some("level") => {
                let _ = handle.emit("recording-level", &ev);
            }
            Some("started") | Some("warning") => {
                let _ = handle.emit("recording-event", &ev);
            }
            Some("caption") => {
                let _ = handle.emit("recording-caption", &ev);
            }
            Some("captions-off") => {
                let _ = handle.emit("recording-event", serde_json::json!({"event": "info", "message": ev["message"]}));
            }
            Some("error") => {
                let msg = ev["message"].as_str().unwrap_or("Recording failed").to_string();
                let _ = meetings::set_status(&state.db.relock(), &meeting_id, "failed", Some(&msg));
                state.recorder.relock().take();
                set_recording_tray(&handle, false);
                let _ = handle.emit("recording-event", &ev);
                let _ = handle.emit("meetings-updated", ());
            }
            Some("exited") => {
                // Normal after "stopped". Otherwise the helper died mid-recording: don't stay "recording".
                let still_recording =
                    meetings::get(&state.db.relock(), &meeting_id).ok().flatten().is_some_and(|(m, _)| m.status == "recording");
                if still_recording {
                    let mine = state.recorder.relock().take_if(|r| r.meeting_id == meeting_id);
                    if let Some(mut r) = mine {
                        let _ = r.child.wait();
                    }
                    let _ = meetings::interrupted_recording(&state.db.relock(), &meeting_id, Some(&rec_dir));
                    set_recording_tray(&handle, false);
                    let _ = handle.emit("recording-event", serde_json::json!({"event": "error", "message": "The recorder stopped unexpectedly"}));
                    let _ = handle.emit("meetings-updated", ());
                }
            }
            Some("stopped") => {
                let ended = derive::fmt_ts(chrono::Utc::now());
                let sources = ev["sources"].as_array().map(|a| {
                    a.iter().filter_map(|s| s.as_str()).collect::<Vec<_>>().join(",")
                });
                let seconds = ev["seconds"].as_i64().unwrap_or(0);
                let _ = meetings::finish_recording(
                    &state.db.relock(),
                    &meeting_id,
                    &ended,
                    seconds,
                    &sources.unwrap_or_default(),
                );
                let _ = handle.emit("recording-event", &ev);
                let _ = handle.emit("meetings-updated", ());
                transcribe_meeting(handle.clone(), meeting_id.clone(), rec_dir.clone());
            }
            _ => {}
        }
    })
    .map_err(|e| {
        let _ = meetings::set_status(&state.db.relock(), &id, "failed", Some(&format!("{e:#}")));
        format!("{e:#}")
    })?;
    *state.recorder.relock() = Some(recording);
    set_recording_tray(&app, true);
    let _ = app.emit("meetings-updated", ());
    Ok(recording_status_of(&state))
}

#[tauri::command]
async fn stop_recording(app: AppHandle) -> CmdResult<()> {
    let recording = app.state::<AppState>().recorder.relock().take().ok_or("Not recording")?;
    tauri::async_runtime::spawn_blocking(move || meetings::stop(recording)).await.map_err(err)?.map_err(err)?;
    set_recording_tray(&app, false);
    Ok(())
}

#[tauri::command(async)]
fn list_meetings(state: State<AppState>) -> CmdResult<Vec<meetings::Meeting>> {
    meetings::list(&state.db.relock()).map_err(err)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MeetingDetail {
    meeting: meetings::Meeting,
    segments: Vec<meetings::Segment>,
    /// The notes' action items, with your progress on them.
    actions: Vec<summary::Action>,
    /// Names of the voices on the call: voice → name.
    speakers: std::collections::HashMap<String, String>,
    /// Voices named automatically from someone you named before (not by you).
    recognised: Vec<String>,
}

#[tauri::command(async)]
fn get_meeting(state: State<AppState>, id: String) -> CmdResult<MeetingDetail> {
    let conn = state.db.relock();
    let (meeting, segments) = meetings::get(&conn, &id).map_err(err)?.ok_or("Unknown meeting")?;
    let actions = summary::meeting_actions(&conn, &id).map_err(err)?;
    let speakers = meetings::speaker_names(&conn, &id).map_err(err)?;
    let recognised = meetings::recognised_voices(&conn, &id).map_err(err)?;
    Ok(MeetingDetail { meeting, segments, actions, speakers, recognised })
}

/// Answer a question about your own work from what the app has recorded (Settings → AI picks the engine).
#[tauri::command]
async fn ask_history(app: AppHandle, question: String) -> CmdResult<ask::Answer> {
    let state = app.state::<AppState>();
    let engine = ai_engine(&state).ok_or("AI features are off (Settings → AI)")?;
    let me = state.setting("jira_account").unwrap_or_default();
    let question = question.trim().to_string();
    if question.is_empty() {
        return Err("Ask a question".into());
    }
    let (facts, sources, period) = {
        let conn = state.db.relock();
        ask::gather(&conn, &question, &me, engine.budget().saturating_sub(1_200)).map_err(err)?
    };
    let by = engine.id().to_string();
    if facts.trim().is_empty() {
        return Ok(ask::Answer { answer: format!("Nothing is recorded for {period} yet."), sources, by, period });
    }
    let prompt = ai::Prompt {
        instructions: ask::INSTRUCTIONS.into(),
        prompt: ask::prompt(&question, &facts, &period),
        max_tokens: 2_000,
    };
    let answer = tauri::async_runtime::spawn_blocking(move || ai::complete(&engine, &prompt))
        .await
        .map_err(err)?
        .map_err(|e| format!("{e:#}"))?;
    Ok(ask::Answer { answer, sources, by, period })
}

/// People whose voices the app recognises (they were named in a meeting).
#[tauri::command(async)]
fn known_voices(state: State<AppState>) -> CmdResult<Vec<voices::KnownVoice>> {
    voices::known(&state.db.relock()).map_err(err)
}

#[tauri::command(async)]
fn forget_voice(state: State<AppState>, id: i64) -> CmdResult<()> {
    voices::forget(&state.db.relock(), id).map_err(err)
}

/// Search every meeting's title, transcript and notes.
#[tauri::command(async)]
fn search_meetings(state: State<AppState>, query: String) -> CmdResult<Vec<search::Result_>> {
    search::search(&state.db.relock(), &query).map_err(err)
}

/// Give a voice on the call a name ("Sam"); empty puts it back to "Speaker N".
#[tauri::command(async)]
fn name_speaker(app: AppHandle, id: String, voice: String, name: String) -> CmdResult<()> {
    let state = app.state::<AppState>();
    let learn = state.setting("recognise_voices").as_deref() != Some("0");
    meetings::name_speaker(&state.db.relock(), &id, &voice, &name, learn).map_err(err)?;
    let _ = app.emit("meetings-updated", ());
    Ok(())
}

#[tauri::command(async)]
fn update_meeting(app: AppHandle, id: String, title: String, ticket: Option<String>) -> CmdResult<()> {
    meetings::update(&app.state::<AppState>().db.relock(), &id, &title, ticket.as_deref()).map_err(err)?;
    let _ = app.emit("meetings-updated", ());
    Ok(())
}

#[tauri::command(async)]
fn delete_meeting(app: AppHandle, id: String) -> CmdResult<()> {
    meetings::delete(&app.state::<AppState>().db.relock(), &id).map_err(err)?;
    let _ = app.emit("meetings-updated", ());
    Ok(())
}

/// Retry transcription (e.g. after it failed, or to pick up a language change).
#[tauri::command(async)]
fn retranscribe_meeting(app: AppHandle, id: String) -> CmdResult<()> {
    let dir = {
        let state = app.state::<AppState>();
        let conn = state.db.relock();
        meetings::set_status(&conn, &id, "transcribing", None).map_err(err)?;
        let (m, _) = meetings::get(&conn, &id).map_err(err)?.ok_or("Unknown meeting")?;
        if !m.has_audio {
            return Err("The audio for this meeting has been deleted".into());
        }
        std::path::PathBuf::from(m.audio_dir.ok_or("No audio")?)
    };
    let _ = app.emit("meetings-updated", ());
    transcribe_meeting(app, id, dir);
    Ok(())
}

// ---------- Day view ----------

/// Every few minutes, note which local clones changed (branch, dirty files, mtimes).
/// Only the repo, branch and time are stored; nothing leaves the machine.
fn start_local_tracker(app: AppHandle) {
    const EVERY: Duration = Duration::from_secs(300);
    std::thread::spawn(move || {
        let mut last: std::collections::HashMap<String, u64> = Default::default();
        let mut ticks: u64 = 0;
        loop {
            let state = app.state::<AppState>();
            if ticks % 12 == 0 {
                let days = audio_retention_days(&state);
                let _ = meetings::apply_retention(&state.db.relock(), days);
            }
            ticks += 1;
            if !state.demo && state.setting("track_local").as_deref() != Some("0") && state.setting("login").is_some() {
                for (repo, path) in clones(&state) {
                    let Some(s) = editor::sample(&path) else { continue };
                    let changed = last.insert(repo.clone(), s.signature).is_some_and(|prev| prev != s.signature);
                    if changed {
                        let _ = db::record_sample(&state.db.relock(), &now(), &repo, s.branch.as_deref());
                    }
                }
            }
            std::thread::sleep(EVERY);
        }
    });
}

/// A day's time report. `start`/`end` are the local day's bounds as ISO instants.
#[tauri::command]
async fn get_day(app: AppHandle, day: String, start: String, end: String) -> CmdResult<day::DayReport> {
    let (Some(s), Some(e)) = (derive::parse_ts(&start), derive::parse_ts(&end)) else {
        return Err("Bad day bounds".into());
    };
    day_report(&app, &day, s, e).await
}

/// The Day view's report for one local day (used by the Day view and the AI summaries).
async fn day_report(app: &AppHandle, day: &str, s: derive::Ts, e: derive::Ts) -> CmdResult<day::DayReport> {
    let state = app.state::<AppState>();
    let me = state.setting("jira_account").unwrap_or_default();
    let (start, end) = (derive::fmt_ts(s), derive::fmt_ts(e));
    let clones = clones(&state);
    let (since, until) = (start.clone(), end.clone());
    // Local git log runs outside the database lock.
    let local = tauri::async_runtime::spawn_blocking(move || {
        clones
            .into_iter()
            .flat_map(|(repo, path)| editor::local_commits(&path, &since, &until).into_iter().map(move |c| (repo.clone(), c)))
            .collect::<Vec<_>>()
    })
    .await
    .map_err(err)?;
    let conn = state.db.relock();
    let titles = jira::titles(&conn).map_err(err)?;
    let mut events = day::gather(&conn, &start, &end).map_err(err)?;
    events.extend(day::local_events(&conn, local).map_err(err)?);
    events.extend(day::jira_events(&conn, &start, &end, &titles).map_err(err)?);
    let meeting_spans = meetings::day_spans(&conn, &start, &end).map_err(err)?;
    let manual = day::manual_blocks(&conn, day).map_err(err)?;
    let mut report = day::build(day, events, manual, s, e, &titles, &meeting_spans);
    report.ticket_moves = jira::moves_between(&conn, &start, &end, &me).map_err(err)?;
    report.meetings = meetings::started_between(&conn, &start, &end).map_err(err)?;
    Ok(report)
}

/// A local calendar day's bounds as instants.
fn local_day(d: chrono::NaiveDate) -> (derive::Ts, derive::Ts) {
    use chrono::TimeZone;
    let at = |d: chrono::NaiveDate| chrono::Local.from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap()).earliest().unwrap().to_utc();
    (at(d), at(d + chrono::Duration::days(1)))
}

fn hours(minutes: i64) -> String {
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    }
}

/// "Fri 16 Oct: 6h 48m — BILL-142 Prorate… 2h 45m; MOB-77 … 2h 31m; …"
fn time_line(date: chrono::NaiveDate, r: &day::DayReport) -> String {
    let top: Vec<String> = r.groups.iter().filter(|g| g.minutes > 0).take(6).map(|g| format!("{} {}", g.label, hours(g.minutes))).collect();
    format!("{}: {} tracked{}", date.format("%a %-d %b"), hours(r.total_minutes), if top.is_empty() { String::new() } else { format!(" — {}", top.join("; ")) })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AiSummary {
    text: String,
    /// "apple" or the Claude model that wrote it.
    by: String,
    created_at: String,
}

/// "day:2026-10-16", or "week:<its Monday>".
fn summary_key(kind: &str, day: &str) -> CmdResult<(String, chrono::NaiveDate)> {
    use chrono::Datelike;
    let date = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").map_err(|_| "Bad date")?;
    match kind {
        "day" => Ok((format!("day:{date}"), date)),
        "week" => {
            let monday = date - chrono::Duration::days(date.weekday().num_days_from_monday() as i64);
            Ok((format!("week:{monday}"), monday))
        }
        _ => Err("Unknown summary".into()),
    }
}

/// A day or week summary written earlier, if any.
#[tauri::command(async)]
fn get_ai_summary(state: State<AppState>, kind: String, day: String) -> CmdResult<Option<AiSummary>> {
    let (key, _) = summary_key(&kind, &day)?;
    state
        .db
        .relock()
        .query_row("SELECT text, written_by, created_at FROM ai_summaries WHERE key = ?1", [&key], |r| {
            Ok(AiSummary { text: r.get(0)?, by: r.get(1)?, created_at: r.get(2)? })
        })
        .optional()
        .map_err(err)
}

const DAY_INSTRUCTIONS: &str = "You write a short, readable summary of the user's work day for their own records and timesheet, from the facts given (\"You\" is the user). 3 to 6 bullet points starting with \"- \", the most time first: say what was worked on (with ticket keys), roughly how long (from the time-spent lines), what moved, and what meetings decided. Plain words, no headings, no preamble, no [S] tags. Only use the facts; don't invent.";

const WEEK_INSTRUCTIONS: &str = "You write the user's weekly review from the facts given (\"You\" is the user). Four short sections, each a heading line then bullets starting with \"- \": \"Shipped\" (merged or opened PRs, tickets moved to done), \"Time\" (where the hours went, by day and by ticket, from the time-spent lines), \"Decided\" (from meeting notes) and \"Still open\" (action items, things in progress). Skip a section with nothing in it. Be concise; no preamble, no [S] tags. Only use the facts; don't invent.";

/// Write (or rewrite) the AI summary of a day or a week (Settings → AI picks the engine).
#[tauri::command]
async fn write_ai_summary(app: AppHandle, kind: String, day: String) -> CmdResult<AiSummary> {
    use chrono::Datelike;
    let (key, first) = summary_key(&kind, &day)?;
    let engine = {
        let state = app.state::<AppState>();
        ai_engine(&state).ok_or("AI features are off (Settings → AI)")?
    };
    let today = chrono::Local::now().date_naive();
    let days: Vec<chrono::NaiveDate> = if kind == "week" {
        (0..7).map(|i| first + chrono::Duration::days(i)).filter(|d| *d <= today).collect()
    } else {
        vec![first]
    };
    let mut time = String::new();
    for d in &days {
        let (s, e) = local_day(*d);
        let r = day_report(&app, &d.to_string(), s, e).await?;
        time.push_str(&time_line(*d, &r));
        time.push('\n');
    }
    let (start, _) = local_day(days[0]);
    let (_, end) = local_day(*days.last().unwrap());
    let words = if kind == "week" {
        format!("the week of {}", first.format("%-d %b"))
    } else {
        first.format("%A %-d %B").to_string()
    };
    let facts = {
        let state = app.state::<AppState>();
        let me = state.setting("jira_account").unwrap_or_default();
        let conn = state.db.relock();
        ask::period_facts(&conn, &derive::fmt_ts(start), &derive::fmt_ts(end), &words, &me, &time, engine.budget().saturating_sub(1_200))
            .map_err(err)?
    };
    let prompt = ai::Prompt {
        instructions: if kind == "week" { WEEK_INSTRUCTIONS } else { DAY_INSTRUCTIONS }.into(),
        prompt: format!("Summarise {words}{}.\n\n{facts}", if first.weekday() == today.weekday() && days.last() == Some(&today) { " (so far — it isn't over)" } else { "" }),
        max_tokens: 2_000,
    };
    let by = engine.id().to_string();
    let text = tauri::async_runtime::spawn_blocking(move || ai::complete(&engine, &prompt))
        .await
        .map_err(err)?
        .map_err(|e| format!("{e:#}"))?;
    let created_at = now();
    app.state::<AppState>()
        .db
        .relock()
        .execute(
            "INSERT OR REPLACE INTO ai_summaries (key, text, written_by, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![key, text, by, created_at],
        )
        .map_err(err)?;
    Ok(AiSummary { text, by, created_at })
}

/// Save corrected blocks for a day (freezes it).
#[tauri::command(async)]
fn save_day(state: State<AppState>, day: String, blocks: Vec<day::Block>) -> CmdResult<()> {
    for b in &blocks {
        match (derive::parse_ts(&b.start), derive::parse_ts(&b.end)) {
            (Some(s), Some(e)) if e > s => {}
            _ => return Err(format!("“{}” ends before it starts", b.label)),
        }
    }
    day::save_blocks(&state.db.relock(), &day, &blocks).map_err(err)
}

/// Throw away corrections and go back to automatic for a day.
#[tauri::command(async)]
fn reset_day(state: State<AppState>, day: String) -> CmdResult<()> {
    day::unfreeze(&state.db.relock(), &day).map_err(err)
}

// ---------- window / tray ----------

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn palette_shortcut() -> Shortcut {
    #[cfg(target_os = "macos")]
    let mods = Modifiers::SUPER | Modifiers::ALT;
    #[cfg(not(target_os = "macos"))]
    let mods = Modifiers::CONTROL | Modifiers::ALT;
    Shortcut::new(Some(mods), Code::KeyK)
}

#[cfg(test)]
mod summary_tests {
    #[test]
    fn day_and_week_keys() {
        assert_eq!(super::summary_key("day", "2026-10-16").unwrap().0, "day:2026-10-16");
        // Friday 16 Oct → its Monday.
        let (key, monday) = super::summary_key("week", "2026-10-16").unwrap();
        assert_eq!((key.as_str(), monday.to_string().as_str()), ("week:2026-10-12", "2026-10-12"));
        assert!(super::summary_key("year", "2026-10-16").is_err());
        assert!(super::summary_key("day", "16/10/2026").is_err());
        assert_eq!(super::hours(165), "2h 45m");
        assert_eq!(super::hours(45), "45m");
        assert_eq!(super::hours(120), "2h");
    }
}

#[cfg(all(test, target_os = "macos"))]
mod notify_tests {
    /// Shows a real notification through the path an unbundled build uses: `cargo test notify_smoke -- --ignored`
    #[test]
    #[ignore]
    fn notify_smoke() {
        assert!(!super::notifications::bundled(), "tests run as a bare binary");
        super::notifications::send("🔔 Dev Inbox test", "acme/web#482 Checkout refactor", "sarah replied in your thread", None, None);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() == ShortcutState::Pressed && *shortcut == palette_shortcut() {
                        show_main(app);
                        let _ = app.emit("open-palette", ());
                    }
                })
                .build(),
        )
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let demo = std::env::var("DEV_INBOX_DEMO").is_ok_and(|v| v == "1");
            let conn = db::open(&dir.join(if demo { "dev-inbox-demo.sqlite3" } else { "dev-inbox.sqlite3" }))?;
            if demo {
                demo::seed(&conn)?;
            }
            app.manage(AppState {
                db: Mutex::new(conn),
                token: Mutex::new(None),
                jira_token: Mutex::new(None),
                anthropic_key: Mutex::new(None),
                jira_lock: tokio::sync::Mutex::new(()),
                recorder: Mutex::new(None),
                jobs: Mutex::new(Default::default()),
                heavy_job: Mutex::new(()),
                sync_lock: tokio::sync::Mutex::new(()),
                wake: tokio::sync::Notify::new(),
                demo,
            });
            if !demo {
                resume_meeting_jobs(app.handle());
            }
            #[cfg(target_os = "macos")]
            {
                let handle = app.handle().clone();
                notifications::init(move |url, item| notification_clicked(&handle, url, item));
                // Test aid: DEV_INBOX_NOTIFY_TEST=1 sends one notification at launch.
                if std::env::var("DEV_INBOX_NOTIFY_TEST").is_ok_and(|v| v == "1") {
                    let handle = app.handle().clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_secs(3));
                        let message = test_notification(handle);
                        eprintln!("notification test: {message}");
                    });
                }
            }

            let menu = tray_menu(app.handle())?;
            // macOS: a monochrome template icon that adapts to light/dark menu bars.
            #[cfg(target_os = "macos")]
            let (icon, template) = (tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?, true);
            #[cfg(not(target_os = "macos"))]
            let (icon, template) = (app.default_window_icon().unwrap().clone(), false);
            create_panel(app.handle())?;
            TrayIconBuilder::with_id("main")
                .icon(icon)
                .icon_as_template(template)
                .tooltip("Dev Inbox")
                .menu(&menu)
                // Left click: the panel. Right click: the short menu.
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| tray_action(app, event.id.as_ref()))
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        button_state: tauri::tray::MouseButtonState::Up,
                        rect,
                        ..
                    } = event
                    {
                        toggle_panel(tray.app_handle(), rect);
                    }
                })
                .build(app)?;
            // Test aid: DEV_INBOX_PANEL_TEST=1 opens the panel a few seconds after launch.
            if std::env::var("DEV_INBOX_PANEL_TEST").is_ok_and(|v| v == "1") {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(4));
                    if let Some(rect) = handle.tray_by_id("main").and_then(|t| t.rect().ok().flatten()) {
                        toggle_panel(&handle, rect);
                    }
                });
            }
            refresh_tray(app.handle());

            if let Err(e) = app.global_shortcut().register(palette_shortcut()) {
                eprintln!("couldn't register global shortcut: {e}");
            }
            start_poller(app.handle().clone());
            start_local_tracker(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            // The menu bar panel goes away when you click anywhere else.
            if window.label() == "tray" {
                if let WindowEvent::Focused(false) = event {
                    let _ = window.hide();
                }
                return;
            }
            // Closing the window keeps the app alive in the tray.
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_session,
            sign_in,
            import_gh_token,
            sign_out,
            get_inbox,
            sync_now,
            set_state,
            mark_seen,
            rerun_checks,
            get_following,
            set_followed,
            test_notification,
            get_day,
            save_day,
            reset_day,
            jira_status,
            jira_connect,
            jira_disconnect,
            jira_sync_now,
            get_tickets,
            get_ticket,
            get_ticket_attention,
            summarise_meeting_now,
            get_meeting_actions,
            search_meetings,
            link_meeting,
            dismiss_suggestion,
            open_in_main,
            jira_transitions,
            jira_move,
            jira_assignable,
            jira_assign,
            hide_panel,
            quit_app,
            explain_ci,
            known_voices,
            ask_history,
            get_ai_summary,
            write_ai_summary,
            forget_voice,
            name_speaker,
            set_action_done,
            snooze_action,
            summary_status,
            set_anthropic_key,
            clear_anthropic_key,
            mark_ticket_seen,
            recording_status,
            start_recording,
            stop_recording,
            list_meetings,
            get_meeting,
            update_meeting,
            delete_meeting,
            retranscribe_meeting,
            submit_review,
            send_comment,
            merge_options,
            merge_pr,
            editor_settings,
            update_editor_settings,
            set_repo_path,
            open_locally,
            clone_and_open,
            get_activity,
            get_item_activity,
            get_repos,
            set_repo_hidden,
            get_settings,
            update_settings,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|_app, _event| {
        // Clicking the dock icon brings the window back after it was closed to the tray.
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = _event {
            show_main(_app);
        }
    });
}
