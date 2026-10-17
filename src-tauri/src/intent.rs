//! Deterministic reading of intent in plain text: PR/issue references, time phrases, and
//! "let's come back to this after X" in the user's own GitHub comments. No AI.

use chrono::{DateTime, Datelike, Duration, Local, NaiveTime, TimeZone, Utc, Weekday};
use regex::Regex;
use std::sync::LazyLock;

use crate::derive::Ts;

static FULL_REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([A-Za-z0-9][\w.-]*/[\w.-]+)#(\d+)\b").unwrap());
static URL_REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"github\.com/([\w.-]+/[\w.-]+)/(?:pull|issues)/(\d+)").unwrap());
static SHORT_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|[\s(\[])#(\d+)\b").unwrap());

/// References to PRs/issues as "owner/repo#123". Bare "#123" resolves against `default_repo`.
pub fn parse_refs(text: &str, default_repo: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |r: String| {
        if !out.iter().any(|x| x.eq_ignore_ascii_case(&r)) {
            out.push(r);
        }
    };
    for c in URL_REF.captures_iter(text) {
        push(format!("{}#{}", &c[1], &c[2]));
    }
    for c in FULL_REF.captures_iter(text) {
        push(format!("{}#{}", &c[1], &c[2]));
    }
    for c in SHORT_REF.captures_iter(text) {
        push(format!("{default_repo}#{}", &c[1]));
    }
    out
}

fn at_nine(date: chrono::NaiveDate) -> Ts {
    let local = Local
        .from_local_datetime(&date.and_time(NaiveTime::from_hms_opt(9, 0, 0).unwrap()))
        .earliest()
        .unwrap_or_else(Local::now);
    local.with_timezone(&Utc)
}

fn next_weekday(from: DateTime<Local>, day: Weekday) -> chrono::NaiveDate {
    let mut d = from.date_naive() + Duration::days(1);
    while d.weekday() != day {
        d += Duration::days(1);
    }
    d
}

static IN_N: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bin (\d+|a|an|one|two|three|a couple of) ?(hours?|hrs?|h|days?|d|weeks?)\b").unwrap());

/// A time phrase ("tomorrow", "next week", "on friday", "in 3 days"), relative to `now`.
pub fn parse_when(text: &str, now: Ts) -> Option<Ts> {
    let t = text.to_lowercase();
    let local = now.with_timezone(&Local);
    if let Some(c) = IN_N.captures(&t) {
        let n: i64 = match &c[1] {
            "a" | "an" | "one" => 1,
            "two" | "a couple of" => 2,
            "three" => 3,
            d => d.parse().ok()?,
        };
        let unit = &c[2];
        return Some(if unit.starts_with('h') {
            now + Duration::hours(n)
        } else if unit.starts_with('w') {
            at_nine(local.date_naive() + Duration::days(7 * n))
        } else {
            at_nine(local.date_naive() + Duration::days(n))
        });
    }
    if t.contains("tomorrow") {
        return Some(at_nine(local.date_naive() + Duration::days(1)));
    }
    if t.contains("next week") || t.contains("monday") {
        return Some(at_nine(next_weekday(local, Weekday::Mon)));
    }
    let days = [
        ("tuesday", Weekday::Tue),
        ("wednesday", Weekday::Wed),
        ("thursday", Weekday::Thu),
        ("friday", Weekday::Fri),
        ("end of the week", Weekday::Fri),
        ("end of week", Weekday::Fri),
    ];
    for (word, day) in days {
        if t.contains(word) {
            return Some(at_nine(next_weekday(local, day)));
        }
    }
    if t.contains("later today") || t.contains("this afternoon") {
        return Some(now + Duration::hours(3));
    }
    None
}

static TRIGGER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(come back to|coming back to|come back on|revisit|re-visit|circle back|follow[- ]up on|look at (this|it) again|take another look|have another look|pick (this|it) (back )?up|return to this|remind me|todo:? (for )?me|(let'?s|we('ll| will| should)|i'?ll) (wait|hold off|park (this|it)))\b",
    )
    .unwrap()
});
/// Looser than TRIGGER: comments that might be promising to come back later, worth asking the AI about
/// when the rules above don't match ("I'll look at this once Sam's PR is in", "will check tomorrow").
static MAYBE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(i'?ll|i will|i'm going to|let me|will (look|check|get|come|follow|circle|pick|do|review|test|try|fix|update|handle|deal)|later|tomorrow|tonight|next week|monday|tuesday|wednesday|thursday|friday|after (the|this|that|lunch|standup|[\w-]+#?\d+)|once|get back to|look into|on my list|todo|park)\b",
    )
    .unwrap()
});

/// Whether a comment the rules didn't catch might still be a "come back to this later" (for the AI).
pub fn maybe_intent(body: &str) -> bool {
    let prose: String = body.lines().filter(|l| !l.trim_start().starts_with('>')).collect::<Vec<_>>().join("\n");
    prose.chars().count() <= 2_000 && MAYBE.is_match(&prose)
}

