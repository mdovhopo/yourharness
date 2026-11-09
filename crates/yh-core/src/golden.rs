//! Golden tests of what a thread makes of its `claude`'s events: the rows it saves and the
//! state it's left in, written as text and compared with `fixtures/expected/<name>.txt`.
//! The app's thread and the engine's reducer run the same sessions through this, so moving
//! the reducer can't change what's saved. `UPDATE_GOLDEN=1 cargo test` writes the files anew.

use crate::claude::{Block, BlockKind, DeltaKind, Event, SubagentUsage, ToolResult};
use crate::db::Row;
use std::fmt::Write;

/// The recorded sessions in `fixtures/`, by name.
pub const SESSIONS: &[(&str, &str)] = &[
    ("plain-tools", include_str!("../fixtures/plain-tools.jsonl")),
    ("background-jobs", include_str!("../fixtures/background-jobs.jsonl")),
    ("background-job-monitor", include_str!("../fixtures/background-job-monitor.jsonl")),
    ("subagent-foreground", include_str!("../fixtures/subagent-foreground.jsonl")),
    ("subagent-background", include_str!("../fixtures/subagent-background.jsonl")),
    ("subagent-background-commands", include_str!("../fixtures/subagent-background-commands.jsonl")),
    ("subagent-continued", include_str!("../fixtures/subagent-continued.jsonl")),
];

/// A session's events as the app takes them: consecutive deltas of a block joined, as one
/// frame's batch joins them.
pub fn session_events(text: &str) -> Vec<Event> {
    let mut out: Vec<Event> = vec![];
    for e in crate::claude::parse_lines(text) {
        if !out.last_mut().is_some_and(|last| last.merge(&e)) {
            out.push(e);
        }
    }
    out
}

/// One thing that happens to a thread: an event from its `claude`, or something the app does.
#[derive(Clone, Debug)]
pub enum Step {
    Event(Event),
    /// Your message: sent (a turn starts), or queued while Claude works.
    User { id: &'static str, text: &'static str, queued: bool },
    /// Stop pressed.
    Interrupt,
    /// The files git saw the turn started by this message change.
    ChangedFiles { message: &'static str, files: Vec<&'static str> },
}

