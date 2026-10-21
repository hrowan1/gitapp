//! The weekly timesheet: one row per Jira ticket, per repository for work that isn't on a ticket,
//! and "Meetings and other work", per day — proposed from the Day view's estimates, rounded to 15
//! minutes, less anything already logged in Harvest for that ticket and day, with what was done
//! (commit messages, PR titles, reviews); you adjust it and submit to Harvest. Buckets (Harvest
//! project + task) are remembered per ticket and Jira project, or per repository and organisation,
//! and follow monthly buckets ("Project Hair September" → "… October").

use anyhow::Result;
use regex::Regex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::LazyLock;

use crate::day::Group;
use crate::harvest::Project;

/// The row for time that's neither on a ticket nor in a repository (meetings, time added by hand).
pub const OTHER: &str = "__other";

/// Rows for work that isn't on a ticket are per repository: "repo:acme/web".
pub const REPO: &str = "repo:";

/// "acme/web" → "acme".
fn org(repo: &str) -> &str {
    repo.split('/').next().unwrap_or(repo)
}

pub const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December",
];

/// Nearest 15 minutes.
pub fn round15(minutes: i64) -> i64 {
    ((minutes.max(0) + 7) / 15) * 15
}

/// A monthly bucket's name for another month: "Project Hair September" → "Project Hair October".
/// None if the name has no month in it, or it's already that month.
pub fn for_month(name: &str, month: u32) -> Option<String> {
    let target = MONTHS.get(month.checked_sub(1)? as usize)?;
    let lower = name.to_lowercase();
    // Whole words only: "Mayfair Website" isn't a May project.
    let word = |at: usize, len: usize| {
        !lower[..at].ends_with(|c: char| c.is_alphanumeric()) && !lower[at + len..].starts_with(|c: char| c.is_alphanumeric())
    };
    for m in MONTHS {
        if let Some(at) = lower.match_indices(&m.to_lowercase()).map(|(i, _)| i).find(|&i| word(i, m.len())) {
            if m == *target {
                return None;
            }
            return Some(format!("{}{}{}", &name[..at], target, &name[at + m.len()..]));
        }
    }
    None
}

/// A Harvest project + task chosen for some time.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bucket {
    pub project_id: i64,
    pub project_name: String,
    pub task_id: i64,
    pub task_name: String,
    /// ticket | project | repo | org | default | rollover — where it came from, for the UI.
    pub from: String,
}

