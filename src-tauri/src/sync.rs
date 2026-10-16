//! One sync pass: fetch from GitHub, derive states, update memory, and work out what to notify.

use anyhow::Result;
use chrono::Utc;
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use crate::db::{self, ActivityRow, Personal, PrRef, Tracked};
use crate::derive::{self, fmt_ts, is_actionable, parse_ts, Ci, Derived};
use crate::github::{Client, Issue, PullRequest};
use crate::intent;

const FULL_REFRESH_MINUTES: i64 = 30;
/// How often default-branch CI is checked, and how far back a repo counts as "active".
const BRANCH_CHECK_MINUTES: i64 = 5;
const ACTIVE_REPO_DAYS: i64 = 14;
const MAX_WATCHED_BRANCHES: usize = 25;
/// A branch that's been red longer than this is ambient noise, not news.
const STALE_BRANCH_DAYS: i64 = 7;
/// A "come back to this" in a comment with no time or reference resurfaces after this many days.
const DEFAULT_INTENT_DAYS: i64 = 3;

pub struct Notice {
    pub title: String,
    pub body: String,
    /// Opened when the notification is clicked.
    pub url: Option<String>,
    pub item_id: Option<String>,
}

pub struct Outcome {
    pub notices: Vec<Notice>,
    pub needs_you: usize,
}

fn notice_title(state: &str, kind: &str) -> &'static str {
    match (state, kind) {
        (derive::NEEDS_REVIEW, _) => "👀 Review requested",
        (derive::NEEDS_REREVIEW, _) => "🔄 Needs re-review",
        (derive::REPLIED, _) => "💬 New reply",
        (derive::NEEDS_CHANGES, "run") => "🔴 You broke the build",
        (derive::NEEDS_CHANGES, _) => "🔧 Your PR needs attention",
        (derive::READY_TO_MERGE, _) => "✅ Ready to merge",
        (derive::ASSIGNED, _) => "📋 Assigned to you",
        (derive::BROKEN, _) => "🔴 Branch failing",
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
        derive::ASSIGNED => "Assigned to you",
        derive::BROKEN => "Branch failing",
        _ => "FYI",
    }
}

fn label(r: &PrRef) -> String {
    if r.number > 0 {
        format!("{}#{} {}", r.repo, r.number, r.title)
    } else {
        format!("{} · {}", r.repo, r.title)
    }
}

fn is_me(a: &Option<crate::github::Actor>, me: &str) -> bool {
    a.as_ref().is_some_and(|a| a.login.eq_ignore_ascii_case(me))
}

/// Record the user's own GitHub actions into the personal timeline.
fn record_my_pr_actions(conn: &Connection, pr: &PullRequest, r: &PrRef, me: &str) -> Result<()> {
    for rev in pr.reviews.iter().filter(|rev| rev.state != "PENDING" && is_me(&rev.author, me)) {
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
    for c in pr.comments.iter().filter(|c| is_me(&c.author, me)) {
        db::record_activity(
            conn,
            &ActivityRow { key: format!("comment:{}", c.id), at: &c.created_at, kind: "commented", pr: Some(r), detail: "Commented".into() },
        )?;
    }
    if is_me(&pr.author, me) {
        db::record_activity(
            conn,
            &ActivityRow { key: format!("opened:{}", pr.id), at: &pr.created_at, kind: "opened", pr: Some(r), detail: "Opened".into() },
        )?;
    }
    // Your commits, with when you actually wrote them (authored date survives rebases).
    let branch = pr.head_ref_name.as_deref().unwrap_or_default();
    for c in pr.recent_commits.iter().map(|c| &c.commit) {
        if !c.author.as_ref().and_then(|a| a.user.as_ref()).is_some_and(|u| u.login.eq_ignore_ascii_case(me)) {
            continue;
        }
        db::record_activity_on_branch(
            conn,
            &ActivityRow {
                key: format!("commit:{}", c.oid),
                at: &c.authored_date,
                kind: "committed",
                pr: Some(r),
                detail: c.message_headline.clone(),
            },
            branch,
        )?;
    }
    Ok(())
}

fn record_my_issue_actions(conn: &Connection, issue: &Issue, r: &PrRef, me: &str) -> Result<()> {
    for c in issue.comments.iter().filter(|c| is_me(&c.author, me)) {
        db::record_activity(
            conn,
            &ActivityRow { key: format!("comment:{}", c.id), at: &c.created_at, kind: "commented", pr: Some(r), detail: "Commented on issue".into() },
        )?;
    }
    if is_me(&issue.author, me) {
        db::record_activity(
            conn,
            &ActivityRow { key: format!("opened:{}", issue.id), at: &issue.created_at, kind: "opened", pr: Some(r), detail: "Opened issue".into() },
        )?;
    }
    Ok(())
}

/// The user's own words on an item: (id, created_at, body). Scanned for "come back to this" intent.
fn my_pr_texts(pr: &PullRequest, me: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for c in pr.comments.iter().filter(|c| is_me(&c.author, me)) {
        out.push((c.id.clone(), c.created_at.clone(), c.body.clone()));
    }
    for r in pr.reviews.iter().filter(|r| is_me(&r.author, me) && !r.body.is_empty()) {
        if let Some(at) = &r.submitted_at {
            out.push((r.id.clone(), at.clone(), r.body.clone()));
        }
    }
    for t in pr.review_threads.iter() {
        for c in t.comments.iter().filter(|c| is_me(&c.author, me)) {
            out.push((c.id.clone(), c.created_at.clone(), c.body.clone()));
        }
    }
    out
}

fn my_issue_texts(issue: &Issue, me: &str) -> Vec<(String, String, String)> {
    issue
        .comments
        .iter()
        .filter(|c| is_me(&c.author, me))
        .map(|c| (c.id.clone(), c.created_at.clone(), c.body.clone()))
        .chain(is_me(&issue.author, me).then(|| (format!("{}:body", issue.id), issue.created_at.clone(), String::new())))
        .filter(|(_, _, b)| !b.is_empty())
        .collect()
}

/// State shared across one sync pass.
struct Pass<'a> {
    sync_id: i64,
    now: chrono::DateTime<Utc>,
    first_sync: bool,
    /// Comments after this time can create reminders (nothing from before the app was installed).
    baseline: Option<String>,
    titles: Vec<(String, String, String)>,
    notices: &'a mut Vec<Notice>,
}

