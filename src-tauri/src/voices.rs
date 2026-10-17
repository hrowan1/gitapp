//! Remembering voices across meetings. When you name a speaker, their voice fingerprint (a unit
//! vector from speaker separation) is kept under that name, on this Mac; in later meetings a voice
//! that clearly matches a known one is named automatically ("recognised"). Nothing is learnt until
//! you name someone, and any voice can be forgotten in Settings.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::HashMap;

/// A voice must be at least this similar (cosine) to a known one to be named automatically…
pub const MATCH: f32 = 0.62;
/// …and clearly closer to it than to anyone else. Measured: the same person scores 0.83–0.93 across
/// meetings, different people at most 0.37.
pub const MARGIN: f32 = 0.08;

pub fn encode(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub fn decode(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

fn similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return -1.0;
    }
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn normalise(mut v: Vec<f32>) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
    v
}

/// Store a meeting's voice fingerprints (voice "1", "2"… → fingerprint), replacing earlier ones.
pub fn save_meeting_voices(conn: &Connection, meeting_id: &str, voices: &HashMap<String, Vec<f32>>) -> Result<()> {
    conn.execute("DELETE FROM meeting_voices WHERE meeting_id = ?1", [meeting_id])?;
    for (voice, embedding) in voices {
        conn.execute(
            "INSERT INTO meeting_voices (meeting_id, voice, embedding) VALUES (?1, ?2, ?3)",
            params![meeting_id, voice, encode(embedding)],
        )?;
    }
    Ok(())
}

