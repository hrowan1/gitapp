//! Deterministic mapping from GitHub data to a personal workflow state.
//! No AI here: everything is derived from who did what, and when.

use crate::github::{Actor, PullRequest};
use chrono::{DateTime, Utc};

pub type Ts = DateTime<Utc>;

pub fn parse_ts(s: &str) -> Option<Ts> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc))
}

pub fn fmt_ts(t: Ts) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Personal workflow states. The first group can also be set manually;
/// `Replied`, `ReadyToMerge`, `Draft` and `Fyi` are only ever derived.
pub const NEEDS_REVIEW: &str = "needs_review";
pub const NEEDS_REREVIEW: &str = "needs_rereview";
pub const NEEDS_CHANGES: &str = "needs_changes";
pub const WAITING: &str = "waiting";
pub const WAITING_CI: &str = "waiting_ci";
pub const LATER: &str = "later";
pub const DONE: &str = "done";
pub const REPLIED: &str = "replied";
pub const READY_TO_MERGE: &str = "ready_to_merge";
pub const DRAFT: &str = "draft";
pub const FYI: &str = "fyi";
/// No activity for `STALE_DAYS`: kept out of "Needs you" so old, abandoned PRs don't drown out today's work.
pub const STALE: &str = "stale";
pub const STALE_DAYS: i64 = 30;
/// Manual only: hidden from Today and never resurfaced by new activity.
pub const MUTED: &str = "muted";

