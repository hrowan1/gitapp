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
            "id": format!("DEMO_C_{i}_{j}"), "createdAt": ago(*h), "author": user(a)
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
    };
    let d = derive::Derived {
        state: derive::BROKEN,
        reason: "Latest commit by james: Bump serde to 1.0.230".into(),
        since: Utc::now() - Duration::hours(2),
        is_mine: false,
        ci: derive::Ci::Failure,
    };
    db::upsert(conn, &run, &d, sync_id)?;

    db::set_setting(conn, "login", ME)?;
    db::set_setting(conn, "last_sync", &fmt_ts(Utc::now()))?;
    db::set_setting(conn, "initialized", "1")?;
    // Things that arrived in the last 6 hours show as "new".
    db::set_setting(conn, "initialized_at", &ago(6))?;
    Ok(())
}