/// What the recorded sessions don't have: permission prompts, MCP questions, a queued message
/// picked up and one taken back, compacting, `/compact`'s repeated outputs, Stop, a cleared
/// conversation, a resume that fails and an exit mid-turn.
pub fn synthetic() -> Vec<Step> {
    let ev = Step::Event;
    let tool = |id: &str, name: &str, input: &str| Block::Tool { id: id.into(), name: name.into(), input: input.into() };
    let result = |id: &str, text: &str, is_error: bool| ToolResult { id: id.into(), text: text.into(), len: text.chars().count(), is_error, full: None };
    vec![
        ev(Event::Session { id: "s1".into() }),
        Step::User { id: "u1", text: "Fix the bug", queued: false },
        ev(Event::BlockStart { message: "m1".into(), index: 0, parent: None, kind: BlockKind::Thinking }),
        ev(Event::Delta { message: "m1".into(), index: 0, parent: None, kind: DeltaKind::Thinking, text: "Looking".into() }),
        ev(Event::Delta { message: "m1".into(), index: 0, parent: None, kind: DeltaKind::Thinking, text: " around".into() }),
        ev(Event::BlockStart { message: "m1".into(), index: 1, parent: None, kind: BlockKind::Tool { id: "t1".into(), name: "Bash".into() } }),
        ev(Event::Delta { message: "m1".into(), index: 1, parent: None, kind: DeltaKind::Input, text: "{\"command\":".into() }),
        ev(Event::Delta { message: "m1".into(), index: 1, parent: None, kind: DeltaKind::Input, text: "\"rm -rf build\"}".into() }),
        ev(Event::Assistant { message: "m1".into(), parent: None, blocks: vec![Block::Thinking("Looking around".into()), tool("t1", "Bash", "{\"command\":\"rm -rf build\"}")] }),
        ev(Event::Permission { request_id: "r1".into(), tool: "Bash".into(), description: "rm -rf build".into(), input: "{\"command\":\"rm -rf build\"}".into() }),
        ev(Event::Permission { request_id: "r2".into(), tool: "Write".into(), description: "a.txt".into(), input: "{}".into() }),
        ev(Event::Permission { request_id: "r1".into(), tool: "Bash".into(), description: "rm -rf build (again)".into(), input: "{}".into() }),
        ev(Event::Elicitation { request_id: "e1".into(), server: "linear".into(), message: "Pick a team".into(), mode: "form".into(), schema: serde_json::json!({}), url: None, elicitation_id: None }),
        ev(Event::Status("compacting".into())),
        ev(Event::Status("".into())),
        Step::User { id: "q1", text: "Also the tests", queued: true },
        Step::User { id: "q2", text: "Never mind", queued: true },
        ev(Event::Lifecycle { id: "q1".into(), state: "started".into() }),
        ev(Event::Lifecycle { id: "q2".into(), state: "cancelled".into() }),
        ev(Event::ToolResults { parent: None, results: vec![result("t1", "removed", false)] }),
        ev(Event::BlockStart { message: "m2".into(), index: 0, parent: None, kind: BlockKind::Text }),
        ev(Event::Delta { message: "m2".into(), index: 0, parent: None, kind: DeltaKind::Text, text: "Done: ".into() }),
        ev(Event::Delta { message: "m2".into(), index: 0, parent: None, kind: DeltaKind::Text, text: "the build is gone.".into() }),
        ev(Event::Assistant { message: "m2".into(), parent: None, blocks: vec![Block::Text("Done: the build is gone.".into())] }),
        ev(Event::Assistant { message: "m2".into(), parent: None, blocks: vec![Block::Text("Done: the build is gone.".into()), Block::Text("And a second block.".into())] }),
        ev(Event::ContextUsage(1234)),
        ev(Event::ContextWindows(vec![("haiku".into(), 200_000), ("opus".into(), 1_000_000)])),
        ev(Event::McpServers(vec![("linear".into(), "needs-auth".into(), "user".into())])),
        ev(Event::Done { is_error: false, text: "".into() }),
        Step::ChangedFiles { message: "u1", files: vec!["src/a.rs", "build/"] },
        ev(Event::CommandOutput { text: "Compacted.".into(), id: Some("c1".into()) }),
        ev(Event::CommandOutput { text: "Compacted.".into(), id: Some("c1".into()) }),
        ev(Event::CommandOutput { text: "No id.".into(), id: None }),
        Step::User { id: "u2", text: "Run it", queued: false },
        ev(Event::Assistant { message: "m3".into(), parent: None, blocks: vec![tool("t2", "Bash", "{\"command\":\"make\"}")] }),
        ev(Event::ToolResults { parent: None, results: vec![result("t2", "make: *** [all] Error 2", true)] }),
        Step::Interrupt,
        ev(Event::Done { is_error: true, text: "error_during_execution".into() }),
        Step::User { id: "u3", text: "Try again", queued: false },
        ev(Event::Done { is_error: true, text: "error_during_execution".into() }),
        Step::User { id: "u4", text: "Once more", queued: false },
        ev(Event::Done { is_error: true, text: "Overloaded".into() }),
        ev(Event::ConversationReset),
        ev(Event::Notice("A notice from the CLI".into())),
        Step::User { id: "u5", text: "Go on", queued: false },
        Step::User { id: "q3", text: "Waiting", queued: true },
        ev(Event::Assistant { message: "m4".into(), parent: None, blocks: vec![tool("t3", "Agent", "{\"description\":\"Look\",\"prompt\":\"look\"}")] }),
        ev(Event::SubagentStarted { tool_id: "t3".into(), description: "Look".into(), task_id: Some("k3".into()), background: false }),
        ev(Event::Assistant { message: "sm1".into(), parent: Some("t3".into()), blocks: vec![Block::Text("Reading".into()), tool("st1", "Read", "{}")] }),
        ev(Event::ToolResults { parent: Some("t3".into()), results: vec![result("st1", "file", false)] }),
        ev(Event::SubagentProgress { tool_id: "t3".into(), activity: Some("Reading a.txt".into()), usage: SubagentUsage { tokens: Some(10), tool_uses: Some(1), duration_ms: None } }),
        ev(Event::Exit { code: 1, tail: "line one\n\nline two\nline three\nline four\n".into() }),
        Step::User { id: "u6", text: "After the crash", queued: false },
        ev(Event::Exit { code: 2, tail: "".into() }),
    ]
}

