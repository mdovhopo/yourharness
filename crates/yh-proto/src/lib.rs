//! The messages YourHarness and its clients trade (the phone app, the `watch` example): JSON
//! text frames tagged with `type`. The phone's development server spoke the first version
//! (docs/mobile.md, "The wire"); this one adds what the engine knows live (a thread working, what
//! waits on you, the queue) and what a client may ask (send, stop, allow or deny, withdraw, send
//! now). Additions only, so a client of either version reads the other. Unknown messages and
//! fields are ignored on both sides.

use serde::{Deserialize, Serialize};

/// 2: `live`, `done`, the actions, a thread's `running` and `waiting`.
/// 3: answering questions and plans, MCP questions, new threads, model, effort and mode, thread
/// actions, whole tool output, photos with a message, `options`, and `done` once the app took
/// the action (with the thread it made).
/// 4: a row's sub-agent summary, a sub-agent's whole run, a row's pictures, a diagram drawn.
/// 5: search across threads, files for `@`, slash commands in `options`, usage limits, asking on
/// the side, and a snapshot from what the client already holds.
pub const PROTOCOL: u32 = 5;

/// A slash command the composer offers: `/name hint`, and what it does.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SlashCommand {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hint: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// A usage window: how much of it is used (0 to 1) and when it starts over.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LimitWindow {
    /// `five_hour`, `seven_day`, `seven_day_opus`…
    pub name: String,
    pub used: f64,
    /// Seconds since 1970.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<f64>,
}

/// Where a search found its words: a thread's row, the words around them in `[brackets]`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub thread: String,
    pub seq: i64,
    pub snippet: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    /// Its permission mode: `auto`, `ask`, `acceptEdits`, `plan`, `bypass`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
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
    /// The effort chosen for it, when one was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
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
    /// A sub-agent's call (`Agent`, `Task`): its run summarized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent: Option<SubagentInfo>,
}

/// A sub-agent's run as its card shows it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SubagentInfo {
    pub description: String,
    /// `running`, `completed`, `failed`, `stopped`.
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_uses: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// What it's doing now ("Reading a.txt").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    #[serde(default)]
    pub background: bool,
}

/// One thing a sub-agent did: a message (`tool` none) or a tool call and its result.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SubagentStep {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// The message, or the call's input (JSON), trimmed to `INPUT_LIMIT` characters.
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default)]
    pub is_error: bool,
}

/// A sub-agent's whole run: what it was asked, its steps (the newest few hundred) and its report.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SubagentRun {
    pub info: SubagentInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default)]
    pub steps: Vec<SubagentStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
}

/// A diagram as shapes to draw: paths in the diagram's own points (y down), then labels.
/// Colours are roles the client resolves in its own light or dark (`text` at an alpha, the
/// `canvas` and `card` backgrounds, the `accent`), or an author's colour.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Drawing {
    pub width: f64,
    pub height: f64,
    /// "Flowchart", or a sequence diagram's title.
    pub kind: String,
    pub paths: Vec<DrawnPath>,
    pub labels: Vec<DrawnLabel>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DrawnPath {
    /// `M x y`, `L x y`, `A rx ry x y` (a clockwise elliptical arc to the point), `Z`.
    pub d: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Ink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<Ink>,
    #[serde(default)]
    pub width: f32,
    #[serde(default)]
    pub dashed: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DrawnLabel {
    pub lines: Vec<String>,
    pub x: f64,
    pub y: f64,
    /// Centred in this width from `x`; left-aligned from `x` without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    pub size: f32,
    pub line_height: f64,
    /// 400 regular, 500 medium, 600 semibold.
    pub weight: u16,
    pub color: Ink,
}

/// A colour: a role and an alpha (`text`, `canvas`, `card`, `accent`), or red, green, blue, alpha.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ink {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rgba: Option<[f32; 4]>,
    pub alpha: f32,
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

/// An MCP server's question mid-call: `form` (accepted with its defaults, or declined) or `url`
/// (a page to visit, on the Mac).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Question {
    pub id: String,
    pub server: String,
    pub message: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// A model a thread can use, as the Mac's picker lists it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelOption {
    /// What the app keeps (`opus`, an alias, or a full ID).
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub efforts: Vec<String>,
    /// Whether the `auto` permission mode works with it.
    #[serde(default)]
    pub auto: bool,
}

