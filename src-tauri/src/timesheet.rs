//! The weekly timesheet: one row per Jira ticket (plus "Other work") per day, proposed from the Day
//! view's estimates, rounded to 15 minutes, less anything already logged in Harvest for that ticket
//! and day; you adjust it and submit to Harvest. Buckets (Harvest project + task) are remembered per
//! ticket and per Jira project, and follow monthly buckets ("Project Hair September" → "… October").

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::HashMap;

use crate::harvest::Project;

/// The row for time not on a ticket (meetings, reviews of others' PRs, unlinked work).
pub const OTHER: &str = "__other";

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
    /// ticket | project | default | rollover — where it came from, for the UI.
    pub from: String,
}

/// Remembered buckets: "ticket:BILL-142", "project:BILL" and "default".
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

/// Remember a bucket for a ticket and its Jira project (or the default, for `OTHER`).
pub fn remember(conn: &Connection, row: &str, b: &Bucket, now: &str) -> Result<()> {
    let scopes: Vec<String> = if row == OTHER {
        vec!["default".into()]
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

/// The bucket for a row on a day: the ticket's, else its Jira project's, else the default for
/// `OTHER` — moved on to this month's bucket if it's a monthly one and that exists.
pub fn bucket_for(row: &str, month: u32, remembered: &HashMap<String, Bucket>, projects: &[Project]) -> Option<Bucket> {
    let b = if row == OTHER {
        remembered.get("default")?.clone()
    } else {
        let project = row.split('-').next().unwrap_or(row);
        remembered.get(&format!("ticket:{row}")).or_else(|| remembered.get(&format!("project:{project}")))?.clone()
    };
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
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    /// The Jira key, or `OTHER`.
    pub key: String,
    pub label: String,
    pub bucket: Option<Bucket>,
    pub cells: Vec<Cell>,
}

/// The Day view's estimate for one day: row → (label, minutes).
pub type DayProposal = HashMap<String, (String, i64)>;

/// A day report as proposals: ticket groups by key, everything else together as `OTHER`.
pub fn propose(report: &crate::day::DayReport) -> DayProposal {
    let mut out: DayProposal = HashMap::new();
    for g in &report.groups {
        if g.minutes <= 0 {
            continue;
        }
        match g.key.strip_prefix("T:") {
            Some(key) => {
                let label = g.label.trim_start_matches(key).trim_start_matches(" · ").trim().to_string();
                out.insert(key.to_string(), (label, g.minutes));
            }
            None => {
                let e = out.entry(OTHER.to_string()).or_insert_with(|| ("Meetings, reviews and other work".into(), 0));
                e.1 += g.minutes;
            }
        }
    }
    out
}

/// A day's proposals in quarter hours that add up to the day's own total (rounded to 15 minutes):
/// each row gets its whole quarters, and the spare quarters go to the rows closest to the next one.
/// Rounding every row on its own made a day of many small things far longer than it was.
pub fn quarters(day: &DayProposal) -> HashMap<String, i64> {
    let target = round15(day.values().map(|(_, m)| *m).sum());
    let mut rows: Vec<(&String, i64, i64)> = day.iter().map(|(k, (_, m))| (k, (m / 15) * 15, m % 15)).collect();
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

/// The week: rows for every ticket with time (proposed, entered or logged), each with a cell per day.
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
        for (k, (label, _)) in proposals.get(d).into_iter().flatten() {
            labels.entry(k.clone()).or_insert_with(|| label.clone());
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
    // Tickets alphabetically, "Other work" last.
    keys.sort_by(|a, b| (a == OTHER).cmp(&(b == OTHER)).then(a.cmp(b)));
    let rounded: HashMap<&String, HashMap<String, i64>> = proposals.iter().map(|(d, p)| (d, quarters(p))).collect();
    keys.into_iter()
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
                    Cell { day: d.clone(), proposed, logged, minutes, edited: s.minutes.is_some(), submitted }
                })
                .collect();
            let label = titles.get(&key).cloned().or_else(|| labels.get(&key).cloned()).unwrap_or_default();
            let first = days.first().map(String::as_str).unwrap_or_default();
            Row { bucket: bucket(&key, first), key, label, cells }
        })
        // A few minutes that rounded to nothing all week isn't worth a row.
        .filter(|r| r.cells.iter().any(|c| c.minutes > 0 || c.proposed > 0 || c.logged > 0 || c.edited))
        .collect()
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
    use crate::harvest::Task;

    #[test]
    fn a_day_rounds_as_a_whole() {
        let p = |rows: &[(&str, i64)]| -> DayProposal { rows.iter().map(|(k, m)| (k.to_string(), (String::new(), *m))).collect() };
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
        proposals.insert(days[0].clone(), HashMap::from([("HAIR-1".to_string(), ("Fix".to_string(), 50))]));
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
            HashMap::from([("HAIR-12".to_string(), ("Fix the header".to_string(), 128)), (OTHER.to_string(), ("Other".to_string(), 20))]),
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