/// States that mean "this needs you now" — these trigger notifications and resurface snoozed items.
pub fn is_actionable(state: &str) -> bool {
    matches!(state, NEEDS_REVIEW | NEEDS_REREVIEW | NEEDS_CHANGES | REPLIED | READY_TO_MERGE)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Ci {
    Success,
    Failure,
    Pending,
    None,
}

impl Ci {
    pub fn from_rollup(state: Option<&str>) -> Self {
        match state {
            Some("SUCCESS") => Ci::Success,
            Some("FAILURE") | Some("ERROR") => Ci::Failure,
            Some("PENDING") | Some("EXPECTED") => Ci::Pending,
            _ => Ci::None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Ci::Success => "success",
            Ci::Failure => "failure",
            Ci::Pending => "pending",
            Ci::None => "none",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Derived {
    pub state: &'static str,
    pub reason: String,
    /// When the event that put the PR in this state happened.
    pub since: Ts,
    pub is_mine: bool,
    pub ci: Ci,
}

struct Event<'a> {
    who: &'a str,
    at: Ts,
    /// Comments and commented/changes-requested reviews. A bare approval isn't a "reply".
    conversational: bool,
}

/// Who "me" is, plus people whose activity should never count (machine users, noisy bots).
pub struct Ctx {
    pub me: String,
    ignored: Vec<String>,
}

impl Ctx {
    pub fn new(me: &str, ignored: &[String]) -> Self {
        Self { me: me.to_string(), ignored: ignored.iter().map(|l| l.trim().to_ascii_lowercase()).collect() }
    }

    pub fn is_ignored(&self, login: &str) -> bool {
        self.ignored.iter().any(|l| l.eq_ignore_ascii_case(login))
    }

    fn human<'a>(&self, a: &'a Option<Actor>) -> Option<&'a Actor> {
        a.as_ref().filter(|a| !a.is_bot() && !self.is_ignored(&a.login))
    }
}

fn join_names(names: &[String]) -> String {
    match names.len() {
        0 => String::new(),
        1 => names[0].clone(),
        2 => format!("{} and {}", names[0], names[1]),
        n => format!("{}, {} and {} others", names[0], names[1], n - 2),
    }
}

pub fn derive(pr: &PullRequest, ctx: &Ctx) -> Derived {
    derive_at(pr, ctx, Utc::now())
}

pub fn derive_at(pr: &PullRequest, ctx: &Ctx, now: Ts) -> Derived {
    let mut d = derive_state(pr, ctx);
    if let Some(author) = pr.author.as_ref().filter(|a| ctx.is_ignored(&a.login)) {
        if d.state != NEEDS_REVIEW && d.state != NEEDS_REREVIEW {
            d.reason = format!("From {} (ignored) · {}", author.login, d.reason);
            d.state = FYI;
            return d;
        }
    }
    let last_activity = parse_ts(&pr.updated_at).unwrap_or(now);
    let quiet_days = (now - last_activity).num_days();
    if d.state != DONE && d.state != FYI && quiet_days >= STALE_DAYS {
        let quiet = if quiet_days >= 60 { format!("{} months", quiet_days / 30) } else { format!("{quiet_days} days") };
        d.reason = format!("Quiet for {quiet} · {}", d.reason);
        d.state = STALE;
    }
    d
}

fn derive_state(pr: &PullRequest, ctx: &Ctx) -> Derived {
    let me = ctx.me.as_str();
    let eq = |login: &str| login.eq_ignore_ascii_case(me);
    let author = pr.author.as_ref().map(|a| a.login.as_str()).unwrap_or("ghost");
    let is_mine = eq(author);
    let created = parse_ts(&pr.created_at).unwrap_or_else(Utc::now);

    // --- CI ---
    let last_commit = pr.commits.iter().last().map(|c| &c.commit);
    let ci = Ci::from_rollup(last_commit.and_then(|c| c.status_check_rollup.as_ref()).map(|r| r.state.as_str()));

    // --- Pushes ---
    let commit_at = last_commit.and_then(|c| c.committed_date.as_deref()).and_then(parse_ts);
    let force_push_at = pr
        .timeline_items
        .iter()
        .filter(|t| t.typename == "HeadRefForcePushedEvent")
        .filter_map(|t| t.created_at.as_deref().and_then(parse_ts))
        .max();
    let last_push_at = commit_at.max(force_push_at);

    // --- Human activity (comments + submitted reviews, incl. thread replies) ---
    let mut activity: Vec<Event> = Vec::new();
    for c in pr.comments.iter() {
        if let (Some(a), Some(at)) = (ctx.human(&c.author), parse_ts(&c.created_at)) {
            activity.push(Event { who: &a.login, at, conversational: true });
        }
    }
    for r in pr.reviews.iter().filter(|r| r.state != "PENDING") {
        if let (Some(a), Some(at)) = (ctx.human(&r.author), r.submitted_at.as_deref().and_then(parse_ts)) {
            let conversational = matches!(r.state.as_str(), "COMMENTED" | "CHANGES_REQUESTED");
            activity.push(Event { who: &a.login, at, conversational });
        }
    }
    let my_last_activity = activity.iter().filter(|e| eq(e.who)).map(|e| e.at).max();
    let others_after = |t: Ts| -> Vec<&Event> { activity.iter().filter(|e| e.conversational && !eq(e.who) && e.at > t).collect() };

    // My most recent *decisive* review (approve / request changes).
    let my_decisive = pr
        .reviews
        .iter()
        .filter(|r| r.author.as_ref().is_some_and(|a| eq(&a.login)))
        .filter(|r| matches!(r.state.as_str(), "APPROVED" | "CHANGES_REQUESTED" | "DISMISSED"))
        .filter_map(|r| Some((r.state.as_str(), r.submitted_at.as_deref().and_then(parse_ts)?)))
        .max_by_key(|(_, at)| *at);
    let i_have_reviewed = pr
        .reviews
        .iter()
        .any(|r| r.state != "PENDING" && r.author.as_ref().is_some_and(|a| eq(&a.login)));

    // Review requests.
    let requested_users: Vec<String> = pr
        .review_requests
        .iter()
        .filter_map(|r| r.requested_reviewer.as_ref())
        .filter_map(|r| r.login.clone().or_else(|| r.slug.as_ref().map(|s| format!("@{s}"))))
        .collect();
    let requested_me_directly = requested_users.iter().any(|u| eq(u));
    let requested_from_me = requested_me_directly || pr.matched_review_requested;
    let review_requested_at = pr
        .timeline_items
        .iter()
        .filter(|t| t.typename == "ReviewRequestedEvent")
        .filter(|t| {
            !requested_me_directly
                || t.requested_reviewer.as_ref().and_then(|r| r.login.as_deref()).is_some_and(|l| eq(l))
        })
        .filter_map(|t| t.created_at.as_deref().and_then(parse_ts))
        .max();
    let assigned_to_me = pr.assignees.iter().any(|a| eq(&a.login));

    let d = |state: &'static str, reason: String, since: Ts| Derived {
        state,
        reason,
        since,
        is_mine,
        ci: ci.clone(),
    };
    let checks_note = if ci == Ci::Failure { " · checks failing" } else { "" };
    let push_or_created = last_push_at.unwrap_or(created);

    if is_mine {
        if pr.is_draft {
            let reason = if ci == Ci::Failure { "Draft · CI failing" } else { "Draft" };
            return d(DRAFT, reason.into(), push_or_created);
        }
        if pr.mergeable.as_deref() == Some("CONFLICTING") {
            return d(NEEDS_CHANGES, "Merge conflicts".into(), push_or_created);
        }
        if ci == Ci::Failure {
            return d(NEEDS_CHANGES, "CI failing".into(), push_or_created);
        }
        // Latest changes-requested review from someone else that's newer than my last push.
        let changes_req = pr
            .reviews
            .iter()
            .filter(|r| r.state == "CHANGES_REQUESTED")
            .filter_map(|r| Some((ctx.human(&r.author)?, r.submitted_at.as_deref().and_then(parse_ts)?)))
            .filter(|(a, _)| !eq(&a.login))
            .max_by_key(|(_, at)| *at);
        if pr.review_decision.as_deref() == Some("CHANGES_REQUESTED") {
            if let Some((who, at)) = changes_req {
                if last_push_at.map_or(true, |p| at > p) {
                    return d(NEEDS_CHANGES, format!("Changes requested by {}", who.login), at);
                }
            }
        }
        let baseline = my_last_activity.unwrap_or(created);
        let replies = others_after(baseline);
        if let Some(last) = replies.iter().max_by_key(|e| e.at) {
            let mut who: Vec<String> = Vec::new();
            for e in &replies {
                if !who.iter().any(|w| w == e.who) {
                    who.push(e.who.to_string());
                }
            }
            let verb = if replies.len() == 1 { "commented" } else { "left comments" };
            return d(REPLIED, format!("{} {verb}", join_names(&who)), last.at);
        }
        if pr.review_decision.as_deref() == Some("APPROVED") {
            if ci == Ci::Pending {
                return d(WAITING_CI, "Approved · checks running".into(), push_or_created);
            }
            return d(READY_TO_MERGE, "Approved · ready to merge".into(), push_or_created);
        }
        if ci == Ci::Pending {
            return d(WAITING_CI, "Checks running".into(), push_or_created);
        }
        let reason = if requested_users.is_empty() {
            "No reviewers requested".to_string()
        } else {
            format!("Waiting for review from {}", join_names(&requested_users))
        };
        return d(WAITING, reason, review_requested_at.unwrap_or(push_or_created).max(push_or_created));
    }

    // Someone else's PR.
    if requested_from_me {
        let since = review_requested_at.unwrap_or(created);
        let via_team = !requested_me_directly;
        if i_have_reviewed {
            return d(NEEDS_REREVIEW, format!("{author} re-requested your review{checks_note}"), since);
        }
        let reason = if via_team {
            let teams: Vec<String> = requested_users.iter().filter(|u| u.starts_with('@')).cloned().collect();
            format!("Team review requested {}{checks_note}", join_names(&teams)).trim_end().to_string()
        } else {
            format!("Review requested{checks_note}")
        };
        return d(NEEDS_REVIEW, reason, since);
    }

    match my_decisive {
        Some(("CHANGES_REQUESTED", reviewed_at)) => {
            if let Some(p) = last_push_at.filter(|p| *p > reviewed_at) {
                return d(NEEDS_REREVIEW, "New commits since you requested changes".into(), p);
            }
            let replies = others_after(my_last_activity.unwrap_or(reviewed_at));
            if let Some(last) = replies.iter().max_by_key(|e| e.at) {
                return d(REPLIED, format!("{} replied to your review", last.who), last.at);
            }
            return d(WAITING, format!("Waiting for {author} to address your changes"), reviewed_at);
        }
        Some(("APPROVED", approved_at)) => {
            let replies = others_after(my_last_activity.unwrap_or(approved_at));
            if let Some(last) = replies.iter().max_by_key(|e| e.at) {
                return d(REPLIED, format!("{} commented after your approval", last.who), last.at);
            }
            return d(DONE, "You approved".into(), approved_at);
        }
        _ => {}
    }

    if let Some(mine) = my_last_activity {
        let replies = others_after(mine);
        if let Some(last) = replies.iter().max_by_key(|e| e.at) {
            let n = replies.len();
            let reason = if n == 1 {
                format!("{} replied after your comment", last.who)
            } else {
                format!("{n} new comments since yours")
            };
            return d(REPLIED, reason, last.at);
        }
        return d(WAITING, "Waiting for a response".into(), mine);
    }

    if assigned_to_me {
        return d(NEEDS_REVIEW, format!("Assigned to you{checks_note}"), created);
    }
    d(FYI, "You're mentioned or subscribed".into(), parse_ts(&pr.updated_at).unwrap_or(created))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pr(extra: serde_json::Value) -> PullRequest {
        let mut base = json!({
            "id": "PR_1", "number": 1, "title": "t", "url": "u", "isDraft": false, "state": "OPEN",
            "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z",
            "mergedAt": null, "closedAt": null, "mergeable": "MERGEABLE", "reviewDecision": null,
            "author": {"__typename": "User", "login": "sarah", "avatarUrl": null},
            "repository": {"nameWithOwner": "acme/web"},
            "assignees": {"nodes": []}, "reviewRequests": {"nodes": []},
            "reviews": {"nodes": []}, "comments": {"nodes": []},
            "commits": {"nodes": [{"commit": {"committedDate": "2026-01-01T00:00:00Z", "statusCheckRollup": {"state": "SUCCESS"}}}]},
            "timelineItems": {"nodes": []}
        });
        for (k, v) in extra.as_object().unwrap() {
            base[k] = v.clone();
        }
        serde_json::from_value(base).unwrap()
    }

    fn derive(p: &PullRequest, me: &str) -> Derived {
        super::derive_at(p, &Ctx::new(me, &["fftbot".into(), "noisy".into()]), parse_ts("2026-01-05T00:00:00Z").unwrap())
    }

    #[test]
    fn stale_after_30_days() {
        let p = pr(json!({"reviewRequests": {"nodes": [{"requestedReviewer": {"__typename": "User", "login": "me"}}]}}));
        let d = super::derive_at(&p, &Ctx::new("me", &[]), parse_ts("2026-03-01T00:00:00Z").unwrap());
        assert_eq!(d.state, STALE);
    }

    fn review(login: &str, state: &str, at: &str) -> serde_json::Value {
        json!({"id": format!("R_{login}_{at}"), "state": state, "submittedAt": at, "author": {"__typename": "User", "login": login}})
    }

    #[test]
    fn review_requested() {
        let p = pr(json!({"reviewRequests": {"nodes": [{"requestedReviewer": {"__typename": "User", "login": "me"}}]}}));
        assert_eq!(derive(&p, "me").state, NEEDS_REVIEW);
    }

    #[test]
    fn new_commits_after_changes_requested() {
        let p = pr(json!({
            "reviews": {"nodes": [review("me", "CHANGES_REQUESTED", "2026-01-02T00:00:00Z")]},
            "commits": {"nodes": [{"commit": {"committedDate": "2026-01-03T00:00:00Z", "statusCheckRollup": null}}]}
        }));
        let d = derive(&p, "me");
        assert_eq!(d.state, NEEDS_REREVIEW);
    }

    #[test]
    fn waiting_for_author_after_changes_requested() {
        let p = pr(json!({"reviews": {"nodes": [review("me", "CHANGES_REQUESTED", "2026-01-02T00:00:00Z")]}}));
        assert_eq!(derive(&p, "me").state, WAITING);
    }

    #[test]
    fn reply_after_my_comment() {
        let p = pr(json!({"comments": {"nodes": [
            {"id": "c1", "createdAt": "2026-01-02T00:00:00Z", "author": {"__typename": "User", "login": "me"}},
            {"id": "c2", "createdAt": "2026-01-03T00:00:00Z", "author": {"__typename": "User", "login": "sarah"}},
            {"id": "c3", "createdAt": "2026-01-04T00:00:00Z", "author": {"__typename": "Bot", "login": "ci-bot"}}
        ]}}));
        let d = derive(&p, "me");
        assert_eq!(d.state, REPLIED);
        assert!(d.reason.contains("sarah"));
    }

    #[test]
    fn ignored_users_are_not_replies() {
        let p = pr(json!({"comments": {"nodes": [
            {"id": "c1", "createdAt": "2026-01-02T00:00:00Z", "author": {"__typename": "User", "login": "me"}},
            {"id": "c2", "createdAt": "2026-01-03T00:00:00Z", "author": {"__typename": "User", "login": "Noisy"}}
        ]}}));
        assert_eq!(derive(&p, "me").state, WAITING);
    }

    #[test]
    fn ignored_authors_are_fyi() {
        let p = pr(json!({"author": {"__typename": "User", "login": "noisy", "avatarUrl": null},
            "comments": {"nodes": [{"id": "c1", "createdAt": "2026-01-02T00:00:00Z", "author": {"__typename": "User", "login": "me"}}]}}));
        assert_eq!(derive(&p, "me").state, FYI);
    }

    #[test]
    fn my_pr_ci_failing() {
        let p = pr(json!({
            "author": {"__typename": "User", "login": "me", "avatarUrl": null},
            "commits": {"nodes": [{"commit": {"committedDate": "2026-01-01T00:00:00Z", "statusCheckRollup": {"state": "FAILURE"}}}]}
        }));
        assert_eq!(derive(&p, "me").state, NEEDS_CHANGES);
    }

    #[test]
    fn my_pr_approved() {
        let p = pr(json!({
            "author": {"__typename": "User", "login": "me", "avatarUrl": null},
            "reviewDecision": "APPROVED",
            "reviews": {"nodes": [review("sarah", "APPROVED", "2026-01-02T00:00:00Z")]}
        }));
        assert_eq!(derive(&p, "me").state, READY_TO_MERGE);
    }

    #[test]
    fn my_pr_waiting() {
        let p = pr(json!({
            "author": {"__typename": "User", "login": "me", "avatarUrl": null},
            "reviewRequests": {"nodes": [{"requestedReviewer": {"__typename": "User", "login": "james"}}]}
        }));
        let d = derive(&p, "me");
        assert_eq!(d.state, WAITING);
        assert!(d.reason.contains("james"));
    }
}