impl Pass<'_> {
    fn notify(&mut self, conn: &Connection, t: &PrRef, title: &str, detail: &str) -> Result<()> {
        if self.first_sync || db::is_repo_hidden(conn, &t.repo)? || !db::is_followed(conn, &t.id)? {
            return Ok(());
        }
        self.notices.push(Notice {
            title: title.into(),
            body: format!("{}\n{detail}", label(t)),
            url: Some(t.url.clone()),
            item_id: Some(t.id.clone()),
        });
        Ok(())
    }

    /// Store one item and react to what changed: log it, resurface stale overrides, notify.
    fn process(&mut self, conn: &Connection, t: &Tracked, d: &Derived) -> Result<()> {
        let prev = db::upsert(conn, t, d, self.sync_id)?;
        let since = fmt_ts(d.since);
        let changed = prev.as_ref().map_or(true, |p| p.auto_state != d.state || p.since != since);
        if !changed || !is_actionable(d.state) {
            return Ok(());
        }
        let r = t.as_ref();
        let mut resurfaced_from = None;
        let mut snoozed = false;
        if let Some(p) = db::get_personal(conn, &t.id)? {
            let timed_or_waiting = p.state == derive::LATER && (p.until.is_some() || p.waits_on.is_some());
            let newer = parse_ts(&p.set_at).map_or(true, |set| d.since > set);
            if timed_or_waiting || p.state == "due" {
                snoozed = timed_or_waiting;
            } else if newer {
                db::clear_personal(conn, &t.id)?;
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
                conn,
                &ActivityRow { key: format!("update:{}:{}:{since}", t.id, d.state), at: &since, kind: "update", pr: Some(&r), detail },
            )?;
        }
        if !snoozed {
            self.notify(conn, &r, notice_title(d.state, t.kind), &d.reason)?;
        }
        Ok(())
    }

    /// Turn "let's come back to this after X" in the user's own comments into reminders.
    fn intents(&mut self, conn: &Connection, t: &Tracked, texts: &[(String, String, String)]) -> Result<()> {
        let Some(baseline) = self.baseline.clone() else { return Ok(()) };
        for (id, at, body) in texts.iter().filter(|(_, at, _)| *at > baseline) {
            let key = format!("intent:{id}");
            if db::activity_exists(conn, &key)? {
                continue;
            }
            let Some(at_ts) = parse_ts(at) else { continue };
            let Some(found) = intent::detect(body, &t.repo, at_ts) else { continue };
            let r = t.as_ref();
            // The user has decided something about this item since writing the comment: leave it.
            if db::get_personal(conn, &t.id)?.is_some_and(|p| p.set_at.as_str() > at.as_str()) {
                db::record_activity(conn, &ActivityRow { key, at, kind: "marked", pr: Some(&r), detail: "Saw a “come back to this” in your comment (already handled)".into() })?;
                continue;
            }
            let waits_on = found
                .waits_on
                .clone()
                .or_else(|| found.condition.as_deref().and_then(|c| intent::match_condition(c, &self.titles, &t.id)));
            let until = found.until.or_else(|| {
                waits_on.is_none().then(|| at_ts + chrono::Duration::days(DEFAULT_INTENT_DAYS))
            });
            let when = match (&waits_on, until) {
                (Some(w), _) => format!("when {w} merges or closes"),
                (None, Some(u)) => format!("on {}", u.with_timezone(&chrono::Local).format("%a %-d %b, %H:%M")),
                (None, None) => "when something changes".into(),
            };
            db::set_personal(
                conn,
                &t.id,
                &Personal {
                    state: derive::LATER.into(),
                    note: Some(format!("“{}”", found.note)),
                    until: until.map(fmt_ts),
                    set_at: fmt_ts(self.now),
                    waits_on: waits_on.clone(),
                },
            )?;
            db::record_activity(
                conn,
                &ActivityRow { key, at, kind: "marked", pr: Some(&r), detail: format!("Remembered from your comment — back {when}") },
            )?;
            self.notify(conn, &r, "📌 Noted from your comment", &format!("Back {when}"))?;
        }
        Ok(())
    }
}

