//! Meetings from the Mac's calendars (EventKit, through the helper; read-only). Events are copied into
//! the database in a rolling window so the Day view, the timesheet and Ask can use them, a reminder
//! can go out before a meeting starts, and the Meetings view can show what's next with a brief.

use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::time::Duration;

use crate::derive::{fmt_ts, parse_ts};

const HELPER_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub uid: String,
    pub calendar_id: String,
    pub calendar: String,
    pub title: String,
    pub start: String,
    pub end: String,
    pub all_day: bool,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub notes: String,
    pub video_url: Option<String>,
    pub attendees: i64,
    #[serde(default)]
    pub attendee_names: Vec<String>,
    pub organiser: Option<String>,
    /// accepted | declined | tentative | pending | organiser | none
    pub my_status: String,
    pub free: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Calendar {
    pub id: String,
    pub title: String,
    pub source: String,
    pub color: String,
}

fn helper(args: &[&str]) -> Result<serde_json::Value> {
    let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    let out = crate::meetings::run_helper(&args, HELPER_TIMEOUT)?;
    let line = out.lines().rev().find(|l| l.trim_start().starts_with('{')).unwrap_or_default();
    let v: serde_json::Value = serde_json::from_str(line)?;
    if v["event"] == "error" {
        bail!("{}", v["message"].as_str().unwrap_or("Calendar helper failed"));
    }
    Ok(v)
}

/// full | notDetermined | denied | restricted | writeOnly
pub fn status() -> Result<String> {
    Ok(helper(&["calendar-status"])?["status"].as_str().unwrap_or("denied").to_string())
}

/// Ask macOS for access (shows its prompt the first time), then the status.
pub fn request_access() -> Result<String> {
    // The prompt waits for you, so allow longer than a normal call.
    let out = crate::meetings::run_helper(&[OsStr::new("calendar-access")], Duration::from_secs(300))?;
    let v: serde_json::Value = serde_json::from_str(out.lines().last().unwrap_or("{}"))?;
    Ok(v["status"].as_str().unwrap_or("denied").to_string())
}

pub fn calendars() -> Result<Vec<Calendar>> {
    Ok(serde_json::from_value(helper(&["calendars"])?["calendars"].take())?)
}

/// Events starting in [from, to) on the chosen calendars (all when `ids` is empty).
pub fn fetch(from: &str, to: &str, ids: &[String]) -> Result<Vec<Event>> {
    let joined = ids.join(",");
    let mut args = vec!["events", from, to];
    if !ids.is_empty() {
        args.push(&joined);
    }
    Ok(serde_json::from_value(helper(&args)?["events"].take())?)
}

