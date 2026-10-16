//! Demo mode (`DEV_INBOX_DEMO=1`): seeds a separate database with fictional PRs so the UI
//! can be developed and screenshotted without a GitHub account. Never syncs.

use anyhow::Result;
use chrono::{Duration, Utc};
use rusqlite::Connection;
use serde_json::{json, Value};

use crate::db;
use crate::derive::{self, fmt_ts};
use crate::github::{Issue, PullRequest};

const ME: &str = "you";

fn ago(hours: i64) -> String {
    fmt_ts(Utc::now() - Duration::hours(hours))
}

fn user(login: &str) -> Value {
    json!({ "__typename": "User", "login": login, "avatarUrl": null })
}

struct Spec {
    repo: &'static str,
    number: i64,
    title: &'static str,
    author: &'static str,
    created_h: i64,
    updated_h: i64,
    draft: bool,
    ci: Option<&'static str>,
    decision: Option<&'static str>,
    requested: &'static [&'static str],
    /// (author, state, hours ago)
    reviews: &'static [(&'static str, &'static str, i64)],
    /// (author, hours ago)
    comments: &'static [(&'static str, i64)],
    push_h: i64,
    review_requested: bool,
}

fn build(i: usize, s: &Spec) -> PullRequest {
    let v = json!({
        "id": format!("DEMO_{i}"),
        "number": s.number,
        "title": s.title,
        "url": format!("https://github.com/{}/pull/{}", s.repo, s.number),
        "isDraft": s.draft,
        "state": "OPEN",
        "createdAt": ago(s.created_h),
        "updatedAt": ago(s.updated_h),
        "mergedAt": null, "closedAt": null,
        "mergeable": "MERGEABLE",
        "reviewDecision": s.decision,
        "author": user(s.author),
        "repository": { "nameWithOwner": s.repo },
        "assignees": { "nodes": [] },
        "reviewRequests": { "nodes": s.requested.iter().map(|l| json!({ "requestedReviewer": user(l) })).collect::<Vec<_>>() },
        "reviews": { "nodes": s.reviews.iter().enumerate().map(|(j, (a, st, h))| json!({
            "id": format!("DEMO_R_{i}_{j}"), "state": st, "submittedAt": ago(*h), "author": user(a)
        })).collect::<Vec<_>>() },
        "comments": { "nodes": s.comments.iter().enumerate().map(|(j, (a, h))| json!({
            "id": format!("DEMO_C_{i}_{j}"), "createdAt": ago(*h), "author": user(a),
            "body": "Good point on the cursor format, I switched to opaque cursors. Mind taking another look?"
        })).collect::<Vec<_>>() },
        "commits": { "nodes": [{ "commit": { "committedDate": ago(s.push_h), "statusCheckRollup": s.ci.map(|c| json!({ "state": c })) } }] },
        "timelineItems": { "nodes": if s.review_requested {
            vec![json!({ "__typename": "ReviewRequestedEvent", "createdAt": ago(s.created_h.min(s.updated_h + 1)), "requestedReviewer": user(ME) })]
        } else { vec![] } },
    });
    let mut pr: PullRequest = serde_json::from_value(v).expect("demo fixture");
    pr.matched_review_requested = s.review_requested;
    pr
}

