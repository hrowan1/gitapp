//! "Ask your history": answer a question about your own work from what Dev Inbox has recorded —
//! PR activity, ticket moves, meetings (notes, action items, transcript lines) and what's open now.
//! Retrieval is deterministic: tickets named in the question, a time range from its wording (default
//! the last two weeks) and keyword matches. The facts go to the model as tagged lines ([S3]) so the
//! answer can cite them, and the app turns citations into links.

use anyhow::Result;
use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone, Weekday};
use regex::Regex;
use rusqlite::Connection;
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::LazyLock;

use crate::derive::{fmt_ts, parse_ts, Ts};

/// Something the answer can cite.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    /// "S1", "S2"…
    pub id: String,
    /// pr | ticket | meeting
    pub kind: String,
    pub label: String,
    pub url: Option<String>,
    /// A ticket key or meeting id to open in the app.
    pub target: Option<String>,
    /// For a meeting: seconds into it (a transcript line).
    pub at: Option<f64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Answer {
    pub answer: String,
    pub sources: Vec<Source>,
    /// "apple" or the Claude model that answered.
    pub by: String,
    /// The period the facts came from, in words ("the last two weeks").
    pub period: String,
}

/// The period a question is about, in local time: (start, end, words).
pub fn period(question: &str, now: chrono::DateTime<Local>) -> (Ts, Ts, String) {
    let q = question.to_lowercase();
    let today = now.date_naive();
    let day_start = |d: NaiveDate| Local.from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap()).earliest().unwrap().to_utc();
    let monday = today - Duration::days(today.weekday().num_days_from_monday() as i64);
    let end = now.to_utc() + Duration::minutes(1);
    let has = |w: &str| Regex::new(&format!(r"\b{w}\b")).unwrap().is_match(&q);
    if has("today") {
        return (day_start(today), end, "today".into());
    }
    if has("yesterday") {
        return (day_start(today - Duration::days(1)), day_start(today), "yesterday".into());
    }
    if has("last week") {
        return (day_start(monday - Duration::days(7)), day_start(monday), "last week".into());
    }
    if has("this week") || has("week") {
        return (day_start(monday), end, "this week".into());
    }
    if has("last month") {
        let first = today.with_day(1).unwrap();
        let prev = (first - Duration::days(1)).with_day(1).unwrap();
        return (day_start(prev), day_start(first), "last month".into());
    }
    if has("this month") || has("month") {
        return (day_start(today.with_day(1).unwrap()), end, "this month".into());
    }
    static LAST_N: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(?:last|past)\s+(\d{1,3})\s+days?\b").unwrap());
    if let Some(c) = LAST_N.captures(&q) {
        let n: i64 = c[1].parse().unwrap_or(14);
        return (day_start(today - Duration::days(n - 1)), end, format!("the last {n} days"));
    }
    for (name, wd) in [
        ("monday", Weekday::Mon),
        ("tuesday", Weekday::Tue),
        ("wednesday", Weekday::Wed),
        ("thursday", Weekday::Thu),
        ("friday", Weekday::Fri),
        ("saturday", Weekday::Sat),
        ("sunday", Weekday::Sun),
    ] {
        if q.contains(name) {
            // The most recent one (today counts).
            let back = (today.weekday().num_days_from_monday() as i64 - wd.num_days_from_monday() as i64).rem_euclid(7);
            let d = today - Duration::days(back);
            return (day_start(d), day_start(d + Duration::days(1)), format!("{} ({})", capitalise(name), d.format("%-d %b")));
        }
    }
    (day_start(today - Duration::days(13)), end, "the last two weeks".into())
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// Ticket keys in a question, any case ("bill-142" → "BILL-142").
pub fn tickets_in(question: &str) -> Vec<String> {
    static KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b([a-z][a-z0-9]{1,9}-\d{1,6})\b").unwrap());
    let mut out: Vec<String> = KEY.captures_iter(question).map(|c| c[1].to_uppercase()).collect();
    out.dedup();
    out
}

