//! One sync pass: fetch from GitHub, derive states, update memory, and work out what to notify.

use anyhow::Result;
use chrono::Utc;
use rusqlite::Connection;
use std::sync::Mutex;

use crate::db::{self, ActivityRow, Personal, PrRef};
use crate::derive::{self, fmt_ts, is_actionable, parse_ts};
use crate::github::{Client, PullRequest};

const FULL_REFRESH_MINUTES: i64 = 30;

pub struct Notice {
    pub title: String,
    pub body: String,
}

pub struct Outcome {
    pub notices: Vec<Notice>,
    pub needs_you: usize,
}

fn notice_title(state: &str) -> &'static str {
    match state {
        derive::NEEDS_REVIEW => "👀 Review requested",
        derive::NEEDS_REREVIEW => "🔄 Needs re-review",
        derive::REPLIED => "💬 New reply",
        derive::NEEDS_CHANGES => "🔧 Your PR needs attention",
        derive::READY_TO_MERGE => "✅ Ready to merge",
        _ => "Dev Inbox",
    }
}

pub fn state_label(state: &str) -> &'static str {
    match state {
        derive::NEEDS_REVIEW => "Need to review",
        derive::NEEDS_REREVIEW => "Needs re-review",
        derive::NEEDS_CHANGES => "I need to make changes",
        derive::WAITING => "Waiting for response",
        derive::WAITING_CI => "Waiting for CI",
        derive::LATER => "Come back later",
        derive::DONE => "Done",
        derive::REPLIED => "Replied",
        derive::READY_TO_MERGE => "Ready to merge",
        derive::DRAFT => "Draft",
        derive::STALE => "Gone quiet",
        derive::MUTED => "Muted",
        _ => "FYI",
    }
}

/// Record the user's own GitHub actions into the personal timeline.
fn record_my_actions(conn: &Connection, pr: &PullRequest, r: &PrRef, me: &str) -> Result<()> {
    let mine = |a: &Option<crate::github::Actor>| a.as_ref().is_some_and(|a| a.login.eq_ignore_ascii_case(me));
    for rev in pr.reviews.iter().filter(|rev| rev.state != "PENDING" && mine(&rev.author)) {
        let Some(at) = rev.submitted_at.as_deref() else { continue };
        let detail = match rev.state.as_str() {
            "APPROVED" => "Approved",
            "CHANGES_REQUESTED" => "Requested changes",
            "DISMISSED" => "Review dismissed",
            _ => "Reviewed",
        };
        db::record_activity(
            conn,
            &ActivityRow { key: format!("review:{}", rev.id), at, kind: "reviewed", pr: Some(r), detail: detail.into() },
        )?;
    }
    for c in pr.comments.iter().filter(|c| mine(&c.author)) {
        db::record_activity(
            conn,
            &ActivityRow {
                key: format!("comment:{}", c.id),
                at: &c.created_at,
                kind: "commented",
                pr: Some(r),
                detail: "Commented".into(),
            },
        )?;
    }
    if mine(&pr.author) {
        db::record_activity(
            conn,
            &ActivityRow {
                key: format!("opened:{}", pr.id),
                at: &pr.created_at,
                kind: "opened",
                pr: Some(r),
                detail: "Opened".into(),
            },
        )?;
    }
    Ok(())
}

