//! The Day view: turn a day's events (commits, pushes, reviews, comments, local editing) into
//! time blocks grouped by ticket. Deterministic; the user can correct blocks, which freezes the day.
//!
//! Blocks are shaped like Jira worklogs (ticket key, start, end, description) so they can be
//! submitted to Jira later.

use anyhow::Result;
use chrono::Duration;
use regex::Regex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::LazyLock;

use crate::derive::{fmt_ts, parse_ts, Ts};

/// Events closer together than this belong to the same working session.
const SESSION_GAP_MIN: i64 = 30;
/// Time after the last event of a session (wrapping up).
const TAIL_MIN: i64 = 5;
/// Sessions shorter than this are padded to it.
const MIN_BLOCK_MIN: i64 = 10;

/// How much work an event implies *before* it happened.
fn lead_minutes(kind: &str) -> i64 {
    match kind {
        "committed" | "local_commit" => 25,
        "reviewed" => 15,
        "pushed" | "opened" => 10,
        "commented" => 5,
        "edit" => 5, // local samples arrive every 5 minutes
        "merged" => 3,
        "jira" => 5,
        _ => 5,
    }
}

#[derive(Debug, Clone)]
pub struct WorkEvent {
    pub at: Ts,
    /// committed | local_commit | pushed | reviewed | commented | opened | merged | edit
    pub kind: String,
    pub repo: String,
    pub branch: Option<String>,
    /// "owner/repo#123", PR title, url — when the event belongs to a known PR.
    pub pr: Option<(String, String, String)>,
    pub text: String,
    pub url: Option<String>,
    /// Dedupe key (e.g. commit sha) so a commit seen locally and on GitHub counts once.
    pub dedupe: Option<String>,
    /// An explicit ticket (Jira events), instead of finding one in the branch or title.
    pub ticket: Option<String>,
    /// Whether it implies time spent. Others moving your ticket is shown but adds no time.
    pub counts: bool,
    /// An explicit (key, label) group, e.g. a recorded meeting with no ticket.
    pub group: Option<(String, String)>,
}

static BRANCH_TICKET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(?:^|[/_\-.])([a-z]{2,}[a-z0-9]*-\d{1,6})(?:$|[/_\-.])").unwrap());
static TEXT_TICKET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b([A-Z]{2,}[A-Z0-9]*-\d{1,6})\b").unwrap());
const NOT_TICKETS: &[&str] = &["UTF", "SHA", "ISO", "RFC", "HTTP", "TLS", "AES", "RSA", "MD", "CVE", "PR", "GH"];

fn valid(t: &str) -> bool {
    let project = t.split('-').next().unwrap_or_default();
    !NOT_TICKETS.contains(&project)
}

/// A Jira-style ticket key from a branch name ("io-2641-dealflow" → "IO-2641"). A version number
/// isn't one: "update-gin-3.1.0" and "drupal-10.6.19" have no ticket.
pub fn ticket_in_branch(branch: &str) -> Option<String> {
    BRANCH_TICKET.captures_iter(branch).find_map(|c| {
        let m = c.get(1)?;
        let mut rest = branch[m.end()..].chars();
        let version = rest.next() == Some('.') && rest.next().is_some_and(|d| d.is_ascii_digit());
        Some(m.as_str().to_ascii_uppercase()).filter(|t| !version && valid(t))
    })
}

/// A ticket key written in a title or message (must be upper-case there: "IO-2641").
pub fn ticket_in_text(text: &str) -> Option<String> {
    TEXT_TICKET.captures(text).map(|c| c[1].to_string()).filter(|t| valid(t))
}


/// A merge commit ("Merge pull request #12 from …", "Merge branch 'main' into …") is a click, not
/// an hour of coding: it counts like merging a PR.
fn is_merge(message: &str) -> bool {
    ["Merge pull request #", "Merge branch ", "Merge remote-tracking branch "].iter().any(|p| message.starts_with(p))
}

fn is_trunk(branch: &str) -> bool {
    matches!(branch, "main" | "master" | "develop" | "dev" | "trunk")
}

fn short_repo(repo: &str) -> &str {
    repo.split('/').nth(1).unwrap_or(repo)
}