/// The words worth searching for: not time words, ticket keys or filler.
pub fn keywords(question: &str) -> Vec<String> {
    const SKIP: &[&str] = &[
        "what", "did", "do", "does", "done", "was", "were", "the", "and", "for", "with", "about", "this", "that", "last",
        "week", "today", "yesterday", "month", "days", "day", "who", "when", "where", "which", "how", "why", "have", "has",
        "had", "any", "all", "say", "said", "says", "tell", "show", "list", "give", "summary", "summarise", "summarize",
        "work", "worked", "working", "on", "in", "of", "to", "my", "me", "i", "we", "our", "is", "are", "it", "its", "a",
        "an", "there", "still", "open", "anything", "something", "past", "monday", "tuesday", "wednesday", "thursday",
        "friday", "saturday", "sunday", "from", "since", "get", "got", "can", "could", "should", "would", "you", "your",
        "decided", "decide", "happen", "happened", "need", "needs",
    ];
    let keys = tickets_in(question);
    question
        .split(|c: char| !c.is_alphanumeric() && c != '-')
        .map(|w| w.trim_matches('-').to_lowercase())
        .filter(|w| w.len() > 2 && !SKIP.contains(&w.as_str()) && !keys.iter().any(|k| k.eq_ignore_ascii_case(w)))
        .filter(|w| !w.chars().all(|c| c.is_ascii_digit()))
        .collect()
}

fn when(at: &str) -> String {
    parse_ts(at).map(|t| t.with_timezone(&Local).format("%a %-d %b %H:%M").to_string()).unwrap_or_default()
}

/// A ticket movement in words: "You moved it To Do → In Progress", "Sam: Assigned to You".
fn move_words(who: &str, kind: &str, detail: &str) -> String {
    match kind {
        "status" => format!("{who} moved it {detail}"),
        "created" => format!("{who} created it"),
        "comment" | "mention" => format!("{who}: {detail}"),
        _ => format!("{who}: {detail}"),
    }
}

fn mmss(s: f64) -> String {
    let t = s.max(0.0) as u64;
    format!("{}:{:02}", t / 60, t % 60)
}

/// Collects tagged facts until the engine's budget is used.
struct Facts {
    text: String,
    sources: Vec<Source>,
    budget: usize,
}

impl Facts {
    fn source(&mut self, kind: &str, label: &str, url: Option<String>, target: Option<String>, at: Option<f64>) -> Option<String> {
        if let Some(s) = self.sources.iter().find(|s| s.kind == kind && s.label == label && s.at == at) {
            return Some(s.id.clone());
        }
        let id = format!("S{}", self.sources.len() + 1);
        self.sources.push(Source { id: id.clone(), kind: kind.into(), label: label.into(), url, target, at });
        Some(id)
    }

    /// Add a block; false once the budget is spent (the block is cut to fit, then nothing more goes in).
    fn block(&mut self, heading: &str, body: &str) -> bool {
        if body.trim().is_empty() {
            return true;
        }
        let room = self.budget.saturating_sub(self.text.chars().count());
        if room < 200 {
            return false;
        }
        let block = format!("## {heading}\n{body}\n");
        self.text.push_str(&crate::ai::fit(&block, room));
        self.text.chars().count() < self.budget
    }
}

