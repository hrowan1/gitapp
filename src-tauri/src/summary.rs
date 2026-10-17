//! Meeting summaries: an overview, decisions and action items from a transcript. Made either on this
//! Mac by Apple Intelligence (through the recorder helper) or by Claude (Anthropic's API, with the
//! user's own API key). Which one is a setting; "off" makes none.

use anyhow::{anyhow, bail, Context, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::meetings::Segment;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub overview: String,
    pub decisions: Vec<String>,
    pub action_items: Vec<ActionItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionItem {
    /// you | others | unclear
    pub owner: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Provider {
    /// Apple Intelligence, on this Mac.
    Apple,
    /// Claude, through Anthropic's API.
    Claude,
    Off,
}

impl Provider {
    pub fn from_setting(s: Option<&str>) -> Self {
        match s {
            Some("claude") => Provider::Claude,
            Some("off") => Provider::Off,
            _ => Provider::Apple,
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Provider::Apple => "apple",
            Provider::Claude => "claude",
            Provider::Off => "off",
        }
    }
}

/// The transcript as the models read it: "[12:03] You: …", "[12:09] Sam: …" lines.
pub fn transcript_text(segments: &[Segment], names: &std::collections::HashMap<String, String>) -> String {
    segments
        .iter()
        .map(|s| {
            let t = s.start.max(0.0) as u64;
            let who = crate::meetings::speaker_label(s, names);
            format!("[{}:{:02}] {who}: {}", t / 60, t % 60, s.text)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------- Apple Intelligence (on this Mac) ----------

/// Summarise with the on-device model via the recorder helper (it chunks long meetings itself).
pub fn on_device(meeting_id: &str, transcript: &str) -> Result<Summary> {
    let path = std::env::temp_dir().join(format!("dev-inbox-transcript-{meeting_id}.txt"));
    std::fs::write(&path, transcript)?;
    let out = crate::meetings::run_helper(
        &[std::ffi::OsStr::new("summarise"), path.as_os_str()],
        std::time::Duration::from_secs(15 * 60),
    );
    let _ = std::fs::remove_file(&path);
    parse_helper_output(&out.context("Summarising on this Mac")?)
}

fn parse_helper_output(stdout: &str) -> Result<Summary> {
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        match v["event"].as_str() {
            Some("summary") => return Ok(serde_json::from_value(v)?),
            Some("error") => bail!("{}", v["message"].as_str().unwrap_or("Summarising failed")),
            _ => {}
        }
    }
    bail!("The on-device summariser stopped without an answer")
}

// ---------- Claude ----------

pub const CLAUDE_MODEL: &str = "claude-opus-5-5";

const CLAUDE_SYSTEM: &str = "You write notes for a meeting from its transcript, for the person who recorded it. \
Lines are labelled \"You\" (that person); other people are labelled by name, \"Speaker 2\" and so on, or \
\"Others\" when voices couldn't be told apart. Use names where the transcript gives them. The transcript comes from \
speech recognition, so expect misheard words and fix obvious ones from context. Only use what is in the \
transcript: don't invent names, numbers or decisions. The overview is two to four plain sentences on what the \
meeting was about and where it ended up. Decisions are things the group settled on. Action items are things \
someone agreed to do, each one short sentence, with the owner \"you\" when it's the person who recorded the \
meeting, \"others\" when it's someone else, or \"unclear\". Leave a list empty rather than padding it.";

fn claude_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "overview": {"type": "string"},
            "decisions": {"type": "array", "items": {"type": "string"}},
            "actionItems": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "owner": {"type": "string", "enum": ["you", "others", "unclear"]},
                        "text": {"type": "string"}
                    },
                    "required": ["owner", "text"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["overview", "decisions", "actionItems"],
        "additionalProperties": false
    })
}

/// The Messages API request body for one meeting.
pub fn claude_request(title: &str, transcript: &str) -> Value {
    json!({
        "model": CLAUDE_MODEL,
        "max_tokens": 16000,
        "system": CLAUDE_SYSTEM,
        "output_config": {"format": {"type": "json_schema", "schema": claude_schema()}},
        "messages": [{
            "role": "user",
            "content": format!("Meeting: {title}\n\n<transcript>\n{transcript}\n</transcript>"),
        }],
    })
}

/// Read the summary out of a Messages API response (structured output: the text block is the JSON).
pub fn parse_claude_response(v: &Value) -> Result<Summary> {
    match v["stop_reason"].as_str() {
        Some("refusal") => bail!("Claude declined to summarise this meeting"),
        Some("max_tokens") => bail!("Claude's summary was cut off; try again"),
        _ => {}
    }
    let text = v["content"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|b| b["type"] == "text")
        .and_then(|b| b["text"].as_str())
        .ok_or_else(|| anyhow!("Claude didn't return a summary"))?;
    serde_json::from_str(text).context("Claude's summary wasn't in the expected shape")
}

pub async fn with_claude(api_key: &str, title: &str, transcript: &str) -> Result<Summary> {
    let http = reqwest::Client::builder().timeout(std::time::Duration::from_secs(600)).build()?;
    let res = http
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&claude_request(title, transcript))
        .send()
        .await
        .context("Couldn't reach Anthropic")?;
    let status = res.status();
    let body: Value = res.json().await.unwrap_or(Value::Null);
    if status == reqwest::StatusCode::UNAUTHORIZED {
        bail!("Anthropic rejected the API key (Settings → Meetings)");
    }
    if !status.is_success() {
        let msg = body["error"]["message"].as_str().unwrap_or("");
        bail!("Anthropic returned {status} {msg}");
    }
    parse_claude_response(&body)
}