static CONDITION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(after|once|until|when)\b\s+(.+)").unwrap());

/// "the API migration lands" from "... after the API migration lands".
pub fn condition_of(text: &str) -> Option<String> {
    CONDITION
        .captures(text)
        .map(|c| c[2].trim_end_matches(|ch: char| !ch.is_alphanumeric()).trim().to_string())
        .filter(|c| !c.is_empty())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Intent {
    /// The sentence that expressed the intent.
    pub note: String,
    pub until: Option<Ts>,
    /// A referenced PR/issue to wait for.
    pub waits_on: Option<String>,
    /// "the API migration" in "after the API migration", for matching against PR titles.
    pub condition: Option<String>,
}

/// Find "come back to this after X" style intent in one of the user's comments.
pub fn detect(body: &str, default_repo: &str, at: Ts) -> Option<Intent> {
    // Ignore quoted replies and code: they're someone else's words or not prose.
    let mut prose = String::new();
    let mut in_code = false;
    for line in body.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code || line.trim_start().starts_with('>') {
            continue;
        }
        prose.push_str(line);
        prose.push('\n');
    }
    let sentence = prose
        .split(['.', '!', '?', '\n'])
        .map(str::trim)
        .find(|s| TRIGGER.is_match(s))?;
    let mut note: String = sentence.chars().take(140).collect();
    if sentence.chars().count() > 140 {
        note.push('…');
    }
    let waits_on = parse_refs(sentence, default_repo).into_iter().next();
    let condition = condition_of(sentence);
    let until = parse_when(sentence, at);
    Some(Intent { note, until, waits_on, condition })
}

const STOP: &[&str] = &[
    "the", "this", "that", "these", "those", "then", "than", "with", "lands", "landed", "merges", "merged", "ships",
    "shipped", "done", "goes", "out", "is", "are", "has", "have", "been", "gets", "get", "in", "and", "for", "into",
];

/// Match a free-text condition ("the API migration lands") to exactly one open PR/issue by title.
/// `candidates` are (id, "owner/repo#n", title). Returns the "owner/repo#n" key.
pub fn match_condition(condition: &str, candidates: &[(String, String, String)], exclude_id: &str) -> Option<String> {
    let words: Vec<String> = condition
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3 && !STOP.contains(w))
        .map(String::from)
        .collect();
    if words.is_empty() {
        return None;
    }
    let hits: Vec<&(String, String, String)> = candidates
        .iter()
        .filter(|(id, _, _)| id != exclude_id)
        .filter(|(_, _, title)| {
            let t = title.to_lowercase();
            words.iter().all(|w| t.contains(w.as_str()))
        })
        .collect();
    (hits.len() == 1).then(|| hits[0].1.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Ts {
        // A Tuesday.
        DateTime::parse_from_rfc3339("2026-10-13T10:00:00Z").unwrap().with_timezone(&Utc)
    }

    #[test]
    fn refs() {
        let r = parse_refs("after #392 and acme/api#12, see https://github.com/acme/web/pull/7", "acme/ios");
        assert_eq!(r, vec!["acme/web#7", "acme/api#12", "acme/ios#392"]);
        assert!(parse_refs("colour #fff and issue#12", "a/b").is_empty());
    }

    #[test]
    fn when() {
        let n = now();
        assert!(parse_when("tomorrow", n).unwrap() > n);
        assert_eq!(parse_when("in 3 hours", n).unwrap(), n + Duration::hours(3));
        let fri = parse_when("by end of the week", n).unwrap().with_timezone(&Local);
        assert_eq!(fri.weekday(), Weekday::Fri);
        assert!(parse_when("whenever", n).is_none());
    }

    #[test]
    fn detects_intent() {
        let i = detect("Looks good overall. Let's come back to this after the API migration lands.", "acme/api", now()).unwrap();
        assert_eq!(i.condition.as_deref(), Some("the API migration lands"));
        assert!(i.waits_on.is_none());
        let i = detect("I'll take another look once #392 merges", "acme/api", now()).unwrap();
        assert_eq!(i.waits_on.as_deref(), Some("acme/api#392"));
        let i = detect("Can we revisit this next week?", "acme/api", now()).unwrap();
        assert!(i.until.is_some());
        assert!(detect("> let's come back to this later\n\nAgreed, ship it", "a/b", now()).is_none());
        assert!(detect("LGTM", "a/b", now()).is_none());
    }

    #[test]
    fn matches_titles() {
        let c = vec![
            ("1".into(), "acme/api#392".into(), "API migration: move orders to v2".into()),
            ("2".into(), "acme/api#400".into(), "Fix pagination".into()),
        ];
        assert_eq!(match_condition("the API migration lands", &c, "x"), Some("acme/api#392".into()));
        assert_eq!(match_condition("the database changes", &c, "x"), None);
    }
}
