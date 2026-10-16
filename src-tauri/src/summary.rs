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

/// The transcript as the models read it: "[12:03] You: …" lines.
pub fn transcript_text(segments: &[Segment]) -> String {
    segments
        .iter()
        .map(|s| {
            let t = s.start.max(0.0) as u64;
            let who = if s.speaker == "you" { "You" } else { "Others" };
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
    let out = std::process::Command::new(crate::meetings::helper_path()?).arg("summarise").arg(&path).output();
    let _ = std::fs::remove_file(&path);
    parse_helper_output(&String::from_utf8_lossy(&out?.stdout))
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
Lines are labelled \"You\" (that person) or \"Others\" (everyone else on the call); the transcript comes from \
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
    conn.execute(
        "UPDATE meetings SET summary = ?2, summary_by = ?3, summary_status = 'done', summary_error = NULL WHERE id = ?1",
        params![meeting_id, serde_json::to_string(summary)?, by.id()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_lines() {
        let s = |start: f64, speaker: &str, text: &str| Segment { start, end: start + 2.0, speaker: speaker.into(), text: text.into() };
        assert_eq!(
            transcript_text(&[s(4.2, "others", "Where does proration happen?"), s(725.0, "you", "On the day.")]),
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
    fn helper_output() {
        let ok = r#"{"event":"summary","overview":"O","decisions":[],"actionItems":[{"owner":"others","text":"T"}]}"#;
        assert_eq!(parse_helper_output(ok).unwrap().action_items[0].owner, "others");
        let err = r#"{"event":"error","message":"Turn on Apple Intelligence"}"#;
        assert_eq!(parse_helper_output(err).unwrap_err().to_string(), "Turn on Apple Intelligence");
    }
}
