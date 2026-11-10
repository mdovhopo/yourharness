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
/// 6: one PR in full (`pull`: its description, checks, comments and reviews) and merging it
/// (`merge_pull`), for reviewing a PR on the phone; its files changed (`pull_files`) and each
/// one's diff (`pull_patch`).
/// 7: a thread's browser on the phone: what it shows streamed as pictures (`watch_browser`,
/// `browser`, `browser_frame`, `browser_ack`), and taps, scrolls, typing and going places sent
/// back (`browser_input`).
pub const PROTOCOL: u32 = 7;

/// A slash command the composer offers: `/name hint`, and what it does.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SlashCommand {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hint: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// The Claude subscription a Mac's `claude` is signed in with: whose limits they are.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Subscription {
    /// "Claude Max", "Claude Pro", "API key".
    pub plan: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
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

/// One open PR as a project's page lists it (protocol 5): in a stack (`stack` is its bottom PR's
/// `repo#number`, `depth` how far up), in a section (`review`: waits for your review, `mine`,
/// `others`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PullInfo {
    /// `owner/repo`.
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub draft: bool,
    /// `approved`, `changesRequested`, `required` or empty.
    #[serde(default)]
    pub review: String,
    /// `passing`, `failing`, `pending` or empty.
    #[serde(default)]
    pub checks: String,
    /// `CLEAN`, `BLOCKED`, `BEHIND`, `DIRTY` (conflicts), `UNSTABLE`, `DRAFT`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_status: Option<String>,
    #[serde(default)]
    pub auto_merge: bool,
    #[serde(default)]
    pub in_merge_queue: bool,
    /// ISO 8601.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    pub head: String,
    pub base: String,
    pub mine: bool,
    pub section: String,
    pub stack: String,
    pub depth: u32,
}

/// One PR as its review page shows it (protocol 6), read through `gh` (or `glab`) on the Mac.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PullDetail {
    /// `owner/repo`.
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    /// `open`, `merged`, `closed`.
    pub state: String,
    pub draft: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// The description, markdown.
    #[serde(default)]
    pub body: String,
    pub head: String,
    pub base: String,
    #[serde(default)]
    pub additions: u64,
    #[serde(default)]
    pub deletions: u64,
    #[serde(default)]
    pub changed_files: u64,
    #[serde(default)]
    pub commits: u64,
    /// ISO 8601.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_at: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// `approved`, `changesRequested`, `required` or empty.
    #[serde(default)]
    pub review: String,
    /// `CLEAN`, `BLOCKED`, `BEHIND`, `DIRTY` (conflicts), `UNSTABLE`, `DRAFT`, `UNKNOWN`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_status: Option<String>,
    #[serde(default)]
    pub auto_merge: bool,
    /// Every check, failing first.
    #[serde(default)]
    pub checks: Vec<PullCheck>,
    /// Comments, reviews and comments on lines of code, oldest first.
    #[serde(default)]
    pub comments: Vec<PullComment>,
    /// How the repo lets it be merged, of `squash`, `merge`, `rebase`; the default first.
    #[serde(default)]
    pub merge_methods: Vec<String>,
}

/// A check on a PR: a CI job or a commit status.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PullCheck {
    pub name: String,
    /// The workflow it's part of, when it's an Actions job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
    /// `passing`, `failing`, `pending`, `skipped`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// A file a PR changes, as its Files tab lists it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PullFile {
    pub path: String,
    /// Where it was before, when it was renamed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_path: Option<String>,
    /// `added`, `removed`, `modified`, `renamed`, `copied`, `changed`.
    pub status: String,
    #[serde(default)]
    pub additions: u64,
    #[serde(default)]
    pub deletions: u64,
    /// Whether its diff can be shown (the forge leaves out huge and binary ones).
    #[serde(default)]
    pub has_patch: bool,
}

/// Something said on a PR.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PullComment {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Markdown.
    pub body: String,
    /// ISO 8601.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// `comment`, `review` or `code` (on a line of the diff).
    pub kind: String,
    /// A review's verdict: `approved`, `changesRequested`, `commented`, `dismissed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    /// A code comment's file and line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
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
    /// Commands and sub-agents running in the background: what goes on after its turn.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub jobs: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
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