/// What an event counts towards: (key, label, url). Ticket > PR > branch > repo. When the user's
/// Jira projects are known (`projects` isn't empty) only their keys are tickets, so a branch named
/// "core-10" doesn't hide the "ES-636" in its PR's title.
pub fn work_key(e: &WorkEvent, projects: &HashSet<String>) -> (String, String, Option<String>) {
    if let Some((key, label)) = &e.group {
        return (key.clone(), label.clone(), e.url.clone());
    }
    if let Some(t) = &e.ticket {
        return (format!("T:{t}"), t.clone(), e.url.clone());
    }
    let title = e.pr.as_ref().map(|p| p.1.as_str());
    let known = |t: &String| projects.is_empty() || t.split('-').next().is_some_and(|p| projects.contains(p));
    let ticket = [e.branch.as_deref().and_then(ticket_in_branch), title.and_then(ticket_in_text), ticket_in_text(&e.text)]
        .into_iter()
        .flatten()
        .find(known);
    if let Some(t) = ticket {
        let what = title.map(String::from).or_else(|| e.branch.clone()).unwrap_or_else(|| e.text.clone());
        // "IO-2641 - Dealflow…" / "io-2641-dealflow" → don't repeat the key.
        let what = match what.get(..t.len()) {
            Some(head) if head.eq_ignore_ascii_case(&t) => what[t.len()..].trim_start_matches([' ', ':', '-', '_', '/', '·']).to_string(),
            _ => what,
        };
        let label = if what.is_empty() { t.clone() } else { format!("{t} · {what}") };
        return (format!("T:{t}"), label, e.pr.as_ref().map(|p| p.2.clone()));
    }
    if let Some((r, title, url)) = &e.pr {
        return (format!("PR:{r}"), title.clone(), Some(url.clone()));
    }
    if let Some(b) = e.branch.as_deref().filter(|b| !is_trunk(b)) {
        return (format!("B:{}:{b}", e.repo), format!("{} · {b}", short_repo(&e.repo)), None);
    }
    (format!("R:{}", e.repo), e.repo.clone(), Some(format!("https://github.com/{}", e.repo)))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    pub id: Option<i64>,
    pub key: String,
    pub label: String,
    pub start: String,
    pub end: String,
    pub minutes: i64,
    pub note: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventOut {
    pub at: String,
    pub kind: String,
    pub text: String,
    pub url: Option<String>,
    pub repo: String,
    pub branch: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub key: String,
    pub label: String,
    pub url: Option<String>,
    /// Jira status, when the group is a known ticket.
    pub status: Option<String>,
    pub minutes: i64,
    pub events: Vec<EventOut>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayReport {
    pub day: String,
    pub frozen: bool,
    pub total_minutes: i64,
    pub blocks: Vec<Block>,
    pub groups: Vec<Group>,
    /// Every movement that day on Jira tickets that involve you (filled in by the caller).
    pub ticket_moves: Vec<crate::jira::DayMove>,
    /// Meetings recorded that day (filled in by the caller; listed even on days you've edited).
    pub meetings: Vec<crate::meetings::Meeting>,
}

/// Total minutes covered by a set of intervals, counting overlaps once.
fn union_minutes(mut spans: Vec<(Ts, Ts)>) -> i64 {
    spans.sort();
    let mut total = Duration::zero();
    let mut cur: Option<(Ts, Ts)> = None;
    for (s, e) in spans {
        match cur {
            Some((cs, ce)) if s <= ce => cur = Some((cs, ce.max(e))),
            Some((cs, ce)) => {
                total += ce - cs;
                cur = Some((s, e));
            }
            None => cur = Some((s, e)),
        }
    }
    if let Some((cs, ce)) = cur {
        total += ce - cs;
    }
    total.num_minutes()
}

/// Minutes per key with overlapping time shared out: while several keys' blocks overlap, each
/// gets an equal part of that stretch. The keys then add up to the day's total — counting each
/// key's own blocks separately made fifteen PR reviews in one sitting fifteen sittings.
fn shared_minutes(spans: &[(&str, Ts, Ts)], total: i64) -> HashMap<String, i64> {
    let mut edges: Vec<Ts> = spans.iter().flat_map(|(_, s, e)| [*s, *e]).collect();
    edges.sort();
    edges.dedup();
    let mut seconds: BTreeMap<&str, f64> = BTreeMap::new();
    for w in edges.windows(2) {
        let (a, b) = (w[0], w[1]);
        let mut active: Vec<&str> = spans.iter().filter(|(_, s, e)| *s <= a && *e >= b).map(|(k, _, _)| *k).collect();
        active.sort();
        active.dedup();
        for k in &active {
            *seconds.entry(k).or_default() += (b - a).num_seconds() as f64 / active.len() as f64;
        }
    }
    // Whole minutes that still add up to `total` (largest remainders get the spare minutes).
    let mut out: Vec<(&str, i64, f64)> =
        seconds.iter().map(|(k, s)| (*k, (s / 60.0).floor() as i64, s / 60.0 - (s / 60.0).floor())).collect();
    let spare = total - out.iter().map(|o| o.1).sum::<i64>();
    out.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    for o in out.iter_mut().take(spare.max(0) as usize) {
        o.1 += 1;
    }
    out.into_iter().map(|(k, m, _)| (k.to_string(), m)).collect()
}

/// Ticket key → (Jira summary, url, status), to label ticket groups with their real titles.
pub type Titles = HashMap<String, (String, String, String)>;

/// Group events into sessions per key. `manual` (a frozen day) replaces the generated blocks.
pub fn build(
    day: &str,
    mut events: Vec<WorkEvent>,
    manual: Option<Vec<Block>>,
    day_start: Ts,
    day_end: Ts,
    titles: &Titles,
    meetings: &[(String, String, String, String)],
) -> DayReport {
    // Recorded meetings are exact: listed as events and added as fixed blocks below.
    for (key, label, start, end) in meetings {
        let (Some(s), Some(e)) = (parse_ts(start), parse_ts(end)) else { continue };
        let ticket = key.strip_prefix("T:").map(String::from);
        let from_calendar = key.starts_with("CAL:") || label.starts_with(crate::calendar::CALENDAR_LABEL);
        events.push(WorkEvent {
            at: s,
            kind: if from_calendar { "calendar" } else { "meeting" }.into(),
            repo: String::new(),
            branch: None,
            pr: None,
            text: if from_calendar {
                let title = label.trim_start_matches(crate::calendar::CALENDAR_LABEL).trim_start_matches("Meeting · ");
                format!("In your calendar: {title} ({}m)", (e - s).num_minutes().max(1))
            } else {
                format!("Recorded meeting ({}m)", (e - s).num_minutes().max(1))
            },
            url: None,
            dedupe: None,
            group: if ticket.is_some() { None } else { Some((key.clone(), label.clone())) },
            ticket,
            counts: false,
        });
    }
    // A commit seen both locally and on GitHub counts once.
    let mut seen = std::collections::HashSet::new();
    events.retain(|e| e.dedupe.as_ref().map_or(true, |d| seen.insert(d.clone())));
    events.sort_by_key(|e| e.at);

    let projects: HashSet<String> = titles.keys().filter_map(|k| k.split('-').next()).map(String::from).collect();
    let mut by_key: BTreeMap<String, (String, Option<String>, Vec<&WorkEvent>)> = BTreeMap::new();
    for e in &events {
        let (key, label, url) = work_key(e, &projects);
        let entry = by_key.entry(key).or_insert_with(|| (label.clone(), url.clone(), Vec::new()));
        // Prefer a label that comes from a PR title.
        if e.pr.is_some() && entry.1.is_none() {
            entry.0 = label;
            entry.1 = url;
        }
        entry.2.push(e);
    }
    // Known Jira tickets get their real title and link.
    for (key, entry) in by_key.iter_mut() {
        if let Some((summary, url, _)) = key.strip_prefix("T:").and_then(|t| titles.get(t)) {
            entry.0 = format!("{} · {summary}", &key[2..]);
            entry.1 = Some(url.clone());
        }
    }

    let mut blocks: Vec<Block> = Vec::new();
    for (key, (label, _, evs)) in &by_key {
        let mut cur: Option<(Ts, Ts)> = None;
        let flush = |span: (Ts, Ts), blocks: &mut Vec<Block>| {
            let (s, mut e) = span;
            if e - s < Duration::minutes(MIN_BLOCK_MIN) {
                e = s + Duration::minutes(MIN_BLOCK_MIN);
            }
            let (s, e) = (s.max(day_start), e.min(day_end));
            blocks.push(Block {
                id: None,
                key: key.clone(),
                label: label.clone(),
                start: fmt_ts(s),
                end: fmt_ts(e),
                minutes: (e - s).num_minutes(),
                note: None,
            });
        };
        for e in evs.iter().filter(|e| e.counts) {
            let s = e.at - Duration::minutes(lead_minutes(&e.kind));
            let end = e.at + Duration::minutes(TAIL_MIN);
            cur = match cur {
                Some((cs, ce)) if s <= ce + Duration::minutes(SESSION_GAP_MIN) => Some((cs, ce.max(end))),
                Some(span) => {
                    flush(span, &mut blocks);
                    Some((s, end))
                }
                None => Some((s, end)),
            };
        }
        if let Some(span) = cur {
            flush(span, &mut blocks);
        }
    }

    for (key, _, start, end) in meetings {
        let (Some(s), Some(e)) = (parse_ts(start), parse_ts(end)) else { continue };
        // Even a very short recording shows up on the timeline.
        let e = e.max(s + Duration::minutes(1));
        let (s, e) = (s.max(day_start), e.min(day_end));
        if e > s {
            let label = by_key.get(key).map(|g| g.0.clone()).unwrap_or_default();
            blocks.push(Block { id: None, key: key.clone(), label, start: fmt_ts(s), end: fmt_ts(e), minutes: (e - s).num_minutes(), note: None });
        }
    }

    let frozen = manual.is_some();
    let blocks = manual.unwrap_or(blocks);
    let span = |b: &Block| Some((parse_ts(&b.start)?, parse_ts(&b.end)?));
    let total_minutes = union_minutes(blocks.iter().filter_map(span).collect());
    let keyed: Vec<(&str, Ts, Ts)> = blocks.iter().filter_map(|b| span(b).map(|(s, e)| (b.key.as_str(), s, e))).collect();
    let minutes_by_key = shared_minutes(&keyed, total_minutes);

    let mut groups: Vec<Group> = by_key
        .iter()
        .map(|(key, (label, url, evs))| Group {
            key: key.clone(),
            label: label.clone(),
            url: url.clone(),
            status: key.strip_prefix("T:").and_then(|t| titles.get(t)).map(|t| t.2.clone()),
            minutes: minutes_by_key.get(key.as_str()).copied().unwrap_or(0),
            events: evs
                .iter()
                .map(|e| EventOut {
                    at: fmt_ts(e.at),
                    kind: e.kind.clone(),
                    text: e.text.clone(),
                    url: e.url.clone(),
                    repo: e.repo.clone(),
                    branch: e.branch.clone(),
                })
                .collect(),
        })
        .collect();
    // Manual blocks for keys with no events (time added by hand) still need a group.
    for b in &blocks {
        if !groups.iter().any(|g| g.key == b.key) {
            groups.push(Group {
                key: b.key.clone(),
                label: b.label.clone(),
                url: None,
                status: None,
                minutes: minutes_by_key.get(b.key.as_str()).copied().unwrap_or(0),
                events: Vec::new(),
            });
        }
    }
    groups.sort_by(|a, b| b.minutes.cmp(&a.minutes).then(a.label.cmp(&b.label)));

    let mut blocks = blocks;
    blocks.sort_by(|a, b| a.start.cmp(&b.start));
    DayReport { day: day.to_string(), frozen, total_minutes, blocks, groups, ticket_moves: Vec::new(), meetings: Vec::new() }
}

// ---------- storage & gathering ----------

/// PR info by id: (repo#n, title, url, branch).
fn pr_info(conn: &Connection) -> Result<HashMap<String, (String, String, String, Option<String>)>> {
    let mut stmt = conn.prepare("SELECT id, repo, number, title, url, branch FROM prs WHERE kind = 'pr'")?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                (format!("{}#{}", r.get::<_, String>(1)?, r.get::<_, i64>(2)?), r.get(3)?, r.get(4)?, r.get(5)?),
            ))
        })?
        .collect::<Result<HashMap<_, _>, _>>()?;
    Ok(rows)
}

