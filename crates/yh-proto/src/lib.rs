//! The messages YourHarness and its clients trade (the phone app, the `watch` example): JSON
//! text frames tagged with `type`. The phone's development server spoke the first version
//! (docs/mobile.md, "The wire"); this one adds what the engine knows live (a thread working, what
//! waits on you, the queue) and what a client may ask (send, stop, allow or deny, withdraw, send
//! now). Additions only, so a client of either version reads the other. Unknown messages and
//! fields are ignored on both sides.

use serde::{Deserialize, Serialize};

/// 2: `live`, `done`, the actions, a thread's `running` and `waiting`.
pub const PROTOCOL: u32 = 2;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub project_id: String,
    pub title: String,
    /// Milliseconds since 1970.
    pub created_at: f64,
    pub updated_at: f64,
    pub pinned: bool,
    pub settled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Working now (`running`), or its rows changed in the last few seconds.
    pub active: bool,
    /// A turn is under way, as the engine says.
    #[serde(default)]
    pub running: bool,
    /// Something waits on you: a tool to allow, a question, a plan.
    #[serde(default)]
    pub waiting: bool,
}

/// One transcript row, as much of it as the phone draws.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub seq: i64,
    /// The database's change counter when it was written: of two copies, the higher is newer,
    /// whichever message brought it.
    pub rev: i64,
    pub id: String,
    /// `user`, `assistant`, `thinking`, `tool`, `notice`, `commandOutput`, or newer kinds.
    pub kind: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Trimmed to `INPUT_LIMIT` characters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<String>,
    /// Trimmed to `RESULT_LIMIT` characters; `tool_result_length` is the whole one's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_result_length: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_is_error: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice_style: Option<String>,
    /// Seconds since 1970.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_ended_at: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub attachments: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub changed_files: Vec<String>,
}

/// A request waiting on you, as the app's card shows it: a tool to allow, Claude's question, or
/// a plan to approve (`tool` says which: `AskUserQuestion`, `ExitPlanMode`, a tool's name).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    pub tool: String,
    pub description: String,
    /// The tool's input as JSON, trimmed to `INPUT_LIMIT` characters.
    pub input: String,
}

/// An MCP server's question mid-call (answered on the Mac for now).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Question {
    pub id: String,
    pub server: String,
    pub message: String,
}

/// Your message waiting for Claude's current step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Queued {
    pub id: String,
    pub text: String,
}

/// What a thread's run is doing, beyond its rows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Live {
    pub running: bool,
    /// What the CLI says it's doing between messages ("Compacting the conversation…").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    /// Oldest first; the app answers the first.
    #[serde(default)]
    pub requests: Vec<Request>,
    #[serde(default)]
    pub questions: Vec<Question>,
    #[serde(default)]
    pub queue: Vec<Queued>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

