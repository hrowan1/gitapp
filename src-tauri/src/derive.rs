//! Deterministic mapping from GitHub data to a personal workflow state.
//! No AI here: everything is derived from who did what, and when.

use crate::github::{Actor, Issue, PullRequest};
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
/// An issue assigned to you that you haven't touched since.
pub const ASSIGNED: &str = "assigned";
/// A default branch whose latest commit is failing CI.
pub const BROKEN: &str = "broken";

/// States that mean "this needs you now" — these trigger notifications and resurface snoozed items.
pub fn is_actionable(state: &str) -> bool {
    matches!(state, NEEDS_REVIEW | NEEDS_REREVIEW | NEEDS_CHANGES | REPLIED | READY_TO_MERGE | ASSIGNED | BROKEN)
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
    /// For replies: what was said, so it can be shown and answered in place.
    pub context: Option<ReplyContext>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplyContext {
    pub who: String,
    pub body: String,
    /// Review thread to reply into; None means a normal conversation comment.
    pub thread_id: Option<String>,
}

fn snippet(body: &str) -> String {
    let flat: String = body
        .lines()
        .filter(|l| !l.trim_start().starts_with('>'))
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if flat.chars().count() > 280 {
        format!("{}…", flat.chars().take(280).collect::<String>())
    } else {
        flat
    }
}

/// One human message on a PR or issue.
struct Msg<'a> {
    who: &'a str,
    at: Ts,
    body: &'a str,
    /// Index of the review thread it belongs to, if any.
    thread: Option<usize>,
    thread_id: Option<&'a str>,
    /// Comments and commented/changes-requested reviews. A bare approval isn't a "reply".
    conversational: bool,
}

/// Does `body` @mention `login`?
pub fn mentions(body: &str, login: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    let needle = format!("@{}", login.to_ascii_lowercase());
    lower.match_indices(&needle).any(|(i, _)| {
        let before_ok = i == 0 || !lower.as_bytes()[i - 1].is_ascii_alphanumeric();
        let after = lower.as_bytes().get(i + needle.len()).copied();
        before_ok && !after.is_some_and(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    })
}

/// Why something counts as a reply to the user.
#[derive(Clone, Copy, PartialEq)]
enum Why {
    Thread,
    Mention,
    Author,
}

struct Reply<'a> {
    who: &'a str,
    at: Ts,
    why: Why,
    body: &'a str,
    thread_id: Option<&'a str>,
}

struct Conversation<'a> {
    msgs: Vec<Msg<'a>>,
    me: &'a str,
}

impl<'a> Conversation<'a> {
    fn mine(&self, m: &Msg) -> bool {
        m.who.eq_ignore_ascii_case(self.me)
    }

    fn my_last(&self) -> Option<Ts> {
        self.msgs.iter().filter(|m| self.mine(m)).map(|m| m.at).max()
    }

    /// Every conversational message by someone else after `t` (used on the user's own PRs/issues).
    fn others_after(&self, t: Ts) -> Vec<&Msg<'a>> {
        self.msgs.iter().filter(|m| m.conversational && !self.mine(m) && m.at > t).collect()
    }

    /// Messages that are actually addressed to the user: replies in threads they took part in,
    /// @mentions, or the author answering after the user's last message.
    fn replies_to_me(&self, author: &str) -> Vec<Reply<'a>> {
        let my_last = self.my_last();
        let mut out = Vec::new();
        for m in self.msgs.iter().filter(|m| m.conversational && !self.mine(m)) {
            let why = if mentions(m.body, self.me) && my_last.map_or(true, |t| m.at > t) {
                Some(Why::Mention)
            } else if let Some(t) = m.thread.and_then(|th| {
                self.msgs.iter().filter(|x| x.thread == Some(th) && self.mine(x)).map(|x| x.at).max()
            }) {
                (m.at > t).then_some(Why::Thread)
            } else if m.who.eq_ignore_ascii_case(author) && my_last.is_some_and(|t| m.at > t) {
                Some(Why::Author)
            } else {
                None
            };
            if let Some(why) = why {
                out.push(Reply { who: m.who, at: m.at, why, body: m.body, thread_id: m.thread_id });
            }
        }
        out
    }
}