/// Replace the stored events starting in [from, to) with `events`, keeping which were reminded.
pub fn store(conn: &Connection, from: &str, to: &str, events: &[Event]) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    let reminded: HashMap<String, String> = {
        let mut stmt = tx.prepare(
            "SELECT uid, reminded_at FROM calendar_events WHERE start_at >= ?1 AND start_at < ?2 AND reminded_at IS NOT NULL",
        )?;
        let rows = stmt.query_map(params![from, to], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
        rows
    };
    tx.execute("DELETE FROM calendar_events WHERE start_at >= ?1 AND start_at < ?2", params![from, to])?;
    for e in events {
        tx.execute(
            "INSERT OR REPLACE INTO calendar_events (uid, calendar_id, calendar, title, start_at, end_at, all_day, location, notes,
                video_url, attendees, attendee_names, organiser, my_status, free, reminded_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                e.uid,
                e.calendar_id,
                e.calendar,
                e.title,
                // Stored in the app's own timestamp format so they compare with everything else.
                parse_ts(&e.start).map(fmt_ts).unwrap_or_else(|| e.start.clone()),
                parse_ts(&e.end).map(fmt_ts).unwrap_or_else(|| e.end.clone()),
                e.all_day,
                e.location,
                e.notes,
                e.video_url,
                e.attendees,
                serde_json::to_string(&e.attendee_names)?,
                e.organiser,
                e.my_status,
                e.free,
                reminded.get(&e.uid),
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

const COLUMNS: &str = "uid, calendar_id, calendar, title, start_at, end_at, all_day, COALESCE(location, ''), COALESCE(notes, ''),
    video_url, attendees, COALESCE(attendee_names, '[]'), organiser, my_status, free";

fn row(r: &rusqlite::Row) -> rusqlite::Result<Event> {
    Ok(Event {
        uid: r.get(0)?,
        calendar_id: r.get(1)?,
        calendar: r.get(2)?,
        title: r.get(3)?,
        start: r.get(4)?,
        end: r.get(5)?,
        all_day: r.get(6)?,
        location: r.get(7)?,
        notes: r.get(8)?,
        video_url: r.get(9)?,
        attendees: r.get(10)?,
        attendee_names: serde_json::from_str(&r.get::<_, String>(11)?).unwrap_or_default(),
        organiser: r.get(12)?,
        my_status: r.get(13)?,
        free: r.get(14)?,
    })
}

/// Stored events that overlap [start, end), earliest first.
pub fn between(conn: &Connection, start: &str, end: &str) -> Result<Vec<Event>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM calendar_events WHERE start_at < ?2 AND end_at > ?1 ORDER BY start_at"
    ))?;
    let rows = stmt.query_map(params![start, end], row)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn get(conn: &Connection, uid: &str) -> Result<Option<Event>> {
    Ok(conn.query_row(&format!("SELECT {COLUMNS} FROM calendar_events WHERE uid = ?1"), [uid], row).optional()?)
}

/// A meeting rather than a personal block ("Lunch", "Focus time"): other people or a call link, not
/// all-day, not declined, not marked free.
pub fn is_meeting(e: &Event) -> bool {
    !e.all_day && !e.free && e.my_status != "declined" && (e.attendees > 0 || e.video_url.is_some())
}

/// The same meeting on two calendars (yours and a shared one) counts once.
fn distinct(events: Vec<Event>) -> Vec<Event> {
    let mut seen = HashSet::new();
    events.into_iter().filter(|e| seen.insert((e.title.trim().to_lowercase(), e.start.clone(), e.end.clone()))).collect()
}

/// The Jira ticket a meeting is about, from its title ("BILL-142 proration sync").
pub fn ticket_of(e: &Event) -> Option<String> {
    crate::day::ticket_in_text(&e.title)
}

/// Calendar meetings as Day-view spans (key, label, start, end), like recorded meetings. One you
/// recorded is already there exactly, so a calendar meeting mostly covered by a recording is left out.
pub fn day_spans(
    conn: &Connection,
    start: &str,
    end: &str,
    recorded: &[(String, String, String, String)],
    now: crate::derive::Ts,
) -> Result<Vec<(String, String, String, String)>> {
    let recorded: Vec<_> = recorded.iter().filter_map(|(_, _, s, e)| Some((parse_ts(s)?, parse_ts(e)?))).collect();
    let mut out = Vec::new();
    for e in distinct(between(conn, start, end)?).into_iter().filter(is_meeting) {
        let (Some(s), Some(en)) = (parse_ts(&e.start), parse_ts(&e.end)) else { continue };
        // Time is what has happened: a meeting later today doesn't count yet, one under way counts so far.
        if s >= now {
            continue;
        }
        let en = en.min(now);
        let length = (en - s).num_seconds().max(1);
        let covered: i64 = recorded.iter().map(|(rs, re)| (en.min(*re) - s.max(*rs)).num_seconds().max(0)).sum();
        if covered * 2 >= length {
            continue;
        }
        let title = if e.title.trim().is_empty() { "Untitled meeting".to_string() } else { e.title.trim().to_string() };
        let (key, label) = match ticket_of(&e) {
            // The label only names it on the timeline; a ticket's group is labelled by the ticket.
            Some(t) => (format!("T:{t}"), format!("{CALENDAR_LABEL}{title}")),
            None => (format!("CAL:{}", e.uid), format!("Meeting · {title}")),
        };
        out.push((key, label, e.start, fmt_ts(en)));
    }
    Ok(out)
}

/// Marks a ticket-linked span as coming from the calendar (see `day_spans`).
pub const CALENDAR_LABEL: &str = "Calendar · ";

/// The meeting happening now (for naming a recording after it), if exactly one is.
pub fn now_on(conn: &Connection, now: &str) -> Result<Option<Event>> {
    let mut current: Vec<Event> = distinct(between(conn, now, now)?).into_iter().filter(is_meeting).collect();
    // Started in the last 10 minutes or about to: the likeliest one you're recording.
    let soon = fmt_ts(parse_ts(now).unwrap_or_default() + chrono::Duration::minutes(5));
    if current.is_empty() {
        current = distinct(between(conn, now, &soon)?).into_iter().filter(is_meeting).collect();
    }
    Ok(if current.len() == 1 { current.pop() } else { None })
}

/// Meetings starting within `minutes` that haven't been reminded about; marks them reminded.
pub fn due_reminders(conn: &Connection, now: &str, minutes: i64) -> Result<Vec<Event>> {
    let until = fmt_ts(parse_ts(now).unwrap_or_default() + chrono::Duration::minutes(minutes));
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM calendar_events WHERE start_at > ?1 AND start_at <= ?2 AND reminded_at IS NULL ORDER BY start_at"
    ))?;
    let due: Vec<Event> = stmt.query_map(params![now, until], row)?.collect::<Result<Vec<_>, _>>()?;
    for e in &due {
        conn.execute("UPDATE calendar_events SET reminded_at = ?2 WHERE uid = ?1", params![e.uid, now])?;
    }
    Ok(distinct(due).into_iter().filter(is_meeting).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(uid: &str, title: &str, start: &str, end: &str) -> Event {
        Event {
            uid: uid.into(),
            calendar_id: "c".into(),
            calendar: "Work".into(),
            title: title.into(),
            start: start.into(),
            end: end.into(),
            all_day: false,
            location: String::new(),
            notes: String::new(),
            video_url: Some("https://meet.google.com/abc".into()),
            attendees: 3,
            attendee_names: vec![],
            organiser: None,
            my_status: "accepted".into(),
            free: false,
        }
    }

    /// A database of the test's own (tests run in parallel; a shared file would race on migrations).
    fn db(test: &str) -> (Connection, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("cal-test-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (crate::db::open(&dir.join("t.sqlite3")).unwrap(), dir)
    }

    #[test]
    fn only_real_meetings_count() {
        let base = ev("a", "Sync", "2026-10-20T10:00:00Z", "2026-10-20T10:30:00Z");
        assert!(is_meeting(&base));
        assert!(!is_meeting(&Event { all_day: true, ..base.clone() }), "all-day");
        assert!(!is_meeting(&Event { my_status: "declined".into(), ..base.clone() }), "declined");
        assert!(!is_meeting(&Event { free: true, ..base.clone() }), "marked free");
        assert!(!is_meeting(&Event { attendees: 0, video_url: None, ..base.clone() }), "just you: a personal block");
        assert!(is_meeting(&Event { attendees: 0, ..base }), "a call link on your own event is a meeting");
    }

    #[test]
    fn spans_skip_what_was_recorded_and_find_tickets() {
        let (conn, dir) = db("spans");
        let events = vec![
            ev("a", "BILL-142 proration sync", "2026-10-20T10:00:00Z", "2026-10-20T10:30:00Z"),
            ev("b", "Standup", "2026-10-20T09:00:00Z", "2026-10-20T09:15:00Z"),
            // The same standup on a shared calendar.
            Event { uid: "b2".into(), calendar: "Team".into(), ..ev("b2", "Standup", "2026-10-20T09:00:00Z", "2026-10-20T09:15:00Z") },
            ev("c", "Planning", "2026-10-20T14:00:00Z", "2026-10-20T15:00:00Z"),
        ];
        store(&conn, "2026-10-20T00:00:00Z", "2026-10-21T00:00:00Z", &events).unwrap();
        // You recorded most of planning.
        let recorded = vec![("MTG:m1".to_string(), "Meeting · Planning".to_string(), "2026-10-20T14:05:00Z".to_string(), "2026-10-20T15:00:00Z".to_string())];
        let later = parse_ts("2026-10-21T00:00:00Z").unwrap();
        let spans = day_spans(&conn, "2026-10-20T00:00:00Z", "2026-10-21T00:00:00Z", &recorded, later).unwrap();
        let keys: Vec<&str> = spans.iter().map(|s| s.0.as_str()).collect();
        assert_eq!(keys, vec!["CAL:b", "T:BILL-142"], "standup once, the ticket meeting under its ticket, planning left to the recording");
        assert_eq!(spans[0].1, "Meeting · Standup");
        // At 10:10 the ticket meeting is under way (counted to now) and planning hasn't started.
        let at = parse_ts("2026-10-20T10:10:00Z").unwrap();
        let spans = day_spans(&conn, "2026-10-20T00:00:00Z", "2026-10-21T00:00:00Z", &[], at).unwrap();
        let shape: Vec<(&str, &str)> = spans.iter().map(|s| (s.0.as_str(), s.3.as_str())).collect();
        assert_eq!(shape, vec![("CAL:b", "2026-10-20T09:15:00Z"), ("T:BILL-142", "2026-10-20T10:10:00Z")]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn reminders_go_once_and_survive_a_refresh() {
        let (conn, dir) = db("reminders");
        let events = vec![ev("a", "Sync", "2026-10-20T10:04:00Z", "2026-10-20T10:30:00Z")];
        let window = ("2026-10-20T00:00:00Z", "2026-10-21T00:00:00Z");
        store(&conn, window.0, window.1, &events).unwrap();
        assert!(due_reminders(&conn, "2026-10-20T09:55:00Z", 5).unwrap().is_empty(), "too early");
        assert_eq!(due_reminders(&conn, "2026-10-20T10:00:00Z", 5).unwrap().len(), 1);
        store(&conn, window.0, window.1, &events).unwrap();
        assert!(due_reminders(&conn, "2026-10-20T10:01:00Z", 5).unwrap().is_empty(), "already reminded");
        let _ = std::fs::remove_dir_all(dir);
    }
}
