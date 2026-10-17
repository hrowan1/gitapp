//! The one way Dev Inbox's AI features talk to a model: Apple Intelligence on this Mac (through the
//! helper) or Claude (Anthropic's Messages API, with the user's own key). Features do their own
//! retrieval and hand over a small, factual prompt; this module runs it, with timeouts, and says how
//! much text each engine can take.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

#[derive(Clone)]
pub enum Engine {
    /// Apple Intelligence, on this Mac.
    Apple,
    /// Claude, through Anthropic's API.
    Claude { key: String, model: &'static str },
}

impl Engine {
    /// Roughly how many characters of prompt (instructions + facts) the engine can take, leaving
    /// room for its answer. Apple's on-device model has a ~4,000-token context.
    pub fn budget(&self) -> usize {
        match self {
            Engine::Apple => 8_000,
            Engine::Claude { .. } => 150_000,
        }
    }

    /// "apple", or the Claude model id — recorded next to whatever it wrote.
    pub fn id(&self) -> &str {
        match self {
            Engine::Apple => "apple",
            Engine::Claude { model, .. } => model,
        }
    }
}

/// Instructions and the prompt (the facts plus the question).
pub struct Prompt {
    pub instructions: String,
    pub prompt: String,
    /// Longest answer wanted, in tokens (Claude; Apple's answers are naturally short).
    pub max_tokens: u32,
}

/// Run a prompt to completion. Blocking: call it from a background thread or `spawn_blocking`.
pub fn complete(engine: &Engine, p: &Prompt) -> Result<String> {
    match engine {
        Engine::Apple => on_device(p),
        Engine::Claude { key, model } => {
            let body = json!({
                "model": model,
                "max_tokens": p.max_tokens.max(1024),
                "system": p.instructions,
                "messages": [{"role": "user", "content": p.prompt}],
            });
            let response = tauri::async_runtime::block_on(claude(key, &body))?;
            claude_text(&response)
        }
    }
}

fn on_device(p: &Prompt) -> Result<String> {
    let path = std::env::temp_dir().join(format!("dev-inbox-ai-{}-{}.json", std::process::id(), uuid_ish()));
    std::fs::write(&path, serde_json::to_vec(&json!({"instructions": p.instructions, "prompt": p.prompt}))?)?;
    let out = crate::meetings::run_helper(
        &[std::ffi::OsStr::new("generate"), path.as_os_str()],
        std::time::Duration::from_secs(5 * 60),
    );
    let _ = std::fs::remove_file(&path);
    for line in out.context("Apple Intelligence")?.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        match v["event"].as_str() {
            Some("text") => return Ok(v["text"].as_str().unwrap_or_default().trim().to_string()),
            Some("error") => bail!("{}", v["message"].as_str().unwrap_or("Apple Intelligence failed")),
            _ => {}
        }
    }
    bail!("Apple Intelligence stopped without an answer")
}

fn uuid_ish() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default()
}

/// One Messages API call. Errors are put in words the user can act on.
pub async fn claude(api_key: &str, body: &Value) -> Result<Value> {
    let http = reqwest::Client::builder().timeout(std::time::Duration::from_secs(600)).build()?;
    let res = http
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(body)
        .send()
        .await
        .context("Couldn't reach Anthropic")?;
    let status = res.status();
    let response: Value = res.json().await.unwrap_or(Value::Null);
    if status == reqwest::StatusCode::UNAUTHORIZED {
        bail!("Anthropic rejected the API key (Settings → AI)");
    }
    if !status.is_success() {
        let msg = response["error"]["message"].as_str().unwrap_or("");
        bail!("Anthropic returned {status} {msg}");
    }
    match response["stop_reason"].as_str() {
        Some("refusal") => bail!("Claude declined to answer this"),
        Some("max_tokens") => bail!("Claude's answer was cut off; try again"),
        _ => Ok(response),
    }
}

/// The text of a Messages API response (thinking blocks skipped).
pub fn claude_text(response: &Value) -> Result<String> {
    let text: Vec<&str> = response["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect();
    if text.is_empty() {
        return Err(anyhow!("Claude didn't return an answer"));
    }
    Ok(text.join("").trim().to_string())
}

/// Cut `text` to at most `max` characters on a line boundary, marking the cut.
pub fn fit(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out = String::new();
    for line in text.lines() {
        if out.chars().count() + line.chars().count() + 1 > max.saturating_sub(40) {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("[… more left out to fit]\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_fitting() {
        let r = json!({"content": [{"type": "thinking", "thinking": "hmm"}, {"type": "text", "text": " Answer [S1]. "}]});
        assert_eq!(claude_text(&r).unwrap(), "Answer [S1].");
        assert!(claude_text(&json!({"content": []})).is_err());

        let long = (0..100).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let cut = fit(&long, 200);
        assert!(cut.chars().count() <= 200, "{}", cut.len());
        assert!(cut.starts_with("line 0\nline 1\n") && cut.ends_with("[… more left out to fit]\n"));
        assert_eq!(fit("short", 200), "short");
    }
}
