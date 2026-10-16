//! Meeting recording and transcription. The Swift helper (`recorder/Recorder.swift`) does the
//! audio capture and Apple on-device transcription; this module drives it, merges the two tracks
//! (your mic = "you", the call = "others") into one transcript, and stores everything locally.

use anyhow::{anyhow, bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};

use crate::derive::fmt_ts;

/// Where the helper binary lives: next to the app binary when bundled, else in `binaries/` (dev).
pub fn helper_path() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let bundled = exe.with_file_name("dev-inbox-recorder");
    if bundled.exists() {
        return Ok(bundled);
    }
    let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("binaries").join(format!("dev-inbox-recorder-{}", env!("RECORDER_TRIPLE")));
    if dev.exists() {
        return Ok(dev);
    }
    Err(anyhow!("The meeting recorder isn't available on this platform"))
}

/// Run the helper to completion and return what it printed. Never waits longer than `timeout`: a
/// stuck helper (speech model download, Apple Intelligence) is killed rather than hanging a job.
pub fn run_helper(args: &[&std::ffi::OsStr], timeout: std::time::Duration) -> Result<String> {
    let mut cmd = Command::new(helper_path()?);
    cmd.args(args);
    run_with_timeout(cmd, timeout)
}

/// Run a command, collect its stdout, and kill it if it runs past `timeout`.
pub fn run_with_timeout(mut cmd: Command, timeout: std::time::Duration) -> Result<String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("Couldn't start the meeting helper")?;
    let mut stdout = child.stdout.take().ok_or_else(|| anyhow!("no helper stdout"))?;
    // Read on another thread so a full pipe can't block the helper while we wait for it.
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut out);
        out
    });
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            bail!("took longer than {} minutes and was stopped", timeout.as_secs().div_ceil(60));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let out = reader.join().unwrap_or_default();
    if out.trim().is_empty() && !status.success() {
        bail!("the meeting helper stopped unexpectedly ({status})");
    }
    Ok(out)
}

/// A running recording: the helper process and the pipe used to tell it to stop.
pub struct Recording {
    pub meeting_id: String,
    pub child: Child,
    pub stdin: ChildStdin,
}

/// Start the helper recording into `dir`. `on_event` receives each JSON line it prints.
pub fn start(meeting_id: &str, dir: &Path, on_event: impl Fn(Value) + Send + 'static) -> Result<Recording> {
    std::fs::create_dir_all(dir)?;
    let mut child = Command::new(helper_path()?)
        .arg("record")
        .arg(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("Couldn't start the recorder")?;
    let stdin = child.stdin.take().ok_or_else(|| anyhow!("no recorder stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("no recorder stdout"))?;
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                on_event(v);
            }
        }
        // The helper is gone — after "stopped" normally, but also if it crashed mid-recording.
        on_event(serde_json::json!({"event": "exited"}));
    });
    Ok(Recording { meeting_id: meeting_id.to_string(), child, stdin })
}

/// Ask the helper to finish writing its files and exit.
pub fn stop(mut r: Recording) -> Result<()> {
    let _ = writeln!(r.stdin, "stop");
    drop(r.stdin);
    // Give it time to close the audio files cleanly; kill only if it hangs.
    for _ in 0..100 {
        if r.child.try_wait()?.is_some() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = r.child.kill();
    Ok(())
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    /// you | others
    pub speaker: String,
    pub text: String,
}

/// Transcribe one audio file with the helper (on-device).
pub fn transcribe_file(path: &Path, speaker: &str, locale: &str, timeout: std::time::Duration) -> Result<Vec<Segment>> {
    let mut args = vec![std::ffi::OsStr::new("transcribe"), path.as_os_str()];
    if !locale.is_empty() {
        args.push(std::ffi::OsStr::new(locale));
    }
    let out = run_helper(&args, timeout).context("Transcription")?;
    let mut segments = Vec::new();
    let mut error = None;
    for line in out.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v["event"] == "error" {
            error = v["message"].as_str().map(String::from);
        }
        if let (Some(start), Some(end), Some(text)) = (v["start"].as_f64(), v["end"].as_f64(), v["text"].as_str()) {
            segments.push(Segment { start, end, speaker: speaker.into(), text: text.into() });
        }
    }
    match error {
        Some(e) if segments.is_empty() => Err(anyhow!(e)),
        _ => Ok(segments),
    }
}