/// (number, title, author, assigned to me, [(comment author, hours ago, body)], created hours ago)
type IssueSpec = (i64, &'static str, &'static str, bool, &'static [(&'static str, i64, &'static str)], i64);

fn build_issue(repo: &str, (number, title, author, assigned, comments, created_h): &IssueSpec) -> Issue {
    let latest = comments.iter().map(|c| c.1).min().unwrap_or(*created_h);
    serde_json::from_value(json!({
        "id": format!("DEMO_ISSUE_{number}"),
        "number": number, "title": title,
        "url": format!("https://github.com/{repo}/issues/{number}"),
        "state": "OPEN", "createdAt": ago(*created_h), "updatedAt": ago(latest), "closedAt": null,
        "author": user(author),
        "repository": { "nameWithOwner": repo },
        "assignees": { "nodes": if *assigned { vec![json!({ "login": ME })] } else { vec![] } },
        "comments": { "nodes": comments.iter().enumerate().map(|(j, (a, h, body))| json!({
            "id": format!("DEMO_IC_{number}_{j}"), "createdAt": ago(*h), "body": body, "author": user(a)
        })).collect::<Vec<_>>() },
        "timelineItems": { "nodes": if *assigned {
            vec![json!({ "__typename": "AssignedEvent", "createdAt": ago(created_h - 1), "assignee": user(ME) })]
        } else { vec![] } },
    }))
    .expect("demo issue")
}

pub fn seed(conn: &Connection) -> Result<()> {
    if db::get_setting(conn, "login")?.is_some() {
        return Ok(());
    }
    let specs = [
        Spec { repo: "acme/web", number: 482, title: "Checkout refactor: split payment step into its own route", author: "sarah", created_h: 14, updated_h: 2, draft: false, ci: Some("FAILURE"), decision: None, requested: &[ME], reviews: &[], comments: &[], push_h: 3, review_requested: true },
        Spec { repo: "acme/api", number: 1290, title: "Auth middleware: rotate session tokens on privilege change", author: "james", created_h: 50, updated_h: 20, draft: false, ci: Some("SUCCESS"), decision: None, requested: &[ME], reviews: &[], comments: &[], push_h: 49, review_requested: true },
        Spec { repo: "acme/api", number: 1271, title: "API pagination for /v2/orders", author: "sarah", created_h: 120, updated_h: 3, draft: false, ci: Some("SUCCESS"), decision: None, requested: &[], reviews: &[(ME, "COMMENTED", 30)], comments: &[("sarah", 3)], push_h: 60, review_requested: false },
        Spec { repo: "acme/payments", number: 421, title: "Retry webhooks with exponential backoff", author: "mike", created_h: 140, updated_h: 5, draft: false, ci: Some("SUCCESS"), decision: Some("CHANGES_REQUESTED"), requested: &[], reviews: &[(ME, "CHANGES_REQUESTED", 96)], comments: &[], push_h: 5, review_requested: false },
        Spec { repo: "acme/billing", number: 77, title: "Proration for mid-cycle plan changes", author: ME, created_h: 30, updated_h: 1, draft: false, ci: Some("FAILURE"), decision: None, requested: &["priya"], reviews: &[], comments: &[], push_h: 1, review_requested: false },
        Spec { repo: "acme/web", number: 476, title: "Dashboard: lazy-load the usage charts", author: ME, created_h: 70, updated_h: 26, draft: false, ci: Some("SUCCESS"), decision: None, requested: &["sarah", "james"], reviews: &[], comments: &[], push_h: 70, review_requested: false },
        Spec { repo: "client-a/dashboard", number: 58, title: "Export reports as CSV", author: ME, created_h: 48, updated_h: 6, draft: false, ci: Some("SUCCESS"), decision: Some("APPROVED"), requested: &[], reviews: &[("lena", "APPROVED", 6)], comments: &[], push_h: 20, review_requested: false },
        Spec { repo: "acme/infrastructure", number: 233, title: "Move staging to the new VPC", author: "priya", created_h: 100, updated_h: 40, draft: false, ci: Some("PENDING"), decision: None, requested: &[], reviews: &[(ME, "CHANGES_REQUESTED", 40)], comments: &[], push_h: 90, review_requested: false },
        Spec { repo: "acme/mobile", number: 912, title: "Offline mode for the order list", author: ME, created_h: 10, updated_h: 2, draft: true, ci: Some("FAILURE"), decision: None, requested: &[], reviews: &[], comments: &[], push_h: 2, review_requested: false },
        Spec { repo: "personal/saas", number: 14, title: "Stripe usage-based billing", author: ME, created_h: 24 * 75, updated_h: 24 * 60, draft: false, ci: Some("FAILURE"), decision: None, requested: &[], reviews: &[], comments: &[], push_h: 24 * 60, review_requested: false },
        Spec { repo: "acme/ios", number: 640, title: "Release 4.12: bump build number", author: "release-train", created_h: 4, updated_h: 4, draft: false, ci: Some("SUCCESS"), decision: None, requested: &[ME], reviews: &[], comments: &[], push_h: 4, review_requested: true },
        Spec { repo: "acme/ios", number: 639, title: "Release 4.12: update localisations", author: "release-train", created_h: 5, updated_h: 5, draft: false, ci: Some("SUCCESS"), decision: None, requested: &[ME], reviews: &[], comments: &[], push_h: 5, review_requested: true },
        Spec { repo: "acme/ios", number: 637, title: "Release 4.12: changelog", author: "release-train", created_h: 9, updated_h: 9, draft: false, ci: Some("FAILURE"), decision: None, requested: &[ME], reviews: &[], comments: &[], push_h: 9, review_requested: true },
        Spec { repo: "acme/ios", number: 636, title: "Release 4.12: feature flags", author: "release-train", created_h: 10, updated_h: 10, draft: false, ci: Some("SUCCESS"), decision: None, requested: &[ME], reviews: &[], comments: &[], push_h: 10, review_requested: true },
        Spec { repo: "acme/ios", number: 634, title: "Release 4.12: App Store metadata", author: "release-train", created_h: 12, updated_h: 12, draft: false, ci: Some("SUCCESS"), decision: None, requested: &[ME], reviews: &[], comments: &[], push_h: 12, review_requested: true },
        Spec { repo: "acme/api", number: 1310, title: "API migration: move orders to v2", author: "james", created_h: 90, updated_h: 8, draft: false, ci: Some("SUCCESS"), decision: None, requested: &[], reviews: &[(ME, "APPROVED", 8)], comments: &[], push_h: 30, review_requested: false },
        Spec { repo: "acme/web", number: 471, title: "Search: debounce the typeahead", author: "priya", created_h: 60, updated_h: 1, draft: false, ci: Some("SUCCESS"), decision: None, requested: &[], reviews: &[(ME, "COMMENTED", 20), ("priya", "COMMENTED", 1)], comments: &[], push_h: 50, review_requested: false },
        Spec { repo: "acme/admin", number: 301, title: "Bulk user import", author: "lena", created_h: 200, updated_h: 150, draft: false, ci: Some("SUCCESS"), decision: Some("APPROVED"), requested: &[], reviews: &[(ME, "APPROVED", 150)], comments: &[], push_h: 160, review_requested: false },
    ];

    let sync_id = Utc::now().timestamp_millis();
    let ctx = derive::Ctx::new(ME, &[]);
    for (i, s) in specs.iter().enumerate() {
        let mut pr = build(i, s);
        if s.number == 471 {
            // A reply inside a review thread the user started.
            pr.review_threads = serde_json::from_value(json!({ "nodes": [{ "comments": { "nodes": [
                { "id": "DEMO_T1", "createdAt": ago(20), "body": "Should this be 300ms?", "author": user(ME) },
                { "id": "DEMO_T2", "createdAt": ago(1), "body": "Good call, changed it", "author": user("priya") }
            ] } }] }))?;
        }
        let d = derive::derive(&pr, &ctx);
        db::upsert(conn, &db::Tracked::from_pr(&pr), &d, sync_id)?;
        let r = db::Tracked::from_pr(&pr).as_ref();
        if s.author == ME {
            db::record_activity(conn, &db::ActivityRow { key: format!("opened:{}", pr.id), at: &pr.created_at, kind: "opened", pr: Some(&r), detail: "Opened".into() })?;
        }
        for (j, (a, st, h)) in s.reviews.iter().enumerate() {
            if *a == ME {
                let detail = if *st == "APPROVED" { "Approved" } else if *st == "CHANGES_REQUESTED" { "Requested changes" } else { "Reviewed" };
                db::record_activity(conn, &db::ActivityRow { key: format!("review:{i}:{j}"), at: &ago(*h), kind: "reviewed", pr: Some(&r), detail: detail.into() })?;
            }
        }
    }
    // A snoozed item with a note, the kind of intent the app remembers.
    let r = db::PrRef { id: "DEMO_SNOOZE".into(), repo: "acme/api".into(), number: 1302, title: "Rename legacy order fields".into(), url: "https://github.com/acme/api/pull/1302".into() };
    let snoozed = build(99, &Spec { repo: "acme/api", number: 1302, title: "Rename legacy order fields", author: "mike", created_h: 80, updated_h: 30, draft: false, ci: Some("SUCCESS"), decision: None, requested: &[], reviews: &[(ME, "COMMENTED", 30)], comments: &[], push_h: 79, review_requested: false });
    let mut snoozed = snoozed;
    snoozed.id = r.id.clone();
    db::upsert(conn, &db::Tracked::from_pr(&snoozed), &derive::derive(&snoozed, &ctx), sync_id)?;
    db::set_personal(
        conn,
        &r.id,
        &db::Personal {
            state: "later".into(),
            note: Some("“Let's come back to this after the API migration lands”".into()),
            until: None,
            set_at: ago(30),
            waits_on: Some("acme/api#1310".into()),
        },
    )?;
    db::record_activity(conn, &db::ActivityRow { key: "marked:demo".into(), at: &ago(30), kind: "marked", pr: Some(&r), detail: "Remembered from your comment — back when acme/api#1310 merges or closes".into() })?;

    // Issues.
    let issues: [IssueSpec; 3] = [
        (88, "Order export times out for large accounts", "lena", true, &[], 5),
        (91, "Flaky test: checkout_spec timeouts on CI", "james", false, &[("james", 2, "@you any idea why this started after the refactor?")], 30),
        (64, "Support SSO with Okta", ME, false, &[("priya", 4, "We need this for the enterprise pilot")], 200),
    ];
    for spec in &issues {
        let issue = build_issue("acme/web", spec);
        let t = db::Tracked::from_issue(&issue);
        db::upsert(conn, &t, &derive::derive_issue(&issue, &ctx), sync_id)?;
    }

    // A red default branch in a repo the user works in.
    let run = db::Tracked {
        id: "RUN:acme/api".into(),
        kind: "run",
        repo: "acme/api".into(),
        number: 0,
        title: "main is failing: test, lint".into(),
        url: "https://github.com/acme/api/actions".into(),
        author: "james".into(),
        author_avatar: None,
        is_draft: false,
        gh_state: "OPEN".into(),
        created_at: ago(2),
        updated_at: ago(2),
        review_requested: false,
        branch: None,
        cross_repo: false,
    };
    let d = derive::Derived {
        state: derive::BROKEN,
        reason: "Latest commit by james: Bump serde to 1.0.230".into(),
        since: Utc::now() - Duration::hours(2),
        is_mine: false,
        ci: derive::Ci::Failure,
        context: None,
    };
    db::upsert(conn, &run, &d, sync_id)?;

    seed_work_day(conn)?;

    db::set_setting(conn, "login", ME)?;
    db::set_setting(conn, "last_sync", &fmt_ts(Utc::now()))?;
    db::set_setting(conn, "initialized", "1")?;
    // Things that arrived in the last 6 hours show as "new".
    db::set_setting(conn, "initialized_at", &ago(6))?;
    Ok(())
}

/// Yesterday as a realistic working day, for the Day view: editing sessions, commits and pushes on
/// ticket branches, a review, a comment and a push to main.
fn seed_work_day(conn: &Connection) -> Result<()> {
    use chrono::{Local, TimeZone};
    let y = Local::now().date_naive() - Duration::days(1);
    let at = |hm: &str| -> String {
        let (h, m) = hm.split_once(':').unwrap();
        let t = y.and_hms_opt(h.parse().unwrap(), m.parse().unwrap(), 0).unwrap();
        fmt_ts(Local.from_local_datetime(&t).earliest().unwrap().with_timezone(&Utc))
    };
    let pr = |id: &str| db::PrRef::load(conn, id).ok().flatten();
    conn.execute("UPDATE prs SET branch = 'BILL-142-proration' WHERE id = 'DEMO_4'", [])?;
    conn.execute("UPDATE prs SET branch = 'mob-77-offline-orders' WHERE id = 'DEMO_8'", [])?;

    let work = |id: &str, branch: &str, repo: &str, samples: (u32, u32), commits: &[(&str, &str)], push: &str| -> Result<()> {
        let r = pr(id);
        let mut m = samples.0;
        while m <= samples.1 {
            db::record_sample(conn, &at(&format!("{}:{:02}", m / 60, m % 60)), repo, Some(branch))?;
            m += 5;
        }
        for (i, (t, msg)) in commits.iter().enumerate() {
            db::record_activity_on_branch(
                conn,
                &db::ActivityRow { key: format!("commit:demo-{id}-{i}"), at: &at(t), kind: "committed", pr: r.as_ref(), detail: msg.to_string() },
                branch,
            )?;
        }
        db::record_activity_on_branch(
            conn,
            &db::ActivityRow { key: format!("push:demo-{id}"), at: &at(push), kind: "pushed", pr: r.as_ref(), detail: format!("Pushed to {branch}") },
            branch,
        )?;
        Ok(())
    };
    work(
        "DEMO_4",
        "BILL-142-proration",
        "acme/billing",
        (9 * 60, 10 * 60 + 55),
        &[("09:42", "Prorate seat changes by remaining days"), ("10:18", "Handle downgrades mid-cycle"), ("11:02", "Fix rounding in proration tests")],
        "11:06",
    )?;
    work(
        "DEMO_8",
        "mob-77-offline-orders",
        "acme/mobile",
        (13 * 60 + 35, 15 * 60 + 40),
        &[("14:21", "Cache order list in SQLite"), ("15:44", "Sync queued edits when back online")],
        "15:47",
    )?;
    if let Some(r) = pr("DEMO_3") {
        db::record_activity(conn, &db::ActivityRow { key: "review:demo-y".into(), at: &at("11:34"), kind: "reviewed", pr: Some(&r), detail: "Requested changes".into() })?;
    }
    if let Some(r) = pr("DEMO_2") {
        db::record_activity(conn, &db::ActivityRow { key: "comment:demo-y".into(), at: &at("16:12"), kind: "commented", pr: Some(&r), detail: "Commented".into() })?;
    }
    db::record_push(conn, "demo-main", &at("16:41"), "acme/infrastructure", "main", "https://github.com/acme/infrastructure")?;

    // Jira: the two coded tickets, one moved with no code (QA), one handed to someone else.
    let site = "https://acme.atlassian.net";
    for (key, summary, status, cat, mine) in [
        ("BILL-142", "Prorate plan changes mid-cycle", "In Review", "indeterminate", true),
        ("MOB-77", "Offline mode for the order list", "In Progress", "indeterminate", true),
        ("ES-310", "QA: checkout on Safari 17", "Done", "done", true),
        ("ES-298", "Spike: payments provider fees", "In Progress", "indeterminate", false),
    ] {
        conn.execute(
            "INSERT OR REPLACE INTO jira_issues (key, id, summary, status, category, issue_type, priority, project, assignee, assigned_to_me, url, updated)
             VALUES (?1, ?1, ?2, ?3, ?4, 'Story', 'Medium', ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![key, summary, status, cat, key.split('-').next().unwrap(), if mine { "You" } else { "Sam" }, mine, format!("{site}/browse/{key}"), at("16:30")],
        )?;
    }
    for (id, key, t, kind, detail, me) in [
        ("j1", "BILL-142", "08:50", "status", "To Do → In Progress", true),
        ("j2", "BILL-142", "11:10", "status", "In Progress → In Review", true),
        ("j3", "MOB-77", "13:20", "assignee", "Assigned to You", false),
        ("j4", "MOB-77", "13:26", "status", "To Do → In Progress", true),
        ("j5", "ES-310", "16:18", "status", "In QA → Done", true),
        ("j6", "ES-310", "16:24", "comment", "Commented: Verified on Safari 17.4 and 17.6, no regressions", true),
        ("j7", "ES-298", "17:05", "assignee", "Assigned to Sam", true),
    ] {
        conn.execute(
            "INSERT OR REPLACE INTO jira_events (id, issue_key, at, kind, from_value, to_value, actor, actor_is_me, detail)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, ?7, ?8)",
            rusqlite::params![id, key, at(t), kind, if kind == "assignee" && !me { Some("demo-me") } else { None }, if me { "You" } else { "Sam" }, me, detail],
        )?;
    }
    db::set_setting(conn, "jira_account", "demo-me")?;
    db::set_setting(conn, "jira_site", site)?;
    db::set_setting(conn, "jira_name", "You")?;
    db::set_setting(conn, "jira_last_sync", &fmt_ts(Utc::now()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    /// The demo work day turns into sensible blocks: `cargo test demo_day -- --nocapture`
    #[test]
    fn demo_day() {
        use chrono::{Local, TimeZone};
        let path = std::env::temp_dir().join(format!("dev-inbox-demo-day-{}.sqlite3", std::process::id()));
        let conn = crate::db::open(&path).unwrap();
        super::seed(&conn).unwrap();
        let y = Local::now().date_naive() - chrono::Duration::days(1);
        let start = Local.from_local_datetime(&y.and_hms_opt(0, 0, 0).unwrap()).earliest().unwrap().with_timezone(&chrono::Utc);
        let end = start + chrono::Duration::days(1);
        let (s, e) = (crate::derive::fmt_ts(start), crate::derive::fmt_ts(end));
        let titles = crate::jira::titles(&conn).unwrap();
        let mut events = crate::day::gather(&conn, &s, &e).unwrap();
        events.extend(crate::day::jira_events(&conn, &s, &e, &titles).unwrap());
        let r = crate::day::build(&y.to_string(), events, None, start, end, &titles);
        if std::env::var("DAY_JSON").is_ok() {
            println!("DAYJSON {}", serde_json::to_string(&r).unwrap());
            println!("TICKETSJSON {}", serde_json::to_string(&crate::jira::list_tickets(&conn).unwrap()).unwrap());
            let detail: std::collections::HashMap<String, Vec<crate::jira::TicketEvent>> = ["BILL-142", "MOB-77", "ES-310", "ES-298"]
                .iter()
                .map(|k| (k.to_string(), crate::jira::ticket_events(&conn, k, "demo-me").unwrap()))
                .collect();
            println!("TICKETEVENTSJSON {}", serde_json::to_string(&detail).unwrap());
        }
        let es = r.groups.iter().find(|g| g.key == "T:ES-310").expect("a Jira-only ticket appears on the day");
        assert_eq!(es.status.as_deref(), Some("Done"));
        assert!(es.minutes > 0, "your own Jira moves count a little");
        let keys: Vec<&str> = r.groups.iter().map(|g| g.key.as_str()).collect();
        assert!(keys.contains(&"T:BILL-142"), "{keys:?}");
        assert!(keys.contains(&"T:MOB-77"), "{keys:?}");
        let bill = r.groups.iter().find(|g| g.key == "T:BILL-142").unwrap();
        assert!(bill.minutes >= 120 && bill.minutes <= 150, "{}", bill.minutes);
        assert!(r.total_minutes > 240, "{}", r.total_minutes);
        let _ = std::fs::remove_file(path);
    }
}