/// Something Claude left running: a command (in the background, or a foreground one that's
/// taking a while), a watch (Monitor) or a sub-agent in the background. Only its start and
/// what it says change it: the phone ticks its clock itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    /// `command`, `watch` or `agent`.
    pub kind: String,
    pub description: String,
    #[serde(default)]
    pub background: bool,
    /// When it started, in seconds since 1970 (whole, so it never changes while it runs).
    pub started_at: f64,
    /// What a watch last saw, or what a sub-agent is doing now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_line: Option<String>,
    /// The running sub-agent that started it (that job's `id`): it shows with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
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
    /// What's running, oldest first: sub-agents, then commands and watches.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<Job>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

/// One tab of a thread's browser (protocol 7).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BrowserTab {
    pub id: u32,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub loading: bool,
}

/// A thread's browser around its pictures (protocol 7): its tabs, where the shown one is, and
/// the page's size in CSS pixels, which is where a tap on a picture lands.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BrowserState {
    #[serde(default)]
    pub tabs: Vec<BrowserTab>,
    /// The tab shown (and streamed); None with no tabs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<u32>,
    #[serde(default)]
    pub can_back: bool,
    #[serde(default)]
    pub can_forward: bool,
    #[serde(default)]
    pub width: f64,
    #[serde(default)]
    pub height: f64,
    /// Taken over on the Mac (Take Control): the agent's actions wait; yours still work.
    #[serde(default)]
    pub user_control: bool,
    /// The focused element takes typing (a field, a text area, something editable).
    #[serde(default)]
    pub editing: bool,
    /// An alert, confirm or prompt the page waits on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialog: Option<BrowserDialog>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BrowserDialog {
    /// `alert`, `confirm` or `prompt`.
    pub kind: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_text: Option<String>,
}