/// Rows as saved, in order, then whatever else the caller says of the thread's state.
pub fn describe(rows: &[(usize, Row)], state: &[String]) -> String {
    let mut sorted: Vec<&(usize, Row)> = rows.iter().collect();
    sorted.sort_by_key(|(seq, _)| *seq);
    let mut out = String::new();
    for (seq, r) in sorted {
        let _ = writeln!(out, "#{seq} {} {}", r.kind, stable_id(&r.id));
        if !r.text.is_empty() {
            let _ = writeln!(out, "  text {:?}", r.text);
        }
        if let Some(name) = &r.tool_name {
            let _ = writeln!(out, "  tool {name} {:?}", r.tool_input.as_deref().unwrap_or(""));
        }
        if let Some(result) = &r.tool_result {
            let _ = writeln!(out, "  result {:?} len={:?} error={:?}", result, r.tool_result_length, r.tool_is_error);
        }
        for (name, value) in [("notice", &r.notice_style), ("attachments", &r.attachments), ("delivery", &r.delivery), ("changed", &r.changed_files)] {
            if let Some(v) = value {
                let _ = writeln!(out, "  {name} {v}");
            }
        }
        if r.turn_ended_at.is_some() {
            let _ = writeln!(out, "  turn ended");
        }
        if let Some(sub) = r.subagent.as_deref().and_then(crate::subagent_record::SubagentRecord::decode) {
            let _ = writeln!(
                out,
                "  sub {:?} {} uses={:?} tokens={:?} ms={:?} report={:?} background={} type={:?}",
                sub.description, sub.status, sub.tool_uses, sub.tokens, sub.duration_ms, sub.report, sub.is_background, sub.agent_type
            );
            for e in &sub.entries {
                let _ = writeln!(out, "    {} {} {:?} {:?} {:?} {:?}", e.kind, e.id, e.text, e.tool_name, e.tool_result, e.tool_is_error);
            }
        }
    }
    for line in state {
        let _ = writeln!(out, "= {line}");
    }
    out
}

/// Ids made up as rows are (notices) differ every run.
fn stable_id(id: &str) -> &str {
    let uuid = id.len() == 36 && id.chars().filter(|c| *c == '-').count() == 4;
    if uuid { "<uuid>" } else { id }
}

/// Compares with `fixtures/expected/<name>.txt`, or writes it with `UPDATE_GOLDEN=1`.
pub fn check(name: &str, actual: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/expected").join(format!("{name}.txt"));
    if std::env::var("UPDATE_GOLDEN").as_deref() == Ok("1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("no {}: run with UPDATE_GOLDEN=1", path.display()));
    if expected != actual {
        let first = expected.lines().zip(actual.lines()).position(|(a, b)| a != b).unwrap_or(expected.lines().count().min(actual.lines().count()));
        panic!(
            "{name} differs from {} at line {}:\n  expected: {:?}\n  actual:   {:?}\n--- actual ---\n{actual}",
            path.display(),
            first + 1,
            expected.lines().nth(first),
            actual.lines().nth(first)
        );
    }
}