/// Everything the user did in [start, end) that the app knows about, except local git commits
/// (those are added by the caller, outside the database lock).
pub fn gather(conn: &Connection, start: &str, end: &str) -> Result<Vec<WorkEvent>> {
    let prs = pr_info(conn)?;
    let by_branch: HashMap<(String, String), &(String, String, String, Option<String>)> = prs
        .values()
        .filter_map(|p| {
            let repo = p.0.split('#').next()?.to_lowercase();
            Some(((repo, p.3.clone()?), p))
        })
        .collect();
    let mut out = Vec::new();

    let mut stmt = conn.prepare(
        "SELECT a.at, a.kind, a.pr_id, a.repo, a.detail, a.url, a.branch, a.key, COALESCE(p.is_mine, 0)
         FROM activity a LEFT JOIN prs p ON p.id = a.pr_id
         WHERE a.at >= ?1 AND a.at < ?2
           AND a.kind IN ('committed', 'pushed', 'reviewed', 'commented', 'opened', 'merged')",
    )?;
    let rows = stmt.query_map(params![start, end], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, Option<String>>(5)?,
            r.get::<_, Option<String>>(6)?,
            r.get::<_, String>(7)?,
            r.get::<_, bool>(8)?,
        ))
    })?;
    for row in rows {
        let (at, kind, pr_id, repo, detail, url, branch, key, is_mine) = row?;
        // Someone else merging your PR isn't time you spent; their PR merging isn't either.
        if kind == "merged" && !is_mine {
            continue;
        }
        let Some(at) = parse_ts(&at) else { continue };
        let pr = pr_id.as_ref().and_then(|id| prs.get(id));
        let branch = branch.filter(|b| !b.is_empty()).or_else(|| pr.and_then(|p| p.3.clone()));
        let repo = repo.or_else(|| pr.map(|p| p.0.split('#').next().unwrap_or_default().to_string())).unwrap_or_default();
        let pr = pr.or_else(|| branch.as_ref().and_then(|b| by_branch.get(&(repo.to_lowercase(), b.clone())).copied()));
        let text = match kind.as_str() {
            "reviewed" => format!("{detail} the PR"),
            "commented" | "opened" | "merged" => detail.clone(),
            _ => detail.clone(),
        };
        out.push(WorkEvent {
            at,
            kind: if kind == "committed" && is_merge(&detail) { "merged".into() } else { kind.clone() },
            repo,
            branch,
            pr: pr.map(|p| (p.0.clone(), p.1.clone(), p.2.clone())),
            text,
            url,
            dedupe: key.strip_prefix("commit:").map(String::from),
            ticket: None,
            counts: true,
            group: None,
        });
    }

    let mut stmt = conn.prepare("SELECT at, repo, branch FROM work_samples WHERE at >= ?1 AND at < ?2")?;
    let rows = stmt.query_map(params![start, end], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?))
    })?;
    for row in rows {
        let (at, repo, branch) = row?;
        let Some(at) = parse_ts(&at) else { continue };
        let pr = branch.as_ref().and_then(|b| by_branch.get(&(repo.to_lowercase(), b.clone())).copied());
        out.push(WorkEvent {
            at,
            kind: "edit".into(),
            text: format!("Edited files{}", branch.as_ref().map(|b| format!(" on {b}")).unwrap_or_default()),
            repo,
            branch,
            pr: pr.map(|p| (p.0.clone(), p.1.clone(), p.2.clone())),
            url: None,
            dedupe: None,
            ticket: None,
            counts: true,
            group: None,
        });
    }
    Ok(out)
}