fn describe_replies(replies: &[Reply]) -> (String, Ts, Option<ReplyContext>) {
    let last = replies.iter().max_by_key(|r| r.at).expect("non-empty");
    let context = Some(ReplyContext {
        who: last.who.to_string(),
        body: snippet(last.body),
        thread_id: last.thread_id.map(String::from),
    });
    let reason = match (replies.len(), last.why) {
        (1, Why::Mention) => format!("{} mentioned you", last.who),
        (1, Why::Thread) => format!("{} replied in your thread", last.who),
        (1, Why::Author) => format!("{} replied", last.who),
        (n, _) => format!("{n} replies to you · latest from {}", last.who),
    };
    (reason, last.at, context)
}

fn context_of(m: &Msg) -> Option<ReplyContext> {
    Some(ReplyContext { who: m.who.to_string(), body: snippet(m.body), thread_id: m.thread_id.map(String::from) })
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
    finish(derive_state(pr, ctx), pr.author.as_ref(), &pr.updated_at, ctx, now)
}

pub fn derive_issue(issue: &Issue, ctx: &Ctx) -> Derived {
    derive_issue_at(issue, ctx, Utc::now())
}

pub fn derive_issue_at(issue: &Issue, ctx: &Ctx, now: Ts) -> Derived {
    finish(derive_issue_state(issue, ctx), issue.author.as_ref(), &issue.updated_at, ctx, now)
}

/// Rules shared by PRs and issues: ignored authors go to FYI, quiet items go stale.
fn finish(mut d: Derived, author: Option<&Actor>, updated_at: &str, ctx: &Ctx, now: Ts) -> Derived {
    if let Some(author) = author.filter(|a| ctx.is_ignored(&a.login)) {
        if !matches!(d.state, NEEDS_REVIEW | NEEDS_REREVIEW | ASSIGNED) {
            d.reason = format!("From {} (ignored) · {}", author.login, d.reason);
            d.state = FYI;
            return d;
        }
    }
    let last_activity = parse_ts(updated_at).unwrap_or(now);
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

    // --- Human conversation: comments, submitted reviews and review-thread comments ---
    // Without thread data (e.g. older fixtures), fall back to bare COMMENTED reviews counting as messages.
    let threads_known = !pr.review_threads.nodes.is_empty();
    let mut msgs: Vec<Msg> = Vec::new();
    for c in pr.comments.iter() {
        if let (Some(a), Some(at)) = (ctx.human(&c.author), parse_ts(&c.created_at)) {
            msgs.push(Msg { who: &a.login, at, body: &c.body, thread: None, thread_id: None, conversational: true });
        }
    }
    for r in pr.reviews.iter().filter(|r| r.state != "PENDING") {
        if let (Some(a), Some(at)) = (ctx.human(&r.author), r.submitted_at.as_deref().and_then(parse_ts)) {
            // A review's thread comments are counted per thread below; its body only if it has one.
            let conversational = r.state == "CHANGES_REQUESTED"
                || (r.state == "COMMENTED" && (!r.body.trim().is_empty() || !threads_known));
            msgs.push(Msg { who: &a.login, at, body: &r.body, thread: None, thread_id: None, conversational });
        }
    }
    for (i, t) in pr.review_threads.iter().enumerate() {
        for c in t.comments.iter() {
            if let (Some(a), Some(at)) = (ctx.human(&c.author), parse_ts(&c.created_at)) {
                let thread_id = (!t.id.is_empty()).then_some(t.id.as_str());
                msgs.push(Msg { who: &a.login, at, body: &c.body, thread: Some(i), thread_id, conversational: true });
            }
        }
    }
    let convo = Conversation { msgs, me };
    let my_last_activity = convo.my_last();
    let others_after = |t: Ts| convo.others_after(t);

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
        context: None,
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
                if !who.iter().any(|w| w.eq_ignore_ascii_case(e.who)) {
                    who.push(e.who.to_string());
                }
            }
            let verb = if replies.len() == 1 { "commented" } else { "left comments" };
            let mut out = d(REPLIED, format!("{} {verb}", join_names(&who)), last.at);
            out.context = context_of(last);
            return out;
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

    let replies = convo.replies_to_me(author);
    let replied = |(reason, at, context): (String, Ts, Option<ReplyContext>)| Derived { context, ..d(REPLIED, reason, at) };
    match my_decisive {
        Some(("CHANGES_REQUESTED", reviewed_at)) => {
            if let Some(p) = last_push_at.filter(|p| *p > reviewed_at) {
                return d(NEEDS_REREVIEW, "New commits since you requested changes".into(), p);
            }
            if !replies.is_empty() {
                return replied(describe_replies(&replies));
            }
            return d(WAITING, format!("Waiting for {author} to address your changes"), reviewed_at);
        }
        Some(("APPROVED", approved_at)) => {
            if !replies.is_empty() {
                return replied(describe_replies(&replies));
            }
            return d(DONE, "You approved".into(), approved_at);
        }
        _ => {}
    }

    if !replies.is_empty() {
        return replied(describe_replies(&replies));
    }
    if let Some(mine) = my_last_activity {
        return d(WAITING, "Waiting for a response".into(), mine);
    }
    if assigned_to_me {
        return d(NEEDS_REVIEW, format!("Assigned to you{checks_note}"), created);
    }
    d(FYI, "You're subscribed".into(), parse_ts(&pr.updated_at).unwrap_or(created))
}

