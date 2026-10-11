//! Local SQLite store: mirrored PR metadata (no source code), personal states,
//! and the personal activity timeline.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::derive::Derived;
use crate::github::PullRequest;

pub fn open(path: &std::path::Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        CREATE TABLE IF NOT EXISTS prs (
            id TEXT PRIMARY KEY,
            repo TEXT NOT NULL,
            number INTEGER NOT NULL,
            title TEXT NOT NULL,
            url TEXT NOT NULL,
            author TEXT NOT NULL,
            author_avatar TEXT,
            is_draft INTEGER NOT NULL,
            is_mine INTEGER NOT NULL,
            ci TEXT NOT NULL,
            auto_state TEXT NOT NULL,
            auto_reason TEXT NOT NULL,
            since TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            gh_state TEXT NOT NULL,
            review_requested INTEGER NOT NULL DEFAULT 0,
            is_open INTEGER NOT NULL DEFAULT 1,
            last_sync INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS personal (
            pr_id TEXT PRIMARY KEY,
            state TEXT NOT NULL,       -- a manual state, 'later', or 'due' (a later whose time has come)
            note TEXT,
            until TEXT,
            set_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS activity (
            key TEXT PRIMARY KEY,      -- dedupe key, e.g. review:<github id>
            at TEXT NOT NULL,
            kind TEXT NOT NULL,        -- reviewed | commented | opened | merged | closed | marked | update
            pr_id TEXT,
            repo TEXT,
            number INTEGER,
            title TEXT,
            url TEXT,
            detail TEXT
        );
        CREATE INDEX IF NOT EXISTS activity_at ON activity(at);
        CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS hidden_repos (repo TEXT PRIMARY KEY);
        CREATE TABLE IF NOT EXISTS seen (pr_id TEXT PRIMARY KEY, at TEXT NOT NULL);
        "#,
    )?;
    Ok(conn)
}

pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
        .optional()?)
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn delete_setting(conn: &Connection, key: &str) -> Result<()> {
    conn.execute("DELETE FROM settings WHERE key = ?1", [key])?;
    Ok(())
}

/// The previously stored derived state of a PR, used to detect transitions.
pub struct Prev {
    pub auto_state: String,
    pub since: String,
}

pub fn upsert_pr(conn: &Connection, pr: &PullRequest, d: &Derived, sync_id: i64) -> Result<Option<Prev>> {
    let prev = conn
        .query_row("SELECT auto_state, since FROM prs WHERE id = ?1", [&pr.id], |r| {
            Ok(Prev { auto_state: r.get(0)?, since: r.get(1)? })
        })
        .optional()?;
    let author = pr.author.as_ref();
    conn.execute(
        "INSERT INTO prs (id, repo, number, title, url, author, author_avatar, is_draft, is_mine, ci,
                          auto_state, auto_reason, since, created_at, updated_at, gh_state, is_open, last_sync,
                          review_requested)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
         ON CONFLICT(id) DO UPDATE SET
            repo = excluded.repo, number = excluded.number, title = excluded.title, url = excluded.url,
            author = excluded.author, author_avatar = excluded.author_avatar, is_draft = excluded.is_draft,
            is_mine = excluded.is_mine, ci = excluded.ci, auto_state = excluded.auto_state,
            auto_reason = excluded.auto_reason, since = excluded.since, updated_at = excluded.updated_at,
            gh_state = excluded.gh_state, is_open = excluded.is_open, last_sync = excluded.last_sync,
            review_requested = excluded.review_requested",
        params![
            pr.id,
            pr.repository.name_with_owner,
            pr.number,
            pr.title,
            pr.url,
            author.map(|a| a.login.as_str()).unwrap_or("ghost"),
            author.and_then(|a| a.avatar_url.as_deref()),
            pr.is_draft,
            d.is_mine,
            d.ci.as_str(),
            d.state,
            d.reason,
            crate::derive::fmt_ts(d.since),
            pr.created_at,
            pr.updated_at,
            pr.state,
            pr.state == "OPEN",
            sync_id,
            pr.matched_review_requested,
        ],
    )?;
    Ok(prev)
}

/// Whether a search hit differs from what we have stored, so its details need refetching.
pub fn needs_refresh(conn: &Connection, hit: &crate::github::Hit) -> Result<bool> {
    let stored = conn
        .query_row(
            "SELECT updated_at, ci, review_requested, is_open FROM prs WHERE id = ?1",
            [&hit.id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, bool>(2)?, r.get::<_, bool>(3)?)),
        )
        .optional()?;
    Ok(match stored {
        None => true,
        Some((updated_at, ci, rr, open)) => {
            updated_at != hit.updated_at || ci != hit.ci || rr != hit.review_requested || !open
        }
    })
}

