mod db;
mod demo;
mod derive;
mod editor;
mod github;
mod intent;
mod sync;

use rusqlite::Connection;
use serde::Serialize;
use std::sync::Mutex;
use std::time::Duration;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
#[cfg(not(target_os = "macos"))]
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

const KEYRING_SERVICE: &str = "dev-inbox";
const KEYRING_USER: &str = "github-token";
const DEFAULT_POLL_SECS: u64 = 120;

pub struct AppState {
    db: Mutex<Connection>,
    token: Mutex<Option<String>>,
    sync_lock: tokio::sync::Mutex<()>,
    wake: tokio::sync::Notify,
    /// `DEV_INBOX_DEMO=1`: fictional data in a separate database, never synced.
    demo: bool,
}

impl AppState {
    /// The GitHub token, read from the OS keychain once per launch.
    fn token(&self) -> Option<String> {
        let mut cached = self.token.lock().unwrap();
        if cached.is_none() {
            *cached = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).ok().and_then(|e| e.get_password().ok());
        }
        cached.clone()
    }

    fn setting(&self, key: &str) -> Option<String> {
        db::get_setting(&self.db.lock().unwrap(), key).ok().flatten()
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

fn update_tray(app: &AppHandle, needs_you: usize) {
    if let Some(tray) = app.tray_by_id("main") {
        let title = (needs_you > 0).then(|| needs_you.to_string());
        let _ = tray.set_title(title);
        let _ = tray.set_tooltip(Some(format!("Dev Inbox — {needs_you} need you")));
    }
}

fn refresh_count(app: &AppHandle) {
    let state = app.state::<AppState>();
    let n = db::list_items(&state.db.lock().unwrap())
        .map(|items| items.iter().filter(|i| i.reminder_due || derive::is_actionable(&i.state)).count())
        .unwrap_or(0);
    update_tray(app, n);
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
            update_tray(app, out.needs_you);
            let _ = app.emit("inbox-updated", ());
            Ok(())
        }
        Err(e) => {
            let msg = format!("{e:#}");
            let _ = db::set_setting(&state.db.lock().unwrap(), "last_error", &msg);
            let _ = app.emit("inbox-updated", ());
            Err(msg)
        }
    }
}

/// Show a desktop notification. On macOS, clicking it opens the item (and marks it seen).
fn notify(app: &AppHandle, n: sync::Notice) {
    #[cfg(target_os = "macos")]
    {
        use std::sync::atomic::{AtomicUsize, Ordering};
        // Each clickable notification parks a thread until it's clicked or cleared; cap how many.
        static WAITING: AtomicUsize = AtomicUsize::new(0);
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            // Unbundled dev builds have no identity of their own; borrow Terminal's like the plugin does.
            let id = if tauri::is_dev() { "com.apple.Terminal".to_string() } else { app.config().identifier.clone() };
            let _ = mac_notification_sys::set_application(&id);
        });
        let app = app.clone();
        let wait = WAITING.fetch_add(1, Ordering::SeqCst) < 12;
        if !wait {
            WAITING.fetch_sub(1, Ordering::SeqCst);
        }
        std::thread::spawn(move || {
            let (subtitle, message) = n.body.split_once('\n').unwrap_or(("", n.body.as_str()));
            let mut opts = mac_notification_sys::Notification::new();
            opts.wait_for_click(wait);
            let res = mac_notification_sys::send_notification(
                &n.title,
                (!subtitle.is_empty()).then_some(subtitle),
                message,
                Some(&opts),
            );
            if wait {
                WAITING.fetch_sub(1, Ordering::SeqCst);
            }
            if let Ok(mac_notification_sys::NotificationResponse::Click) = res {
                if let Some(id) = &n.item_id {
                    let state = app.state::<AppState>();
                    let _ = db::mark_seen(&state.db.lock().unwrap(), id, &now());
                    let _ = app.emit("inbox-updated", ());
                }
                match &n.url {
                    Some(url) => {
                        let _ = app.opener().open_url(url, None::<&str>);
                    }
                    None => show_main(&app),
                }
            }
        });
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

