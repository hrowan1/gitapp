//! Suggesting the ticket an unlinked meeting is about, from its notes and your open tickets. Only a
//! suggestion: the meeting shows "Looks like BILL-142 — Link · Not this", and nothing is linked
//! until you say so.

use anyhow::Result;
use rusqlite::Connection;

/// Tickets a meeting could be about: open Jira tickets that involve you, plus ticket keys from open
/// PRs (branches and titles) for teams without Jira connected. (key, title), most recent first.
pub fn candidates(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut out: Vec<(String, String)> = conn
        .prepare("SELECT key, summary FROM jira_issues WHERE category != 'done' ORDER BY COALESCE(updated, '') DESC LIMIT 40")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let prs: Vec<(String, Option<String>)> = conn
        .prepare("SELECT title, branch FROM prs WHERE kind = 'pr' AND is_open ORDER BY updated_at DESC LIMIT 40")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    for (title, branch) in prs {
        let key = branch.as_deref().and_then(crate::day::ticket_in_branch).or_else(|| crate::day::ticket_in_text(&title));
        if let Some(key) = key {
            if !out.iter().any(|(k, _)| *k == key) {
                out.push((key, title));
            }
        }
    }
    Ok(out)
}

pub const INSTRUCTIONS: &str = "You decide which single ticket a meeting was mainly about, from its notes and a list \
of candidate tickets. Answer with only the ticket key, exactly as listed — or NONE if the meeting covered several \
tickets (like a standup), was about something not in the list, or you're not sure. Never guess.";

/// The prompt: the meeting's title and notes, then the candidates.
pub fn prompt(title: &str, notes: &crate::summary::Summary, candidates: &[(String, String)]) -> String {
    let mut p = format!("Meeting: {title}\nNotes: {}\n", notes.overview);
    for d in &notes.decisions {
        p.push_str(&format!("Decided: {d}\n"));
    }
    for a in &notes.action_items {
        p.push_str(&format!("Action: {}\n", a.text));
    }
    p.push_str("\nCandidate tickets:\n");
    for (key, title) in candidates {
        p.push_str(&format!("{key} — {title}\n"));
    }
    p.push_str("\nWhich ticket was this meeting about? Answer with the key or NONE.");
    p
}

/// The model's answer, if it named exactly one candidate.
pub fn pick<'a>(answer: &str, candidates: &'a [(String, String)]) -> Option<&'a (String, String)> {
    let named: Vec<&(String, String)> =
        candidates.iter().filter(|(key, _)| answer.to_uppercase().split(|c: char| !c.is_alphanumeric() && c != '-').any(|w| w == key)).collect();
    match named.as_slice() {
        [one] => Some(one),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_one_named_candidate_or_nothing() {
        let c = vec![("BILL-142".to_string(), "Prorate plan changes".to_string()), ("ES-305".to_string(), "VAT rate".to_string())];
        assert_eq!(pick("BILL-142", &c).map(|t| t.0.as_str()), Some("BILL-142"));
        assert_eq!(pick("It was about bill-142.", &c).map(|t| t.0.as_str()), Some("BILL-142"));
        assert!(pick("NONE", &c).is_none());
        assert!(pick("BILL-1420", &c).is_none(), "not a different key that starts the same");
        assert!(pick("BILL-142 or ES-305", &c).is_none(), "two is no answer");
        assert!(pick("MOB-77", &c).is_none(), "only listed tickets");
    }

    #[test]
    fn candidates_include_jira_and_pr_keys() {
        let path = std::env::temp_dir().join(format!("dev-inbox-suggest-{}.sqlite3", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let conn = crate::db::open(&path).unwrap();
        crate::demo::seed(&conn).unwrap();
        let keys: Vec<String> = candidates(&conn).unwrap().into_iter().map(|c| c.0).collect();
        assert!(keys.contains(&"BILL-142".to_string()) && keys.contains(&"MOB-77".to_string()), "{keys:?}");
        assert!(!keys.contains(&"ES-310".to_string()), "done tickets aren't candidates");
        let _ = std::fs::remove_file(&path);
    }
}