/// Local commits from the user's clones, tied to PRs by branch where possible.
pub fn local_events(conn: &Connection, commits: Vec<(String, crate::editor::LocalCommit)>) -> Result<Vec<WorkEvent>> {
    let mut out = Vec::new();
    for (repo, c) in commits {
        let Some(at) = parse_ts(&c.at) else { continue };
        let pr = match &c.branch {
            Some(b) => crate::db::pr_for_branch(conn, &repo, b)?,
            None => None,
        };
        out.push(WorkEvent {
            at,
            kind: if is_merge(&c.message) { "merged" } else { "local_commit" }.into(),
            repo: repo.clone(),
            branch: c.branch.clone(),
            pr: pr.map(|p| (format!("{}#{}", p.repo, p.number), p.title, p.url)),
            text: c.message,
            url: Some(format!("https://github.com/{repo}/commit/{}", c.oid)),
            dedupe: Some(c.oid),
            ticket: None,
            counts: true,
            group: None,
        });
    }
    Ok(out)
}

/// Jira movements as day events: yours count as a little time, others' are context only.
pub fn jira_events(conn: &Connection, start: &str, end: &str, titles: &Titles) -> Result<Vec<WorkEvent>> {
    Ok(crate::jira::events_between(conn, start, end)?
        .into_iter()
        .filter_map(|(key, at, detail, by_me)| {
            Some(WorkEvent {
                at: parse_ts(&at)?,
                kind: "jira".into(),
                repo: String::new(),
                branch: None,
                pr: None,
                text: if by_me { detail } else { format!("{detail} (by someone else)") },
                url: titles.get(&key).map(|t| t.1.clone()),
                dedupe: None,
                ticket: Some(key),
                counts: by_me,
                group: None,
            })
        })
        .collect())
}