#[tauri::command]
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
        let conn = state.db.lock().unwrap();
        // A different account's memory must never bleed into this one.
        if db::get_setting(&conn, "login").map_err(err)?.is_some_and(|l| l != viewer.login) {
            db::reset(&conn).map_err(err)?;
        }
        db::set_setting(&conn, "login", &viewer.login).map_err(err)?;
        db::set_setting(&conn, "avatar", &viewer.avatar_url).map_err(err)?;
    }
    *state.token.lock().unwrap() = Some(token);
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

#[tauri::command]
fn sign_out(state: State<AppState>, delete_data: bool) -> CmdResult<()> {
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER) {
        let _ = entry.delete_credential();
    }
    *state.token.lock().unwrap() = None;
    let conn = state.db.lock().unwrap();
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

#[tauri::command]
fn get_inbox(state: State<AppState>) -> CmdResult<Inbox> {
    let syncing = state.sync_lock.try_lock().is_err();
    let conn = state.db.lock().unwrap();
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
#[tauri::command]
fn set_state(
    app: AppHandle,
    pr_ids: Vec<String>,
    state: Option<String>,
    until: Option<String>,
    note: Option<String>,
) -> CmdResult<()> {
    {
        let st = app.state::<AppState>();
        let conn = st.db.lock().unwrap();
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
#[tauri::command]
fn test_notification(app: AppHandle) {
    notify(
        &app,
        sync::Notice {
            title: "🔔 Dev Inbox notifications work".into(),
            body: "Click me\nThis would open the PR or issue that changed".into(),
            url: Some("https://github.com/notifications".into()),
            item_id: None,
        },
    );
}

#[tauri::command]
fn get_following(state: State<AppState>) -> CmdResult<Vec<db::FollowItem>> {
    db::list_following(&state.db.lock().unwrap()).map_err(err)
}

/// Follow or stop following items. Unfollowed items vanish from Today, counts and notifications.
#[tauri::command]
fn set_followed(app: AppHandle, ids: Vec<String>, followed: bool) -> CmdResult<()> {
    {
        let st = app.state::<AppState>();
        let conn = st.db.lock().unwrap();
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
#[tauri::command]
fn mark_seen(app: AppHandle, pr_ids: Vec<String>) -> CmdResult<()> {
    {
        let st = app.state::<AppState>();
        let conn = st.db.lock().unwrap();
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
    let _ = db::mark_seen(&state.db.lock().unwrap(), &item_id, &now());
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
    db::set_setting(&state.db.lock().unwrap(), "clones", &serde_json::to_string(&found).map_err(err)?).map_err(err)?;
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
        let conn = state.db.lock().unwrap();
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
#[tauri::command]
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
    db::set_setting(&state.db.lock().unwrap(), "repo_paths", &serde_json::to_string(&overrides).map_err(err)?).map_err(err)
}

/// Open an item's repo in the editor, optionally switching to the PR's branch first.
#[tauri::command]
async fn open_locally(app: AppHandle, item_id: String, checkout: bool) -> CmdResult<String> {
    if app.state::<AppState>().setting("clones").is_none() {
        scan(&app).await?;
    }
    let state = app.state::<AppState>();
    let (kind, repo, number, branch, cross_repo) =
        db::checkout_info(&state.db.lock().unwrap(), &item_id).map_err(err)?.ok_or("Unknown item")?;
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
    let _ = db::mark_seen(&state.db.lock().unwrap(), &item_id, &now());
    Ok(msg)
}

/// Clone a repo into the clone folder (default ~/code), remember it, then open the item.
#[tauri::command]
async fn clone_and_open(app: AppHandle, item_id: String, checkout: bool) -> CmdResult<String> {
    let (repo, parent) = {
        let state = app.state::<AppState>();
        let repo = db::checkout_info(&state.db.lock().unwrap(), &item_id).map_err(err)?.ok_or("Unknown item")?.1;
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
        db::set_setting(&state.db.lock().unwrap(), "clones", &serde_json::to_string(&found).map_err(err)?).map_err(err)?;
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
    let pr = db::PrRef::load(&state.db.lock().unwrap(), &pr_id).map_err(err)?;
    if let Some(pr) = pr {
        let at = now();
        let _ = db::record_activity(
            &state.db.lock().unwrap(),
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

#[tauri::command]
fn get_activity(state: State<AppState>, days: i64) -> CmdResult<Vec<db::Activity>> {
    let since = derive::fmt_ts(chrono::Utc::now() - chrono::Duration::days(days));
    db::list_activity(&state.db.lock().unwrap(), &since).map_err(err)
}

#[tauri::command]
fn get_repos(state: State<AppState>) -> CmdResult<Vec<db::RepoInfo>> {
    db::list_repos(&state.db.lock().unwrap()).map_err(err)
}

#[tauri::command]
fn set_repo_hidden(app: AppHandle, repo: String, hidden: bool) -> CmdResult<()> {
    db::set_repo_hidden(&app.state::<AppState>().db.lock().unwrap(), &repo, hidden).map_err(err)?;
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

#[tauri::command]
fn get_settings(state: State<AppState>) -> Settings {
    Settings {
        poll_seconds: poll_secs(&state),
        notifications: state.setting("notifications").as_deref() != Some("0"),
        ignored_users: ignored_users(&state),
        digest: state.setting("digest").as_deref() != Some("0"),
        digest_hour: digest_hour(&state),
    }
}

#[tauri::command]
fn update_settings(state: State<AppState>, settings: Settings) -> CmdResult<()> {
    {
        let conn = state.db.lock().unwrap();
        db::set_setting(&conn, "poll_seconds", &settings.poll_seconds.max(30).to_string()).map_err(err)?;
        db::set_setting(&conn, "notifications", if settings.notifications { "1" } else { "0" }).map_err(err)?;
        db::set_setting(&conn, "digest", if settings.digest { "1" } else { "0" }).map_err(err)?;
        db::set_setting(&conn, "digest_hour", &settings.digest_hour.min(23).to_string()).map_err(err)?;
        let ignored = settings.ignored_users.join(",");
        if db::get_setting(&conn, "ignored_users").map_err(err)?.unwrap_or_default() != ignored {
            db::set_setting(&conn, "ignored_users", &ignored).map_err(err)?;
            // Every PR's state may change: force the next sync to re-derive everything.
            db::delete_setting(&conn, "last_full_sync").map_err(err)?;
        }
    }
    state.wake.notify_one();
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
    let conn = state.db.lock().unwrap();
    let _ = db::set_setting(&conn, "last_digest", &today);
    if let Ok(Some(summary)) = sync::digest(&conn) {
        drop(conn);
        notify(app, sync::Notice { title: "☀️ Today in Dev Inbox".into(), body: summary, url: None, item_id: None });
    }
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

#[cfg(all(test, target_os = "macos"))]
mod notify_tests {
    /// Shows a real notification through the same macOS path the app uses (no click wait):
    /// `cargo test notify_smoke -- --ignored`
    #[test]
    #[ignore]
    fn notify_smoke() {
        let _ = mac_notification_sys::set_application("com.apple.Terminal");
        let res = mac_notification_sys::send_notification(
            "🔔 Dev Inbox test",
            Some("acme/web#482 Checkout refactor"),
            "sarah replied in your thread",
            None,
        );
        assert!(res.is_ok(), "{res:?}");
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
                sync_lock: tokio::sync::Mutex::new(()),
                wake: tokio::sync::Notify::new(),
                demo,
            });

            let open = MenuItem::with_id(app, "open", "Open Dev Inbox", true, None::<&str>)?;
            let sync = MenuItem::with_id(app, "sync", "Sync now", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &sync, &PredefinedMenuItem::separator(app)?, &quit])?;
            // macOS: a monochrome template icon that adapts to light/dark menu bars.
            #[cfg(target_os = "macos")]
            let (icon, template) = (tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?, true);
            #[cfg(not(target_os = "macos"))]
            let (icon, template) = (app.default_window_icon().unwrap().clone(), false);
            TrayIconBuilder::with_id("main")
                .icon(icon)
                .icon_as_template(template)
                .tooltip("Dev Inbox")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show_main(app),
                    "sync" => app.state::<AppState>().wake.notify_one(),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                        show_main(tray.app_handle());
                    }
                })
                .build(app)?;

            if let Err(e) = app.global_shortcut().register(palette_shortcut()) {
                eprintln!("couldn't register global shortcut: {e}");
            }
            start_poller(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
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
