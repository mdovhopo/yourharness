//! A turn cut off by a quit or crash (`turnInProgress` still set at launch): what Claude did
//! until then is in its session file, which the CLI writes as it goes. Lined up with the saved
//! rows, the rest is filled in and saved, with a note saying so. The port of the Swift app's
//! `SessionBackfill` and the web rewrite's `Recovery`.

use crate::db::{Database, Row};
use crate::history;
use crate::library::{EntryData, Kind};
use std::collections::HashMap;

/// What a session's own history has that a saved transcript lacks.
#[derive(Debug, Default)]
pub struct Backfill {
    /// Rows the transcript has only the start of, by index: a reply cut off mid-stream, a tool
    /// call with no result yet.
    pub completed: HashMap<usize, EntryData>,
    /// What came after the transcript's last message, in order.
    pub missing: Vec<EntryData>,
}

impl Backfill {
    pub fn is_empty(&self) -> bool {
        self.completed.is_empty() && self.missing.is_empty()
    }

    /// Lines `known` (the transcript, oldest first) up with `session`. It anchors on the newest
    /// tool call or message you sent that both have, then walks forward; where they disagree
    /// it returns nothing rather than guess.
    pub fn new(known: &[EntryData], session: &[EntryData]) -> Backfill {
        let ours: Vec<usize> = (0..known.len()).filter(|&i| comparable(&known[i])).collect();
        let theirs: Vec<usize> = (0..session.len()).filter(|&i| comparable(&session[i])).collect();
        let (mut tool_at, mut user_at) = (HashMap::new(), HashMap::new());
        for (position, &i) in theirs.iter().enumerate() {
            match session[i].kind {
                Kind::Tool => {
                    tool_at.insert(session[i].id.as_str(), position);
                }
                Kind::User => {
                    user_at.insert(session[i].text.as_str(), position);
                }
                _ => {}
            }
        }
        let mut anchor = None;
        for position in (0..ours.len()).rev() {
            let e = &known[ours[position]];
            anchor = match e.kind {
                Kind::Tool => tool_at.get(e.id.as_str()).map(|&t| (position, t)),
                Kind::User => user_at.get(e.text.as_str()).map(|&t| (position, t)),
                _ => continue,
            };
            if anchor.is_some() {
                break;
            }
        }
        let Some((mut mine, mut theirs_at)) = anchor else { return Backfill::default() };
        let mut result = Backfill::default();
        while mine < ours.len() {
            if theirs_at >= theirs.len() {
                return Backfill::default();
            }
            let (a, b) = (&known[ours[mine]], &session[theirs[theirs_at]]);
            match compare(a, b) {
                Comparison::Same => {}
                Comparison::Completes => {
                    result.completed.insert(ours[mine], b.clone());
                }
                Comparison::Different => return Backfill::default(),
            }
            mine += 1;
            theirs_at += 1;
        }
        let last = theirs[theirs_at - 1];
        result.missing = session[last + 1..].to_vec();
        result
    }
}

/// Replies, tool calls and what you typed; not notices or thinking, which each side records its
/// own way, nor slash commands, which the CLI keeps as notices.
fn comparable(e: &EntryData) -> bool {
    match e.kind {
        Kind::User => !e.text.starts_with('/'),
        Kind::Assistant | Kind::Tool => true,
        _ => false,
    }
}

enum Comparison {
    Same,
    Completes,
    Different,
}

fn compare(mine: &EntryData, theirs: &EntryData) -> Comparison {
    match (mine.kind, theirs.kind) {
        (Kind::User, Kind::User) if mine.text == theirs.text => Comparison::Same,
        (Kind::Assistant, Kind::Assistant) => {
            let a = mine.text.trim();
            if a == theirs.text {
                Comparison::Same
            } else if theirs.text.starts_with(a) {
                Comparison::Completes
            } else {
                Comparison::Different
            }
        }
        (Kind::Tool, Kind::Tool) if mine.id == theirs.id => {
            if mine.tool_result.is_none() && theirs.tool_result.is_some() {
                Comparison::Completes
            } else {
                Comparison::Same
            }
        }
        _ => Comparison::Different,
    }
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::User => "user",
        Kind::Assistant => "assistant",
        Kind::Thinking => "thinking",
        Kind::Tool => "tool",
        Kind::Notice => "notice",
        Kind::Command => "commandOutput",
    }
}

pub fn row_of(e: &EntryData, id: String, date: Option<f64>) -> Row {
    Row {
        id,
        kind: kind_name(e.kind).into(),
        text: e.text.clone(),
        tool_name: e.tool_name.clone(),
        tool_input: e.tool_input.clone(),
        tool_result: e.tool_result.clone(),
        tool_is_error: (e.kind == Kind::Tool).then_some(e.is_error),
        date,
        ..Default::default()
    }
}

