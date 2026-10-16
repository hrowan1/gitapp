//! Local SQLite store: mirrored PR/issue/branch metadata (no source code), personal states,
//! the follow list, and the personal activity timeline.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::derive::Derived;
use crate::github::{Issue, PullRequest};

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
    migrate(&conn)?;
    Ok(conn)
}

/// Schema changes for databases created by earlier versions.
fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 1 {
        conn.execute_batch(
            r#"
            BEGIN;
            ALTER TABLE prs ADD COLUMN kind TEXT NOT NULL DEFAULT 'pr';   -- pr | issue | run
            ALTER TABLE personal ADD COLUMN waits_on TEXT;                -- "owner/repo#123" this snooze waits for
            CREATE TABLE IF NOT EXISTS unfollowed (id TEXT PRIMARY KEY, at TEXT NOT NULL);
            -- "Mute" became "stop following".
            INSERT OR IGNORE INTO unfollowed (id, at) SELECT pr_id, set_at FROM personal WHERE state = 'muted';
            DELETE FROM personal WHERE state = 'muted';
            PRAGMA user_version = 1;
            COMMIT;
            "#,
        )?;
    }
    if version < 2 {
        conn.execute_batch(
            r#"
            BEGIN;
            ALTER TABLE prs ADD COLUMN branch TEXT;                          -- PR head branch
            ALTER TABLE prs ADD COLUMN cross_repo INTEGER NOT NULL DEFAULT 0; -- PR from a fork
            ALTER TABLE prs ADD COLUMN context TEXT;                         -- JSON ReplyContext
            PRAGMA user_version = 2;
            COMMIT;
            "#,
        )?;
    }
    if version < 3 {
        conn.execute_batch(
            r#"
            BEGIN;
            ALTER TABLE activity ADD COLUMN branch TEXT;
            -- Local editing activity: a clone's branch/files changed since the last sample.
            CREATE TABLE IF NOT EXISTS work_samples (at TEXT NOT NULL, repo TEXT NOT NULL, branch TEXT);
            CREATE INDEX IF NOT EXISTS work_samples_at ON work_samples(at);
            -- Time blocks the user has corrected; a day with any is "frozen" (no longer auto-generated).
            CREATE TABLE IF NOT EXISTS time_blocks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                day TEXT NOT NULL,
                key TEXT NOT NULL,
                label TEXT NOT NULL,
                start TEXT NOT NULL,
                end TEXT NOT NULL,
                note TEXT
            );
            CREATE INDEX IF NOT EXISTS time_blocks_day ON time_blocks(day);
            CREATE TABLE IF NOT EXISTS frozen_days (day TEXT PRIMARY KEY);
            PRAGMA user_version = 3;
            COMMIT;
            "#,
        )?;
    }
    if version < 4 {
        conn.execute_batch(
            r#"
            BEGIN;
            -- Jira tickets that are or were assigned to the user (read-only mirror).
            CREATE TABLE IF NOT EXISTS jira_issues (
                key TEXT PRIMARY KEY,
                id TEXT NOT NULL,
                summary TEXT NOT NULL,
                status TEXT NOT NULL,
                category TEXT NOT NULL,      -- new | indeterminate | done
                issue_type TEXT NOT NULL,
                priority TEXT,
                project TEXT NOT NULL,
                assignee TEXT,
                assigned_to_me INTEGER NOT NULL DEFAULT 0,
                url TEXT NOT NULL,
                created TEXT,
                updated TEXT,
                resolved TEXT
            );
            -- How they moved: status / assignee / resolution changes, the user's comments, creation.
            CREATE TABLE IF NOT EXISTS jira_events (
                id TEXT PRIMARY KEY,
                issue_key TEXT NOT NULL,
                at TEXT NOT NULL,
                kind TEXT NOT NULL,
                from_value TEXT,
                to_value TEXT,
                actor TEXT,
                actor_is_me INTEGER NOT NULL DEFAULT 0,
                detail TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS jira_events_at ON jira_events(at);
            CREATE INDEX IF NOT EXISTS jira_events_issue ON jira_events(issue_key);
            PRAGMA user_version = 4;
            COMMIT;
            "#,
        )?;
    }
    if version < 5 {
        conn.execute_batch(
            r#"
            BEGIN;
            -- Meetings the user recorded. Audio lives in <app data>/meetings/<id>/ until retention removes it.
            CREATE TABLE IF NOT EXISTS meetings (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                started_at TEXT NOT NULL,
                ended_at TEXT,
                seconds INTEGER,
                status TEXT NOT NULL,          -- recording | transcribing | done | failed
                error TEXT,
                ticket TEXT,                   -- Jira-style key the meeting counts towards
                audio_dir TEXT,
                audio_deleted INTEGER NOT NULL DEFAULT 0,
                sources TEXT                   -- "mic,system"
            );
            CREATE TABLE IF NOT EXISTS meeting_segments (
                meeting_id TEXT NOT NULL,
                start REAL NOT NULL,
                end REAL NOT NULL,
                speaker TEXT NOT NULL,         -- you | others
                text TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS meeting_segments_meeting ON meeting_segments(meeting_id);
            PRAGMA user_version = 5;
            COMMIT;
            "#,
        )?;
    }
    if version < 6 {
        conn.execute_batch(
            r#"
            BEGIN;
            -- When you last looked at a ticket (Today stops showing what came before).
            ALTER TABLE jira_issues ADD COLUMN seen_at TEXT;
            -- Whether a desktop notification went out for this event; existing ones never will.
            ALTER TABLE jira_events ADD COLUMN notified INTEGER NOT NULL DEFAULT 0;
            UPDATE jira_events SET notified = 1;
            PRAGMA user_version = 6;
            COMMIT;
            "#,
        )?;
    }
    if version < 7 {
        conn.execute_batch(
            r#"
            BEGIN;
            -- The ticket's description as plain text (paragraphs and list items on their own lines).
            ALTER TABLE jira_issues ADD COLUMN description TEXT;
            PRAGMA user_version = 7;
            COMMIT;
            "#,
        )?;
    }
    if version < 8 {
        conn.execute_batch(
            r#"
            BEGIN;
            -- AI meeting notes: JSON {overview, decisions, actionItems}, who made them, and progress.
            ALTER TABLE meetings ADD COLUMN summary TEXT;
            ALTER TABLE meetings ADD COLUMN summary_by TEXT;       -- apple | claude
            ALTER TABLE meetings ADD COLUMN summary_status TEXT;   -- summarising | done | failed
            ALTER TABLE meetings ADD COLUMN summary_error TEXT;
            PRAGMA user_version = 8;
            COMMIT;
            "#,
        )?;
    }
    Ok(())
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

/// Anything we track: a PR, an issue, or a failing default branch ("run").
pub struct Tracked {
    pub id: String,
    pub kind: &'static str,
    pub repo: String,
    pub number: i64,
    pub title: String,
    pub url: String,
    pub author: String,
    pub author_avatar: Option<String>,
    pub is_draft: bool,
    pub gh_state: String,
    pub created_at: String,
    pub updated_at: String,
    pub review_requested: bool,
    pub branch: Option<String>,
    pub cross_repo: bool,
}

impl Tracked {
    pub fn from_pr(pr: &PullRequest) -> Self {
        Self {
            id: pr.id.clone(),
            kind: "pr",
            repo: pr.repository.name_with_owner.clone(),
            number: pr.number,
            title: pr.title.clone(),
            url: pr.url.clone(),
            author: pr.author.as_ref().map_or("ghost".into(), |a| a.login.clone()),
            author_avatar: pr.author.as_ref().and_then(|a| a.avatar_url.clone()),
            is_draft: pr.is_draft,
            gh_state: pr.state.clone(),
            created_at: pr.created_at.clone(),
            updated_at: pr.updated_at.clone(),
            review_requested: pr.matched_review_requested,
            branch: pr.head_ref_name.clone(),
            cross_repo: pr.is_cross_repository.unwrap_or(false),
        }
    }

    pub fn from_issue(i: &Issue) -> Self {
        Self {
            id: i.id.clone(),
            kind: "issue",
            repo: i.repository.name_with_owner.clone(),
            number: i.number,
            title: i.title.clone(),
            url: i.url.clone(),
            author: i.author.as_ref().map_or("ghost".into(), |a| a.login.clone()),
            author_avatar: i.author.as_ref().and_then(|a| a.avatar_url.clone()),
            is_draft: false,
            gh_state: i.state.clone(),
            created_at: i.created_at.clone(),
            updated_at: i.updated_at.clone(),
            review_requested: false,
            branch: None,
            cross_repo: false,
        }
    }

    pub fn as_ref(&self) -> PrRef {
        PrRef { id: self.id.clone(), repo: self.repo.clone(), number: self.number, title: self.title.clone(), url: self.url.clone() }
    }
}

pub fn upsert(conn: &Connection, t: &Tracked, d: &Derived, sync_id: i64) -> Result<Option<Prev>> {
    let prev = conn
        .query_row("SELECT auto_state, since FROM prs WHERE id = ?1", [&t.id], |r| {
            Ok(Prev { auto_state: r.get(0)?, since: r.get(1)? })
        })
        .optional()?;
    conn.execute(
        "INSERT INTO prs (id, repo, number, title, url, author, author_avatar, is_draft, is_mine, ci,
                          auto_state, auto_reason, since, created_at, updated_at, gh_state, is_open, last_sync,
                          review_requested, kind, branch, cross_repo, context)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)
         ON CONFLICT(id) DO UPDATE SET
            repo = excluded.repo, number = excluded.number, title = excluded.title, url = excluded.url,
            author = excluded.author, author_avatar = excluded.author_avatar, is_draft = excluded.is_draft,
            is_mine = excluded.is_mine, ci = excluded.ci, auto_state = excluded.auto_state,
            auto_reason = excluded.auto_reason, since = excluded.since, updated_at = excluded.updated_at,
            gh_state = excluded.gh_state, is_open = excluded.is_open, last_sync = excluded.last_sync,
            review_requested = excluded.review_requested, kind = excluded.kind, branch = excluded.branch,
            cross_repo = excluded.cross_repo, context = excluded.context",
        params![
            t.id,
            t.repo,
            t.number,
            t.title,
            t.url,
            t.author,
            t.author_avatar,
            t.is_draft,
            d.is_mine,
            d.ci.as_str(),
            d.state,
            d.reason,
            crate::derive::fmt_ts(d.since),
            t.created_at,
            t.updated_at,
            t.gh_state,
            t.gh_state == "OPEN",
            sync_id,
            t.review_requested,
            t.kind,
            t.branch,
            t.cross_repo,
            d.context.as_ref().and_then(|c| serde_json::to_string(c).ok()),
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
    let mut stmt = conn.prepare("SELECT id FROM prs WHERE is_open = 1 AND last_sync != ?1 AND kind != 'run'")?;
    let ids = stmt.query_map([sync_id], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?;
    Ok(ids)
}

pub fn mark_gone(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("UPDATE prs SET is_open = 0 WHERE id = ?1", [id])?;
    Ok(())
}

/// Ids of failing-branch items currently open.
pub fn open_runs(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT id FROM prs WHERE is_open = 1 AND kind = 'run'")?;
    let ids = stmt.query_map([], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?;
    Ok(ids)
}

/// Repos with recent activity among followed items: where we watch default-branch CI.
pub fn active_repos(conn: &Connection, since: &str, limit: usize) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT repo FROM prs
         WHERE kind != 'run' AND updated_at >= ?1
           AND repo NOT IN (SELECT repo FROM hidden_repos)
           AND id NOT IN (SELECT id FROM unfollowed)
         GROUP BY repo ORDER BY MAX(updated_at) DESC LIMIT ?2",
    )?;
    let repos = stmt.query_map(params![since, limit as i64], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?;
    Ok(repos)
}

/// (key "owner/repo#n", title) for every open PR/issue, used to match "after the API migration" to a PR.
pub fn open_titles(conn: &Connection) -> Result<Vec<(String, String, String)>> {
    let mut stmt = conn.prepare("SELECT id, repo || '#' || number, title FROM prs WHERE is_open = 1 AND kind != 'run'")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Personal {
    pub state: String,
    pub note: Option<String>,
    pub until: Option<String>,
    pub set_at: String,
    /// "owner/repo#123": come back when that PR/issue merges or closes.
    pub waits_on: Option<String>,
}

pub fn get_personal(conn: &Connection, pr_id: &str) -> Result<Option<Personal>> {
    Ok(conn
        .query_row("SELECT state, note, until, set_at, waits_on FROM personal WHERE pr_id = ?1", [pr_id], |r| {
            Ok(Personal { state: r.get(0)?, note: r.get(1)?, until: r.get(2)?, set_at: r.get(3)?, waits_on: r.get(4)? })
        })
        .optional()?)
}

pub fn set_personal(conn: &Connection, pr_id: &str, p: &Personal) -> Result<()> {
    conn.execute(
        "INSERT INTO personal (pr_id, state, note, until, set_at, waits_on) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(pr_id) DO UPDATE SET state = excluded.state, note = excluded.note,
            until = excluded.until, set_at = excluded.set_at, waits_on = excluded.waits_on",
        params![pr_id, p.state, p.note, p.until, p.set_at, p.waits_on],
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

/// Snoozes waiting on another PR/issue: (pr_id, waits_on, note).
pub fn waiting_on_refs(conn: &Connection) -> Result<Vec<(String, String, Option<String>)>> {
    let mut stmt = conn.prepare("SELECT pr_id, waits_on, note FROM personal WHERE state = 'later' AND waits_on IS NOT NULL")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
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
    pub fn load(conn: &Connection, id: &str) -> Result<Option<Self>> {
        Ok(conn
            .query_row("SELECT id, repo, number, title, url FROM prs WHERE id = ?1", [id], |r| {
                Ok(Self { id: r.get(0)?, repo: r.get(1)?, number: r.get(2)?, title: r.get(3)?, url: r.get(4)? })
            })
            .optional()?)
    }
}

pub fn activity_exists(conn: &Connection, key: &str) -> Result<bool> {
    Ok(conn.query_row("SELECT 1 FROM activity WHERE key = ?1", [key], |_| Ok(())).optional()?.is_some())
}

/// Like `record_activity`, also noting the branch (pushes, commits).
pub fn record_activity_on_branch(conn: &Connection, a: &ActivityRow, branch: &str) -> Result<bool> {
    let new = record_activity(conn, a)?;
    if new {
        conn.execute("UPDATE activity SET branch = ?2 WHERE key = ?1", params![a.key, branch])?;
    }
    Ok(new)
}

/// A push with no tracked PR behind it: stored with just the repo and branch.
pub fn record_push(conn: &Connection, id: &str, at: &str, repo: &str, branch: &str, url: &str) -> Result<bool> {
    let n = conn.execute(
        "INSERT OR IGNORE INTO activity (key, at, kind, repo, title, url, detail, branch)
         VALUES (?1, ?2, 'pushed', ?3, ?4, ?5, ?6, ?4)",
        params![format!("push:{id}"), at, repo, branch, url, format!("Pushed to {branch}")],
    )?;
    Ok(n > 0)
}

pub fn record_sample(conn: &Connection, at: &str, repo: &str, branch: Option<&str>) -> Result<()> {
    conn.execute("INSERT INTO work_samples (at, repo, branch) VALUES (?1, ?2, ?3)", params![at, repo, branch])?;
    Ok(())
}

/// (open or closed) PR id for a repo + branch, used to tie pushes and local edits to a PR.
pub fn pr_for_branch(conn: &Connection, repo: &str, branch: &str) -> Result<Option<PrRef>> {
    Ok(conn
        .query_row(
            "SELECT id, repo, number, title, url FROM prs
             WHERE kind = 'pr' AND lower(repo) = lower(?1) AND branch = ?2 ORDER BY updated_at DESC LIMIT 1",
            params![repo, branch],
            |r| Ok(PrRef { id: r.get(0)?, repo: r.get(1)?, number: r.get(2)?, title: r.get(3)?, url: r.get(4)? }),
        )
        .optional()?)
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
    /// The item is the user's own PR/issue.
    pub is_mine: bool,
    pub item_kind: Option<String>,
}

/// What's needed to open a PR locally: (kind, repo, number, branch, cross_repo).
pub fn checkout_info(conn: &Connection, id: &str) -> Result<Option<(String, String, i64, Option<String>, bool)>> {
    Ok(conn
        .query_row("SELECT kind, repo, number, branch, cross_repo FROM prs WHERE id = ?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .optional()?)
}

/// The timeline of one item, newest first.
pub fn item_activity(conn: &Connection, pr_id: &str, limit: i64) -> Result<Vec<Activity>> {
    let mut stmt = conn.prepare(
        "SELECT a.at, a.kind, a.pr_id, a.repo, a.number, a.title, a.url, a.detail, COALESCE(p.is_mine, 0), p.kind
         FROM activity a LEFT JOIN prs p ON p.id = a.pr_id
         WHERE a.pr_id = ?1 ORDER BY a.at DESC LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![pr_id, limit], |r| {
            Ok(Activity {
                at: r.get(0)?,
                kind: r.get(1)?,
                pr_id: r.get(2)?,
                repo: r.get(3)?,
                number: r.get(4)?,
                title: r.get(5)?,
                url: r.get(6)?,
                detail: r.get(7)?,
                is_mine: r.get(8)?,
                item_kind: r.get(9)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn list_activity(conn: &Connection, since: &str) -> Result<Vec<Activity>> {
    let mut stmt = conn.prepare(
        "SELECT a.at, a.kind, a.pr_id, a.repo, a.number, a.title, a.url, a.detail, COALESCE(p.is_mine, 0), p.kind
         FROM activity a LEFT JOIN prs p ON p.id = a.pr_id
         WHERE a.at >= ?1 ORDER BY a.at DESC LIMIT 2000",
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
                is_mine: r.get(8)?,
                item_kind: r.get(9)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    /// pr | issue | run
    pub kind: String,
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
    pub branch: Option<String>,
    /// What the latest reply said (for "Replied to you").
    pub context: Option<crate::derive::ReplyContext>,
}

pub fn list_items(conn: &Connection) -> Result<Vec<Item>> {
    let mut stmt = conn.prepare(
        "SELECT p.id, p.repo, p.number, p.title, p.url, p.author, p.author_avatar, p.is_draft, p.is_mine, p.ci,
                p.auto_state, p.auto_reason, p.since, p.updated_at,
                s.state, s.note, s.until, s.set_at,
                COALESCE(v.at, (SELECT value FROM settings WHERE key = 'initialized_at'), ''),
                p.kind, s.waits_on, p.branch, p.context
         FROM prs p LEFT JOIN personal s ON s.pr_id = p.id LEFT JOIN seen v ON v.pr_id = p.id
         WHERE p.is_open = 1 AND p.repo NOT IN (SELECT repo FROM hidden_repos)
           AND p.id NOT IN (SELECT id FROM unfollowed)
         ORDER BY p.since DESC",
    )?;
    let rows = stmt
        .query_map([], |r| {
            let personal: Option<Personal> = r.get::<_, Option<String>>(14)?.map(|state| Personal {
                state,
                note: r.get(15).unwrap_or(None),
                until: r.get(16).unwrap_or(None),
                set_at: r.get(17).unwrap_or_default(),
                waits_on: r.get(20).unwrap_or(None),
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
                kind: r.get(19)?,
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
                branch: r.get(21)?,
                context: r.get::<_, Option<String>>(22)?.and_then(|c| serde_json::from_str(&c).ok()),
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FollowItem {
    pub id: String,
    pub kind: String,
    pub repo: String,
    pub number: i64,
    pub title: String,
    pub author: String,
    pub is_mine: bool,
    pub state: String,
    pub updated_at: String,
    pub followed: bool,
}

/// Every open item, followed or not, for the Following screen.
pub fn list_following(conn: &Connection) -> Result<Vec<FollowItem>> {
    let mut stmt = conn.prepare(
        "SELECT p.id, p.kind, p.repo, p.number, p.title, p.author, p.is_mine, p.auto_state, p.updated_at,
                NOT EXISTS(SELECT 1 FROM unfollowed u WHERE u.id = p.id)
         FROM prs p WHERE p.is_open = 1 AND p.repo NOT IN (SELECT repo FROM hidden_repos)
         ORDER BY p.repo COLLATE NOCASE, p.updated_at DESC",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(FollowItem {
                id: r.get(0)?,
                kind: r.get(1)?,
                repo: r.get(2)?,
                number: r.get(3)?,
                title: r.get(4)?,
                author: r.get(5)?,
                is_mine: r.get(6)?,
                state: r.get(7)?,
                updated_at: r.get(8)?,
                followed: r.get(9)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn set_followed(conn: &Connection, id: &str, followed: bool, at: &str) -> Result<()> {
    if followed {
        conn.execute("DELETE FROM unfollowed WHERE id = ?1", [id])?;
    } else {
        conn.execute("INSERT OR IGNORE INTO unfollowed (id, at) VALUES (?1, ?2)", params![id, at])?;
    }
    Ok(())
}

pub fn is_followed(conn: &Connection, id: &str) -> Result<bool> {
    Ok(conn.query_row("SELECT 1 FROM unfollowed WHERE id = ?1", [id], |_| Ok(())).optional()?.is_none())
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
        "DELETE FROM prs; DELETE FROM personal; DELETE FROM activity; DELETE FROM settings; DELETE FROM hidden_repos; DELETE FROM seen; DELETE FROM unfollowed; DELETE FROM work_samples; DELETE FROM time_blocks; DELETE FROM frozen_days; DELETE FROM jira_issues; DELETE FROM jira_events; DELETE FROM meetings; DELETE FROM meeting_segments;",
    )?;
    Ok(())
}

#[cfg(test)]
mod migration_tests {
    /// Opens a copy of a real database with the current schema code:
    /// `DEV_INBOX_DB_COPY=/path/to/copy.sqlite3 cargo test migrate_copy -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn migrate_copy() {
        let path = std::env::var("DEV_INBOX_DB_COPY").expect("DEV_INBOX_DB_COPY");
        let conn = super::open(std::path::Path::new(&path)).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        let items = super::list_items(&conn).unwrap();
        let following = super::list_following(&conn).unwrap();
        println!("user_version={v} items={} following={} kinds={:?}", items.len(), following.len(),
            items.iter().map(|i| i.kind.as_str()).collect::<std::collections::BTreeSet<_>>());
        // Opening twice must be a no-op.
        drop(conn);
        super::open(std::path::Path::new(&path)).unwrap();
    }
}