/// A picture sent with a message: JPEG or PNG, base64.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Image {
    pub media_type: String,
    pub data: String,
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
    /// An action was taken (`error`: why not), for the action that carried this `id`; `thread`:
    /// the one it made (`new_thread`).
    Done { id: String, #[serde(skip_serializing_if = "Option::is_none")] error: Option<String>, #[serde(default, skip_serializing_if = "Option::is_none")] thread: Option<String> },
    /// The models and permission modes to choose from, and each project's slash commands: after
    /// `threads`, and when they change.
    Options { models: Vec<ModelOption>, modes: Vec<String>, #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")] commands: std::collections::BTreeMap<String, Vec<SlashCommand>> },
    /// Usage limits as Claude last reported them: after `options`, and when they change.
    Limits { windows: Vec<LimitWindow> },
    /// What `search` found, best first.
    SearchResults { query: String, hits: Vec<SearchHit> },
    /// A thread's files matching `query`, for `@`.
    Files { thread: String, query: String, files: Vec<String> },
    /// A sub-agent's whole run, asked for with `subagent`.
    Subagent { thread: String, seq: i64, run: SubagentRun },
    /// A row's picture (`index`: which of its pictures), asked for with `media`: base64, or why not.
    Media { thread: String, seq: i64, index: usize, #[serde(default, skip_serializing_if = "Option::is_none")] media_type: Option<String>, #[serde(default, skip_serializing_if = "Option::is_none")] data: Option<String>, #[serde(default, skip_serializing_if = "Option::is_none")] error: Option<String> },
    /// A Mermaid diagram laid out and drawn, asked for with `diagram` (`error`: why it can't be).
    Diagram { key: String, #[serde(default, skip_serializing_if = "Option::is_none")] drawing: Option<Drawing>, #[serde(default, skip_serializing_if = "Option::is_none")] error: Option<String> },
    /// A row's whole tool input and result, asked for with `tool_result`.
    ToolResult { thread: String, seq: i64, #[serde(skip_serializing_if = "Option::is_none")] input: Option<String>, #[serde(skip_serializing_if = "Option::is_none")] result: Option<String> },
    Error { message: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FromApp {
    SubscribeApp,
    /// Watch this thread (one at a time): answered with a `snapshot`, then `rows_changed`.
    /// `known`: how many rows the client holds already (reconnecting): the snapshot then starts
    /// a little before them, and the client keeps what it has before that.
    SubscribeThread { thread: String, #[serde(default)] limit: Option<i64>, #[serde(default)] known: Option<i64> },
    UnsubscribeThread,
    /// Rows before `before_seq`, at most `limit`.
    Page { thread: String, before_seq: i64, #[serde(default)] limit: Option<i64> },
    /// Your message, as typed in the app's composer: it starts a turn, or steers into the one
    /// under way (`now`: stops it for this message). Commands the app answers (`/model`…) work.
    Send { #[serde(default)] id: Option<String>, thread: String, text: String, #[serde(default)] now: bool, #[serde(default, skip_serializing_if = "Vec::is_empty")] images: Vec<Image> },
    /// Stop, as ⌘. does: queued messages go back to the Mac's composer.
    Stop { #[serde(default)] id: Option<String>, thread: String },
    /// Allows or denies a request waiting on you, by its id.
    Respond { #[serde(default)] id: Option<String>, thread: String, request_id: String, allow: bool },
    /// Takes back a queued message.
    Withdraw { #[serde(default)] id: Option<String>, thread: String, message: String },
    /// A queued message, right away: the turn is stopped for it.
    SendNow { #[serde(default)] id: Option<String>, thread: String, message: String },
    /// Answers Claude's question (`AskUserQuestion`): each question's text, and its answer (the
    /// options chosen, joined with ", ", or what was written).
    Answer { #[serde(default)] id: Option<String>, thread: String, request_id: String, answers: std::collections::BTreeMap<String, String> },
    /// Declines Claude's question: it carries on with its best judgment.
    SkipQuestion { #[serde(default)] id: Option<String>, thread: String, request_id: String },
    /// Approves Claude's plan into a permission mode (which becomes the project's); `feedback`
    /// follows as your next message.
    ApprovePlan { #[serde(default)] id: Option<String>, thread: String, request_id: String, mode: String, #[serde(default)] feedback: Option<String> },
    /// Keeps planning, with what to change.
    KeepPlanning { #[serde(default)] id: Option<String>, thread: String, request_id: String, #[serde(default)] feedback: Option<String> },
    /// An MCP server's question: accepted (a form with its defaults) or declined.
    AnswerQuestion { #[serde(default)] id: Option<String>, thread: String, request_id: String, accept: bool },
    /// A new thread in a project (`text`: its first message); `done` names it.
    NewThread { #[serde(default)] id: Option<String>, project: String, #[serde(default)] text: Option<String> },
    SetModel { #[serde(default)] id: Option<String>, thread: String, #[serde(default)] model: Option<String> },
    SetEffort { #[serde(default)] id: Option<String>, thread: String, #[serde(default)] effort: Option<String> },
    /// The permission mode of the thread's project.
    SetMode { #[serde(default)] id: Option<String>, thread: String, mode: String },
    Rename { #[serde(default)] id: Option<String>, thread: String, title: String },
    Pin { #[serde(default)] id: Option<String>, thread: String, on: bool },
    /// On its project's Settled shelf (`on`), or back.
    Settle { #[serde(default)] id: Option<String>, thread: String, on: bool },
    Delete { #[serde(default)] id: Option<String>, thread: String },
    /// A row's whole tool input and result: answered with `tool_result`.
    ToolResult { thread: String, seq: i64 },
    /// A sub-agent's whole run (its call's row): answered with `subagent`.
    Subagent { thread: String, seq: i64 },
    /// A row's picture: yours (an attachment) or one Claude showed (`show_media`), by its place
    /// among the row's pictures. Answered with `media`.
    Media { thread: String, seq: i64, index: usize },
    /// A Mermaid block's source, laid out and drawn as the Mac draws it: answered with `diagram`
    /// under the same `key`.
    Diagram { key: String, source: String },
    /// Words across every thread's transcript: answered with `search_results`.
    Search { query: String },
    /// A thread's files matching `query` (for `@`): answered with `files`.
    Files { thread: String, query: String },
    /// A question on the side of `thread`, as `/btw` asks it: in its side chat (`side:<thread>`).
    AskAside { #[serde(default)] id: Option<String>, thread: String, text: String },
    #[serde(other)]
    Unknown,
}

impl FromApp {
    /// The id an action carries, for its `done`.
    pub fn action_id(&self) -> Option<&str> {
        use FromApp::*;
        match self {
            Send { id, .. } | Stop { id, .. } | Respond { id, .. } | Withdraw { id, .. } | SendNow { id, .. } | AskAside { id, .. } | Answer { id, .. } | SkipQuestion { id, .. } | ApprovePlan { id, .. } | KeepPlanning { id, .. } | AnswerQuestion { id, .. } | NewThread { id, .. } | SetModel { id, .. } | SetEffort { id, .. } | SetMode { id, .. } | Rename { id, .. } | Pin { id, .. } | Settle { id, .. } | Delete { id, .. } => id.as_deref(),
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
        assert_eq!(m, FromApp::SubscribeThread { thread: "t".into(), limit: None, known: None });
    }

    #[test]
    fn tagged_as_the_app_reads_them() {
        let s = serde_json::to_string(&ToApp::ThreadRemoved { id: "a".into() }).unwrap();
        assert_eq!(s, r#"{"type":"thread_removed","id":"a"}"#);
    }

    #[test]
    fn actions_as_a_client_writes_them() {
        let m: FromApp = serde_json::from_str(r#"{"type":"send","id":"a1","thread":"t","text":"hi"}"#).unwrap();
        assert_eq!(m, FromApp::Send { id: Some("a1".into()), thread: "t".into(), text: "hi".into(), now: false, images: vec![] });
        assert_eq!(m.action_id(), Some("a1"));
        let m: FromApp = serde_json::from_str(r#"{"type":"respond","thread":"t","request_id":"r","allow":true}"#).unwrap();
        assert_eq!(m, FromApp::Respond { id: None, thread: "t".into(), request_id: "r".into(), allow: true });
        let s = serde_json::to_string(&ToApp::Done { id: "a1".into(), error: None, thread: None }).unwrap();
        assert_eq!(s, r#"{"type":"done","id":"a1"}"#);
        let m: FromApp = serde_json::from_str(r#"{"type":"answer","id":"a2","thread":"t","request_id":"r","answers":{"Which?":"Rust"}}"#).unwrap();
        assert_eq!(m.action_id(), Some("a2"));
        let m: FromApp = serde_json::from_str(r#"{"type":"new_thread","project":"p","text":"hi"}"#).unwrap();
        assert_eq!(m, FromApp::NewThread { id: None, project: "p".into(), text: Some("hi".into()) });
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