fn words(s: &str) -> std::collections::HashSet<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2)
        .map(String::from)
        .collect()
}

/// Merge both tracks by time. When you're on speakers, your mic also hears the call, so a "you"
/// line that overlaps a near-identical "others" line is an echo and is dropped.
pub fn merge(mut you: Vec<Segment>, others: Vec<Segment>) -> Vec<Segment> {
    you.retain(|y| {
        let yw = words(&y.text);
        !others.iter().any(|o| {
            let overlap = y.start < o.end + 1.0 && o.start < y.end + 1.0;
            if !overlap || yw.is_empty() {
                return false;
            }
            let ow = words(&o.text);
            let shared = yw.intersection(&ow).count() as f64;
            shared / (yw.len().min(ow.len()).max(1) as f64) >= 0.6
        })
    });
    let mut all: Vec<Segment> = you.into_iter().chain(others).collect();
    all.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
    all
}

/// The ticket a meeting is about: the key in its title ("IO-2641 refinement"). Not the transcript: a
/// standup mentions tickets without being about them, and spoken keys rarely transcribe cleanly anyway.
pub fn guess_ticket(title: &str) -> Option<String> {
    crate::day::ticket_in_text(title)
}

// ---------- storage ----------

pub fn create(conn: &Connection, id: &str, title: &str, started: &str, dir: &Path) -> Result<()> {
    conn.execute(
        "INSERT INTO meetings (id, title, started_at, status, audio_dir) VALUES (?1, ?2, ?3, 'recording', ?4)",
        params![id, title, started, dir.to_string_lossy()],
    )?;
    Ok(())
}

pub fn set_status(conn: &Connection, id: &str, status: &str, error: Option<&str>) -> Result<()> {
    conn.execute("UPDATE meetings SET status = ?2, error = ?3 WHERE id = ?1", params![id, status, error])?;
    Ok(())
}

pub fn finish_recording(conn: &Connection, id: &str, ended: &str, seconds: i64, sources: &str) -> Result<()> {
    conn.execute(
        "UPDATE meetings SET ended_at = ?2, seconds = ?3, sources = ?4, status = 'transcribing' WHERE id = ?1",
        params![id, ended, seconds, sources],
    )?;
    Ok(())
}

pub fn save_transcript(conn: &Connection, id: &str, segments: &[Segment]) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM meeting_segments WHERE meeting_id = ?1", [id])?;
    for s in segments {
        tx.execute(
            "INSERT INTO meeting_segments (meeting_id, start, end, speaker, text) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, s.start, s.end, s.speaker, s.text],
        )?;
    }
    // Only fill the ticket in if the user hasn't chosen one.
    let title: String = tx.query_row("SELECT title FROM meetings WHERE id = ?1", [id], |r| r.get(0)).unwrap_or_default();
    if let Some(t) = guess_ticket(&title) {
        tx.execute("UPDATE meetings SET ticket = COALESCE(ticket, ?2) WHERE id = ?1", params![id, t])?;
    }
    tx.execute("UPDATE meetings SET status = 'done', error = NULL WHERE id = ?1", [id])?;
    tx.commit()?;
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Meeting {
    pub id: String,
    pub title: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub seconds: Option<i64>,
    pub status: String,
    pub error: Option<String>,
    pub ticket: Option<String>,
    pub has_audio: bool,
    pub audio_dir: Option<String>,
    pub sources: Option<String>,
    /// First words of the transcript, for the list.
    pub preview: Option<String>,
    /// AI notes, once made.
    pub summary: Option<crate::summary::Summary>,
    /// apple | claude
    pub summary_by: Option<String>,
    /// summarising | done | failed; None when no summary was asked for.
    pub summary_status: Option<String>,
    pub summary_error: Option<String>,
}

