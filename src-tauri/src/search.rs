//! Full-text search over meetings (SQLite FTS5): titles, every transcript line (with who said it)
//! and the AI notes. A meeting's rows are rebuilt from its current state whenever any of those change.

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::Serialize;

/// Rebuild one meeting's search rows (or remove them, if the meeting is gone).
pub fn reindex(conn: &Connection, meeting_id: &str) -> Result<()> {
    conn.execute("DELETE FROM meeting_search WHERE meeting_id = ?1", [meeting_id])?;
    let Some((meeting, segments)) = crate::meetings::get(conn, meeting_id)? else { return Ok(()) };
    let add = |kind: &str, start: Option<f64>, speaker: &str, text: &str| -> Result<()> {
        if !text.trim().is_empty() {
            conn.execute(
                "INSERT INTO meeting_search (meeting_id, kind, start, speaker, text) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![meeting_id, kind, start, speaker, text],
            )?;
        }
        Ok(())
    };
    add("title", None, "", &meeting.title)?;
    let names = crate::meetings::speaker_names(conn, meeting_id)?;
    for s in &segments {
        add("line", Some(s.start), &crate::meetings::speaker_label(s, &names), &s.text)?;
    }
    if let Some(n) = &meeting.summary {
        let mut notes = vec![n.overview.clone()];
        notes.extend(n.decisions.iter().cloned());
        notes.extend(n.action_items.iter().map(|a| a.text.clone()));
        add("notes", None, "", &notes.join("\n"))?;
    }
    Ok(())
}

/// What the user typed as an FTS5 query: every word must appear (as a prefix, so "prorat" finds
/// "proration"). Quoted, so punctuation and FTS syntax in the input can't break the query.
pub fn fts_query(input: &str) -> Option<String> {
    let terms: Vec<String> = input
        .split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '-')
        .map(|w| w.trim_matches(|c| c == '\'' || c == '-'))
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{}\"*", w.replace('"', "")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    /// title | line | notes
    pub kind: String,
    /// Seconds into the meeting, for transcript lines.
    pub start: Option<f64>,
    pub speaker: String,
    /// The matching text with matches wrapped in \u{2} … \u{3}.
    pub snippet: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Result_ {
    pub meeting_id: String,
    pub title: String,
    pub started_at: String,
    pub ticket: Option<String>,
    pub hits: Vec<Hit>,
}

/// Meetings matching `input`, best first, each with up to a few matching lines.
pub fn search(conn: &Connection, input: &str) -> Result<Vec<Result_>> {
    let Some(q) = fts_query(input) else { return Ok(Vec::new()) };
    let mut stmt = conn.prepare(
        "SELECT s.meeting_id, m.title, m.started_at, m.ticket, s.kind, s.start, s.speaker,
                snippet(meeting_search, 4, char(2), char(3), '…', 16)
         FROM meeting_search s JOIN meetings m ON m.id = s.meeting_id
         WHERE meeting_search MATCH ?1
         ORDER BY bm25(meeting_search) LIMIT 300",
    )?;
    let mut out: Vec<Result_> = Vec::new();
    let rows = stmt.query_map([q], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?,
            Hit { kind: r.get(4)?, start: r.get(5)?, speaker: r.get(6)?, snippet: r.get(7)? },
        ))
    })?;
    for row in rows {
        let (meeting_id, title, started_at, ticket, hit) = row?;
        match out.iter().position(|m| m.meeting_id == meeting_id) {
            Some(i) if out[i].hits.len() < 5 => out[i].hits.push(hit),
            Some(_) => {}
            None if out.len() < 30 => out.push(Result_ { meeting_id, title, started_at, ticket, hits: vec![hit] }),
            None => {}
        }
    }
    // Transcript lines in the order they were said.
    for m in &mut out {
        m.hits.sort_by(|a, b| a.start.unwrap_or(-1.0).total_cmp(&b.start.unwrap_or(-1.0)));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meetings::{self, Segment};

    #[test]
    fn query_is_safe_and_prefix_matched() {
        assert_eq!(fts_query("VAT rate").as_deref(), Some(r#""VAT"* "rate"*"#));
        assert_eq!(fts_query(r#"credit "note" OR (x) NEAR"#).as_deref(), Some(r#""credit"* "note"* "OR"* "x"* "NEAR"*"#));
        assert_eq!(fts_query("  ?! "), None);
    }

    #[test]
    fn finds_lines_notes_and_speakers() {
        let path = std::env::temp_dir().join(format!("dev-inbox-search-{}.sqlite3", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let conn = crate::db::open(&path).unwrap();
        let audio = std::env::temp_dir().join(format!("dev-inbox-search-audio-{}", std::process::id()));
        meetings::create(&conn, "m1", "Proration sync", "2026-10-16T14:00:00Z", &audio).unwrap();
        let seg = |start: f64, speaker: &str, voice: Option<&str>, text: &str| Segment {
            start,
            end: start + 3.0,
            speaker: speaker.into(),
            text: text.into(),
            voice: voice.map(String::from),
        };
        meetings::save_transcript(
            &conn,
            "m1",
            &[
                seg(4.0, "others", Some("1"), "Where does proration happen?"),
                seg(9.0, "you", None, "On the day the plan changes."),
                seg(15.0, "others", Some("2"), "Downgrades need a credit note."),
            ],
        )
        .unwrap();

        let r = search(&conn, "prorat").unwrap();
        assert_eq!(r.len(), 1);
        let kinds: Vec<&str> = r[0].hits.iter().map(|h| h.kind.as_str()).collect();
        assert!(kinds.contains(&"title") && kinds.contains(&"line"), "{kinds:?}");
        let line = r[0].hits.iter().find(|h| h.kind == "line").unwrap();
        assert_eq!(line.start, Some(4.0));
        assert!(line.snippet.contains("\u{2}proration\u{3}"), "{}", line.snippet);

        // Naming a voice makes it searchable by name.
        meetings::name_speaker(&conn, "m1", "2", "Sam").unwrap();
        let r = search(&conn, "sam credit").unwrap();
        assert_eq!(r[0].hits[0].speaker, "Sam");
        assert!(search(&conn, "nothing-like-this").unwrap().is_empty());

        meetings::delete(&conn, "m1").unwrap();
        assert!(search(&conn, "prorat").unwrap().is_empty(), "deleted meetings leave the index");
        let _ = std::fs::remove_file(&path);
    }
}