fn derive_issue_state(issue: &Issue, ctx: &Ctx) -> Derived {
    let me = ctx.me.as_str();
    let eq = |login: &str| login.eq_ignore_ascii_case(me);
    let author = issue.author.as_ref().map(|a| a.login.as_str()).unwrap_or("ghost");
    let created = parse_ts(&issue.created_at).unwrap_or_else(Utc::now);
    let authored = eq(author);
    let assigned = issue.assignees.iter().any(|a| eq(&a.login));
    let d = |state: &'static str, reason: String, since: Ts| Derived {
        state,
        reason,
        since,
        is_mine: authored || assigned,
        ci: Ci::None,
        context: None,
    };

    let mut msgs: Vec<Msg> = Vec::new();
    for c in issue.comments.iter() {
        if let (Some(a), Some(at)) = (ctx.human(&c.author), parse_ts(&c.created_at)) {
            msgs.push(Msg { who: &a.login, at, body: &c.body, thread: None, thread_id: None, conversational: true });
        }
    }
    let convo = Conversation { msgs, me };
    let my_last = convo.my_last();

    if authored {
        let replies = convo.others_after(my_last.unwrap_or(created));
        if let Some(last) = replies.iter().max_by_key(|m| m.at) {
            let reason = if replies.len() == 1 {
                format!("{} commented on your issue", last.who)
            } else {
                format!("{} new comments on your issue", replies.len())
            };
            return Derived { context: context_of(last), ..d(REPLIED, reason, last.at) };
        }
    } else {
        let replies = convo.replies_to_me(author);
        if !replies.is_empty() {
            let (reason, at, context) = describe_replies(&replies);
            return Derived { context, ..d(REPLIED, reason, at) };
        }
    }

    if assigned {
        let assigned_at = issue
            .timeline_items
            .iter()
            .filter(|t| t.typename == "AssignedEvent")
            .filter(|t| t.assignee.as_ref().and_then(|a| a.login.as_deref()).is_some_and(|l| eq(l)))
            .filter_map(|t| t.created_at.as_deref().and_then(parse_ts))
            .max()
            .unwrap_or(created);
        if my_last.map_or(true, |t| t < assigned_at) {
            return d(ASSIGNED, "Assigned to you".into(), assigned_at);
        }
        return d(WAITING, "Assigned to you · waiting for a response".into(), my_last.unwrap_or(assigned_at));
    }
    if let Some(t) = my_last {
        let reason = if authored { "Your issue · no new replies" } else { "Waiting for a response" };
        return d(WAITING, reason.into(), t);
    }
    if authored {
        return d(WAITING, "Your issue · no replies yet".into(), created);
    }
    d(FYI, "You're subscribed".into(), parse_ts(&issue.updated_at).unwrap_or(created))
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

    fn comment(id: &str, login: &str, at: &str, body: &str) -> serde_json::Value {
        json!({"id": id, "createdAt": at, "body": body, "author": {"__typename": "User", "login": login}})
    }

    #[test]
    fn third_party_chatter_is_not_a_reply() {
        // I commented; then someone else (not the author, no mention) talks to someone else.
        let p = pr(json!({"comments": {"nodes": [
            comment("c1", "me", "2026-01-02T00:00:00Z", "Looks fine"),
            comment("c2", "bob", "2026-01-03T00:00:00Z", "@carol can you check the migration?")
        ]}}));
        assert_eq!(derive(&p, "me").state, WAITING);
    }

    #[test]
    fn mention_is_a_reply() {
        let p = pr(json!({"comments": {"nodes": [comment("c1", "bob", "2026-01-03T00:00:00Z", "cc @me, thoughts?")]}}));
        let d = derive(&p, "me");
        assert_eq!(d.state, REPLIED);
        assert!(d.reason.contains("mentioned"));
        assert!(!mentions("email me@example.com", "me"));
        assert!(!mentions("@meghan", "me"));
    }

    #[test]
    fn reply_in_my_thread() {
        let p = pr(json!({
            "reviews": {"nodes": [review("me", "COMMENTED", "2026-01-02T00:00:00Z")]},
            "reviewThreads": {"nodes": [{"comments": {"nodes": [
                comment("t1", "me", "2026-01-02T00:00:00Z", "Why 500ms?"),
                comment("t2", "bob", "2026-01-03T00:00:00Z", "Fair, changed")
            ]}}, {"comments": {"nodes": [
                comment("t3", "carol", "2026-01-04T00:00:00Z", "unrelated thread")
            ]}}]}
        }));
        let d = derive(&p, "me");
        assert_eq!(d.state, REPLIED);
        assert!(d.reason.contains("bob") && d.reason.contains("thread"), "{}", d.reason);
    }

    fn issue(extra: serde_json::Value) -> Issue {
        let mut base = json!({
            "id": "I_1", "number": 5, "title": "Bug", "url": "u", "state": "OPEN",
            "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z", "closedAt": null,
            "author": {"__typename": "User", "login": "sarah", "avatarUrl": null},
            "repository": {"nameWithOwner": "acme/web"},
            "assignees": {"nodes": []}, "comments": {"nodes": []}, "timelineItems": {"nodes": []}
        });
        for (k, v) in extra.as_object().unwrap() {
            base[k] = v.clone();
        }
        serde_json::from_value(base).unwrap()
    }

    #[test]
    fn issue_states() {
        let at = parse_ts("2026-01-05T00:00:00Z").unwrap();
        let c = Ctx::new("me", &[]);
        let assigned = issue(json!({"assignees": {"nodes": [{"login": "me"}]},
            "timelineItems": {"nodes": [{"__typename": "AssignedEvent", "createdAt": "2026-01-02T00:00:00Z", "assignee": {"__typename": "User", "login": "me"}}]}}));
        assert_eq!(derive_issue_at(&assigned, &c, at).state, ASSIGNED);
        let mine = issue(json!({"author": {"__typename": "User", "login": "me", "avatarUrl": null},
            "comments": {"nodes": [comment("c1", "bob", "2026-01-03T00:00:00Z", "I can repro")]}}));
        assert_eq!(derive_issue_at(&mine, &c, at).state, REPLIED);
        let subscribed = issue(json!({"comments": {"nodes": [comment("c1", "bob", "2026-01-03T00:00:00Z", "hm")]}}));
        assert_eq!(derive_issue_at(&subscribed, &c, at).state, FYI);
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
