//! Explaining a failed CI check: the failing step, the check's annotations and the interesting part of
//! its log (error lines with what led up to them, plus the end), handed to the AI to say what broke,
//! where, and the likely fix.

use regex::Regex;
use std::sync::LazyLock;

/// The useful part of a CI log within `max` characters: every error with the lines before it, then the
/// end of the log. Timestamps and colour codes are stripped.
pub fn excerpt(log: &str, max: usize) -> String {
    static STAMP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{4}-\d\d-\d\dT[\d:.]+Z ").unwrap());
    static ANSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*[A-Za-z]").unwrap());
    static ERROR: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)##\[error\]|\berror\b[:\[]|\bfailed\b|\bfailure\b|panicked at|assert|exception|traceback|✕|✗").unwrap()
    });
    let lines: Vec<String> = log
        .lines()
        .map(|l| ANSI.replace_all(&STAMP.replace(l, ""), "").trim_end().to_string())
        .filter(|l| !l.trim().is_empty() && !l.starts_with("##[group]") && !l.starts_with("##[endgroup]"))
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    // Error windows: 12 lines before each error line, 3 after.
    let mut keep = vec![false; lines.len()];
    for (i, l) in lines.iter().enumerate() {
        if ERROR.is_match(l) {
            for k in keep.iter_mut().take((i + 4).min(lines.len())).skip(i.saturating_sub(12)) {
                *k = true;
            }
        }
    }
    // …and the last 30 lines, where most runners report the failure.
    for k in keep.iter_mut().skip(lines.len().saturating_sub(30)) {
        *k = true;
    }
    let mut out: Vec<String> = Vec::new();
    let mut gap = false;
    for (l, k) in lines.iter().zip(&keep) {
        if *k {
            if gap && !out.is_empty() {
                out.push("…".into());
            }
            out.push(l.chars().take(400).collect());
            gap = false;
        } else {
            gap = true;
        }
    }
    let text = out.join("\n");
    if text.chars().count() <= max {
        return text;
    }
    // Too long: keep the end, which has the final errors.
    let chars: Vec<char> = text.chars().collect();
    format!("…\n{}", chars[chars.len() - max.saturating_sub(4)..].iter().collect::<String>())
}

pub const INSTRUCTIONS: &str = "You explain why a CI check failed, to the developer whose pull request it is, from the \
check details and log excerpt given. Say what broke (which test, lint rule or build step), where (file and line when \
shown), the likely cause, and a suggested fix — 2 to 5 short bullet points starting with \"- \". If several checks \
failed, give each its own short heading line. Quote exact error text sparingly. Only use what's in the logs; if the \
cause isn't clear from them, say so and what to look at.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_errors_and_the_end() {
        let mut log = String::new();
        for i in 0..200 {
            log.push_str(&format!("2026-10-16T10:00:{:02}.1234567Z step output line {i}\n", i % 60));
        }
        log.push_str("2026-10-16T10:01:00.0000000Z \x1b[31m##[error]src/billing.rs:42: expected 1099, got 1100\x1b[0m\n");
        for i in 0..100 {
            log.push_str(&format!("2026-10-16T10:02:00.0000000Z cleanup {i}\n"));
        }
        log.push_str("2026-10-16T10:03:00.0000000Z ##[error]Process completed with exit code 1.\n");
        let e = excerpt(&log, 10_000);
        assert!(e.contains("##[error]src/billing.rs:42: expected 1099, got 1100"), "{e}");
        assert!(e.contains("step output line 199"), "context before the error");
        assert!(!e.contains("step output line 100"), "not the whole log");
        assert!(e.ends_with("##[error]Process completed with exit code 1."));
        assert!(!e.contains("2026-10-16T") && !e.contains('\x1b'), "timestamps and colours stripped");
        assert!(excerpt(&log, 300).chars().count() <= 300);
        assert_eq!(excerpt("", 100), "");
    }
}

#[cfg(test)]
mod live_tests {
    /// Read-only, against a real PR: `GITHUB_TOKEN=$(gh auth token) PR_ID=… cargo test failed_checks_live -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn failed_checks_live() {
        let token = std::env::var("GITHUB_TOKEN").expect("GITHUB_TOKEN");
        let pr = std::env::var("PR_ID").expect("PR_ID");
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let client = crate::github::Client::new(&token);
            let failed = client.failed_checks(&pr).await.unwrap();
            assert!(!failed.is_empty(), "the PR has failing checks");
            for c in failed.iter().take(2) {
                println!("== {} ({}) step={:?} job={:?} annotations={}", c.name, c.app, c.failed_step, c.job_id, c.annotations.len());
                for a in c.annotations.iter().take(3) {
                    println!("   {a}");
                }
                if let Some(job) = c.job_id {
                    let log = client.job_log(&c.repo, job).await.unwrap();
                    let e = super::excerpt(&log, 2_500);
                    println!("-- log {} bytes -> excerpt {} chars:\n{e}", log.len(), e.chars().count());
                }
            }
        });
    }
}