pub async fn run(client: &Client, db_lock: &Mutex<Connection>, ctx: &derive::Ctx) -> Result<Outcome> {
    let me = ctx.me.as_str();
    let now = Utc::now();
    let now_s = fmt_ts(now);
    let sync_id = now.timestamp_millis();
    let mut notices = Vec::new();

    // Phase 1: cheap search, then full details only for items that changed
    // (plus a periodic full refresh as a safety net).
    let hits = client.involved_hits().await?;
    // Pushes age out of GitHub's event feed quickly, so capture them every sync. Not fatal if it fails.
    let pushes = client.recent_pushes(me).await.unwrap_or_default();
    let (to_fetch, full, check_branches, first_branch_check, active) = {
        let conn = db_lock.lock().unwrap();
        let older_than = |key: &str, minutes: i64| -> Result<bool> {
            Ok(db::get_setting(&conn, key)?.and_then(|s| parse_ts(&s)).map_or(true, |t| now - t > chrono::Duration::minutes(minutes)))
        };
        let full = older_than("last_full_sync", FULL_REFRESH_MINUTES)?;
        let check_branches = full || older_than("last_branch_check", BRANCH_CHECK_MINUTES)?;
        let first_branch_check = db::get_setting(&conn, "last_branch_check")?.is_none();
        let mut ids = Vec::new();
        for h in &hits {
            if full || db::needs_refresh(&conn, h)? {
                ids.push(h.id.clone());
            }
        }
        let active = db::active_repos(&conn, &fmt_ts(now - chrono::Duration::days(ACTIVE_REPO_DAYS)), MAX_WATCHED_BRANCHES)?;
        (ids, full, check_branches, first_branch_check, active)
    };
    let mut fetched = if to_fetch.is_empty() { Default::default() } else { client.items_by_id(&to_fetch).await? };
    let requested: HashSet<&str> = hits.iter().filter(|h| h.review_requested).map(|h| h.id.as_str()).collect();
    for pr in &mut fetched.prs {
        pr.matched_review_requested = requested.contains(pr.id.as_str());
    }
    let branches = if check_branches && !active.is_empty() { Some(client.default_branches(&active).await?) } else { None };

    let stale = {
        let conn = db_lock.lock().unwrap();
        let mut pass = Pass {
            sync_id,
            now,
            first_sync: db::get_setting(&conn, "initialized")?.is_none(),
            baseline: db::get_setting(&conn, "initialized_at")?,
            titles: db::open_titles(&conn)?,
            notices: &mut notices,
        };
        let tx = conn.unchecked_transaction()?;
        for h in &hits {
            db::touch(&tx, &h.id, sync_id)?;
        }
        for pr in &fetched.prs {
            let t = Tracked::from_pr(pr);
            pass.process(&tx, &t, &derive::derive(pr, ctx))?;
            record_my_pr_actions(&tx, pr, &t.as_ref(), me)?;
            pass.intents(&tx, &t, &my_pr_texts(pr, me))?;
        }
        for issue in &fetched.issues {
            let t = Tracked::from_issue(issue);
            pass.process(&tx, &t, &derive::derive_issue(issue, ctx))?;
            record_my_issue_actions(&tx, issue, &t.as_ref(), me)?;
            pass.intents(&tx, &t, &my_issue_texts(issue, me))?;
        }

        // Default branches: a red branch in an active repo becomes an item; green closes it.
        if let Some(branches) = &branches {
            // Branches already red the first time we look are the baseline, not news.
            let quiet = pass.first_sync;
            pass.first_sync = quiet || first_branch_check;
            let open_runs: HashSet<String> = db::open_runs(&tx)?.into_iter().collect();
            for b in branches {
                let id = format!("RUN:{}", b.repo);
                if b.ci == "failure" {
                    let names: Vec<&str> = b.failures.iter().map(|(n, _)| n.as_str()).collect();
                    let author = b.author_login.clone().unwrap_or_else(|| "someone".into());
                    let mine = author.eq_ignore_ascii_case(me);
                    let t = Tracked {
                        id: id.clone(),
                        kind: "run",
                        repo: b.repo.clone(),
                        number: 0,
                        title: if names.is_empty() {
                            format!("{} is failing", b.branch)
                        } else {
                            format!("{} is failing: {}", b.branch, names.join(", "))
                        },
                        url: b.failures.iter().find_map(|(_, u)| u.clone()).unwrap_or_else(|| b.commit_url.clone()),
                        author: author.clone(),
                        author_avatar: b.author_avatar.clone(),
                        is_draft: false,
                        gh_state: "OPEN".into(),
                        created_at: b.committed_date.clone(),
                        updated_at: b.committed_date.clone(),
                        review_requested: false,
                        branch: None,
                        cross_repo: false,
                    };
                    let since = parse_ts(&b.committed_date).unwrap_or(now);
                    let red_days = (now - since).num_days();
                    let mut d = Derived {
                        state: if mine { derive::NEEDS_CHANGES } else { derive::BROKEN },
                        reason: if mine {
                            format!("Your commit broke {}: {}", b.branch, b.headline)
                        } else {
                            format!("Latest commit by {author}: {}", b.headline)
                        },
                        since,
                        is_mine: mine,
                        ci: Ci::Failure,
                        context: None,
                    };
                    if red_days >= STALE_BRANCH_DAYS {
                        d.state = derive::STALE;
                        d.reason = format!("Red for {red_days} days · {}", d.reason);
                    }
                    pass.process(&tx, &t, &d)?;
                } else if b.ci != "pending" && open_runs.contains(&id) {
                    db::mark_gone(&tx, &id)?;
                    if let Some(r) = PrRef::load(&tx, &id)? {
                        db::record_activity(
                            &tx,
                            &ActivityRow { key: format!("fixed:{id}:{}", b.oid), at: &now_s, kind: "update", pr: Some(&r), detail: format!("{} is green again", b.branch) },
                        )?;
                    }
                }
            }
            pass.first_sync = quiet;
        }
        for p in &pushes {
            match db::pr_for_branch(&tx, &p.repo, &p.branch)? {
                Some(r) => {
                    db::record_activity_on_branch(
                        &tx,
                        &ActivityRow { key: format!("push:{}", p.id), at: &p.at, kind: "pushed", pr: Some(&r), detail: format!("Pushed to {}", p.branch) },
                        &p.branch,
                    )?;
                }
                None => {
                    db::record_push(&tx, &p.id, &p.at, &p.repo, &p.branch, &format!("https://github.com/{}/tree/{}", p.repo, p.branch))?;
                }
            }
        }
        tx.commit()?;
        db::stale_open_ids(&conn, sync_id)?
    };

    // Items that dropped out of search: find out whether they merged/closed or just stopped involving us.
    let refreshed = if stale.is_empty() { Default::default() } else { client.items_by_id(&stale).await? };

    // Snoozes waiting on another PR/issue ("come back after #392 merges").
    let waits = db::waiting_on_refs(&db_lock.lock().unwrap())?;
    let keys: Vec<String> = waits.iter().map(|(_, k, _)| k.clone()).collect::<HashSet<_>>().into_iter().collect();
    let ref_states = if keys.is_empty() { HashMap::new() } else { client.ref_states(&keys).await? };

    let conn = db_lock.lock().unwrap();
    let mut pass = Pass { sync_id, now, first_sync: false, baseline: None, titles: Vec::new(), notices: &mut notices };
    for id in &stale {
        db::mark_gone(&conn, id)?;
    }
    let closed = refreshed
        .prs
        .iter()
        .filter(|p| p.state != "OPEN")
        .map(|p| (Tracked::from_pr(p), if p.state == "MERGED" { ("merged", p.merged_at.clone(), "Merged") } else { ("closed", p.closed_at.clone(), "Closed") }))
        .chain(refreshed.issues.iter().filter(|i| i.state != "OPEN").map(|i| (Tracked::from_issue(i), ("closed", i.closed_at.clone(), "Closed issue"))));
    for (t, (kind, at, detail)) in closed {
        let at = at.unwrap_or_else(|| now_s.clone());
        db::record_activity(&conn, &ActivityRow { key: format!("{kind}:{}", t.id), at: &at, kind, pr: Some(&t.as_ref()), detail: detail.into() })?;
        db::clear_personal(&conn, &t.id)?;
    }

    for (pr_id, key, note) in waits {
        let Some(rs) = ref_states.get(&key).filter(|s| s.state != "OPEN") else { continue };
        let Some(r) = PrRef::load(&conn, &pr_id)? else { continue };
        let what = if rs.state == "MERGED" { "merged" } else { "closed" };
        let detail = format!("{key} {what}: {}", rs.title);
        db::set_personal(
            &conn,
            &pr_id,
            &Personal { state: "due".into(), note: Some(format!("{}{detail}", note.map(|n| format!("{n} · ")).unwrap_or_default())), until: None, set_at: now_s.clone(), waits_on: None },
        )?;
        db::record_activity(&conn, &ActivityRow { key: format!("waited:{pr_id}:{key}"), at: &now_s, kind: "update", pr: Some(&r), detail: format!("Back: {detail}") })?;
        pass.notify(&conn, &r, "⏰ Come back to this", &detail)?;
    }

    // Snoozes whose time has come.
    for (pr_id, note) in db::due_reminders(&conn, &now_s)? {
        let Some(r) = PrRef::load(&conn, &pr_id)? else { continue };
        db::set_personal(
            &conn,
            &pr_id,
            &Personal { state: "due".into(), note: note.clone(), until: None, set_at: now_s.clone(), waits_on: None },
        )?;
        let detail = match &note {
            Some(n) if !n.is_empty() => format!("Reminder: {n}"),
            _ => "Reminder due".into(),
        };
        db::record_activity(
            &conn,
            &ActivityRow { key: format!("due:{pr_id}:{now_s}"), at: &now_s, kind: "update", pr: Some(&r), detail: detail.clone() },
        )?;
        pass.notify(&conn, &r, "⏰ Come back to this", &detail)?;
    }

    db::set_setting(&conn, "initialized", "1")?;
    // Baseline for "new since you last looked" and for reading intent in comments.
    if db::get_setting(&conn, "initialized_at")?.is_none() {
        db::set_setting(&conn, "initialized_at", &now_s)?;
    }
    if full {
        db::set_setting(&conn, "last_full_sync", &now_s)?;
    }
    if branches.is_some() {
        db::set_setting(&conn, "last_branch_check", &now_s)?;
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
        notices = vec![Notice { title: "Dev Inbox".into(), body: format!("{n} things changed that need your attention"), url: None, item_id: None }];
    }
    Ok(Outcome { notices, needs_you })
}