/// Everything relevant to a question, as tagged lines, within `budget` characters.
pub fn gather(conn: &Connection, question: &str, me: &str, budget: usize) -> Result<(String, Vec<Source>, String)> {
    let (start, end, period_words) = period(question, Local::now());
    let (start_s, end_s) = (fmt_ts(start), fmt_ts(end));
    let mut f = Facts { text: String::new(), sources: Vec::new(), budget };

    // 1. Tickets named in the question: what they are, how they moved, their PRs and meetings.
    for key in tickets_in(question).into_iter().take(3) {
        let mut body = String::new();
        let info: Option<(String, String, Option<String>, Option<String>, String)> = conn
            .query_row(
                "SELECT summary, status, assignee, description, url FROM jira_issues WHERE key = ?1",
                [&key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .ok();
        if let Some((summary, status, assignee, description, url)) = &info {
            let s = f.source("ticket", &format!("{key} {summary}"), Some(url.clone()), Some(key.clone()), None).unwrap();
            body.push_str(&format!("[{s}] {key} \"{summary}\" — now {status}, assigned to {}\n", assignee.as_deref().unwrap_or("nobody")));
            if let Some(d) = description {
                body.push_str(&format!("Description: {}\n", d.chars().take(400).collect::<String>().replace('\n', " ")));
            }
            for e in crate::jira::ticket_events(conn, &key, me)?.into_iter().take(12) {
                let who = if e.actor_is_me { "You".to_string() } else { e.actor.unwrap_or_else(|| "Someone".into()) };
                body.push_str(&format!("[{s}] {} {}\n", when(&e.at), move_words(&who, &e.kind, &e.detail)));
            }
        }
        let like = format!("%{key}%");
        let mut stmt = conn.prepare(
            "SELECT repo, number, title, url, is_open FROM prs WHERE kind = 'pr' AND (upper(title) LIKE ?1 OR upper(COALESCE(branch, '')) LIKE ?1)
             ORDER BY updated_at DESC LIMIT 5",
        )?;
        let prs: Vec<(String, i64, String, String, bool)> =
            stmt.query_map([&like], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?.collect::<Result<_, _>>()?;
        for (repo, number, title, url, open) in prs {
            let s = f.source("pr", &format!("{repo}#{number} {title}"), Some(url), None, None).unwrap();
            body.push_str(&format!("[{s}] PR {repo}#{number} \"{title}\" ({})\n", if open { "open" } else { "closed/merged" }));
        }
        for m in crate::meetings::for_ticket(conn, &key)?.into_iter().take(4) {
            meeting_lines(conn, &mut f, &mut body, &m)?;
        }
        if info.is_none() && body.is_empty() {
            body.push_str(&format!("Nothing recorded about {key}.\n"));
        }
        if !f.block(&format!("Ticket {key}"), &body) {
            return Ok((f.text, f.sources, period_words));
        }
    }

    // 2. Keyword matches anywhere: meeting lines and notes, PR titles, tickets.
    let words = keywords(question);
    if !words.is_empty() {
        let mut body = String::new();
        for r in crate::search::search(conn, &words.join(" "))?.into_iter().take(4) {
            for h in r.hits.iter().filter(|h| h.kind != "title").take(3) {
                let text = h.snippet.replace(['\u{2}', '\u{3}'], "");
                let label = format!("{} ({})", r.title, when(&r.started_at));
                let s = f.source("meeting", &label, None, Some(r.meeting_id.clone()), h.start).unwrap();
                match h.start {
                    Some(t) => body.push_str(&format!("[{s}] Meeting \"{}\" {} at {}: {}: {text}\n", r.title, when(&r.started_at), mmss(t), h.speaker)),
                    None => body.push_str(&format!("[{s}] Meeting \"{}\" {} notes: {text}\n", r.title, when(&r.started_at))),
                }
            }
        }
        let pattern = format!("%{}%", words.join("%"));
        let mut stmt = conn.prepare("SELECT repo, number, title, url, is_open FROM prs WHERE lower(title) LIKE ?1 ORDER BY updated_at DESC LIMIT 5")?;
        let prs: Vec<(String, i64, String, String, bool)> =
            stmt.query_map([&pattern], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?.collect::<Result<_, _>>()?;
        for (repo, number, title, url, open) in prs {
            let s = f.source("pr", &format!("{repo}#{number} {title}"), Some(url), None, None).unwrap();
            body.push_str(&format!("[{s}] PR {repo}#{number} \"{title}\" ({})\n", if open { "open" } else { "closed/merged" }));
        }
        let mut stmt = conn.prepare("SELECT key, summary, status, url FROM jira_issues WHERE lower(summary) LIKE ?1 ORDER BY updated DESC LIMIT 5")?;
        let tickets: Vec<(String, String, String, String)> =
            stmt.query_map([&pattern], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<Result<_, _>>()?;
        for (key, summary, status, url) in tickets {
            let s = f.source("ticket", &format!("{key} {summary}"), Some(url), Some(key.clone()), None).unwrap();
            body.push_str(&format!("[{s}] Ticket {key} \"{summary}\" — {status}\n"));
        }
        if !f.block(&format!("Mentions of “{}”", words.join(" ")), &body) {
            return Ok((f.text, f.sources, period_words));
        }
    }

    // 3. The period: meetings, ticket moves, PR activity.
    let mut body = String::new();
    for m in crate::meetings::started_between(conn, &start_s, &end_s)? {
        meeting_lines(conn, &mut f, &mut body, &m)?;
    }
    if !f.block(&format!("Meetings — {period_words}"), &body) {
        return Ok((f.text, f.sources, period_words));
    }

    let mut body = String::new();
    let mut by_ticket: BTreeMap<String, Vec<crate::jira::DayMove>> = BTreeMap::new();
    for m in crate::jira::moves_between(conn, &start_s, &end_s, me)? {
        by_ticket.entry(m.key.clone()).or_default().push(m);
    }
    for (key, moves) in by_ticket {
        let first = &moves[0];
        let s = f.source("ticket", &format!("{key} {}", first.summary), Some(first.url.clone()), Some(key.clone()), None).unwrap();
        body.push_str(&format!("[{s}] {key} \"{}\" (now {}):", first.summary, first.status));
        for m in &moves {
            let who = if m.actor_is_me { "You".to_string() } else { m.actor.clone().unwrap_or_else(|| "Someone".into()) };
            body.push_str(&format!(" {} {};", when(&m.at), move_words(&who, &m.kind, &m.detail)));
        }
        body.push('\n');
    }
    if !f.block(&format!("Jira ticket moves — {period_words}"), &body) {
        return Ok((f.text, f.sources, period_words));
    }

    let mut body = String::new();
    // Per PR (or branch): events in time order; local editing samples collapse into a count.
    struct Work {
        title: String,
        url: Option<String>,
        events: Vec<(Ts, String)>,
        edits: usize,
    }
    let mut by_pr: BTreeMap<String, Work> = BTreeMap::new();
    for e in crate::day::gather(conn, &start_s, &end_s)? {
        let (key, title, url) = match &e.pr {
            Some((key, title, url)) => (key.clone(), title.clone(), Some(url.clone())),
            None => (format!("{} {}", e.repo, e.branch.clone().unwrap_or_default()).trim().to_string(), String::new(), e.url.clone()),
        };
        let w = by_pr.entry(key).or_insert(Work { title, url, events: Vec::new(), edits: 0 });
        if e.kind == "edit" {
            w.edits += 1;
        } else {
            w.events.push((e.at, e.text));
        }
    }
    for (key, mut w) in by_pr {
        w.events.sort_by_key(|(at, _)| *at);
        let label = if w.title.is_empty() { key.clone() } else { format!("{key} {}", w.title) };
        let s = f.source("pr", &label, w.url, None, None).unwrap();
        let mut parts: Vec<String> =
            w.events.iter().take(8).map(|(at, text)| format!("{} {text}", at.with_timezone(&Local).format("%a %H:%M"))).collect();
        if w.events.len() > 8 {
            parts.push(format!("…and {} more", w.events.len() - 8));
        }
        if w.edits > 0 {
            parts.push(format!("edited files locally (~{} min)", w.edits * 5));
        }
        let title = if w.title.is_empty() { String::new() } else { format!(" \"{}\"", w.title) };
        body.push_str(&format!("[{s}] {key}{title}: {}\n", parts.join("; ")));
    }
    if !f.block(&format!("Your GitHub activity — {period_words}"), &body) {
        return Ok((f.text, f.sources, period_words));
    }

    // 4. What's open right now.
    let mut body = String::new();
    for a in crate::summary::open_actions(conn, &fmt_ts(chrono::Utc::now()))?.into_iter().take(10) {
        let s = f.source("meeting", &format!("{} ({})", a.meeting_title, when(&a.meeting_at)), None, Some(a.meeting_id.clone()), None).unwrap();
        body.push_str(&format!("[{s}] Open action item for you from \"{}\": {}\n", a.meeting_title, a.text));
    }
    for item in crate::db::list_items(conn)?.into_iter().filter(|i| crate::derive::is_actionable(&i.state)).take(10) {
        let s = f.source("pr", &format!("{}#{} {}", item.repo, item.number, item.title), Some(item.url.clone()), None, None).unwrap();
        body.push_str(&format!("[{s}] Needs you: {}#{} \"{}\" — {}\n", item.repo, item.number, item.title, item.reason));
    }
    f.block("Open right now", &body);
    Ok((f.text, f.sources, period_words))
}

fn meeting_lines(conn: &Connection, f: &mut Facts, body: &mut String, m: &crate::meetings::Meeting) -> Result<()> {
    let label = format!("{} ({})", m.title, when(&m.started_at));
    let s = f.source("meeting", &label, None, Some(m.id.clone()), None).unwrap();
    body.push_str(&format!("[{s}] Meeting \"{}\" {}", m.title, when(&m.started_at)));
    if let Some(t) = &m.ticket {
        body.push_str(&format!(" (ticket {t})"));
    }
    body.push('\n');
    match &m.summary {
        Some(n) => {
            body.push_str(&format!("[{s}] Notes: {}\n", n.overview));
            for d in &n.decisions {
                body.push_str(&format!("[{s}] Decided: {d}\n"));
            }
            let progress = crate::summary::meeting_actions(conn, &m.id)?;
            for a in &n.action_items {
                let done = progress.iter().any(|p| p.text == a.text.trim() && p.done);
                body.push_str(&format!("[{s}] Action ({}){}: {}\n", a.owner, if done { ", done" } else { "" }, a.text));
            }
        }
        None => {
            if let Some(p) = &m.preview {
                body.push_str(&format!("[{s}] Starts: {}\n", p.chars().take(200).collect::<String>()));
            }
        }
    }
    Ok(())
}

pub const INSTRUCTIONS: &str = "You answer questions about the user's own work, using only the facts given: their \
GitHub activity, Jira ticket moves, meetings (notes, decisions, action items, transcript lines) and what's open. \
\"You\" in the facts is the user. Cite the facts you use with their tags, like [S3], right after the claim. Be \
brief: one or two sentences, or a few short bullet points for lists. If the facts don't answer the question, say \
so plainly and suggest what to look at instead — never guess or invent names, dates or numbers.";

/// The prompt for one question.
pub fn prompt(question: &str, facts: &str, period: &str) -> String {
    let today = Local::now().format("%A %-d %B %Y, %H:%M");
    format!("Today is {today}. The facts below cover {period} unless they say otherwise.\n\n{facts}\nQuestion: {question}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_periods_tickets_and_keywords() {
        // Thursday 16 Oct 2026, 15:00 local.
        let now = Local.with_ymd_and_hms(2026, 10, 15, 15, 0, 0).unwrap() + Duration::days(1);
        let local_date = |t: Ts| t.with_timezone(&Local).date_naive().to_string();
        let (s, _, w) = period("what did I do this week?", now);
        assert_eq!((local_date(s).as_str(), w.as_str()), ("2026-10-12", "this week"));
        let (s, e, _) = period("anything yesterday", now);
        assert_eq!((local_date(s).as_str(), local_date(e).as_str()), ("2026-10-15", "2026-10-16"));
        let (s, e, w) = period("what was decided on Tuesday?", now);
        assert_eq!((local_date(s).as_str(), local_date(e).as_str()), ("2026-10-13", "2026-10-14"));
        assert!(w.starts_with("Tuesday"));
        let (s, _, w) = period("past 3 days", now);
        assert_eq!((local_date(s).as_str(), w.as_str()), ("2026-10-14", "the last 3 days"));
        assert_eq!(period("what about proration", now).2, "the last two weeks");

        assert_eq!(tickets_in("what happened on bill-142 and ES-305?"), ["BILL-142", "ES-305"]);
        assert_eq!(keywords("What did Sam say about VAT on ES-305 last week?"), ["sam", "vat"]);
    }

    /// Print what the model would be given: `QUESTION="…" cargo test show_facts -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn show_facts() {
        let path = std::env::temp_dir().join(format!("dev-inbox-ask-show-{}.sqlite3", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let conn = crate::db::open(&path).unwrap();
        crate::demo::seed(&conn).unwrap();
        let q = std::env::var("QUESTION").unwrap_or_else(|_| "what did I do this week?".into());
        let (facts, sources, period) = gather(&conn, &q, "demo-me", 8_000).unwrap();
        println!("{}\n---\n{facts}\n--- {} sources", prompt(&q, "", &period), sources.len());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn gathers_tagged_facts_within_budget() {
        let path = std::env::temp_dir().join(format!("dev-inbox-ask-{}.sqlite3", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let conn = crate::db::open(&path).unwrap();
        crate::demo::seed(&conn).unwrap();
        let (facts, sources, _) = gather(&conn, "what happened with BILL-142 proration?", "demo-me", 8_000).unwrap();
        assert!(facts.contains("## Ticket BILL-142"), "{facts}");
        assert!(facts.contains("Decided: Prorate on the day the plan changes"), "{facts}");
        assert!(sources.iter().any(|s| s.kind == "meeting" && s.target.as_deref() == Some("demo-proration")));
        assert!(facts.chars().count() <= 8_000);
        // Every tag in the facts is a real source.
        for tag in Regex::new(r"\[(S\d+)\]").unwrap().captures_iter(&facts) {
            assert!(sources.iter().any(|s| s.id == tag[1]), "{}", &tag[1]);
        }
        let (small, _, _) = gather(&conn, "what did I do this week?", "demo-me", 1_000).unwrap();
        assert!(small.chars().count() <= 1_000);
        let _ = std::fs::remove_file(&path);
    }
}
