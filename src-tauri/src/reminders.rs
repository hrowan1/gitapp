//! Smarter reminders: the AI reads your comments that the keyword rules (intent.rs) didn't match but
//! that might still mean "I'll come back to this later", and parks the PR the same way the rules do.
//! Each comment is read once.

use anyhow::Result;
use chrono::Duration;
use rusqlite::{params, Connection};

use crate::db::{self, ActivityRow, Personal, PrRef};
use crate::derive::{self, fmt_ts, parse_ts, Ts};

pub const INSTRUCTIONS: &str = "You read a comment the user wrote on a pull request or issue, and decide whether they \
said they'd come back to it later (look at it again, review it after something happens, follow up). Reply with \
exactly one line: NONE if not, or LATER | <when, in their words, or \"unspecified\"> | <the sentence that says it>. \
Things they'll do right now, or asking others to do things, are NONE.";

/// What the AI read: none, or come back (when words, the sentence).
#[derive(Debug, PartialEq)]
pub enum Verdict {
    None,
    Later { when: Option<String>, note: String },
}

pub fn parse(answer: &str) -> Verdict {
    let line = answer.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
    let mut parts = line.split('|').map(str::trim);
    if !parts.next().unwrap_or_default().eq_ignore_ascii_case("LATER") {
        return Verdict::None;
    }
    let when = parts.next().filter(|w| !w.is_empty() && !w.eq_ignore_ascii_case("unspecified")).map(String::from);
    let note: String = parts.next().unwrap_or_default().trim_matches('"').chars().take(140).collect();
    Verdict::Later { when, note }
}

/// Comments waiting for the AI, oldest first: (key, pr id, at, body).
pub fn pending(conn: &Connection, limit: usize) -> Result<Vec<(String, String, String, String)>> {
    let mut stmt = conn.prepare("SELECT comment_key, pr_id, at, body FROM intent_queue WHERE NOT checked ORDER BY at LIMIT ?1")?;
    let rows = stmt.query_map([limit as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<Result<_, _>>()?;
    Ok(rows)
}

pub fn mark_checked(conn: &Connection, key: &str) -> Result<()> {
    conn.execute("UPDATE intent_queue SET checked = 1 WHERE comment_key = ?1", [key])?;
    Ok(())
}

/// Park the PR as the rules would. Returns (title for a notification, PR) when a reminder was set.
pub fn apply(conn: &Connection, key: &str, pr_id: &str, at: &str, verdict: &Verdict, now: Ts) -> Result<Option<(String, PrRef)>> {
    let Verdict::Later { when, note } = verdict else { return Ok(None) };
    if db::activity_exists(conn, key)? {
        return Ok(None);
    }
    let Some(pr) = PrRef::load(conn, pr_id)? else { return Ok(None) };
    let Some(at_ts) = parse_ts(at) else { return Ok(None) };
    // You've decided something about the item since writing the comment: leave it alone.
    if db::get_personal(conn, pr_id)?.is_some_and(|p| p.set_at.as_str() > at) {
        return Ok(None);
    }
    let until = when.as_deref().and_then(|w| crate::intent::parse_when(w, at_ts)).unwrap_or(at_ts + Duration::days(3));
    let back = format!("on {}", until.with_timezone(&chrono::Local).format("%a %-d %b, %H:%M"));
    db::set_personal(
        conn,
        pr_id,
        &Personal {
            state: derive::LATER.into(),
            note: (!note.is_empty()).then(|| format!("“{note}”")),
            until: Some(fmt_ts(until)),
            set_at: fmt_ts(now),
            waits_on: None,
        },
    )?;
    db::record_activity(
        conn,
        &ActivityRow { key: key.to_string(), at, kind: "marked", pr: Some(&pr), detail: format!("Remembered from your comment (AI) — back {back}") },
    )?;
    conn.execute("UPDATE intent_queue SET checked = 1 WHERE comment_key = ?1", params![key])?;
    Ok(Some((format!("Back {back}"), pr)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_ai_answer() {
        assert_eq!(parse("NONE"), Verdict::None);
        assert_eq!(parse("  none\n"), Verdict::None);
        assert_eq!(
            parse("LATER | tomorrow | I'll give this another pass tomorrow."),
            Verdict::Later { when: Some("tomorrow".into()), note: "I'll give this another pass tomorrow.".into() }
        );
        assert_eq!(parse("later | unspecified | will check once CI is green"), Verdict::Later { when: None, note: "will check once CI is green".into() });
        assert_eq!(parse("Sure! LATER"), Verdict::None, "only a clean answer counts");
    }

    #[test]
    fn maybe_intent_prefilter() {
        assert!(crate::intent::maybe_intent("I'll look at this once Sam's PR is in"));
        assert!(crate::intent::maybe_intent("Will check tomorrow"));
        assert!(!crate::intent::maybe_intent("LGTM, nice work"));
        assert!(!crate::intent::maybe_intent("> I'll do it later\n\nThanks!"), "quoted text is someone else's");
    }
}