/// What the phone does to a thread's browser (protocol 7), as you'd do it in the Mac's panel.
/// Points are in the page's CSS pixels (`BrowserState`'s `width` × `height`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserInput {
    Tap { x: f64, y: f64 },
    /// Scrolls what's under the point (the page when nothing there scrolls) by `dx`, `dy`.
    Scroll { x: f64, y: f64, dx: f64, dy: f64 },
    /// Text typed into the focused element.
    Text { text: String },
    /// A key with modifiers, named as the agent's `press` names them (`Enter`, `Backspace`,
    /// `Shift+Tab`).
    Key { key: String },
    /// The shown tab goes to an address (a new tab when there's none).
    Navigate { url: String },
    Back,
    Forward,
    Reload,
    /// A new tab at an address, shown: a `localhost` link tapped on the phone.
    Open { url: String },
    SelectTab { tab: u32 },
    CloseTab { tab: u32 },
    /// The page laid out at a size: `phone`, `tablet`, `laptop`, `desktop`, or `fit` (the Mac's
    /// panel), as the agent's `resize` presets; `width` × `height` CSS pixels instead when given
    /// (the phone's own screen, so the page fits it exactly).
    Size { preset: String, #[serde(default, skip_serializing_if = "Option::is_none")] width: Option<f64>, #[serde(default, skip_serializing_if = "Option::is_none")] height: Option<f64> },
    /// The waiting alert, confirm or prompt answered.
    Dialog { accept: bool, #[serde(default, skip_serializing_if = "Option::is_none")] text: Option<String> },
}

pub const INPUT_LIMIT: usize = 4000;
pub const RESULT_LIMIT: usize = 2000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToApp {
    /// `mac` (protocol 5): this Mac's lasting ID, the same whatever address it's reached at, so a
    /// phone knows a Mac it paired with before.
    Hello { protocol: u32, server: String, data: String, #[serde(default, skip_serializing_if = "Option::is_none")] mac: Option<String>,
        /// Every address the Mac can be reached at (local network, tailnet), the likeliest first:
        /// a phone tries them all and keeps the one that answers.
        #[serde(default, skip_serializing_if = "Vec::is_empty")] hosts: Vec<String> },
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
    /// Usage limits as Claude last reported them: after `options`, and when they change. With
    /// the subscription they belong to (one per Mac for now), from Macs that know it.
    Limits { windows: Vec<LimitWindow>, #[serde(default, skip_serializing_if = "Option::is_none")] subscription: Option<Subscription> },
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
    /// Protocol 5: a project's open PRs, as its page on the Mac lists them; `errors` say why some
    /// couldn't be read (`gh` missing, offline), `accounts` who the forges know you as.
    Pulls { project: String, pulls: Vec<PullInfo>, #[serde(default)] errors: Vec<String>, #[serde(default)] accounts: Vec<String> },
    /// Protocol 5: paired with this Mac (the connection carried a pairing code): connect with
    /// `?token=` from now on. Sent once, right after `hello`.
    Paired { device: String, token: String },
    /// Protocol 6: one PR in full, asked for with `pull` (`error`: why it couldn't be read).
    Pull { url: String, #[serde(default, skip_serializing_if = "Option::is_none")] pull: Option<PullDetail>, #[serde(default, skip_serializing_if = "Option::is_none")] error: Option<String> },
    /// Protocol 6: the files a PR changes, asked for with `pull_files`.
    PullFiles { url: String, #[serde(default)] files: Vec<PullFile>, #[serde(default, skip_serializing_if = "Option::is_none")] error: Option<String> },
    /// Protocol 6: one file's unified diff (its hunks, as `git diff` prints them after the
    /// header), asked for with `pull_patch`.
    PullPatch { url: String, path: String, #[serde(default, skip_serializing_if = "Option::is_none")] patch: Option<String>, #[serde(default, skip_serializing_if = "Option::is_none")] error: Option<String> },
    /// Protocol 7: the watched browser's tabs and page: after `watch_browser`, and when they change.
    Browser { thread: String, state: BrowserState },
    /// Protocol 7: what the watched browser's shown tab looks like now, a JPEG (base64)
    /// `width` × `height` pixels. The next comes once this one's `browser_ack` is in, and only
    /// when the page changed.
    BrowserFrame { thread: String, tab: u32, seq: u64, width: u32, height: u32, data: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FromApp {
    SubscribeApp,
    /// Watch this thread (one at a time, plus a side chat beside it: `side:<thread>`; protocol
    /// 5): answered with a `snapshot`, then `rows_changed`. `unsubscribe_thread` stops both.
    /// `known`: how many rows the client holds already (reconnecting): the snapshot then starts
    /// a little before them, and the client keeps what it has before that.
    SubscribeThread { thread: String, #[serde(default)] limit: Option<i64>, #[serde(default)] known: Option<i64> },
    /// Stop watching: `thread` alone (a side chat closed; protocol 5), or everything watched.
    UnsubscribeThread { #[serde(default, skip_serializing_if = "Option::is_none")] thread: Option<String> },
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
    /// A new thread in a project (`text`: its first message), with the model, effort and
    /// permission mode (its project's) chosen for it; `done` names it.
    NewThread {
        #[serde(default)]
        id: Option<String>,
        project: String,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        effort: Option<String>,
        #[serde(default)]
        mode: Option<String>,
    },
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
    /// Protocol 5: a project's open PRs (`fresh`: asked of the forges again, not the last
    /// minute's answer); answered with `pulls`.
    Pulls { project: String, #[serde(default)] fresh: bool },
    /// Protocol 6: a PR by its URL, read from its forge: answered with `pull`.
    Pull { url: String },
    /// Protocol 6: merges a PR (`method`: `squash`, `merge` or `rebase`; `auto`: once its checks
    /// and reviews allow), as `gh pr merge` does: answered with `done`, then a fresh `pull`.
    MergePull { #[serde(default)] id: Option<String>, url: String, method: String, #[serde(default)] auto: bool },
    /// Protocol 6: the files a PR changes: answered with `pull_files`.
    PullFiles { url: String },
    /// Protocol 6: one of them's diff: answered with `pull_patch`.
    PullPatch { url: String, path: String },
    /// Protocol 7: streams a thread's browser to this client (one at a time): `browser`, then
    /// `browser_frame`s. `width`: the most pixels across it can use.
    WatchBrowser { thread: String, #[serde(default)] width: Option<u32> },
    UnwatchBrowser,
    /// Protocol 7: the frame `seq` is drawn: the next may come.
    BrowserAck { seq: u64 },
    /// Protocol 7: a tap, scroll, typing or going somewhere in a thread's browser; `done` when
    /// it carries an `id`.
    BrowserInput { #[serde(default)] id: Option<String>, thread: String, input: BrowserInput },
    #[serde(other)]
    Unknown,
}

impl FromApp {
    /// The id an action carries, for its `done`.
    pub fn action_id(&self) -> Option<&str> {
        use FromApp::*;
        match self {
            Send { id, .. } | Stop { id, .. } | Respond { id, .. } | Withdraw { id, .. } | SendNow { id, .. } | AskAside { id, .. } | Answer { id, .. } | SkipQuestion { id, .. } | ApprovePlan { id, .. } | KeepPlanning { id, .. } | AnswerQuestion { id, .. } | NewThread { id, .. } | SetModel { id, .. } | SetEffort { id, .. } | SetMode { id, .. } | Rename { id, .. } | Pin { id, .. } | Settle { id, .. } | Delete { id, .. } | MergePull { id, .. } | BrowserInput { id, .. } => id.as_deref(),
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
        assert_eq!(m, FromApp::NewThread { id: None, project: "p".into(), text: Some("hi".into()), model: None, effort: None, mode: None });
        let m: FromApp = serde_json::from_str(r#"{"type":"new_thread","project":"p","text":"hi","model":"haiku","effort":"low","mode":"plan"}"#).unwrap();
        assert!(matches!(m, FromApp::NewThread { model: Some(ref m), effort: Some(ref e), mode: Some(ref p), .. } if m == "haiku" && e == "low" && p == "plan"));
    }

    #[test]
    fn browser_messages() {
        let m: FromApp = serde_json::from_str(r#"{"type":"browser_input","id":"b1","thread":"t","input":{"kind":"tap","x":10,"y":20.5}}"#).unwrap();
        assert_eq!(m, FromApp::BrowserInput { id: Some("b1".into()), thread: "t".into(), input: BrowserInput::Tap { x: 10., y: 20.5 } });
        assert_eq!(m.action_id(), Some("b1"));
        let m: FromApp = serde_json::from_str(r#"{"type":"browser_input","thread":"t","input":{"kind":"back"}}"#).unwrap();
        assert_eq!(m, FromApp::BrowserInput { id: None, thread: "t".into(), input: BrowserInput::Back });
        let m: FromApp = serde_json::from_str(r#"{"type":"watch_browser","thread":"t","width":1179}"#).unwrap();
        assert_eq!(m, FromApp::WatchBrowser { thread: "t".into(), width: Some(1179) });
        let m: FromApp = serde_json::from_str(r#"{"type":"browser_ack","seq":3}"#).unwrap();
        assert_eq!(m, FromApp::BrowserAck { seq: 3 });
        let s = serde_json::to_string(&ToApp::Browser { thread: "t".into(), state: BrowserState { selected: Some(1), ..Default::default() } }).unwrap();
        assert_eq!(s, r#"{"type":"browser","thread":"t","state":{"tabs":[],"selected":1,"can_back":false,"can_forward":false,"width":0.0,"height":0.0,"user_control":false,"editing":false}}"#);
    }

    #[test]
    fn pull_messages() {
        let m: FromApp = serde_json::from_str(r#"{"type":"merge_pull","id":"m1","url":"https://github.com/o/r/pull/1","method":"squash"}"#).unwrap();
        assert_eq!(m, FromApp::MergePull { id: Some("m1".into()), url: "https://github.com/o/r/pull/1".into(), method: "squash".into(), auto: false });
        assert_eq!(m.action_id(), Some("m1"));
        let s = serde_json::to_string(&ToApp::Pull { url: "u".into(), pull: None, error: Some("no".into()) }).unwrap();
        assert_eq!(s, r#"{"type":"pull","url":"u","error":"no"}"#);
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