/// The once-a-day summary: what needs you, by kind, plus reminders due today.
pub fn digest(conn: &Connection) -> Result<Option<String>> {
    let items = db::list_items(conn)?;
    let count = |f: &dyn Fn(&db::Item) -> bool| items.iter().filter(|i| f(i)).count();
    let reviews = count(&|i| matches!(i.state.as_str(), derive::NEEDS_REVIEW | derive::NEEDS_REREVIEW));
    let replies = count(&|i| i.state == derive::REPLIED);
    let fixes = count(&|i| i.state == derive::NEEDS_CHANGES || i.state == derive::BROKEN);
    let assigned = count(&|i| i.state == derive::ASSIGNED);
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
    if fixes > 0 { parts.push(plural(fixes, "thing to fix", "things to fix")); }
    if assigned > 0 { parts.push(plural(assigned, "new issue", "new issues")); }
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
        // The Day view for the most recent day with pushes or commits.
        let day = std::env::var("DAY").unwrap_or_else(|_| "2026-10-14".into());
        let (start, end) = (format!("{day}T00:00:00Z"), format!("{day}T23:59:59Z"));
        let events = crate::day::gather(&conn, &start, &end).unwrap();
        let r = crate::day::build(&day, events, None, crate::derive::parse_ts(&start).unwrap(), crate::derive::parse_ts(&end).unwrap(), &Default::default());
        println!("day {day}: total {}m in {} blocks", r.total_minutes, r.blocks.len());
        for g in &r.groups {
            println!("  {:>4}m  {}  ({} events: {})", g.minutes, g.label, g.events.len(),
                g.events.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>().join(","));
        }
        drop(conn);
        let _ = std::fs::remove_file(path);
    }
}