/// Mark a PR as still present in this sync without rewriting it.
pub fn touch(conn: &Connection, id: &str, sync_id: i64) -> Result<()> {
    conn.execute("UPDATE prs SET last_sync = ?2 WHERE id = ?1", params![id, sync_id])?;
    Ok(())
}

/// Open PRs we know about that weren't returned by the latest sync.
pub fn stale_open_ids(conn: &Connection, sync_id: i64) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT id FROM prs WHERE is_open = 1 AND last_sync != ?1")?;
    let ids = stmt.query_map([sync_id], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?;
    Ok(ids)
}

pub fn mark_gone(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("UPDATE prs SET is_open = 0 WHERE id = ?1", [id])?;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Personal {
    pub state: String,
    pub note: Option<String>,
    pub until: Option<String>,
    pub set_at: String,
}

pub fn get_personal(conn: &Connection, pr_id: &str) -> Result<Option<Personal>> {
    Ok(conn
        .query_row("SELECT state, note, until, set_at FROM personal WHERE pr_id = ?1", [pr_id], |r| {
            Ok(Personal { state: r.get(0)?, note: r.get(1)?, until: r.get(2)?, set_at: r.get(3)? })
        })
        .optional()?)
}

pub fn set_personal(conn: &Connection, pr_id: &str, p: &Personal) -> Result<()> {
    conn.execute(
        "INSERT INTO personal (pr_id, state, note, until, set_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(pr_id) DO UPDATE SET state = excluded.state, note = excluded.note,
            until = excluded.until, set_at = excluded.set_at",
        params![pr_id, p.state, p.note, p.until, p.set_at],
    )?;
    Ok(())
}

pub fn mark_seen(conn: &Connection, pr_id: &str, at: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO seen (pr_id, at) VALUES (?1, ?2) ON CONFLICT(pr_id) DO UPDATE SET at = excluded.at",
        params![pr_id, at],
    )?;
    Ok(())
}

pub fn clear_personal(conn: &Connection, pr_id: &str) -> Result<()> {
    conn.execute("DELETE FROM personal WHERE pr_id = ?1", [pr_id])?;
    Ok(())
}

/// Snoozes whose time has come.
pub fn due_reminders(conn: &Connection, now: &str) -> Result<Vec<(String, Option<String>)>> {
    let mut stmt = conn.prepare(
        "SELECT pr_id, note FROM personal WHERE state = 'later' AND until IS NOT NULL AND until <= ?1",
    )?;
    let rows = stmt
        .query_map([now], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub struct ActivityRow<'a> {
    pub key: String,
    pub at: &'a str,
    pub kind: &'a str,
    pub pr: Option<&'a PrRef>,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct PrRef {
    pub id: String,
    pub repo: String,
    pub number: i64,
    pub title: String,
    pub url: String,
}

impl PrRef {
    pub fn from_pr(pr: &PullRequest) -> Self {
        Self {
            id: pr.id.clone(),
            repo: pr.repository.name_with_owner.clone(),
            number: pr.number,
            title: pr.title.clone(),
            url: pr.url.clone(),
        }
    }

    pub fn load(conn: &Connection, id: &str) -> Result<Option<Self>> {
        Ok(conn
            .query_row("SELECT id, repo, number, title, url FROM prs WHERE id = ?1", [id], |r| {
                Ok(Self { id: r.get(0)?, repo: r.get(1)?, number: r.get(2)?, title: r.get(3)?, url: r.get(4)? })
            })
            .optional()?)
    }
}

/// Returns true if the row was new.
pub fn record_activity(conn: &Connection, a: &ActivityRow) -> Result<bool> {
    let n = conn.execute(
        "INSERT OR IGNORE INTO activity (key, at, kind, pr_id, repo, number, title, url, detail)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            a.key,
            a.at,
            a.kind,
            a.pr.map(|p| &p.id),
            a.pr.map(|p| &p.repo),
            a.pr.map(|p| p.number),
            a.pr.map(|p| &p.title),
            a.pr.map(|p| &p.url),
            a.detail,
        ],
    )?;
    Ok(n > 0)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    pub at: String,
    pub kind: String,
    pub pr_id: Option<String>,
    pub repo: Option<String>,
    pub number: Option<i64>,
    pub title: Option<String>,
    pub url: Option<String>,
    pub detail: String,
}