/// Name the meeting's voices that clearly match someone you've named before. Leaves names you set
/// yourself alone, and uses each known name at most once per meeting.
pub fn recognise(conn: &Connection, meeting_id: &str) -> Result<()> {
    conn.execute("DELETE FROM meeting_speakers WHERE meeting_id = ?1 AND recognised", [meeting_id])?;
    let named_by_you: Vec<String> = conn
        .prepare("SELECT voice FROM meeting_speakers WHERE meeting_id = ?1")?
        .query_map([meeting_id], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let voices: Vec<(String, Vec<f32>)> = conn
        .prepare("SELECT voice, embedding FROM meeting_voices WHERE meeting_id = ?1")?
        .query_map([meeting_id], |r| Ok((r.get(0)?, decode(&r.get::<_, Vec<u8>>(1)?))))?
        .collect::<Result<_, _>>()?;
    let known: Vec<(String, Vec<f32>)> = conn
        .prepare("SELECT name, embedding FROM known_voices")?
        .query_map([], |r| Ok((r.get(0)?, decode(&r.get::<_, Vec<u8>>(1)?))))?
        .collect::<Result<_, _>>()?;
    let mut taken: Vec<String> = conn
        .prepare("SELECT name FROM meeting_speakers WHERE meeting_id = ?1")?
        .query_map([meeting_id], |r| r.get(0))?
        .collect::<Result<_, _>>()?;

    // Confident candidates, best first.
    let mut candidates: Vec<(f32, String, String)> = Vec::new();
    for (voice, fingerprint) in voices.iter().filter(|(v, _)| !named_by_you.contains(v)) {
        let mut scores: Vec<(f32, &str)> = known.iter().map(|(name, k)| (similarity(fingerprint, k), name.as_str())).collect();
        scores.sort_by(|a, b| b.0.total_cmp(&a.0));
        if let Some(&(best, name)) = scores.first() {
            let runner_up = scores.get(1).map_or(-1.0, |s| s.0);
            if best >= MATCH && best - runner_up >= MARGIN {
                candidates.push((best, voice.clone(), name.to_string()));
            }
        }
    }
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut done: Vec<String> = Vec::new();
    for (_, voice, name) in candidates {
        if done.contains(&voice) || taken.iter().any(|t| t.eq_ignore_ascii_case(&name)) {
            continue;
        }
        conn.execute(
            "INSERT OR REPLACE INTO meeting_speakers (meeting_id, voice, name, recognised) VALUES (?1, ?2, ?3, 1)",
            params![meeting_id, voice, name],
        )?;
        done.push(voice);
        taken.push(name);
    }
    Ok(())
}

/// You named a voice: remember (or refine) what that person sounds like.
pub fn learn(conn: &Connection, meeting_id: &str, voice: &str, name: &str, now: &str) -> Result<()> {
    let Some(fingerprint) = conn
        .query_row(
            "SELECT embedding FROM meeting_voices WHERE meeting_id = ?1 AND voice = ?2",
            params![meeting_id, voice],
            |r| r.get::<_, Vec<u8>>(0),
        )
        .optional()?
        .map(|b| decode(&b))
    else {
        return Ok(());
    };
    let existing = conn
        .query_row("SELECT id, embedding, samples FROM known_voices WHERE name = ?1 COLLATE NOCASE", [name], |r| {
            Ok((r.get::<_, i64>(0)?, decode(&r.get::<_, Vec<u8>>(1)?), r.get::<_, i64>(2)?))
        })
        .optional()?;
    match existing {
        // A running average over every meeting you've named them in.
        Some((id, old, samples)) if old.len() == fingerprint.len() => {
            let n = samples as f32;
            let merged = normalise(old.iter().zip(&fingerprint).map(|(o, f)| (o * n + f) / (n + 1.0)).collect());
            conn.execute(
                "UPDATE known_voices SET embedding = ?2, samples = samples + 1, updated_at = ?3 WHERE id = ?1",
                params![id, encode(&merged), now],
            )?;
        }
        _ => {
            conn.execute(
                "INSERT INTO known_voices (name, embedding, samples, updated_at) VALUES (?1, ?2, 1, ?3)
                 ON CONFLICT(name) DO UPDATE SET embedding = excluded.embedding, samples = 1, updated_at = excluded.updated_at",
                params![name.trim(), encode(&fingerprint), now],
            )?;
        }
    }
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownVoice {
    pub id: i64,
    pub name: String,
    /// How many meetings you've named them in.
    pub samples: i64,
    pub updated_at: String,
}

pub fn known(conn: &Connection) -> Result<Vec<KnownVoice>> {
    let mut stmt = conn.prepare("SELECT id, name, samples, updated_at FROM known_voices ORDER BY name COLLATE NOCASE")?;
    let rows = stmt
        .query_map([], |r| Ok(KnownVoice { id: r.get(0)?, name: r.get(1)?, samples: r.get(2)?, updated_at: r.get(3)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Forget a voice. Meetings keep the names already shown; it just won't be recognised again.
pub fn forget(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM known_voices WHERE id = ?1", [id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(seed: &[f32]) -> Vec<f32> {
        normalise(seed.to_vec())
    }

    #[test]
    fn named_voices_are_recognised_later() {
        let path = std::env::temp_dir().join(format!("dev-inbox-voices-{}.sqlite3", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let conn = crate::db::open(&path).unwrap();
        let sam = unit(&[1.0, 0.1, 0.0, 0.0]);
        let priya = unit(&[0.0, 1.0, 0.1, 0.0]);
        let stranger = unit(&[0.0, 0.0, 0.2, 1.0]);

        // Meeting 1: you name both.
        let voices = HashMap::from([("1".to_string(), sam.clone()), ("2".to_string(), priya.clone())]);
        save_meeting_voices(&conn, "m1", &voices).unwrap();
        learn(&conn, "m1", "1", "Sam", "t1").unwrap();
        learn(&conn, "m1", "2", "Priya", "t1").unwrap();
        assert_eq!(known(&conn).unwrap().len(), 2);

        // Meeting 2: Sam (slightly different) and a stranger.
        let sam_again = unit(&[0.95, 0.15, 0.05, 0.0]);
        let voices = HashMap::from([("1".to_string(), stranger), ("2".to_string(), sam_again.clone())]);
        save_meeting_voices(&conn, "m2", &voices).unwrap();
        recognise(&conn, "m2").unwrap();
        let names = crate::meetings::speaker_names(&conn, "m2").unwrap();
        assert_eq!(names.get("2").map(String::as_str), Some("Sam"));
        assert_eq!(names.get("1"), None, "an unknown voice isn't guessed");

        // Your own name for a voice always wins, and learning refines the stored voice.
        crate::meetings::name_speaker(&conn, "m2", "2", "Samuel", false).unwrap();
        recognise(&conn, "m2").unwrap();
        assert_eq!(crate::meetings::speaker_names(&conn, "m2").unwrap()["2"], "Samuel");
        learn(&conn, "m2", "2", "Sam", "t2").unwrap();
        assert_eq!(known(&conn).unwrap().iter().find(|k| k.name == "Sam").unwrap().samples, 2);

        let sam_id = known(&conn).unwrap().iter().find(|k| k.name == "Sam").unwrap().id;
        forget(&conn, sam_id).unwrap();
        conn.execute("DELETE FROM meeting_speakers WHERE meeting_id = 'm2'", []).unwrap();
        recognise(&conn, "m2").unwrap();
        assert!(crate::meetings::speaker_names(&conn, "m2").unwrap().is_empty(), "forgotten voices aren't recognised");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn blobs_round_trip() {
        let v = vec![0.25f32, -1.5, 3.0];
        assert_eq!(decode(&encode(&v)), v);
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// Real helper on three recordings: `VOICES_DIR=… MODELS=… cargo test voices_across_meetings -- --ignored --nocapture`
    /// (system.m4a: Daniel + Karen; b.m4a: Daniel + Fred; c.m4a: Karen + Fred).
    #[test]
    #[ignore]
    fn voices_across_meetings() {
        let dir = std::path::PathBuf::from(std::env::var("VOICES_DIR").expect("VOICES_DIR"));
        let models = std::path::PathBuf::from(std::env::var("MODELS").expect("MODELS"));
        let path = std::env::temp_dir().join(format!("dev-inbox-voices-live-{}.sqlite3", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let conn = crate::db::open(&path).unwrap();
        let minutes = std::time::Duration::from_secs(600);
        // Returns voice number → who it really is (by first turn order, which we know for these files).
        let load = |id: &str, file: &str| {
            let found = crate::meetings::diarize_file(&dir.join(file), &models, minutes).unwrap();
            let numbering = crate::meetings::assign_voices(&mut [], &found.turns);
            let voices: HashMap<String, Vec<f32>> =
                found.voices.into_iter().filter_map(|(s, v)| Some((numbering.get(&s)?.clone(), v))).collect();
            save_meeting_voices(&conn, id, &voices).unwrap();
            recognise(&conn, id).unwrap();
            crate::meetings::speaker_names(&conn, id).unwrap()
        };
        let a = load("a", "system.m4a");
        assert!(a.is_empty(), "nobody is known yet");
        crate::meetings::name_speaker(&conn, "a", "1", "Daniel", true).unwrap();
        crate::meetings::name_speaker(&conn, "a", "2", "Karen", true).unwrap();

        let b = load("b", "b.m4a");
        println!("meeting B: {b:?}");
        assert_eq!(b.get("1").map(String::as_str), Some("Daniel"));
        assert_eq!(b.get("2"), None, "Fred is new");
        crate::meetings::name_speaker(&conn, "b", "2", "Fred", true).unwrap();

        let c = load("c", "c.m4a");
        println!("meeting C: {c:?}");
        assert_eq!(c.get("1").map(String::as_str), Some("Karen"));
        assert_eq!(c.get("2").map(String::as_str), Some("Fred"));
        let _ = std::fs::remove_file(&path);
    }
}