fn row_to_meeting(r: &rusqlite::Row) -> rusqlite::Result<Meeting> {
    let deleted: bool = r.get(8)?;
    Ok(Meeting {
        id: r.get(0)?,
        title: r.get(1)?,
        started_at: r.get(2)?,
        ended_at: r.get(3)?,
        seconds: r.get(4)?,
        status: r.get(5)?,
        error: r.get(6)?,
        ticket: r.get(7)?,
        has_audio: !deleted,
        audio_dir: r.get(9)?,
        sources: r.get(10)?,
        preview: r.get(11)?,
        summary: r.get::<_, Option<String>>(12)?.and_then(|s| serde_json::from_str(&s).ok()),
        summary_by: r.get(13)?,
        summary_status: r.get(14)?,
        summary_error: r.get(15)?,
    })
}

const MEETING_COLUMNS: &str = "m.id, m.title, m.started_at, m.ended_at, m.seconds, m.status, m.error, m.ticket, m.audio_deleted, m.audio_dir, m.sources,
    (SELECT group_concat(text, ' ') FROM (SELECT text FROM meeting_segments s WHERE s.meeting_id = m.id ORDER BY start LIMIT 3)),
    m.summary, m.summary_by, m.summary_status, m.summary_error";

pub fn list(conn: &Connection) -> Result<Vec<Meeting>> {
    let mut stmt = conn.prepare(&format!("SELECT {MEETING_COLUMNS} FROM meetings m ORDER BY m.started_at DESC"))?;
    let rows = stmt.query_map([], row_to_meeting)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Meetings that started in `[start, end)`, oldest first.
pub fn started_between(conn: &Connection, start: &str, end: &str) -> Result<Vec<Meeting>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {MEETING_COLUMNS} FROM meetings m WHERE m.started_at >= ?1 AND m.started_at < ?2 ORDER BY m.started_at"
    ))?;
    let rows = stmt.query_map(params![start, end], row_to_meeting)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Meetings whose recording, transcription or notes never finished: (id, status, summary status, audio dir).