/// Remembered buckets: "ticket:BILL-142", "project:BILL", "repo:acme/web", "org:acme" and "default".
pub fn buckets(conn: &Connection) -> Result<HashMap<String, Bucket>> {
    let mut stmt = conn.prepare("SELECT scope, project_id, project_name, task_id, task_name FROM harvest_buckets")?;
    let rows = stmt
        .query_map([], |r| {
            let scope: String = r.get(0)?;
            let from = scope.split(':').next().unwrap_or("default").to_string();
            Ok((scope, Bucket { project_id: r.get(1)?, project_name: r.get(2)?, task_id: r.get(3)?, task_name: r.get(4)?, from }))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// Remember a bucket for a ticket and its Jira project, a repository and its organisation, or the
/// default (for `OTHER`).
pub fn remember(conn: &Connection, row: &str, b: &Bucket, now: &str) -> Result<()> {
    let scopes: Vec<String> = if row == OTHER {
        vec!["default".into()]
    } else if let Some(repo) = row.strip_prefix(REPO) {
        vec![format!("repo:{repo}"), format!("org:{}", org(repo))]
    } else {
        let project = row.split('-').next().unwrap_or(row);
        vec![format!("ticket:{row}"), format!("project:{project}")]
    };
    for scope in scopes {
        conn.execute(
            "INSERT INTO harvest_buckets (scope, project_id, project_name, task_id, task_name, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(scope) DO UPDATE SET project_id = excluded.project_id, project_name = excluded.project_name,
                task_id = excluded.task_id, task_name = excluded.task_name, updated_at = excluded.updated_at",
            params![scope, b.project_id, b.project_name, b.task_id, b.task_name, now],
        )?;
    }
    Ok(())
}

/// The bucket for a row on a day: the ticket's, else its Jira project's; the repository's, else its
/// organisation's, else the default; the default for `OTHER` — moved on to this month's bucket if
/// it's a monthly one and that exists.
pub fn bucket_for(row: &str, month: u32, remembered: &HashMap<String, Bucket>, projects: &[Project]) -> Option<Bucket> {
    let b = if row == OTHER {
        remembered.get("default")
    } else if let Some(repo) = row.strip_prefix(REPO) {
        remembered
            .get(&format!("repo:{repo}"))
            .or_else(|| remembered.get(&format!("org:{}", org(repo))))
            .or_else(|| remembered.get("default"))
    } else {
        let project = row.split('-').next().unwrap_or(row);
        remembered.get(&format!("ticket:{row}")).or_else(|| remembered.get(&format!("project:{project}")))
    }?
    .clone();
    let Some(name) = for_month(&b.project_name, month) else { return Some(b) };
    let Some(p) = projects.iter().find(|p| p.name.eq_ignore_ascii_case(&name)) else { return Some(b) };
    let task = p.tasks.iter().find(|t| t.name.eq_ignore_ascii_case(&b.task_name)).or(p.tasks.first())?;
    Some(Bucket { project_id: p.id, project_name: p.name.clone(), task_id: task.id, task_name: task.name.clone(), from: "rollover".into() })
}

/// What you entered or submitted for a row on a day.
#[derive(Debug, Clone, Default)]
pub struct Stored {
    /// Your hours (minutes) if you changed them; None means "use the proposal".
    pub minutes: Option<i64>,
    pub harvest_id: Option<i64>,
    pub submitted_minutes: Option<i64>,
}

pub fn stored(conn: &Connection, from: &str, to: &str) -> Result<HashMap<(String, String), Stored>> {
    let mut stmt = conn.prepare(
        "SELECT day, row_key, minutes, harvest_id, submitted_minutes FROM timesheet_rows WHERE day >= ?1 AND day <= ?2",
    )?;
    let rows = stmt
        .query_map(params![from, to], |r| {
            Ok((
                (r.get::<_, String>(0)?, r.get::<_, String>(1)?),
                Stored { minutes: r.get(2)?, harvest_id: r.get(3)?, submitted_minutes: r.get(4)? },
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// Your hours for a row on a day (None: back to the proposal).
pub fn set_minutes(conn: &Connection, day: &str, row: &str, minutes: Option<i64>) -> Result<()> {
    conn.execute(
        "INSERT INTO timesheet_rows (day, row_key, minutes) VALUES (?1, ?2, ?3)
         ON CONFLICT(day, row_key) DO UPDATE SET minutes = excluded.minutes",
        params![day, row, minutes.map(round15)],
    )?;
    Ok(())
}

/// After submitting a row for a day: what Harvest now has.
pub fn submitted(conn: &Connection, day: &str, row: &str, harvest_id: Option<i64>, minutes: i64, now: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO timesheet_rows (day, row_key, harvest_id, submitted_minutes, submitted_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(day, row_key) DO UPDATE SET harvest_id = excluded.harvest_id,
            submitted_minutes = excluded.submitted_minutes, submitted_at = excluded.submitted_at",
        params![day, row, harvest_id, minutes, now],
    )?;
    Ok(())
}

pub fn harvest_ids(conn: &Connection) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare("SELECT harvest_id FROM timesheet_rows WHERE harvest_id IS NOT NULL")?;
    let rows = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// One cell: a row's time on one day.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cell {
    pub day: String,
    /// The Day view's estimate, rounded.
    pub proposed: i64,
    /// Already in Harvest for this ticket and day, logged some other way (e.g. Jira's Harvest button).
    pub logged: i64,
    /// What will be submitted: your hours, else the proposal less what's already logged.
    pub minutes: i64,
    pub edited: bool,
    /// Harvest has exactly this.
    pub submitted: bool,
    /// What was done that day (commit messages, PR titles, reviews), most time first.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    /// The Jira key, `REPO` + "owner/name", or `OTHER`.
    pub key: String,
    pub label: String,
    pub bucket: Option<Bucket>,
    pub cells: Vec<Cell>,
}

/// One row's estimate for one day.
#[derive(Debug, Clone, Default)]
pub struct Proposal {
    pub label: String,
    pub minutes: i64,
    /// What was done, most time first.
    pub notes: Vec<String>,
}

/// The Day view's estimate for one day, by row.
pub type DayProposal = HashMap<String, Proposal>;

/// The repository a Day view group is in: "PR:acme/web#12", "B:acme/web:fix-nav", "R:acme/web".
fn repo_of(key: &str) -> Option<&str> {
    if let Some(r) = key.strip_prefix("PR:") {
        return r.split('#').next();
    }
    key.strip_prefix("B:").and_then(|r| r.split(':').next()).or_else(|| key.strip_prefix("R:"))
}

/// Only reviewing or commenting on a PR, no work of your own on it.
fn just_reviewing(g: &Group) -> bool {
    !g.events.is_empty() && g.events.iter().all(|e| matches!(e.kind.as_str(), "reviewed" | "commented"))
}

/// "IO-2647 - Add a tool" → "Add a tool": the row already says IO-2647.
fn strip_key<'a>(line: &'a str, key: &str) -> &'a str {
    match line.get(..key.len()) {
        Some(head) if head.eq_ignore_ascii_case(key) => line[key.len()..].trim_start_matches([' ', ':', '-', '–', '_', '/', '·']),
        _ => line,
    }
}

/// "Sync files from tools-template@b8173e230664" → "Sync files from tools-template", so a run of
/// automated commits reads as one thing.
static SHA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@[0-9a-f]{7,40}\b").unwrap());

fn push_new(out: &mut Vec<String>, note: String) {
    if !note.is_empty() && !out.contains(&note) {
        out.push(note);
    }
}

/// What a group says was done: its commit messages (first lines) and merged PRs, else what you did
/// (opened, merged), its PR title, branch or meeting. A ticket's or repository's own name isn't
/// news, so those add nothing.
fn did(g: &Group, ticket: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in &g.events {
        let line = e.text.lines().next().unwrap_or_default().trim();
        match e.kind.as_str() {
            "committed" | "local_commit" => push_new(&mut out, SHA.replace_all(ticket.map_or(line, |t| strip_key(line, t)), "").trim().to_string()),
            "merged" => {
                if let Some(n) = line.strip_prefix("Merge pull request #").and_then(|r| r.split_whitespace().next()) {
                    push_new(&mut out, format!("Merged PR #{n}"));
                }
            }
            _ => {}
        }
    }
    if !out.is_empty() {
        return out;
    }
    if let Some(rest) = g.key.strip_prefix("B:") {
        out.extend(rest.split_once(':').map(|(_, branch)| branch.to_string()));
    } else if g.key.starts_with("T:") || g.key.starts_with("R:") {
        for e in &g.events {
            match e.kind.as_str() {
                "opened" => push_new(&mut out, "Opened a PR".into()),
                "merged" => push_new(&mut out, "Merged a PR".into()),
                _ => {}
            }
        }
    } else {
        out.push(g.label.clone());
    }
    out
}

/// A day report as proposals: a row per ticket, per repository for work that isn't on a ticket, and
/// `OTHER` for the rest (meetings, time added by hand) — each with what was done.
pub fn propose(report: &crate::day::DayReport) -> DayProposal {
    let mut out: DayProposal = HashMap::new();
    // Reviews are counted: "Reviewed 12 PRs" reads better than twelve titles.
    let mut reviews: HashMap<String, Vec<String>> = HashMap::new();
    for g in &report.groups {
        if g.minutes <= 0 {
            continue;
        }
        let ticket = g.key.strip_prefix("T:");
        let (row, label) = match (ticket, repo_of(&g.key)) {
            (Some(key), _) => (key.to_string(), g.label.trim_start_matches(key).trim_start_matches(" · ").trim().to_string()),
            (None, Some(repo)) => (format!("{REPO}{repo}"), repo.to_string()),
            (None, None) => (OTHER.to_string(), "Meetings and other work".to_string()),
        };
        let p = out.entry(row.clone()).or_insert_with(|| Proposal { label, ..Default::default() });
        p.minutes += g.minutes;
        if just_reviewing(g) {
            reviews.entry(row).or_default().push(if ticket.is_some() { String::new() } else { g.label.clone() });
            continue;
        }
        for n in did(g, ticket) {
            if !p.notes.contains(&n) {
                p.notes.push(n);
            }
        }
    }
    for (row, titles) in reviews {
        let note = match titles.as_slice() {
            [one] if !one.is_empty() => format!("Reviewed “{one}”"),
            [_] => "Reviewed a PR".to_string(),
            many => format!("Reviewed {} PRs", many.len()),
        };
        out.entry(row).or_default().notes.push(note);
    }
    // Likewise a run of merges: "Merged 12 PRs", where the first one was.
    for p in out.values_mut() {
        let merged = p.notes.iter().filter(|n| n.starts_with("Merged PR #")).count();
        if merged > 2 {
            let at = p.notes.iter().position(|n| n.starts_with("Merged PR #")).unwrap_or(0);
            p.notes.retain(|n| !n.starts_with("Merged PR #"));
            p.notes.insert(at.min(p.notes.len()), format!("Merged {merged} PRs"));
        }
    }
    out
}

/// Harvest's notes for a row's day: what the row is, then what was done.
pub fn notes(row: &Row, cell: &Cell) -> String {
    let what = match row.key.strip_prefix(REPO) {
        Some(repo) => repo.rsplit('/').next().unwrap_or(repo).to_string(),
        None if row.key == OTHER => String::new(),
        None => format!("{} {}", row.key, row.label).trim().to_string(),
    };
    match (what.as_str(), cell.notes.join("; ").as_str()) {
        ("", "") => row.label.clone(),
        ("", done) => done.to_string(),
        (what, "") => what.to_string(),
        (what, done) => format!("{what} — {done}"),
    }
}

/// A day's proposals in quarter hours that add up to the day's own total (rounded to 15 minutes):
/// each row gets its whole quarters, and the spare quarters go to the rows closest to the next one.
/// Rounding every row on its own made a day of many small things far longer than it was.
pub fn quarters(day: &DayProposal) -> HashMap<String, i64> {
    let target = round15(day.values().map(|p| p.minutes).sum());
    let mut rows: Vec<(&String, i64, i64)> = day.iter().map(|(k, p)| (k, (p.minutes / 15) * 15, p.minutes % 15)).collect();
    let mut spare = (target - rows.iter().map(|r| r.1).sum::<i64>()) / 15;
    // Largest remainder first; ties to the bigger row, then by key so it's stable.
    rows.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.cmp(&a.1)).then(a.0.cmp(b.0)));
    for r in rows.iter_mut() {
        if spare <= 0 {
            break;
        }
        r.1 += 15;
        spare -= 1;
    }
    rows.into_iter().map(|(k, m, _)| (k.clone(), m)).collect()
}

/// The week: rows for every ticket or repository with time (proposed, entered or logged), each with
/// a cell per day — most time first, "Meetings and other work" last.
pub fn week(
    days: &[String],
    proposals: &HashMap<String, DayProposal>,
    stored: &HashMap<(String, String), Stored>,
    logged: &HashMap<(String, String), i64>,
    bucket: &dyn Fn(&str, &str) -> Option<Bucket>,
    titles: &HashMap<String, String>,
) -> Vec<Row> {
    let mut keys: Vec<String> = Vec::new();
    let mut labels: HashMap<String, String> = HashMap::new();
    for d in days {
        for (k, p) in proposals.get(d).into_iter().flatten() {
            labels.entry(k.clone()).or_insert_with(|| p.label.clone());
            if !keys.contains(k) {
                keys.push(k.clone());
            }
        }
    }
    for (d, k) in stored.keys().chain(logged.keys()) {
        if days.contains(d) && !keys.contains(k) {
            keys.push(k.clone());
        }
    }
    let rounded: HashMap<&String, HashMap<String, i64>> = proposals.iter().map(|(d, p)| (d, quarters(p))).collect();
    let mut rows: Vec<Row> = keys
        .into_iter()
        .map(|key| {
            let cells: Vec<Cell> = days
                .iter()
                .map(|d| {
                    let proposed = rounded.get(d).and_then(|p| p.get(&key)).copied().unwrap_or(0);
                    let logged = round15(logged.get(&(d.clone(), key.clone())).copied().unwrap_or(0));
                    let s = stored.get(&(d.clone(), key.clone())).cloned().unwrap_or_default();
                    let minutes = s.minutes.unwrap_or_else(|| (proposed - logged).max(0));
                    let submitted = s.harvest_id.is_some() && s.submitted_minutes == Some(minutes)
                        || s.harvest_id.is_none() && minutes == 0;
                    let notes = proposals.get(d).and_then(|p| p.get(&key)).map(|p| p.notes.clone()).unwrap_or_default();
                    Cell { day: d.clone(), proposed, logged, minutes, edited: s.minutes.is_some(), submitted, notes }
                })
                .collect();
            let label = titles.get(&key).cloned().or_else(|| labels.get(&key).cloned()).unwrap_or_default();
            let first = days.first().map(String::as_str).unwrap_or_default();
            Row { bucket: bucket(&key, first), key, label, cells }
        })
        // A few minutes that rounded to nothing all week isn't worth a row.
        .filter(|r| r.cells.iter().any(|c| c.minutes > 0 || c.proposed > 0 || c.logged > 0 || c.edited))
        .collect();
    let total = |r: &Row| r.cells.iter().map(|c| c.minutes).sum::<i64>();
    rows.sort_by(|a, b| (a.key == OTHER).cmp(&(b.key == OTHER)).then(total(b).cmp(&total(a))).then(a.key.cmp(&b.key)));
    rows
}

pub fn stored_row(conn: &Connection, day: &str, row: &str) -> Result<Option<Stored>> {
    Ok(conn
        .query_row(
            "SELECT minutes, harvest_id, submitted_minutes FROM timesheet_rows WHERE day = ?1 AND row_key = ?2",
            params![day, row],
            |r| Ok(Stored { minutes: r.get(0)?, harvest_id: r.get(1)?, submitted_minutes: r.get(2)? }),
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::day::{DayReport, EventOut};
    use crate::harvest::Task;

    fn prop(label: &str, minutes: i64) -> Proposal {
        Proposal { label: label.into(), minutes, notes: Vec::new() }
    }

    fn group(key: &str, label: &str, minutes: i64, events: &[(&str, &str)]) -> Group {
        Group {
            key: key.into(),
            label: label.into(),
            url: None,
            status: None,
            minutes,
            events: events
                .iter()
                .map(|(kind, text)| EventOut {
                    at: String::new(),
                    kind: kind.to_string(),
                    text: text.to_string(),
                    url: None,
                    repo: String::new(),
                    branch: None,
                })
                .collect(),
        }
    }

    /// Work that isn't on a ticket is split by repository and says what was done.
    #[test]
    fn proposals_say_what_was_done() {
        let report = DayReport {
            day: "2026-10-16".into(),
            frozen: false,
            total_minutes: 0,
            blocks: Vec::new(),
            groups: vec![
                group("T:IO-2647", "IO-2647 · Add an MCP tool", 89, &[("committed", "IO-2647 - Resolve contacts by domain\n\nBody"), ("pushed", "Pushed")]),
                group(
                    "R:acme/app",
                    "acme/app",
                    128,
                    &[
                        ("local_commit", "Fix the launch screen"),
                        ("merged", "Merge pull request #12 from acme/fix-nav"),
                        ("committed", "Sync files from acme/template@b8173e230664"),
                        ("committed", "Sync files from acme/template@ea96e54b9a37"),
                    ],
                ),
                group("T:ES-636", "ES-636 · Upgrade core", 14, &[("opened", "Opened"), ("pushed", "Pushed to core-10-6"), ("merged", "Merged")]),
                group("B:acme/app:drop-dev", "app · drop-dev", 20, &[("local_commit", "")]),
                group("PR:acme/app#605", "Test-user back door", 10, &[("reviewed", "Approved the PR")]),
                group("PR:acme/app#560", "Deep-link corpus", 7, &[("reviewed", "Approved the PR"), ("commented", "Commented")]),
                group(
                    "PR:acme/tools#87",
                    "Label each deploy",
                    11,
                    &[
                        ("opened", "Opened"),
                        ("merged", "Merge pull request #3 from acme/a"),
                        ("merged", "Merge pull request #4 from acme/b"),
                        ("merged", "Merge pull request #5 from acme/c"),
                    ],
                ),
                group("MTG:m1", "Planning", 14, &[("meeting", "Recorded meeting (14m)")]),
            ],
            ticket_moves: Vec::new(),
            meetings: Vec::new(),
        };
        let p = propose(&report);
        let keys: std::collections::BTreeSet<&str> = p.keys().map(String::as_str).collect();
        assert_eq!(keys, ["ES-636", "IO-2647", OTHER, "repo:acme/app", "repo:acme/tools"].into(), "a row per repo, meetings in other");
        assert_eq!(p["IO-2647"].notes, ["Resolve contacts by domain"], "first line, without the key");
        assert_eq!(p["ES-636"].notes, ["Opened a PR", "Merged a PR"], "no commits: what you did");
        let app = &p["repo:acme/app"];
        assert_eq!(app.minutes, 128 + 20 + 10 + 7);
        assert_eq!(
            app.notes,
            ["Fix the launch screen", "Merged PR #12", "Sync files from acme/template", "drop-dev", "Reviewed 2 PRs"],
            "commit SHAs dropped so repeats read once; reviews counted"
        );
        assert_eq!(p["repo:acme/tools"].notes, ["Merged 3 PRs"], "a run of merges is counted");
        assert_eq!((p[OTHER].label.as_str(), p[OTHER].notes.as_slice()), ("Meetings and other work", ["Planning".to_string()].as_slice()));
    }

    #[test]
    fn harvest_notes() {
        let cell = |notes: &[&str]| Cell {
            day: "2026-10-16".into(),
            proposed: 0,
            logged: 0,
            minutes: 60,
            edited: false,
            submitted: false,
            notes: notes.iter().map(|n| n.to_string()).collect(),
        };
        let row = |key: &str, label: &str| Row { key: key.into(), label: label.into(), bucket: None, cells: Vec::new() };
        assert_eq!(notes(&row("repo:acme/app", "acme/app"), &cell(&["Fix nav", "Reviewed 2 PRs"])), "app — Fix nav; Reviewed 2 PRs");
        assert_eq!(notes(&row("IO-1", "Search"), &cell(&["Add index"])), "IO-1 Search — Add index", "the key stays first for Jira links");
        assert_eq!(notes(&row("IO-1", "Search"), &cell(&[])), "IO-1 Search");
        assert_eq!(notes(&row(OTHER, "Meetings and other work"), &cell(&[])), "Meetings and other work");
        assert_eq!(notes(&row(OTHER, "Meetings and other work"), &cell(&["Planning"])), "Planning");
    }

    #[test]
    fn repo_buckets_fall_back_to_their_org_then_the_default() {
        let b = |id: i64, from: &str| Bucket { project_id: id, project_name: format!("P{id}"), task_id: 1, task_name: "Dev".into(), from: from.into() };
        let mut remembered = HashMap::from([("default".to_string(), b(1, "default"))]);
        assert_eq!(bucket_for("repo:acme/app", 10, &remembered, &[]).unwrap().from, "default");
        remembered.insert("org:acme".into(), b(2, "org"));
        assert_eq!(bucket_for("repo:acme/app", 10, &remembered, &[]).unwrap().project_id, 2, "any acme repo");
        remembered.insert("repo:acme/app".into(), b(3, "repo"));
        assert_eq!(bucket_for("repo:acme/app", 10, &remembered, &[]).unwrap().project_id, 3);
        assert_eq!(bucket_for("repo:other/x", 10, &remembered, &[]).unwrap().project_id, 1);
        assert!(bucket_for("IO-1", 10, &remembered, &[]).is_none(), "tickets don't take the default");
    }

    #[test]
    fn a_day_rounds_as_a_whole() {
        let p = |rows: &[(&str, i64)]| -> DayProposal {
            rows.iter().map(|(k, m)| (k.to_string(), Proposal { minutes: *m, ..Default::default() })).collect()
        };
        // Six 10-minute things are an hour, not an hour and a half.
        let q = quarters(&p(&[("A-1", 10), ("A-2", 10), ("A-3", 10), ("A-4", 10), ("A-5", 10), ("A-6", 10)]));
        assert_eq!(q.values().sum::<i64>(), 60);
        assert!(q.values().all(|m| m % 15 == 0));
        // 175 minutes is three hours; the spare quarters go to the rows nearest their next one.
        let q = quarters(&p(&[("A-1", 128), ("A-2", 44), ("A-3", 3)]));
        assert_eq!(q.values().sum::<i64>(), 180);
        assert_eq!(q["A-1"], 135);
        assert_eq!(q["A-2"], 45);
        assert_eq!(q["A-3"], 0);
    }

    /// Edit a cell, submit it, edit it again: it's waiting again, and going back to the proposal
    /// after submitting still counts as a change.
    #[test]
    fn edits_and_submissions_round_trip() {
        let dir = std::env::temp_dir().join(format!("ts-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let conn = crate::db::open(&dir.join("t.sqlite3")).unwrap();
        let days = vec!["2026-10-12".to_string()];
        let mut proposals = HashMap::new();
        proposals.insert(days[0].clone(), HashMap::from([("HAIR-1".to_string(), prop("Fix", 50))]));
        let none = |_: &str, _: &str| None;
        let cell = |conn: &Connection| {
            let stored = stored(conn, "2026-10-12", "2026-10-12").unwrap();
            week(&days, &proposals, &stored, &HashMap::new(), &none, &HashMap::new())[0].cells[0].clone()
        };

        let c = cell(&conn);
        assert_eq!((c.minutes, c.edited, c.submitted), (45, false, false), "proposal, rounded, not sent");
        submitted(&conn, "2026-10-12", "HAIR-1", Some(7), 45, "now").unwrap();
        assert!(cell(&conn).submitted);

        set_minutes(&conn, "2026-10-12", "HAIR-1", Some(70)).unwrap();
        let c = cell(&conn);
        assert_eq!((c.minutes, c.edited, c.submitted), (75, true, false), "edited after sending: waiting again");
        assert_eq!(harvest_ids(&conn).unwrap().len(), 1);

        set_minutes(&conn, "2026-10-12", "HAIR-1", None).unwrap();
        let c = cell(&conn);
        assert_eq!((c.minutes, c.submitted), (45, true), "back to what was sent");
        let s = stored_row(&conn, "2026-10-12", "HAIR-1").unwrap().unwrap();
        assert_eq!(s.harvest_id, Some(7), "clearing your hours keeps the Harvest entry it belongs to");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rounds_to_quarter_hours() {
        assert_eq!(round15(0), 0);
        assert_eq!(round15(7), 0);
        assert_eq!(round15(8), 15);
        assert_eq!(round15(52), 45);
        assert_eq!(round15(53), 60);
        assert_eq!(round15(165), 165);
    }

    #[test]
    fn monthly_buckets_roll_over() {
        assert_eq!(for_month("Project Hair September", 10).as_deref(), Some("Project Hair October"));
        assert_eq!(for_month("project hair september retainer", 1).as_deref(), Some("project hair January retainer"));
        assert_eq!(for_month("Project Hair October", 10), None, "already this month");
        assert_eq!(for_month("Internal", 10), None, "not monthly");
        assert_eq!(for_month("Mayfair Website", 10), None, "a word that starts with a month");

        let projects = vec![Project {
            id: 2,
            name: "Project Hair October".into(),
            code: None,
            client: None,
            tasks: vec![Task { id: 9, name: "Design".into() }, Task { id: 10, name: "Development".into() }],
        }];
        let mut remembered = HashMap::new();
        remembered.insert(
            "project:HAIR".to_string(),
            Bucket { project_id: 1, project_name: "Project Hair September".into(), task_id: 5, task_name: "Development".into(), from: "project".into() },
        );
        let b = bucket_for("HAIR-12", 10, &remembered, &projects).unwrap();
        assert_eq!((b.project_id, b.task_id, b.from.as_str()), (2, 10, "rollover"), "same task name in the new month's bucket");
        assert_eq!(bucket_for("HAIR-12", 9, &remembered, &projects).unwrap().project_id, 1, "September stays September");
        assert!(bucket_for("BILL-1", 10, &remembered, &projects).is_none(), "nothing remembered for BILL");
    }

    #[test]
    fn week_subtracts_what_is_already_logged() {
        let days = vec!["2026-10-12".to_string(), "2026-10-13".to_string()];
        let mut proposals = HashMap::new();
        proposals.insert(
            "2026-10-12".to_string(),
            HashMap::from([("HAIR-12".to_string(), prop("Fix the header", 128)), (OTHER.to_string(), prop("Other", 20))]),
        );
        let logged = HashMap::from([(("2026-10-12".to_string(), "HAIR-12".to_string()), 60)]);
        let mut stored = HashMap::new();
        stored.insert(("2026-10-13".to_string(), "HAIR-12".to_string()), Stored { minutes: Some(30), ..Default::default() });
        let rows = week(&days, &proposals, &stored, &logged, &|_, _| None, &HashMap::new());
        assert_eq!(rows.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(), ["HAIR-12", OTHER], "other work last");
        let hair = &rows[0].cells;
        assert_eq!((hair[0].proposed, hair[0].logged, hair[0].minutes), (135, 60, 75), "2h 08m → 2h 15m, less the hour already logged");
        assert!(!hair[0].submitted);
        assert_eq!((hair[1].minutes, hair[1].edited), (30, true), "your own hours win");
        assert_eq!(rows[1].cells[0].minutes, 15);
    }
}