pub fn list_activity(conn: &Connection, since: &str) -> Result<Vec<Activity>> {
    let mut stmt = conn.prepare(
        "SELECT at, kind, pr_id, repo, number, title, url, detail FROM activity
         WHERE at >= ?1 ORDER BY at DESC LIMIT 500",
    )?;
    let rows = stmt
        .query_map([since], |r| {
            Ok(Activity {
                at: r.get(0)?,
                kind: r.get(1)?,
                pr_id: r.get(2)?,
                repo: r.get(3)?,
                number: r.get(4)?,
                title: r.get(5)?,
                url: r.get(6)?,
                detail: r.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub repo: String,
    pub number: i64,
    pub title: String,
    pub url: String,
    pub author: String,
    pub author_avatar: Option<String>,
    pub is_draft: bool,
    pub is_mine: bool,
    pub ci: String,
    /// What the app derived from GitHub.
    pub auto_state: String,
    pub reason: String,
    pub since: String,
    pub updated_at: String,
    /// The user's own override, if any.
    pub personal: Option<Personal>,
    /// The state to display: the personal override if set, else the derived one.
    pub state: String,
    pub reminder_due: bool,
    /// Needs you, and arrived since you last opened it in the app.
    pub is_new: bool,
}

pub fn list_items(conn: &Connection) -> Result<Vec<Item>> {
    let mut stmt = conn.prepare(
        "SELECT p.id, p.repo, p.number, p.title, p.url, p.author, p.author_avatar, p.is_draft, p.is_mine, p.ci,
                p.auto_state, p.auto_reason, p.since, p.updated_at,
                s.state, s.note, s.until, s.set_at,
                COALESCE(v.at, (SELECT value FROM settings WHERE key = 'initialized_at'), '')
         FROM prs p LEFT JOIN personal s ON s.pr_id = p.id LEFT JOIN seen v ON v.pr_id = p.id
         WHERE p.is_open = 1 AND p.repo NOT IN (SELECT repo FROM hidden_repos)
         ORDER BY p.since DESC",
    )?;
    let rows = stmt
        .query_map([], |r| {
            let personal: Option<Personal> = r.get::<_, Option<String>>(14)?.map(|state| Personal {
                state,
                note: r.get(15).unwrap_or(None),
                until: r.get(16).unwrap_or(None),
                set_at: r.get(17).unwrap_or_default(),
            });
            let auto_state: String = r.get(10)?;
            let (state, reminder_due) = match personal.as_ref().map(|p| p.state.as_str()) {
                Some("due") => (auto_state.clone(), true),
                Some(s) => (s.to_string(), false),
                None => (auto_state.clone(), false),
            };
            let since: String = r.get(12)?;
            let seen_at: String = r.get(18)?;
            let event_at = if reminder_due { personal.as_ref().map_or(since.clone(), |p| p.set_at.clone()) } else { since.clone() };
            let is_new = (reminder_due || crate::derive::is_actionable(&state)) && event_at > seen_at;
            Ok(Item {
                id: r.get(0)?,
                repo: r.get(1)?,
                number: r.get(2)?,
                title: r.get(3)?,
                url: r.get(4)?,
                author: r.get(5)?,
                author_avatar: r.get(6)?,
                is_draft: r.get(7)?,
                is_mine: r.get(8)?,
                ci: r.get(9)?,
                auto_state,
                reason: r.get(11)?,
                since,
                updated_at: r.get(13)?,
                personal,
                state,
                reminder_due,
                is_new,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoInfo {
    pub repo: String,
    pub open_prs: i64,
    pub hidden: bool,
}

pub fn list_repos(conn: &Connection) -> Result<Vec<RepoInfo>> {
    let mut stmt = conn.prepare(
        "SELECT p.repo, SUM(p.is_open), EXISTS(SELECT 1 FROM hidden_repos h WHERE h.repo = p.repo)
         FROM prs p GROUP BY p.repo ORDER BY p.repo COLLATE NOCASE",
    )?;
    let rows = stmt
        .query_map([], |r| Ok(RepoInfo { repo: r.get(0)?, open_prs: r.get(1)?, hidden: r.get(2)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn set_repo_hidden(conn: &Connection, repo: &str, hidden: bool) -> Result<()> {
    if hidden {
        conn.execute("INSERT OR IGNORE INTO hidden_repos (repo) VALUES (?1)", [repo])?;
    } else {
        conn.execute("DELETE FROM hidden_repos WHERE repo = ?1", [repo])?;
    }
    Ok(())
}

pub fn is_repo_hidden(conn: &Connection, repo: &str) -> Result<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM hidden_repos WHERE repo = ?1", [repo], |_| Ok(()))
        .optional()?
        .is_some())
}

/// Wipes all mirrored and personal data (used on sign-out).
pub fn reset(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DELETE FROM prs; DELETE FROM personal; DELETE FROM activity; DELETE FROM settings; DELETE FROM hidden_repos; DELETE FROM seen;",
    )?;
    Ok(())
}