pub fn unfinished(conn: &Connection) -> Result<Vec<(String, String, Option<String>, Option<String>)>> {
    let mut stmt = conn.prepare(
        "SELECT id, status, summary_status, CASE WHEN audio_deleted THEN NULL ELSE audio_dir END FROM meetings
         WHERE status IN ('recording', 'transcribing') OR summary_status = 'summarising'",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// A recording that ended without the helper saying "stopped" (it crashed, or the app quit): keep
/// what was saved, end it when the audio files were last written, and offer to transcribe it.
pub fn interrupted_recording(conn: &Connection, id: &str, dir: Option<&Path>) -> Result<()> {
    let saved: Vec<(&str, std::fs::Metadata)> = ["mic", "system"]
        .into_iter()
        .filter_map(|name| {
            let meta = std::fs::metadata(dir?.join(format!("{name}.m4a"))).ok()?;
            (meta.len() > 1024).then_some((name, meta))
        })
        .collect();
    let last_write = saved.iter().filter_map(|(_, m)| m.modified().ok()).max().map(|t| fmt_ts(t.into()));
    let sources = saved.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(",");
    let message = if saved.is_empty() {
        "Recording was interrupted before anything was saved."
    } else {
        "Recording was interrupted. Try again to transcribe what was saved."
    };
    conn.execute(
        "UPDATE meetings SET status = 'failed', error = ?2, sources = ?3,
             ended_at = COALESCE(ended_at, ?4, started_at),
             seconds = COALESCE(seconds, CAST(strftime('%s', COALESCE(?4, started_at)) - strftime('%s', started_at) AS INTEGER))
         WHERE id = ?1 AND status = 'recording'",
        params![id, message, sources, last_write],
    )?;
    Ok(())
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<(Meeting, Vec<Segment>)>> {
    let Some(m) = conn
        .query_row(&format!("SELECT {MEETING_COLUMNS} FROM meetings m WHERE m.id = ?1"), [id], row_to_meeting)
        .optional()?
    else {
        return Ok(None);
    };
    let mut stmt = conn.prepare("SELECT start, end, speaker, text FROM meeting_segments WHERE meeting_id = ?1 ORDER BY start")?;
    let segments = stmt
        .query_map([id], |r| Ok(Segment { start: r.get(0)?, end: r.get(1)?, speaker: r.get(2)?, text: r.get(3)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some((m, segments)))
}

pub fn update(conn: &Connection, id: &str, title: &str, ticket: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE meetings SET title = ?2, ticket = ?3 WHERE id = ?1",
        params![id, title.trim(), ticket.map(str::trim).filter(|t| !t.is_empty())],
    )?;
    Ok(())
}

pub fn delete(conn: &Connection, id: &str) -> Result<()> {
    if let Some(dir) = conn
        .query_row("SELECT audio_dir FROM meetings WHERE id = ?1", [id], |r| r.get::<_, Option<String>>(0))
        .optional()?
        .flatten()
    {
        let _ = std::fs::remove_dir_all(dir);
    }
    conn.execute("DELETE FROM meeting_segments WHERE meeting_id = ?1", [id])?;
    conn.execute("DELETE FROM meeting_actions WHERE meeting_id = ?1", [id])?;
    conn.execute("DELETE FROM meetings WHERE id = ?1", [id])?;
    Ok(())
}

/// Delete audio older than `days` (transcripts stay). `days == 0` keeps audio forever.
pub fn apply_retention(conn: &Connection, days: i64) -> Result<usize> {
    if days <= 0 {
        return Ok(0);
    }
    let cutoff = fmt_ts(chrono::Utc::now() - chrono::Duration::days(days));
    let mut stmt = conn.prepare(
        "SELECT id, audio_dir FROM meetings WHERE audio_deleted = 0 AND status != 'recording' AND started_at < ?1",
    )?;
    let old: Vec<(String, Option<String>)> = stmt.query_map([cutoff], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    for (id, dir) in &old {
        if let Some(dir) = dir {
            let _ = std::fs::remove_dir_all(dir);
        }
        conn.execute("UPDATE meetings SET audio_deleted = 1 WHERE id = ?1", [id])?;
    }
    Ok(old.len())
}

/// Meetings overlapping [start, end) as exact Day-view blocks: (key, label, start, end).
pub fn day_spans(conn: &Connection, start: &str, end: &str) -> Result<Vec<(String, String, String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, started_at, COALESCE(ended_at, started_at), ticket FROM meetings
         WHERE started_at < ?2 AND COALESCE(ended_at, started_at) >= ?1 AND status != 'recording'",
    )?;
    let rows = stmt
        .query_map(params![start, end], |r| {
            let id: String = r.get(0)?;
            let title: String = r.get(1)?;
            let ticket: Option<String> = r.get(4)?;
            let key = match &ticket {
                Some(t) => format!("T:{t}"),
                None => format!("MTG:{id}"),
            };
            let label = match &ticket {
                Some(t) => t.clone(),
                None if title.to_lowercase().starts_with("meeting") => title,
                None => format!("Meeting · {title}"),
            };
            Ok((key, label, r.get(2)?, r.get(3)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: f64, end: f64, speaker: &str, text: &str) -> Segment {
        Segment { start, end, speaker: speaker.into(), text: text.into() }
    }

    #[test]
    fn helper_runs_are_bounded() {
        let mut echo = Command::new("sh");
        echo.args(["-c", "echo '{\"event\":\"done\"}'"]);
        assert_eq!(run_with_timeout(echo, std::time::Duration::from_secs(5)).unwrap().trim(), r#"{"event":"done"}"#);

        let started = std::time::Instant::now();
        let mut stuck = Command::new("sleep");
        stuck.arg("30");
        let err = run_with_timeout(stuck, std::time::Duration::from_millis(300)).unwrap_err();
        assert!(err.to_string().contains("was stopped"), "{err}");
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "a stuck helper is killed, not waited for");

        let mut crash = Command::new("sh");
        crash.args(["-c", "kill -9 $$"]);
        assert!(run_with_timeout(crash, std::time::Duration::from_secs(5)).unwrap_err().to_string().contains("unexpectedly"));
    }

    #[test]
    fn interrupted_work_is_found_and_settled() {
        let base = std::env::temp_dir().join(format!("dev-inbox-interrupted-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let conn = crate::db::open(&base.join("db.sqlite3")).unwrap();
        let dir = base.join("m1");
        create(&conn, "m1", "Standup", "2026-10-16T09:00:00Z", &dir).unwrap();
        create(&conn, "m2", "Planning", "2026-10-16T10:00:00Z", &base.join("m2")).unwrap();
        finish_recording(&conn, "m2", "2026-10-16T10:30:00Z", 1800, "mic").unwrap();
        let ids: Vec<(String, String)> = unfinished(&conn).unwrap().into_iter().map(|(id, status, ..)| (id, status)).collect();
        assert_eq!(ids, [("m1".to_string(), "recording".to_string()), ("m2".to_string(), "transcribing".to_string())]);

        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mic.m4a"), vec![0u8; 4096]).unwrap();
        interrupted_recording(&conn, "m1", Some(&dir)).unwrap();
        let (m, _) = get(&conn, "m1").unwrap().unwrap();
        assert_eq!(m.status, "failed");
        assert_eq!(m.sources.as_deref(), Some("mic"));
        assert!(m.error.unwrap().contains("Try again"));
        assert!(m.ended_at.is_some() && m.seconds.is_some());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn merges_and_drops_echo() {
        let you = vec![
            seg(0.0, 3.0, "you", "Morning, shall we start with the release?"),
            seg(10.2, 13.0, "you", "the country search filter is done and in review"), // echo of the call
        ];
        let others = vec![seg(4.0, 9.0, "others", "Yes, IO-2641 first."), seg(10.0, 13.0, "others", "The country search filter is done and in review.")];
        let merged = merge(you, others);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].speaker, "you");
        assert_eq!(merged[1].text, "Yes, IO-2641 first.");
        assert_eq!(merged[2].speaker, "others", "the echo on the mic is dropped");
        assert_eq!(guess_ticket("ES-12 sync").as_deref(), Some("ES-12"));
        assert_eq!(guess_ticket("Standup"), None, "mentions in the transcript don't count");
    }
}

#[cfg(test)]
mod live_tests {
    /// Real helper, real on-device transcription: `MEETING_DIR=... cargo test meeting_pipeline -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn meeting_pipeline() {
        let dir = std::path::PathBuf::from(std::env::var("MEETING_DIR").expect("MEETING_DIR"));
        let you = super::transcribe_file(&dir.join("mic.m4a"), "you", "en_GB", std::time::Duration::from_secs(600)).unwrap();
        let others = super::transcribe_file(&dir.join("system.m4a"), "others", "en_GB", std::time::Duration::from_secs(600)).unwrap();
        let merged = super::merge(you, others);
        for s in &merged {
            println!("{:>5.1}s  {:<6}  {}", s.start, s.speaker, s.text);
        }
        assert!(merged.iter().any(|s| s.speaker == "you") && merged.iter().any(|s| s.speaker == "others"));
    }
}