pub async fn run(client: &Client, db_lock: &Mutex<Connection>, ctx: &derive::Ctx) -> Result<Outcome> {
    let me = ctx.me.as_str();
    let now = Utc::now();
    let now_s = fmt_ts(now);
    let sync_id = now.timestamp_millis();
    let mut notices = Vec::new();

    // Phase 1: cheap search, then full details only for PRs that changed
    // (plus a periodic full refresh as a safety net).
    let hits = client.involved_hits().await?;
    let (to_fetch, full) = {
        let conn = db_lock.lock().unwrap();
        let full = db::get_setting(&conn, "last_full_sync")?
            .and_then(|s| parse_ts(&s))
            .map_or(true, |t| now - t > chrono::Duration::minutes(FULL_REFRESH_MINUTES));
        let mut ids = Vec::new();
        for h in &hits {
            if full || db::needs_refresh(&conn, h)? {
                ids.push(h.id.clone());
            }
        }
        (ids, full)
    };
    let mut prs = if to_fetch.is_empty() { Vec::new() } else { client.prs_by_id(&to_fetch).await? };
    let requested: std::collections::HashSet<&str> =
        hits.iter().filter(|h| h.review_requested).map(|h| h.id.as_str()).collect();
    for pr in &mut prs {
        pr.matched_review_requested = requested.contains(pr.id.as_str());
    }

    let stale = {
        let conn = db_lock.lock().unwrap();
        let first_sync = db::get_setting(&conn, "initialized")?.is_none();
        let tx = conn.unchecked_transaction()?;
        for h in &hits {
            db::touch(&tx, &h.id, sync_id)?;
        }
        for pr in &prs {
            let d = derive::derive(pr, ctx);
            let prev = db::upsert_pr(&tx, pr, &d, sync_id)?;
            let r = PrRef::from_pr(pr);
            record_my_actions(&tx, pr, &r, me)?;

            let since = fmt_ts(d.since);
            let changed = prev.as_ref().map_or(true, |p| p.auto_state != d.state || p.since != since);
            if !changed || !is_actionable(d.state) {
                continue;
            }
            // Something new needs the user. Log it, and let it override stale personal state.
            let mut resurfaced_from = None;
            let mut snoozed = false;
            if let Some(p) = db::get_personal(&tx, &pr.id)? {
                let timed_snooze = p.state == derive::LATER && p.until.is_some();
                let newer = parse_ts(&p.set_at).map_or(true, |set| d.since > set);
                let muted = p.state == derive::MUTED;
                if timed_snooze || muted || p.state == "due" {
                    snoozed = timed_snooze || muted;
                } else if newer {
                    db::clear_personal(&tx, &pr.id)?;
                    resurfaced_from = Some(p.state);
                } else {
                    snoozed = true;
                }
            }
            if prev.is_some() {
                let detail = match &resurfaced_from {
                    Some(from) => format!("{} (was “{}”)", d.reason, state_label(from)),
                    None => d.reason.clone(),
                };
                db::record_activity(
                    &tx,
                    &ActivityRow { key: format!("update:{}:{}:{since}", pr.id, d.state), at: &since, kind: "update", pr: Some(&r), detail },
                )?;
            }
            if !first_sync && !snoozed && !db::is_repo_hidden(&tx, &r.repo)? {
                notices.push(Notice {
                    title: notice_title(d.state).into(),
                    body: format!("{}#{} {}\n{}", r.repo, r.number, r.title, d.reason),
                });
            }
        }
        tx.commit()?;
        db::stale_open_ids(&conn, sync_id)?
    };

    // PRs that dropped out of search: find out whether they merged/closed or just stopped involving us.
    let refreshed = if stale.is_empty() { Vec::new() } else { client.prs_by_id(&stale).await? };

    let conn = db_lock.lock().unwrap();
    for id in &stale {
        db::mark_gone(&conn, id)?;
    }
    for pr in &refreshed {
        if pr.state == "OPEN" {
            continue;
        }
        let r = PrRef::from_pr(pr);
        let (kind, at) = if pr.state == "MERGED" {
            ("merged", pr.merged_at.as_deref())
        } else {
            ("closed", pr.closed_at.as_deref())
        };
        let at = at.unwrap_or(&now_s);
        let detail = if kind == "merged" { "Merged" } else { "Closed" };
        db::record_activity(&conn, &ActivityRow { key: format!("{kind}:{}", pr.id), at, kind, pr: Some(&r), detail: detail.into() })?;
        db::clear_personal(&conn, &pr.id)?;
    }

    // Snoozes whose time has come.
    for (pr_id, note) in db::due_reminders(&conn, &now_s)? {
        let Some(r) = PrRef::load(&conn, &pr_id)? else { continue };
        db::set_personal(
            &conn,
            &pr_id,
            &Personal { state: "due".into(), note: note.clone(), until: None, set_at: now_s.clone() },
        )?;
        let detail = match &note {
            Some(n) if !n.is_empty() => format!("Reminder: {n}"),
            _ => "Reminder due".into(),
        };
        db::record_activity(
            &conn,
            &ActivityRow { key: format!("due:{pr_id}:{now_s}"), at: &now_s, kind: "update", pr: Some(&r), detail: detail.clone() },
        )?;
        notices.push(Notice { title: "⏰ Come back to this".into(), body: format!("{}#{} {}\n{detail}", r.repo, r.number, r.title) });
    }

    db::set_setting(&conn, "initialized", "1")?;
    // Baseline for "new since you last looked": anything older than the first sync counts as seen.
    if db::get_setting(&conn, "initialized_at")?.is_none() {
        db::set_setting(&conn, "initialized_at", &now_s)?;
    }
    if full {
        db::set_setting(&conn, "last_full_sync", &now_s)?;
    }
    db::set_setting(&conn, "last_sync", &now_s)?;
    db::delete_setting(&conn, "last_error")?;

    let needs_you = db::list_items(&conn)?
        .iter()
        .filter(|i| i.reminder_due || is_actionable(&i.state))
        .count();

    // Don't spam: collapse bursts into one notification.
    if notices.len() > 3 {
        let n = notices.len();
        notices = vec![Notice { title: "Dev Inbox".into(), body: format!("{n} things changed that need your attention") }];
    }
    Ok(Outcome { notices, needs_you })
}