pub fn manual_blocks(conn: &Connection, day: &str) -> Result<Option<Vec<Block>>> {
    let frozen = conn.query_row("SELECT 1 FROM frozen_days WHERE day = ?1", [day], |_| Ok(())).optional()?.is_some();
    if !frozen {
        return Ok(None);
    }
    let mut stmt = conn.prepare("SELECT id, key, label, start, end, note FROM time_blocks WHERE day = ?1 ORDER BY start")?;
    let blocks = stmt
        .query_map([day], |r| {
            let start: String = r.get(3)?;
            let end: String = r.get(4)?;
            let minutes = match (parse_ts(&start), parse_ts(&end)) {
                (Some(s), Some(e)) => (e - s).num_minutes().max(0),
                _ => 0,
            };
            Ok(Block { id: r.get(0)?, key: r.get(1)?, label: r.get(2)?, start, end, minutes, note: r.get(5)? })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(blocks))
}

/// Replace a day's blocks with the user's corrected set and freeze the day.
pub fn save_blocks(conn: &Connection, day: &str, blocks: &[Block]) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM time_blocks WHERE day = ?1", [day])?;
    for b in blocks {
        tx.execute(
            "INSERT INTO time_blocks (day, key, label, start, end, note) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![day, b.key, b.label, b.start, b.end, b.note],
        )?;
    }
    tx.execute("INSERT OR IGNORE INTO frozen_days (day) VALUES (?1)", [day])?;
    tx.commit()?;
    Ok(())
}