// ---------- storage ----------

/// status: summarising | done | failed (None: not summarised, e.g. summaries are off).
pub fn set_status(conn: &Connection, meeting_id: &str, status: Option<&str>, error: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE meetings SET summary_status = ?2, summary_error = ?3 WHERE id = ?1",
        params![meeting_id, status, error],
    )?;
    Ok(())
}

pub fn save(conn: &Connection, meeting_id: &str, summary: &Summary, by: Provider) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE meetings SET summary = ?2, summary_by = ?3, summary_status = 'done', summary_error = NULL WHERE id = ?1",
        params![meeting_id, serde_json::to_string(summary)?, by.id()],
    )?;
    sync_actions(&tx, meeting_id, summary)?;
    crate::search::reindex(&tx, meeting_id)?;
    tx.commit()?;
    Ok(())
}

// ---------- action items ----------

/// Keep a meeting's action items in step with its notes. Rewritten notes keep the progress (done,
/// snoozed) of items whose wording didn't change; items no longer in the notes go.
pub fn sync_actions(conn: &Connection, meeting_id: &str, summary: &Summary) -> Result<()> {
    let texts: Vec<&str> = summary.action_items.iter().map(|a| a.text.trim()).collect();
    let mut stmt = conn.prepare("SELECT id, text FROM meeting_actions WHERE meeting_id = ?1")?;
    let existing: Vec<(i64, String)> =
        stmt.query_map([meeting_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    for (id, text) in existing {
        if !texts.contains(&text.as_str()) {
            conn.execute("DELETE FROM meeting_actions WHERE id = ?1", [id])?;
        }
    }
    for (position, a) in summary.action_items.iter().enumerate() {
        conn.execute(
            "INSERT INTO meeting_actions (meeting_id, owner, text, position) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(meeting_id, text) DO UPDATE SET owner = excluded.owner, position = excluded.position",
            params![meeting_id, a.owner, a.text.trim(), position as i64],
        )?;
    }
    Ok(())
}

/// One action item from a meeting, with your progress on it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub id: i64,
    pub meeting_id: String,
    pub meeting_title: String,
    /// When the meeting started.
    pub meeting_at: String,
    pub ticket: Option<String>,
    /// you | others | unclear
    pub owner: String,
    pub text: String,
    pub done: bool,
    pub snoozed_until: Option<String>,
}

const ACTION_SQL: &str = "SELECT a.id, a.meeting_id, m.title, m.started_at, m.ticket, a.owner, a.text, a.done_at IS NOT NULL, a.snoozed_until
    FROM meeting_actions a JOIN meetings m ON m.id = a.meeting_id";

fn action_rows(conn: &Connection, sql: &str, args: impl rusqlite::Params) -> Result<Vec<Action>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(args, |r| {
            Ok(Action {
                id: r.get(0)?,
                meeting_id: r.get(1)?,
                meeting_title: r.get(2)?,
                meeting_at: r.get(3)?,
                ticket: r.get(4)?,
                owner: r.get(5)?,
                text: r.get(6)?,
                done: r.get(7)?,
                snoozed_until: r.get(8)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Your open action items for Today: not done, not snoozed past `now`. Newest meeting first.
pub fn open_actions(conn: &Connection, now: &str) -> Result<Vec<Action>> {
    action_rows(
        conn,
        &format!(
            "{ACTION_SQL} WHERE a.owner = 'you' AND a.done_at IS NULL AND (a.snoozed_until IS NULL OR a.snoozed_until <= ?1)
             ORDER BY m.started_at DESC, a.position"
        ),
        [now],
    )
}

/// All of one meeting's action items, in the notes' order.
pub fn meeting_actions(conn: &Connection, meeting_id: &str) -> Result<Vec<Action>> {
    action_rows(conn, &format!("{ACTION_SQL} WHERE a.meeting_id = ?1 ORDER BY a.position"), [meeting_id])
}

pub fn set_done(conn: &Connection, id: i64, done: bool, now: &str) -> Result<()> {
    conn.execute(
        "UPDATE meeting_actions SET done_at = CASE WHEN ?2 THEN ?3 END, snoozed_until = NULL WHERE id = ?1",
        params![id, done, now],
    )?;
    Ok(())
}

/// Hide until `until` (None: show again now).
pub fn snooze(conn: &Connection, id: i64, until: Option<&str>) -> Result<()> {
    conn.execute("UPDATE meeting_actions SET snoozed_until = ?2 WHERE id = ?1", params![id, until])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_lines() {
        let s = |start: f64, speaker: &str, text: &str| Segment { start, end: start + 2.0, speaker: speaker.into(), text: text.into(), voice: None };
        assert_eq!(
            transcript_text(&[s(4.2, "others", "Where does proration happen?"), s(725.0, "you", "On the day.")], &Default::default()),
            "[0:04] Others: Where does proration happen?\n[12:05] You: On the day."
        );
    }

    #[test]
    fn claude_request_and_response() {
        let req = claude_request("Proration sync", "[0:04] You: hi");
        assert_eq!(req["model"], CLAUDE_MODEL);
        assert_eq!(req["output_config"]["format"]["type"], "json_schema");
        assert_eq!(req["output_config"]["format"]["schema"]["additionalProperties"], false);
        assert!(req["messages"][0]["content"].as_str().unwrap().contains("<transcript>\n[0:04] You: hi\n</transcript>"));

        let res = json!({"stop_reason": "end_turn", "content": [
            {"type": "thinking", "thinking": "…"},
            {"type": "text", "text": r#"{"overview":"Agreed proration.","decisions":["Prorate daily"],"actionItems":[{"owner":"you","text":"Add credit notes"}]}"#}
        ]});
        let s = parse_claude_response(&res).unwrap();
        assert_eq!(s.decisions, ["Prorate daily"]);
        assert_eq!(s.action_items[0], ActionItem { owner: "you".into(), text: "Add credit notes".into() });
        assert!(parse_claude_response(&json!({"stop_reason": "refusal", "content": []})).is_err());
    }

    #[test]
    fn action_items_follow_the_notes() {
        let path = std::env::temp_dir().join(format!("dev-inbox-actions-{}.sqlite3", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let conn = crate::db::open(&path).unwrap();
        crate::meetings::create(&conn, "m1", "Proration sync", "2026-10-16T14:00:00Z", &std::env::temp_dir().join(format!("dev-inbox-actions-audio-{}", std::process::id()))).unwrap();
        let item = |owner: &str, text: &str| ActionItem { owner: owner.into(), text: text.into() };
        let notes = |items: Vec<ActionItem>| Summary { overview: "O".into(), decisions: vec![], action_items: items };
        save(&conn, "m1", &notes(vec![item("you", "Add credit notes"), item("others", "Check rounding"), item("you", "Email finance")]), Provider::Apple).unwrap();

        let now = "2026-10-16T15:00:00Z";
        let open: Vec<String> = open_actions(&conn, now).unwrap().into_iter().map(|a| a.text).collect();
        assert_eq!(open, ["Add credit notes", "Email finance"], "only yours, in order");

        let first = open_actions(&conn, now).unwrap()[0].id;
        set_done(&conn, first, true, now).unwrap();
        let second = open_actions(&conn, now).unwrap()[0].id;
        snooze(&conn, second, Some("2026-10-17T09:00:00Z")).unwrap();
        assert!(open_actions(&conn, now).unwrap().is_empty());
        assert_eq!(open_actions(&conn, "2026-10-17T09:30:00Z").unwrap().len(), 1, "back after the snooze");

        // Rewritten notes keep what you ticked off, drop what's gone, add what's new.
        save(&conn, "m1", &notes(vec![item("you", "Add credit notes"), item("you", "Book the retro")]), Provider::Claude).unwrap();
        let all = meeting_actions(&conn, "m1").unwrap();
        assert_eq!(all.iter().map(|a| (a.text.as_str(), a.done)).collect::<Vec<_>>(), [("Add credit notes", true), ("Book the retro", false)]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn helper_output() {
        let ok = r#"{"event":"summary","overview":"O","decisions":[],"actionItems":[{"owner":"others","text":"T"}]}"#;
        assert_eq!(parse_helper_output(ok).unwrap().action_items[0].owner, "others");
        let err = r#"{"event":"error","message":"Turn on Apple Intelligence"}"#;
        assert_eq!(parse_helper_output(err).unwrap_err().to_string(), "Turn on Apple Intelligence");
    }
}