pub const INPUT_LIMIT: usize = 4000;
pub const RESULT_LIMIT: usize = 2000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToApp {
    Hello { protocol: u32, server: String, data: String },
    /// Every project and thread, in sidebar order.
    Threads { projects: Vec<Project>, threads: Vec<Thread> },
    ThreadUpserted { thread: Thread },
    ThreadRemoved { id: String },
    /// The newest rows of the thread subscribed to: `rows` are `first_seq..count`.
    Snapshot { thread: String, first_seq: i64, count: i64, rows: Vec<Row> },
    /// Rows added or changed, oldest first; `count` is how many the transcript has now.
    RowsChanged { thread: String, count: i64, rows: Vec<Row> },
    /// Earlier rows asked for with `page`.
    PageRows { thread: String, rows: Vec<Row> },
    /// The subscribed thread's live state: after `snapshot`, and whenever it changes.
    Live { thread: String, live: Live },
    /// An action was taken (`error`: why not), for the action that carried this `id`.
    Done { id: String, #[serde(skip_serializing_if = "Option::is_none")] error: Option<String> },
    Error { message: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FromApp {
    SubscribeApp,
    /// Watch this thread (one at a time): answered with a `snapshot`, then `rows_changed`.
    SubscribeThread { thread: String, #[serde(default)] limit: Option<i64> },
    UnsubscribeThread,
    /// Rows before `before_seq`, at most `limit`.
    Page { thread: String, before_seq: i64, #[serde(default)] limit: Option<i64> },
    /// Your message, as typed in the app's composer: it starts a turn, or steers into the one
    /// under way (`now`: stops it for this message). Commands the app answers (`/model`…) work.
    Send { #[serde(default)] id: Option<String>, thread: String, text: String, #[serde(default)] now: bool },
    /// Stop, as ⌘. does: queued messages go back to the Mac's composer.
    Stop { #[serde(default)] id: Option<String>, thread: String },
    /// Allows or denies a request waiting on you, by its id.
    Respond { #[serde(default)] id: Option<String>, thread: String, request_id: String, allow: bool },
    /// Takes back a queued message.
    Withdraw { #[serde(default)] id: Option<String>, thread: String, message: String },
    /// A queued message, right away: the turn is stopped for it.
    SendNow { #[serde(default)] id: Option<String>, thread: String, message: String },
    #[serde(other)]
    Unknown,
}

impl FromApp {
    /// The id an action carries, for its `done`.
    pub fn action_id(&self) -> Option<&str> {
        match self {
            FromApp::Send { id, .. } | FromApp::Stop { id, .. } | FromApp::Respond { id, .. } | FromApp::Withdraw { id, .. } | FromApp::SendNow { id, .. } => id.as_deref(),
            _ => None,
        }
    }
}

/// `s` cut to at most `limit` characters, on a character boundary.
pub fn trim(s: String, limit: usize) -> String {
    match s.char_indices().nth(limit) {
        Some((at, _)) => s[..at].to_string(),
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_messages_and_fields_are_ignored() {
        let m: FromApp = serde_json::from_str(r#"{"type":"sing","text":"hi"}"#).unwrap();
        assert_eq!(m, FromApp::Unknown);
        let m: FromApp = serde_json::from_str(r#"{"type":"subscribe_thread","thread":"t","extra":1}"#).unwrap();
        assert_eq!(m, FromApp::SubscribeThread { thread: "t".into(), limit: None });
    }

    #[test]
    fn tagged_as_the_app_reads_them() {
        let s = serde_json::to_string(&ToApp::ThreadRemoved { id: "a".into() }).unwrap();
        assert_eq!(s, r#"{"type":"thread_removed","id":"a"}"#);
    }

    #[test]
    fn actions_as_a_client_writes_them() {
        let m: FromApp = serde_json::from_str(r#"{"type":"send","id":"a1","thread":"t","text":"hi"}"#).unwrap();
        assert_eq!(m, FromApp::Send { id: Some("a1".into()), thread: "t".into(), text: "hi".into(), now: false });
        assert_eq!(m.action_id(), Some("a1"));
        let m: FromApp = serde_json::from_str(r#"{"type":"respond","thread":"t","request_id":"r","allow":true}"#).unwrap();
        assert_eq!(m, FromApp::Respond { id: None, thread: "t".into(), request_id: "r".into(), allow: true });
        let s = serde_json::to_string(&ToApp::Done { id: "a1".into(), error: None }).unwrap();
        assert_eq!(s, r#"{"type":"done","id":"a1"}"#);
    }

    /// A first-version client's thread (no `running`, `waiting`) still reads.
    #[test]
    fn reads_a_thread_without_live_fields() {
        let t: Thread = serde_json::from_str(r#"{"id":"t","project_id":"p","title":"T","created_at":1,"updated_at":2,"pinned":false,"settled":false,"active":true}"#).unwrap();
        assert!(t.active && !t.running && !t.waiting);
    }

    #[test]
    fn trims_on_character_boundaries() {
        assert_eq!(trim("héllo".into(), 2), "hé");
        assert_eq!(trim("hi".into(), 5), "hi");
    }
}