/// Back to automatic: drop the corrections.
pub fn unfreeze(conn: &Connection, day: &str) -> Result<()> {
    conn.execute("DELETE FROM time_blocks WHERE day = ?1", [day])?;
    conn.execute("DELETE FROM frozen_days WHERE day = ?1", [day])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Ts {
        parse_ts(&format!("2026-10-14T{s}:00Z")).unwrap()
    }

    fn ev(at: &str, kind: &str, branch: Option<&str>, text: &str) -> WorkEvent {
        WorkEvent {
            at: t(at),
            kind: kind.into(),
            repo: "acme/web".into(),
            branch: branch.map(String::from),
            pr: None,
            text: text.into(),
            url: None,
            dedupe: None,
            ticket: None,
            counts: true,
            group: None,
        }
    }

    #[test]
    fn tickets() {
        assert_eq!(ticket_in_branch("IO-2641-dealflow-country-search").as_deref(), Some("IO-2641"));
        assert_eq!(ticket_in_branch("hair-55-remove-duplicate").as_deref(), Some("HAIR-55"));
        assert_eq!(ticket_in_branch("feature/abc-12").as_deref(), Some("ABC-12"));
        assert_eq!(ticket_in_branch("seb/trust-preview-host"), None);
        assert_eq!(ticket_in_text("feat(p4-02): register deep links"), None, "lower-case or one-letter isn't a ticket");
        assert_eq!(ticket_in_text("Fix UTF-8 handling"), None);
        assert_eq!(ticket_in_text("IO-2641 country search"), Some("IO-2641".into()));
        // Version numbers in branch names aren't tickets.
        assert_eq!(ticket_in_branch("update-gin-3.1.0"), None);
        assert_eq!(ticket_in_branch("drupal-10.6.19"), None);
        assert_eq!(ticket_in_branch("es-636-upgrade-core-10.6.18").as_deref(), Some("ES-636"));
        assert_eq!(ticket_in_branch("release-2").as_deref(), Some("RELEASE-2"), "no version after it");
    }

    #[test]
    fn merge_commits_are_merges() {
        assert!(is_merge("Merge pull request #1312 from fullfatthings/KEY-1072"));
        assert!(is_merge("Merge branch 'main' into RF-305"));
        assert!(is_merge("Merge remote-tracking branch 'origin/main' into io-2635"));
        assert!(!is_merge("Merge the two settings pages"), "a commit about merging things");
        assert_eq!(lead_minutes("merged"), 3, "a click, not 25 minutes of coding");
    }

    #[test]
    fn known_jira_projects_win() {
        let mut e = ev("09:00", "committed", Some("core-10-upgrade"), "Upgrade core");
        e.pr = Some(("acme/site#9".into(), "ES-636 - Upgrade Drupal core".into(), "https://github.com/acme/site/pull/9".into()));
        let projects: HashSet<String> = ["ES".to_string(), "IO".to_string()].into();
        assert_eq!(work_key(&e, &projects).0, "T:ES-636", "the branch's CORE-10 isn't a Jira project");
        assert_eq!(work_key(&e, &HashSet::new()).0, "T:CORE-10", "without Jira, the branch still wins");
        e.pr = None;
        assert_eq!(work_key(&e, &projects).0, "B:acme/web:core-10-upgrade", "no ticket at all: the branch");
    }

    #[test]
    fn sessions_and_totals() {
        let events = vec![
            ev("09:00", "edit", Some("io-2641-search"), "edit"),
            ev("09:05", "edit", Some("io-2641-search"), "edit"),
            ev("09:40", "committed", Some("io-2641-search"), "Add search"), // lead 25 → 09:15, within 30m gap
            ev("13:00", "committed", Some("io-2641-search"), "Fix tests"),  // new session
            ev("10:30", "reviewed", None, "Approved the PR"),
        ];
        let r = build("2026-10-14", events, None, t("00:00"), t("23:59"), &Titles::new(), &[]);
        let io: Vec<&Block> = r.blocks.iter().filter(|b| b.key == "T:IO-2641").collect();
        assert_eq!(io.len(), 2, "{:?}", r.blocks);
        assert_eq!(io[0].start, "2026-10-14T08:55:00Z");
        assert_eq!(io[0].end, "2026-10-14T09:45:00Z");
        let group = r.groups.iter().find(|g| g.key == "T:IO-2641").unwrap();
        assert_eq!(group.minutes, 50 + 30);
        // The review session overlaps nothing, so the day total is the sum.
        assert_eq!(r.total_minutes, 50 + 30 + 20);
    }

    #[test]
    fn overlapping_groups_share_time() {
        // Three PR reviews in one sitting, overlapping a ticket session: the groups add up to
        // the day's total rather than counting the shared stretch three or four times.
        let events = vec![
            ev("10:00", "reviewed", None, "review a"),
            ev("10:02", "reviewed", None, "review b"),
            ev("10:04", "reviewed", None, "review c"),
            ev("10:05", "committed", Some("io-1-thing"), "work"),
        ];
        let events: Vec<_> = events
            .into_iter()
            .enumerate()
            .map(|(i, mut e)| {
                if e.kind == "reviewed" {
                    e.pr = Some((format!("acme/web#{i}"), format!("Review {i}"), format!("https://x/{i}")));
                }
                e
            })
            .collect();
        let r = build("2026-10-14", events, None, t("00:00"), t("23:59"), &Titles::new(), &[]);
        let sum: i64 = r.groups.iter().map(|g| g.minutes).sum();
        assert_eq!(sum, r.total_minutes, "{:?}", r.groups.iter().map(|g| (&g.key, g.minutes)).collect::<Vec<_>>());
        assert!(r.groups.iter().all(|g| g.minutes < r.total_minutes));
    }

    #[test]
    fn overlaps_count_once_and_dedupe() {
        let mut a = ev("09:30", "committed", Some("abc-1-x"), "one");
        a.dedupe = Some("sha1".into());
        let mut b = ev("09:30", "local_commit", Some("abc-1-x"), "one");
        b.dedupe = Some("sha1".into());
        let c = ev("09:20", "reviewed", None, "Approved");
        let r = build("d", vec![a, b, c], None, t("00:00"), t("23:59"), &Titles::new(), &[]);
        assert_eq!(r.groups.iter().map(|g| g.events.len()).sum::<usize>(), 2, "duplicate commit dropped");
        // 09:05–09:35 (commit) and 09:05–09:25 (review) overlap: 30 minutes total.
        assert_eq!(r.total_minutes, 30);
    }

    #[test]
    fn jira_events_join_tickets() {
        let mut moved = ev("11:00", "jira", None, "In Progress → In Review");
        moved.ticket = Some("ABC-1".into());
        let mut others = ev("15:00", "jira", None, "In Review → Done (by someone else)");
        others.ticket = Some("ABC-1".into());
        others.counts = false;
        let commit = ev("10:50", "committed", Some("abc-1-thing"), "Fix it");
        let mut titles = Titles::new();
        titles.insert("ABC-1".into(), ("Make it work".into(), "https://x.atlassian.net/browse/ABC-1".into(), "Done".into()));
        let r = build("d", vec![moved, others, commit], None, t("00:00"), t("23:59"), &titles, &[]);
        assert_eq!(r.groups.len(), 1, "commit and Jira moves are the same ticket");
        let g = &r.groups[0];
        assert_eq!(g.label, "ABC-1 · Make it work");
        assert_eq!(g.status.as_deref(), Some("Done"));
        assert_eq!(g.events.len(), 3);
        // 10:25–11:05 from the commit + your 11:00 move; the 15:00 move by someone else adds nothing.
        assert_eq!(r.total_minutes, 40);
    }

    #[test]
    fn meetings_are_exact_blocks() {
        let meetings = vec![
            ("T:ABC-1".to_string(), "ABC-1".to_string(), "2026-10-14T10:00:00Z".to_string(), "2026-10-14T10:45:00Z".to_string()),
            ("MTG:m1".to_string(), "Meeting · Standup".to_string(), "2026-10-14T09:00:00Z".to_string(), "2026-10-14T09:15:00Z".to_string()),
        ];
        let commit = ev("10:30", "committed", Some("abc-1-x"), "work");
        let r = build("d", vec![commit], None, t("00:00"), t("23:59"), &Titles::new(), &meetings);
        let standup = r.groups.iter().find(|g| g.key == "MTG:m1").expect("meeting without a ticket gets its own group");
        assert_eq!(standup.minutes, 15);
        assert_eq!(standup.label, "Meeting · Standup");
        let abc = r.groups.iter().find(|g| g.key == "T:ABC-1").unwrap();
        // Commit 10:05–10:35 overlaps the 10:00–10:45 meeting: counted once.
        assert_eq!(abc.minutes, 45);
        assert_eq!(r.total_minutes, 60);
    }

    #[test]
    fn save_freeze_reset_roundtrip() {
        let path = std::env::temp_dir().join(format!("dev-inbox-day-{}.sqlite3", std::process::id()));
        let conn = crate::db::open(&path).unwrap();
        assert!(manual_blocks(&conn, "2026-10-14").unwrap().is_none(), "not frozen yet");
        let b = Block {
            id: None,
            key: "T:ABC-1".into(),
            label: "ABC-1 · Thing".into(),
            start: "2026-10-14T09:00:00Z".into(),
            end: "2026-10-14T10:30:00Z".into(),
            minutes: 90,
            note: Some("pairing".into()),
        };
        save_blocks(&conn, "2026-10-14", &[b]).unwrap();
        let saved = manual_blocks(&conn, "2026-10-14").unwrap().expect("frozen");
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].minutes, 90);
        assert_eq!(saved[0].note.as_deref(), Some("pairing"));
        // Saving an empty day keeps it frozen (the user deleted everything on purpose).
        save_blocks(&conn, "2026-10-14", &[]).unwrap();
        assert_eq!(manual_blocks(&conn, "2026-10-14").unwrap().map(|b| b.len()), Some(0));
        unfreeze(&conn, "2026-10-14").unwrap();
        assert!(manual_blocks(&conn, "2026-10-14").unwrap().is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn manual_blocks_win() {
        let manual = vec![Block {
            id: Some(1),
            key: "T:ABC-1".into(),
            label: "ABC-1 · Thing".into(),
            start: "2026-10-14T09:00:00Z".into(),
            end: "2026-10-14T11:00:00Z".into(),
            minutes: 120,
            note: None,
        }];
        let r = build("d", vec![ev("15:00", "committed", Some("abc-1-x"), "late")], Some(manual), t("00:00"), t("23:59"), &Titles::new(), &[]);
        assert!(r.frozen);
        assert_eq!(r.total_minutes, 120);
        assert_eq!(r.blocks.len(), 1);
        assert_eq!(r.groups[0].events.len(), 1, "events still listed for context");
    }
}