/// The once-a-day summary: what needs you, by kind, plus reminders due today.
pub fn digest(conn: &Connection) -> Result<Option<String>> {
    let items = db::list_items(conn)?;
    let count = |f: &dyn Fn(&db::Item) -> bool| items.iter().filter(|i| f(i)).count();
    let reviews = count(&|i| matches!(i.state.as_str(), derive::NEEDS_REVIEW | derive::NEEDS_REREVIEW));
    let replies = count(&|i| i.state == derive::REPLIED);
    let fixes = count(&|i| i.state == derive::NEEDS_CHANGES);
    let merge = count(&|i| i.state == derive::READY_TO_MERGE);
    let due = count(&|i| i.reminder_due);
    let end_of_day = fmt_ts(Utc::now() + chrono::Duration::hours(16));
    let later_today = count(&|i| {
        i.personal.as_ref().is_some_and(|p| p.state == derive::LATER && p.until.as_deref().is_some_and(|u| u <= end_of_day.as_str()))
    });
    let mut parts = Vec::new();
    let plural = |n: usize, one: &str, many: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {many}") };
    if reviews > 0 { parts.push(plural(reviews, "review", "reviews")); }
    if replies > 0 { parts.push(plural(replies, "reply", "replies")); }
    if fixes > 0 { parts.push(plural(fixes, "PR to fix", "PRs to fix")); }
    if merge > 0 { parts.push(plural(merge, "ready to merge", "ready to merge")); }
    if due + later_today > 0 { parts.push(plural(due + later_today, "reminder", "reminders")); }
    if parts.is_empty() {
        return Ok(None);
    }
    Ok(Some(parts.join(" · ")))
}

#[cfg(test)]
mod live_tests {
    /// End-to-end sync into a throwaway database (read-only against GitHub):
    /// `GITHUB_TOKEN=$(gh auth token) cargo test live_sync -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_sync() {
        let token = std::env::var("GITHUB_TOKEN").expect("GITHUB_TOKEN");
        let client = crate::github::Client::new(&token);
        let me = client.viewer().await.unwrap().login;
        let ctx = crate::derive::Ctx::new(&me, &[]);
        let path = std::env::temp_dir().join(format!("dev-inbox-test-{}.sqlite3", std::process::id()));
        let db = std::sync::Mutex::new(crate::db::open(&path).unwrap());
        let t = std::time::Instant::now();
        let first = super::run(&client, &db, &ctx).await.unwrap();
        println!("first sync: needs_you={} notices={} in {:?}", first.needs_you, first.notices.len(), t.elapsed());
        let t = std::time::Instant::now();
        let second = super::run(&client, &db, &ctx).await.unwrap();
        println!("second sync: needs_you={} notices={} in {:?}", second.needs_you, second.notices.len(), t.elapsed());
        let conn = db.lock().unwrap();
        let items = crate::db::list_items(&conn).unwrap();
        let mut counts = std::collections::BTreeMap::new();
        for i in &items {
            *counts.entry(i.state.clone()).or_insert(0) += 1;
        }
        println!("items={} by state={counts:?}", items.len());
        for i in items.iter().filter(|i| crate::derive::is_actionable(&i.state)) {
            println!("  {:<15} {:<10} {} {}#{} {}", i.state, &i.since[..10], if i.is_mine {"mine"} else {&i.author}, i.repo, i.number, i.reason);
        }
        println!("activity rows={}", crate::db::list_activity(&conn, "2000-01-01T00:00:00Z").unwrap().len());
        drop(conn);
        let _ = std::fs::remove_file(path);
    }
}