/// Fills in a cut-off turn from its session file and saves it, with a note at the end. Off the
/// main thread: it reads the whole transcript. Returns how many rows the transcript has after.
pub fn recover(db: &Database, thread: &str, session_id: Option<&str>) -> usize {
    let now = crate::library::now_ms() / 1000.0;
    let total = db.transcript_count(thread);
    let rows = db.read_rows(thread, 0, total);
    let known: Vec<EntryData> = rows.iter().cloned().filter_map(EntryData::from_row).collect();
    let session = session_id
        .and_then(|s| history::locate(&history::config_directory(), s))
        .map(|p| history::load_with_limit(&p, usize::MAX))
        .unwrap_or_default();
    // Rows of a kind this app doesn't know would shift the indexes: only line up when none.
    let backfill = if known.len() == rows.len() { Backfill::new(&known, &session) } else { Backfill::default() };
    let mut changes: Vec<(usize, Row)> = vec![];
    for (&index, e) in &backfill.completed {
        changes.push((index, row_of(e, rows[index].id.clone(), rows[index].date)));
    }
    let mut count = rows.len();
    for e in &backfill.missing {
        changes.push((count, row_of(e, if e.kind == Kind::Tool { e.id.clone() } else { format!("r{count}") }, Some(now))));
        count += 1;
    }
    let note = if backfill.is_empty() {
        "YourHarness closed while Claude was working, so this turn stopped here."
    } else {
        "YourHarness closed while Claude was working. What Claude did until then was recovered from its session history."
    };
    let mut notice = Row { id: format!("r{count}"), kind: "notice".into(), text: note.into(), date: Some(now), ..Default::default() };
    notice.notice_style = Some("info".into());
    changes.push((count, notice));
    count += 1;
    db.save_rows(thread, count, changes);
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(t: &str) -> EntryData {
        EntryData::new("u", Kind::User, t)
    }
    fn reply(t: &str) -> EntryData {
        EntryData::new("a", Kind::Assistant, t)
    }
    fn tool(id: &str, result: Option<&str>) -> EntryData {
        let mut e = EntryData::new(id, Kind::Tool, "");
        e.tool_name = Some("Bash".into());
        e.tool_result = result.map(str::to_string);
        e
    }

    #[test]
    fn fills_in_a_tool_call_still_running_and_what_came_after() {
        let known = vec![user("go"), tool("t1", Some("ok")), reply("Half a sen"), tool("t2", None)];
        let session = vec![user("go"), tool("t1", Some("ok")), reply("Half a sentence."), tool("t2", Some("done")), reply("All done.")];
        let b = Backfill::new(&known, &session);
        // Anchored on the newest tool call both have: what's before it isn't revisited.
        assert_eq!(b.completed.len(), 1);
        assert_eq!(b.completed[&3].tool_result.as_deref(), Some("done"));
        assert_eq!(b.missing.len(), 1);
        assert_eq!(b.missing[0].text, "All done.");
    }

    #[test]
    fn fills_in_a_reply_cut_off_mid_stream() {
        let known = vec![user("go"), tool("t1", Some("ok")), reply("Half a sen")];
        let session = vec![user("go"), tool("t1", Some("ok")), reply("Half a sentence."), reply("More.")];
        let b = Backfill::new(&known, &session);
        assert_eq!(b.completed[&2].text, "Half a sentence.");
        assert_eq!(b.missing.len(), 1);
    }

    #[test]
    fn disagreement_returns_nothing() {
        let known = vec![user("go"), reply("Something else")];
        let session = vec![user("go"), reply("Not that")];
        assert!(Backfill::new(&known, &session).is_empty());
    }

    #[test]
    fn no_anchor_returns_nothing() {
        assert!(Backfill::new(&[user("a")], &[user("b"), reply("x")]).is_empty());
    }

    #[test]
    fn recovers_into_the_database() {
        let dir = std::env::temp_dir().join(format!("yh-recover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let config = dir.join("claude");
        std::fs::create_dir_all(config.join("projects/p")).unwrap();
        let lines = [
            r#"{"type":"user","uuid":"1","parentUuid":null,"message":{"role":"user","content":"go"}}"#,
            r#"{"type":"assistant","uuid":"2","parentUuid":"1","message":{"id":"m","content":[{"type":"text","text":"Working on it."}]}}"#,
            r#"{"type":"assistant","uuid":"3","parentUuid":"2","message":{"id":"m2","content":[{"type":"text","text":"Done."}]}}"#,
        ];
        std::fs::write(config.join("projects/p/S1.jsonl"), lines.join("\n")).unwrap();
        // SAFETY: tests in this module don't run threads that read the environment.
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", &config) };
        let db = Database::open(&dir.join("data")).unwrap();
        db.save_rows("T", 2, vec![
            (0, Row { id: "u1".into(), kind: "user".into(), text: "go".into(), ..Default::default() }),
            (1, Row { id: "a1".into(), kind: "assistant".into(), text: "Working".into(), ..Default::default() }),
        ]);
        let count = recover(&db, "T", Some("S1"));
        assert_eq!(count, 4);
        let rows = db.read_rows("T", 0, 10);
        assert_eq!(rows[1].text, "Working on it.");
        assert_eq!(rows[1].id, "a1");
        assert_eq!(rows[2].text, "Done.");
        assert_eq!(rows[3].kind, "notice");
        unsafe { std::env::remove_var("CLAUDE_CONFIG_DIR") };
    }
}
