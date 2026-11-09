//! A thread's run: its `claude`'s events turned into transcript rows and live state (requests
//! waiting for you, the queue, jobs, context), with no drawing.
//!
//! It holds only the rows it may still change: the turn going now, sub-agents and jobs still
//! running, and rows not saved yet. Everything it changes comes out as `RowChange`s, which
//! whoever shows the thread applies to its own rows (`take_changes`), and as rows to save
//! (`take_saves`). When an event names a row it doesn't hold (a sub-agent from an earlier
//! turn), it asks a `RowSource` for it.

use crate::claude::{Block, BlockKind, DeltaKind, Event};
use crate::db::Row;
use crate::jobs::Jobs;
use crate::library::{now_ms, new_uuid, EntryData, Kind};
use crate::subagent_record::{SubagentEntry, SubagentRecord, SubagentSummary};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// Tools whose call is a sub-agent: shown as a card of its own.
pub const AGENT_TOOLS: &[&str] = &["Agent", "Task"];

/// How many of a sub-agent's steps are kept for following it.
const SUBAGENT_LOG: usize = 200;

#[derive(Clone, Debug)]
pub struct Subagent {
    pub description: String,
    pub status: String,
    pub steps: usize,
    /// What it reported, when it came apart from the tool's result (a background sub-agent).
    pub report: Option<String>,
    /// What it said and did, for following it in the right sidebar: its finished messages
    /// and tool calls, the newest `SUBAGENT_LOG` of them.
    pub log: Vec<SubStep>,
    /// What `stop_task` takes to stop it (the right sidebar's Stop).
    pub task_id: Option<String>,
    /// What it's doing now, as the CLI says it (`task_progress`: "Reading a.txt").
    pub activity: Option<String>,
    /// Tokens it has used so far, and its tool calls, as the CLI counts them.
    pub tokens: Option<u64>,
    pub tool_uses: Option<u64>,
    /// When it started (seconds since 1970), and how long it ran, as the CLI counts it.
    pub started_at: f64,
    pub duration_ms: Option<u64>,
    /// What it had used before it was continued (`SendMessage`): the CLI counts a continued
    /// run afresh, so its numbers go on from these.
    base: crate::claude::SubagentUsage,
}

/// One thing a sub-agent did: a message (`tool` None) or a tool call with its result.
#[derive(Clone, Debug)]
pub struct SubStep {
    pub tool: Option<String>,
    pub id: String,
    pub text: String,
    pub result: Option<String>,
    pub is_error: bool,
}

impl Subagent {
    pub fn new(description: String) -> Subagent {
        Subagent {
            description,
            status: "running".into(),
            steps: 0,
            report: None,
            log: vec![],
            task_id: None,
            activity: None,
            tokens: None,
            tool_uses: None,
            started_at: now_ms() / 1000.0,
            duration_ms: None,
            base: Default::default(),
        }
    }

    /// As saved (the record 0.0.x wrote, `SubagentSnapshot`): with what it was asked, from its call.
    pub fn record(&self, input: Option<&str>) -> SubagentRecord {
        let input: serde_json::Value = input.and_then(|i| serde_json::from_str(i).ok()).unwrap_or_default();
        SubagentRecord {
            description: self.description.clone(),
            agent_type: input["subagent_type"].as_str().map(str::to_string),
            prompt: input["prompt"].as_str().unwrap_or("").to_string(),
            is_background: input["run_in_background"] == true,
            status: self.status.clone(),
            tool_uses: self.tool_uses.or_else(|| (self.steps > 0).then_some(self.steps as u64)),
            tokens: self.tokens,
            duration_ms: self.duration_ms,
            report: self.report.clone(),
            started_at: self.started_at,
            entries: self
                .log
                .iter()
                .enumerate()
                .map(|(n, step)| match &step.tool {
                    Some(tool) => SubagentEntry {
                        id: step.id.clone(),
                        kind: "tool".into(),
                        text: String::new(),
                        tool_name: Some(tool.clone()),
                        tool_input: Some(step.text.clone()),
                        tool_result: step.result.clone(),
                        tool_is_error: step.result.is_some().then_some(step.is_error),
                    },
                    None => SubagentEntry { id: format!("s{n}"), kind: "assistant".into(), text: step.text.clone(), ..Default::default() },
                })
                .collect(),
        }
    }

    /// What was saved, back into a run (one still running when saved has stopped).
    pub fn from_record(r: SubagentRecord) -> Subagent {
        let log: Vec<SubStep> = r
            .entries
            .into_iter()
            .filter_map(|e| match e.kind.as_str() {
                "tool" => Some(SubStep { tool: e.tool_name.or(Some("Tool".into())), id: e.id, text: e.tool_input.unwrap_or_default(), result: e.tool_result, is_error: e.tool_is_error == Some(true) }),
                "assistant" if !e.text.is_empty() => Some(SubStep { tool: None, id: String::new(), text: e.text, result: None, is_error: false }),
                _ => None,
            })
            .collect();
        let steps = log.iter().filter(|s| s.tool.is_some()).count();
        Subagent {
            description: r.description,
            status: r.status,
            steps: r.tool_uses.map_or(steps, |n| n as usize),
            report: r.report,
            log,
            task_id: None,
            activity: None,
            tokens: r.tokens,
            tool_uses: r.tool_uses,
            started_at: r.started_at,
            duration_ms: r.duration_ms,
            base: Default::default(),
        }
    }

    /// A saved run as the panel lists it before its steps are read.
    pub fn from_summary(s: SubagentSummary) -> Subagent {
        Subagent {
            description: s.description,
            status: s.status,
            steps: s.tool_uses.unwrap_or(0) as usize,
            report: None,
            log: vec![],
            task_id: None,
            activity: None,
            tokens: s.tokens,
            tool_uses: s.tool_uses,
            started_at: s.started_at,
            duration_ms: s.duration_ms,
            base: Default::default(),
        }
    }

    /// The same run without its steps and report, as a list of runs keeps it.
    pub fn summarized(&self) -> Subagent {
        Subagent { log: vec![], report: None, ..self.clone() }
    }

    fn apply(&mut self, usage: crate::claude::SubagentUsage) {
        let plus = |n: Option<u64>, base: Option<u64>| n.map(|n| n + base.unwrap_or(0));
        self.tokens = plus(usage.tokens, self.base.tokens).or(self.tokens);
        self.tool_uses = plus(usage.tool_uses, self.base.tool_uses).or(self.tool_uses);
        self.duration_ms = plus(usage.duration_ms, self.base.duration_ms).or(self.duration_ms);
    }

    /// Continued after it ended (`SendMessage` to it): working again, its numbers going on.
    fn resume(&mut self) {
        self.base = crate::claude::SubagentUsage { tokens: self.tokens, tool_uses: self.tool_uses.or((self.steps > 0).then_some(self.steps as u64)), duration_ms: self.duration_ms };
        self.status = "running".into();
        self.activity = None;
    }
}

/// One transcript row: what's saved of it, plus what a run knows while it's written.
#[derive(Clone, Debug)]
pub struct TranscriptRow {
    /// Its number in the whole transcript.
    pub seq: usize,
    pub data: EntryData,
    /// Still being written by Claude.
    pub live: bool,
    pub sub: Option<Subagent>,
    /// When it was written (seconds since 1970), for rows made here.
    pub date: Option<f64>,
    /// How your message reached Claude: `steered` (joined the turn), `interrupted` (stopped it).
    pub delivery: Option<String>,
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

impl TranscriptRow {
    /// A row as read from storage: a sub-agent's call with its run as it was saved.
    pub fn saved(mut data: EntryData, seq: usize) -> TranscriptRow {
        let date = data.date;
        let sub = data.subagent.take().and_then(|b| SubagentRecord::decode(&b)).map(Subagent::from_record);
        TranscriptRow { seq, data, live: false, sub, date, delivery: None }
    }

    /// As a row of `entries`, with the whole tool result when only its head is held.
    pub fn row(&self, full_result: Option<String>) -> Row {
        Row {
            id: self.data.id.clone(),
            kind: kind_name(self.data.kind).into(),
            text: self.data.text.clone(),
            tool_name: self.data.tool_name.clone(),
            tool_input: self.data.tool_input.clone(),
            tool_result: full_result.or_else(|| self.data.tool_result.clone()),
            tool_is_error: (self.data.kind == Kind::Tool && self.data.tool_result.is_some()).then_some(self.data.is_error),
            date: self.date,
            turn_ended_at: self.data.turn_ended_at,
            delivery: self.delivery.clone(),
            attachments: (!self.data.attachments.is_empty()).then(|| serde_json::to_string(&self.data.attachments).unwrap_or_default()),
            notice_style: (self.data.kind == Kind::Notice && self.data.is_error).then(|| "error".into()),
            subagent: self.sub.as_ref().map(|s| s.record(self.data.tool_input.as_deref()).encode()),
            // As `JSONEncoder` writes `[String]`: slashes escaped.
            changed_files: self.data.changed_files.as_ref().map(|f| serde_json::to_string(f).unwrap_or_default().replace('/', "\\/")),
            ..Default::default()
        }
    }

    /// A call that runs a sub-agent: an Agent (or Task) call, or one whose tool isn't known yet
    /// but already has a run.
    pub fn is_agent(&self) -> bool {
        match self.data.tool_name.as_deref() {
            Some(name) => AGENT_TOOLS.contains(&name),
            None => self.sub.is_some(),
        }
    }

    /// Your message that starts a turn: one that joined Claude's turn (steered) doesn't.
    pub fn starts_turn(&self) -> bool {
        self.data.kind == Kind::User && self.delivery.as_deref() != Some("steered")
    }
}

/// How a streamed text changed since it was last told.
#[derive(Clone, Debug, PartialEq)]
pub enum TextPatch {
    Same,
    Append(String),
    Set(String),
}

impl TextPatch {
    pub fn apply_to(self, text: &mut String) {
        match self {
            TextPatch::Same => {}
            TextPatch::Append(more) => text.push_str(&more),
            TextPatch::Set(all) => *text = all,
        }
    }
}

/// What changed in the rows, for whoever shows them.
#[derive(Clone, Debug)]
pub enum RowChange {
    /// A new row at the end (its `seq` is the next).
    Added(TranscriptRow),
    /// A row changed: `row` is the whole of it when more than its text did; otherwise only its
    /// text and tool input moved on, as `text` and `input` say.
    Changed { seq: usize, text: TextPatch, input: TextPatch, row: Option<Box<TranscriptRow>> },
}

/// Your message sent while Claude works, waiting for its current step.
#[derive(Clone, Debug)]
pub struct Queued {
    pub id: String,
    pub text: String,
    /// Images (names in `attachments/`), then files.
    pub attachments: Vec<String>,
}

/// An MCP server's question mid-call, waiting for you.
#[derive(Clone, Debug)]
pub struct Elicitation {
    pub id: String,
    pub server: String,
    pub message: String,
    /// `form` or `url`.
    pub mode: String,
    pub schema: serde_json::Value,
    pub url: Option<String>,
    /// A page's question: what its end (`elicitation_complete`) names it by.
    pub elicitation_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Permission {
    pub id: String,
    pub tool: String,
    pub description: String,
    pub input: String,
}

/// What the composer shows of a thread beyond its rows.
#[derive(Clone, Default)]
pub struct ComposerState {
    /// Tokens in the context after Claude's latest message, and the model's window.
    pub context_tokens: Option<u64>,
    pub context_window: Option<u64>,
    /// Queued messages you took back that go back into the composer when the CLI lets go of
    /// them: (id, text, attachments).
    pub restore_on_withdraw: HashSet<String>,
    pub restored: Vec<(String, String, Vec<String>)>,
    /// Queued messages you asked to go now (Claude is being stopped for them).
    pub sending_now: HashSet<String>,
    /// MCP servers as last heard: (name, status, scope).
    pub mcp_servers: Vec<(String, String, String)>,
}

/// Where a run finds rows it doesn't hold: whoever holds the thread's rows (the window's
/// pages, or the database).
pub trait RowSource {
    /// A tool call's row, by its call id.
    fn tool(&self, id: &str) -> Option<TranscriptRow>;
    /// A row by its id.
    fn by_id(&self, id: &str) -> Option<TranscriptRow>;
    /// The Agent call of a sub-agent by its task id: a run that names it, or a result that
    /// does ("agentId: a5b8…", a sub-agent from before a restart).
    fn agent_call(&self, task_id: &str) -> Option<String>;
    /// Whether a command's output with this id is in the transcript.
    fn has_command(&self, id: &str) -> bool;
    /// Your newest message that started a turn.
    fn last_turn_start(&self) -> Option<TranscriptRow>;
}

/// No rows beyond what the run holds.
pub struct NoRows;

impl RowSource for NoRows {
    fn tool(&self, _: &str) -> Option<TranscriptRow> {
        None
    }
    fn by_id(&self, _: &str) -> Option<TranscriptRow> {
        None
    }
    fn agent_call(&self, _: &str) -> Option<String> {
        None
    }
    fn has_command(&self, _: &str) -> bool {
        false
    }
    fn last_turn_start(&self) -> Option<TranscriptRow> {
        None
    }
}

/// What an event changed beyond the rows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Outcome {
    /// What the sidebar or the composer shows changed (a request, the turn's end, jobs).
    pub chrome: bool,
    /// A task tool call got its result: the checklist is worked out again.
    pub plan: bool,
    /// A shell command touched pull requests.
    pub pulls: bool,
    /// The session couldn't be resumed: the thread forgets it.
    pub session_reset: bool,
}

impl Outcome {
    fn chrome(chrome: bool) -> Outcome {
        Outcome { chrome, ..Default::default() }
    }
}

/// What a run knows beyond its rows: told whole after every change, for whoever shows the
/// thread to hold a copy of.
#[derive(Clone, Default)]
pub struct LiveState {
    pub running: bool,
    /// What the CLI is doing between messages, when it says ("Compacting the conversation…").
    pub activity: Option<String>,
    pub session_id: Option<String>,
    /// The session's start (`init`) came from this `claude`: a resume that never got one failed.
    pub received_init: bool,
    /// You pressed Stop: the turn's end is "Interrupted", not an error, and tells nobody.
    pub interrupt_requested: bool,
    /// The turn that just ended was stopped by you (read as it ends).
    pub stopped_by_you: bool,
    /// Requests waiting for you, oldest first: parallel sub-agents can each ask at once. The
    /// composer shows the first; each is answered by its own request id.
    pub permissions: Vec<Permission>,
    /// MCP servers' questions mid-call, oldest first: the composer shows the first.
    pub elicitations: Vec<Elicitation>,
    /// Your messages sent while Claude works, oldest first, until it picks them up: shown above
    /// the composer, not in the transcript.
    pub queue: Vec<Queued>,
    pub composer: ComposerState,
    /// What Claude left running (background commands, watches): never saved.
    pub jobs: Jobs,
    /// Rows in the whole transcript: the next row's number. `None` until known (read from the
    /// database before the first new row).
    pub total: Option<usize>,
    /// Goes up whenever a sub-agent's log changes, for the right sidebar.
    pub subagents_version: u64,

}

impl LiveState {
    /// The request the composer shows: the oldest waiting.
    pub fn permission(&self) -> Option<&Permission> {
        self.permissions.first()
    }

    pub fn waiting_for_you(&self) -> bool {
        !self.permissions.is_empty()
    }

    /// The MCP question the composer shows: the oldest waiting.
    pub fn elicitation(&self) -> Option<&Elicitation> {
        self.elicitations.first()
    }

    /// Queued messages still waiting for Claude, oldest first: (id, text, attachments).
    pub fn queued_messages(&self) -> Vec<(String, String, usize)> {
        self.queue.iter().map(|q| (q.id.clone(), q.text.clone(), q.attachments.len())).collect()
    }
}

#[derive(Default)]
pub struct ThreadRun {
    pub live: LiveState,

    /// The rows held, by number.
    rows: BTreeMap<usize, TranscriptRow>,
    /// The turn's streamed blocks (`message:index`) and tool calls (by id), by row number.
    blocks: HashMap<String, usize>,
    tools: HashMap<String, usize>,
    /// Calls that continued a sub-agent (`SendMessage` to it), by the Agent call that started
    /// it: the CLI reports the continued run under the new call, the run is the same.
    agent_alias: HashMap<String, String>,
    /// Rows changed since the last save.
    dirty: BTreeSet<usize>,
    /// Whole tool results the rows hold only a head of, until saved.
    full_results: HashMap<String, String>,
    /// Changed rows let go of before they were saved: saved with the next changes.
    unsaved: Vec<(usize, Row)>,
    /// Since the last `take_changes`: rows added, rows changed beyond their text, rows whose
    /// text or input was set anew rather than added to, and what was told of each one's text
    /// and input (their lengths).
    added: BTreeSet<usize>,
    changed: BTreeSet<usize>,
    streamed: BTreeSet<usize>,
    told: HashMap<usize, (usize, usize)>,
}

impl ThreadRun {
    pub fn new(session_id: Option<String>, context_tokens: Option<u64>, context_window: Option<u64>) -> ThreadRun {
        let composer = ComposerState { context_tokens, context_window, ..Default::default() };
        ThreadRun { live: LiveState { session_id, composer, ..Default::default() }, ..Default::default() }
    }

    /// Takes a request off the list once answered.
    pub fn answered(&mut self, request_id: &str) -> Option<Permission> {
        let i = self.live.permissions.iter().position(|p| p.id == request_id)?;
        Some(self.live.permissions.remove(i))
    }

    /// Your queued messages go back into the composer: when `claude` is gone with them unread.
    pub fn restore_queued(&mut self) {
        for q in std::mem::take(&mut self.live.queue) {
            self.live.composer.restore_on_withdraw.remove(&q.id);
            self.live.composer.restored.push((q.id, q.text, q.attachments));
        }
    }

    /// Rows held now, for tests and checks.
    pub fn held(&self) -> usize {
        self.rows.len()
    }

    // Rows.

    /// A row changed beyond its text: told whole, and saved.
    fn touch(&mut self, seq: usize) {
        self.dirty.insert(seq);
        self.changed.insert(seq);
    }

    /// A row changed beyond its text: told whole, not saved for it.
    fn show(&mut self, seq: usize) {
        self.changed.insert(seq);
    }

    /// A row's text or input grew: told as what was added, and saved.
    fn stream(&mut self, seq: usize) {
        self.dirty.insert(seq);
        self.streamed.insert(seq);
    }

    /// Puts a new row at the end.
    fn append(&mut self, data: EntryData) -> usize {
        let kind = data.kind;
        let seq = self.live.total.unwrap_or(0);
        self.live.total = Some(seq + 1);
        self.dirty.insert(seq);
        self.added.insert(seq);
        let id = data.id.clone();
        self.rows.insert(seq, TranscriptRow { seq, data, live: false, sub: None, date: Some(now_ms() / 1000.0), delivery: None });
        if kind == Kind::Tool {
            self.tools.insert(id, seq);
        }
        seq
    }

    /// Takes a row it doesn't hold from the source, to change it.
    fn adopt(&mut self, row: TranscriptRow) -> usize {
        let seq = row.seq;
        if self.rows.contains_key(&seq) {
            return seq;
        }
        self.told.insert(seq, (row.data.text.len(), row.data.tool_input.as_ref().map_or(0, String::len)));
        if row.data.kind == Kind::Tool {
            self.tools.insert(row.data.id.clone(), seq);
        }
        self.rows.insert(seq, row);
        seq
    }

    /// A tool call's row by its id: held, or taken from the source.
    fn tool(&mut self, id: &str, src: &dyn RowSource) -> Option<usize> {
        if let Some(seq) = self.tools.get(id) {
            return Some(*seq);
        }
        let row = src.tool(id)?;
        Some(self.adopt(row))
    }

    fn row(&self, seq: usize) -> &TranscriptRow {
        &self.rows[&seq]
    }

    fn row_mut(&mut self, seq: usize) -> &mut TranscriptRow {
        self.rows.get_mut(&seq).expect("a held row")
    }

    /// Your message, as sent: `id` is what the CLI's lifecycle events name it by. Sent while
    /// Claude works (`queued`), it waits above the composer and joins the transcript where
    /// Claude picks it up: never in both places.
    pub fn add_user(&mut self, text: &str, images: &[String], id: &str, queued: bool, delivery: Option<&str>) {
        if queued {
            self.live.queue.push(Queued { id: id.to_string(), text: text.to_string(), attachments: images.to_vec() });
            return;
        }
        let mut data = EntryData::new(id, Kind::User, text);
        data.attachments = images.to_vec();
        let seq = self.append(data);
        self.row_mut(seq).delivery = delivery.map(str::to_string);
        // Jobs started from here on are what Claude waits on.
        self.live.jobs.turn = Some(id.to_string());
    }

    pub fn notice(&mut self, text: &str) {
        self.append(EntryData::new(new_uuid(), Kind::Notice, text));
    }

    /// A notice in the error colour (saved as such: `notice_style` "error").
    pub fn error_notice(&mut self, text: &str) {
        let mut data = EntryData::new(new_uuid(), Kind::Notice, text);
        data.is_error = true;
        self.append(data);
    }

    /// The files git saw a turn change, on the message that started it (by its id).
    pub fn set_changed_files(&mut self, message: &str, files: Vec<String>, src: &dyn RowSource) {
        let held = self.rows.values().find(|r| r.data.id == message).map(|r| r.seq);
        let Some(seq) = held.or_else(|| src.by_id(message).map(|r| self.adopt(r))) else { return };
        self.row_mut(seq).data.changed_files = Some(files);
        self.touch(seq);
    }

    /// The CLI says where your message is. True when the composer has to show it (a queued
    /// message picked up or taken back).
    fn lifecycle(&mut self, id: &str, state: &str) -> bool {
        let Some(at) = self.live.queue.iter().position(|q| q.id == id) else { return false };
        match state {
            "started" | "completed" => {
                // Claude has it now: into the transcript, where it entered the conversation.
                let q = self.live.queue.remove(at);
                let delivery = if self.live.composer.sending_now.remove(id) {
                    Some("interrupted")
                } else if self.live.running {
                    Some("steered")
                } else {
                    // Picked up after the turn ended or was stopped for it: a turn of its own.
                    None
                };
                self.live.running = true;
                self.add_user(&q.text, &q.attachments, &q.id, false, delivery);
                true
            }
            "cancelled" => {
                let q = self.live.queue.remove(at);
                self.live.composer.sending_now.remove(id);
                if self.live.composer.restore_on_withdraw.remove(id) {
                    self.live.composer.restored.push((q.id, q.text, q.attachments));
                }
                true
            }
            _ => false,
        }
    }

    /// Applies one event from the thread's `claude`.
    pub fn apply(&mut self, event: Event, src: &dyn RowSource) -> Outcome {
        match event {
            Event::Session { id } => {
                self.live.session_id = Some(id);
                self.live.received_init = true;
                Outcome::default()
            }
            Event::Status(status) => {
                let activity = (status == "compacting").then(|| "Compacting the conversation…".to_string());
                let changed = activity != self.live.activity;
                self.live.activity = activity;
                Outcome::chrome(changed)
            }
            Event::SubagentProgress { tool_id, activity, usage } => {
                let tool_id = self.agent_alias.get(&tool_id).cloned().unwrap_or(tool_id);
                if let Some(a) = &activity {
                    self.live.jobs.agent_activity(&tool_id, a);
                }
                if let Some(seq) = self.tool(&tool_id, src) {
                    let r = self.row_mut(seq);
                    if !r.is_agent() {
                        return Outcome::default();
                    }
                    let sub = r.sub.get_or_insert_with(|| Subagent::new(String::new()));
                    if activity.is_some() {
                        sub.activity = activity;
                    }
                    sub.apply(usage);
                    self.live.subagents_version += 1;
                    self.show(seq);
                }
                Outcome::default()
            }
            // Answered by whoever holds the process, which takes it off the list.
            Event::ElicitationComplete { .. } => Outcome::default(),
            Event::Lifecycle { id, state } => Outcome::chrome(self.lifecycle(&id, &state)),
            Event::CommandOutput { text, id } => {
                // After `/compact` the CLI sends the outputs it kept again, same `uuid`: shown once.
                let seen = id.as_deref().is_some_and(|id| self.rows.values().any(|r| r.data.kind == Kind::Command && r.data.id == id) || src.has_command(id));
                if seen {
                    return Outcome::default();
                }
                let id = id.unwrap_or_else(new_uuid);
                self.append(EntryData::new(id, Kind::Command, &text));
                Outcome::default()
            }
            Event::ContextUsage(tokens) => {
                self.live.composer.context_tokens = Some(tokens);
                Outcome::default()
            }
            Event::ContextWindows(windows) => {
                self.live.composer.context_window = windows.iter().map(|(_, w)| *w).max();
                Outcome::default()
            }
            Event::ConversationReset => {
                self.notice("Context cleared. Claude starts fresh from here.");
                self.live.composer.context_tokens = None;
                Outcome::chrome(true)
            }
            Event::McpServers(servers) => {
                let changed = self.live.composer.mcp_servers != servers;
                self.live.composer.mcp_servers = servers;
                Outcome::chrome(changed)
            }
            Event::TitleChanged(_) | Event::Skills(_) => Outcome::default(),
            Event::Catalog { .. } | Event::Limits(_) => Outcome::default(),
            Event::BlockStart { message, index, parent, kind } => {
                if let Some(parent) = parent {
                    self.subagent_step(&parent, src);
                    return Outcome::default();
                }
                let key = format!("{message}:{index}");
                if self.blocks.contains_key(&key) {
                    return Outcome::default();
                }
                let data = match kind {
                    BlockKind::Text => EntryData::new(key.clone(), Kind::Assistant, ""),
                    BlockKind::Thinking => EntryData::new(key.clone(), Kind::Thinking, ""),
                    BlockKind::Tool { id, name } => {
                        let mut d = EntryData::new(id, Kind::Tool, "");
                        d.tool_name = Some(name);
                        d
                    }
                };
                let seq = self.append(data);
                self.row_mut(seq).live = true;
                self.blocks.insert(key, seq);
                // Claude woken by a job's end: a turn with no message of yours.
                let woke = !self.live.running && self.live.jobs.version > 0;
                if woke {
                    self.live.running = true;
                }
                Outcome::chrome(woke)
            }
            Event::Delta { message, index, parent, kind, text } => {
                if parent.is_some() {
                    return Outcome::default();
                }
                let key = format!("{message}:{index}");
                let seq = match self.blocks.get(&key) {
                    Some(seq) => *seq,
                    None => {
                        let k = if kind == DeltaKind::Thinking { Kind::Thinking } else { Kind::Assistant };
                        let seq = self.append(EntryData::new(key.clone(), k, ""));
                        self.row_mut(seq).live = true;
                        self.blocks.insert(key, seq);
                        seq
                    }
                };
                let r = self.row_mut(seq);
                if kind == DeltaKind::Input {
                    r.data.tool_input.get_or_insert_with(String::new).push_str(&text);
                } else {
                    r.data.text.push_str(&text);
                }
                self.stream(seq);
                Outcome::default()
            }
            Event::Assistant { message, parent, blocks } => {
                if let Some(parent) = parent {
                    for block in &blocks {
                        if matches!(block, Block::Tool { .. }) {
                            self.subagent_step(&parent, src);
                        }
                    }
                    self.subagent_log(&parent, src, |log| {
                        for block in blocks {
                            let step = match block {
                                Block::Text(text) => SubStep { tool: None, id: String::new(), text, result: None, is_error: false },
                                Block::Tool { id, name, input } => SubStep { tool: Some(name), id, text: input, result: None, is_error: false },
                                Block::Thinking(_) => continue,
                            };
                            // The CLI may send a block again as it grows: keep the latest.
                            match log.iter_mut().rev().find(|s| (step.tool.is_some() && s.id == step.id) || (step.tool.is_none() && s.tool.is_none() && step.text.starts_with(&s.text))) {
                                Some(existing) if step.tool.is_some() || !existing.text.is_empty() => {
                                    existing.text = step.text;
                                }
                                _ => log.push(step),
                            }
                        }
                    });
                    return Outcome::default();
                }
                // The CLI sends a message's blocks one `assistant` event each, all under the
                // message's ID: a tool block is its tool call; a text or thinking block is the
                // message's streamed block of that kind still being written, else one that
                // already says the same, else a new row.
                for (index, block) in blocks.into_iter().enumerate() {
                    let mut key = format!("{message}:{index}");
                    let existing = match &block {
                        Block::Tool { id, .. } => self.tool(id, src),
                        Block::Text(text) | Block::Thinking(text) => {
                            let kind = if matches!(block, Block::Text(_)) { Kind::Assistant } else { Kind::Thinking };
                            let prefix = format!("{message}:");
                            let mut candidates: Vec<(&String, &usize)> = self.blocks.iter().filter(|(k, seq)| k.starts_with(&prefix) && self.rows[seq].data.kind == kind).collect();
                            candidates.sort_by_key(|(k, _)| k.rsplit(':').next().and_then(|n| n.parse::<u64>().ok()).unwrap_or(0));
                            let found = candidates
                                .iter()
                                .find(|(_, seq)| self.rows[seq].live)
                                .or_else(|| candidates.iter().find(|(_, seq)| self.rows[seq].data.text == *text))
                                .map(|(k, seq)| ((*k).clone(), **seq));
                            match found {
                                Some((k, seq)) => {
                                    key = k;
                                    Some(seq)
                                }
                                // A new block of this message: a key no stream block has.
                                None => {
                                    key = format!("{message}:a{}", candidates.len() + index);
                                    None
                                }
                            }
                        }
                    };
                    let seq = match existing {
                        Some(seq) => seq,
                        None => {
                            let data = match &block {
                                Block::Text(_) => EntryData::new(key.clone(), Kind::Assistant, ""),
                                Block::Thinking(_) => EntryData::new(key.clone(), Kind::Thinking, ""),
                                Block::Tool { id, .. } => EntryData::new(id.clone(), Kind::Tool, ""),
                            };
                            let seq = self.append(data);
                            self.blocks.insert(key, seq);
                            seq
                        }
                    };
                    let r = self.row_mut(seq);
                    match block {
                        Block::Text(t) | Block::Thinking(t) => r.data.text = t,
                        Block::Tool { name, input, .. } => {
                            r.data.tool_name = Some(name);
                            r.data.tool_input = Some(input);
                        }
                    }
                    r.live = false;
                    self.touch(seq);
                }
                Outcome::default()
            }
            Event::ToolResults { parent, results } => {
                let pulls = self.touches_pulls(parent.as_deref(), &results, src);
                if let Some(parent) = parent {
                    self.subagent_log(&parent, src, |log| {
                        for r in results {
                            if let Some(step) = log.iter_mut().rev().find(|s| s.tool.is_some() && s.id == r.id) {
                                step.result = Some(r.text);
                                step.is_error = r.is_error;
                            }
                        }
                    });
                    return Outcome { pulls, ..Default::default() };
                }
                let mut touches_plan = false;
                for r in &results {
                    if let Some(seq) = self.tool(&r.id, src) {
                        let d = &self.row(seq).data;
                        touches_plan |= d.tool_name.as_deref().is_some_and(|n| crate::plan::is_plan_tool(n, d.tool_input.as_deref().unwrap_or("")));
                    }
                }
                let jobs_before = self.live.jobs.running.len();
                for r in results {
                    self.live.jobs.tool_result(&r.id, &r.text);
                    let Some(seq) = self.tool(&r.id, src) else { continue };
                    if let Some(full) = r.full {
                        self.full_results.insert(r.id.clone(), full);
                    }
                    let row = self.row_mut(seq);
                    row.data.tool_result = Some(r.text);
                    row.data.result_len = r.len;
                    row.data.is_error = r.is_error;
                    row.live = false;
                    self.touch(seq);
                }
                if touches_plan {
                    return Outcome { chrome: true, plan: true, pulls, ..Default::default() };
                }
                Outcome { chrome: self.live.jobs.running.len() != jobs_before, pulls, ..Default::default() }
            }
            Event::SubagentStarted { tool_id, description, task_id, background } => {
                // A sub-agent continued (`SendMessage` to it): the same task under a new call.
                // Its steps keep coming under the call that started it; the run goes on there.
                let original = task_id.as_deref().and_then(|t| self.agent_call_of(t, src)).filter(|call| *call != tool_id);
                let resumed = original.is_some();
                let tool_id = match original {
                    Some(call) => {
                        self.agent_alias.insert(tool_id, call.clone());
                        call
                    }
                    None => tool_id,
                };
                // In the background it goes on after the turn: the thread isn't done.
                if background {
                    self.live.jobs.start_agent(task_id.clone(), tool_id.clone(), description.clone());
                }
                if let Some(seq) = self.tool(&tool_id, src) {
                    let r = self.row_mut(seq);
                    // A task the CLI started for a call that isn't an agent's: none of ours.
                    if !r.is_agent() {
                        return Outcome::chrome(background);
                    }
                    // Keep what it already did when the start comes after its first steps.
                    match &mut r.sub {
                        Some(sub) => sub.description = description,
                        None => r.sub = Some(Subagent::new(description)),
                    }
                    if let Some(sub) = &mut r.sub {
                        sub.task_id = task_id.or(sub.task_id.take());
                        if resumed {
                            sub.resume();
                        }
                    }
                    if resumed {
                        self.touch(seq);
                    } else {
                        self.show(seq);
                    }
                    self.live.subagents_version += 1;
                }
                Outcome::chrome(background)
            }
            Event::SubagentFinished { tool_id, status, summary, usage } => {
                let tool_id = self.agent_alias.get(&tool_id).cloned().unwrap_or(tool_id);
                let was_background = self.live.jobs.finish_agent(&tool_id);
                if let Some(seq) = self.tool(&tool_id, src) {
                    let r = self.row_mut(seq);
                    // A job's end the CLI no longer knew as one (a watch from before a
                    // restart): never made a sub-agent of it.
                    if !r.is_agent() {
                        return Outcome::chrome(was_background);
                    }
                    let sub = r.sub.get_or_insert_with(|| Subagent::new(String::new()));
                    sub.status = status;
                    sub.activity = None;
                    sub.apply(usage);
                    if summary.is_some() {
                        sub.report = summary;
                    }
                    self.touch(seq);
                }
                Outcome::chrome(was_background)
            }
            Event::Elicitation { request_id, server, message, mode, schema, url, elicitation_id } => {
                // Several servers (or calls) can ask at once: each waits its turn.
                let asked = Elicitation { id: request_id, server, message, mode, schema, url, elicitation_id };
                match self.live.elicitations.iter_mut().find(|e| e.id == asked.id) {
                    Some(e) => *e = asked,
                    None => self.live.elicitations.push(asked),
                }
                Outcome::chrome(true)
            }
            Event::Permission { request_id, tool, description, input } => {
                // The same request again (a resumed stream) replaces itself; a new one waits its turn.
                let asked = Permission { id: request_id, tool, description, input };
                match self.live.permissions.iter_mut().find(|p| p.id == asked.id) {
                    Some(p) => *p = asked,
                    None => self.live.permissions.push(asked),
                }
                Outcome::chrome(true)
            }
            Event::Notice(text) => {
                self.notice(&text);
                Outcome::default()
            }
            Event::Done { is_error, text } => {
                self.end_turn(src);
                // Stopped for a message sent now, which carries that on itself; stopped by you,
                // a plain note (the CLI ends it as `error_during_execution`); anything else, the
                // CLI's error.
                let for_message = !self.live.composer.sending_now.is_empty();
                if is_error && !for_message {
                    if self.live.interrupt_requested {
                        self.notice("Interrupted");
                    } else if text.is_empty() || (text.starts_with("error_") && !text.contains(' ')) {
                        let why = if text.is_empty() { String::new() } else { format!(" ({text})") };
                        self.error_notice(&format!("Claude stopped with an error{why}."));
                    } else {
                        self.error_notice(&text);
                    }
                }
                self.live.stopped_by_you = self.live.interrupt_requested || for_message;
                self.live.interrupt_requested = false;
                Outcome::chrome(true)
            }
            Event::JobStarted { task_id, tool_id, description, background, parent_task } => {
                // A sub-agent's call isn't among the thread's tools: its log has it.
                let own = tool_id.as_ref().and_then(|id| self.tool(id, src)).and_then(|seq| self.row(seq).data.tool_name.clone());
                let tool = own.or_else(|| {
                    let id = tool_id.as_deref()?;
                    let call = parent_task.as_deref().and_then(|t| self.agent_call_of(t, src))?;
                    let seq = self.tool(&call, src)?;
                    self.row(seq).sub.as_ref()?.log.iter().rev().find(|s| s.id == id).and_then(|s| s.tool.clone())
                });
                let owner = parent_task.map(|t| {
                    let call = self.agent_call_of(&t, src);
                    (t, call)
                });
                // A sub-agent's: the Sub-agents panel lists it under that agent.
                if owner.is_some() {
                    self.live.subagents_version += 1;
                }
                self.live.jobs.start(task_id, tool_id, tool.as_deref(), description, background, owner);
                Outcome::chrome(true)
            }
            Event::JobFinished { task_id, status, summary, output_file } => {
                if self.live.jobs.running.iter().any(|j| j.id == task_id && j.owner_task.is_some()) {
                    self.live.subagents_version += 1;
                }
                let code = summary.as_deref().and_then(crate::claude::job_exit_code);
                match self.live.jobs.finish(&task_id, &status, code, output_file) {
                    Some((note, true)) => self.error_notice(&note),
                    Some((note, false)) => self.notice(&note),
                    None => {}
                }
                Outcome::chrome(true)
            }
            Event::JobOutput { task_id, text } => {
                self.live.jobs.saw(&task_id, &text);
                Outcome::default()
            }
            Event::JobsRunning(ids) => {
                let notes = self.live.jobs.keep_only(&ids);
                for note in &notes {
                    self.notice(note);
                }
                Outcome::chrome(!notes.is_empty())
            }
            Event::AgentsRunning(ids) => {
                let gone = self.live.jobs.keep_agents(&ids);
                for (tool, note) in &gone {
                    self.agent_stopped(tool, src);
                    self.notice(note);
                }
                Outcome::chrome(!gone.is_empty())
            }
            Event::Exit { code, tail } => {
                self.cut_off_jobs("stopped when Claude exited", src);
                self.end_turn(src);
                // Whatever still waited never reached Claude: back to the composer.
                self.restore_queued();
                self.live.composer.sending_now.clear();
                self.live.interrupt_requested = false;
                self.live.elicitations.clear();
                let mut session_reset = false;
                if code != 0 && !self.live.received_init && self.live.session_id.is_some() {
                    // Its session couldn't be resumed (gone, or another folder's): start afresh.
                    self.live.session_id = None;
                    session_reset = true;
                    self.error_notice("Couldn't resume the previous session. Your next message starts a new one.");
                } else if code != 0 {
                    let detail: Vec<&str> = tail.lines().filter(|l| !l.trim().is_empty()).collect();
                    let detail = detail[detail.len().saturating_sub(3)..].join("\n");
                    self.error_notice(format!("Claude exited (code {code}). {detail}").trim_end());
                }
                self.live.received_init = false;
                Outcome { chrome: true, session_reset, ..Default::default() }
            }
        }
    }

    /// A `Bash` call among the results (the thread's own, or a sub-agent's) that touched PRs
    /// and didn't fail.
    fn touches_pulls(&mut self, parent: Option<&str>, results: &[crate::claude::ToolResult], src: &dyn RowSource) -> bool {
        results.iter().filter(|r| !r.is_error).any(|r| {
            let input = match parent {
                None => self.tool(&r.id, src).and_then(|seq| {
                    let d = &self.row(seq).data;
                    (d.tool_name.as_deref() == Some("Bash")).then(|| d.tool_input.clone()).flatten()
                }),
                Some(p) => self
                    .tool(p, src)
                    .and_then(|seq| self.row(seq).sub.as_ref().and_then(|s| s.log.iter().find(|s| s.id == r.id && s.tool.as_deref() == Some("Bash")).map(|s| s.text.clone()))),
            };
            input.is_some_and(|i| crate::forge::touches_pull_requests(&i))
        })
    }

    /// Its `claude` is going: the jobs it ran go with it, the thread says which, and the
    /// sub-agents among them show as stopped. True when there were any.
    pub fn cut_off_jobs(&mut self, outcome: &str, src: &dyn RowSource) -> bool {
        let agents: Vec<String> = self.live.jobs.agents.iter().filter_map(|j| j.tool_id.clone()).collect();
        let notes = self.live.jobs.cut_off(outcome);
        for tool in &agents {
            self.agent_stopped(tool, src);
        }
        for note in &notes {
            self.notice(note);
        }
        !notes.is_empty() || !agents.is_empty()
    }

    /// The Agent call of a sub-agent, by its task ID: a background one from the jobs, a
    /// foreground one from its call's start, one from before a restart from its call's result.
    fn agent_call_of(&self, task_id: &str, src: &dyn RowSource) -> Option<String> {
        self.live.jobs
            .agent_call(task_id)
            .or_else(|| self.rows.values().find(|r| r.sub.as_ref().is_some_and(|s| s.task_id.as_deref() == Some(task_id))).map(|r| r.data.id.clone()))
            .or_else(|| {
                let named = format!("agentId: {task_id}");
                self.rows
                    .values()
                    .find(|r| r.data.tool_name.as_deref().is_some_and(|n| AGENT_TOOLS.contains(&n)) && r.data.tool_result.as_deref().is_some_and(|t| t.contains(&named)))
                    .map(|r| r.data.id.clone())
            })
            .or_else(|| src.agent_call(task_id))
    }

    /// A background sub-agent that ended without word: its call says it stopped.
    fn agent_stopped(&mut self, tool_id: &str, src: &dyn RowSource) {
        let Some(seq) = self.tool(tool_id, src) else { return };
        let r = self.row_mut(seq);
        let Some(sub) = r.sub.as_mut().filter(|s| s.status == "running") else { return };
        sub.status = "stopped".into();
        sub.activity = None;
        self.live.subagents_version += 1;
        self.touch(seq);
    }

    fn subagent_step(&mut self, parent: &str, src: &dyn RowSource) {
        let Some(seq) = self.tool(parent, src) else { return };
        let sub = self.row_mut(seq).sub.get_or_insert_with(|| Subagent::new("Sub-agent".into()));
        sub.steps += 1;
        self.touch(seq);
    }

    /// Changes what a sub-agent did (its log), saved with its call (the `subagent` column).
    fn subagent_log(&mut self, parent: &str, src: &dyn RowSource, change: impl FnOnce(&mut Vec<SubStep>)) {
        let Some(seq) = self.tool(parent, src) else { return };
        let sub = self.row_mut(seq).sub.get_or_insert_with(|| Subagent::new("Sub-agent".into()));
        change(&mut sub.log);
        if sub.log.len() > SUBAGENT_LOG {
            let extra = sub.log.len() - SUBAGENT_LOG;
            sub.log.drain(..extra);
        }
        self.live.subagents_version += 1;
        self.touch(seq);
    }

    fn end_turn(&mut self, src: &dyn RowSource) {
        let live: Vec<usize> = self.blocks.values().copied().filter(|seq| self.rows[seq].live).collect();
        for seq in live {
            self.row_mut(seq).live = false;
            self.touch(seq);
        }
        self.blocks.clear();
        self.live.running = false;
        self.live.activity = None;
        self.live.permissions.clear();
        // How long the turn took goes on the message that started it: the newest one, held
        // or not.
        let held = self.rows.values().rev().find(|r| r.starts_turn()).map(|r| r.seq);
        let start = match (held, src.last_turn_start()) {
            (Some(h), Some(other)) if other.seq > h => Some(self.adopt(other)),
            (Some(h), _) => Some(h),
            (None, Some(other)) => Some(self.adopt(other)),
            (None, None) => None,
        };
        if let Some(seq) = start {
            self.row_mut(seq).data.turn_ended_at = Some(now_ms() / 1000.0);
            self.touch(seq);
        }
    }

    // What comes out.

    /// What changed in the rows since the last call, oldest first: new rows whole, changed
    /// ones whole or as what their text gained.
    pub fn take_changes(&mut self) -> Vec<RowChange> {
        let mut out = vec![];
        let added = std::mem::take(&mut self.added);
        for seq in &added {
            if let Some(r) = self.rows.get(seq) {
                self.told.insert(*seq, (r.data.text.len(), r.data.tool_input.as_ref().map_or(0, String::len)));
                out.push(RowChange::Added(r.clone()));
            }
        }
        let changed = std::mem::take(&mut self.changed);
        let streamed = std::mem::take(&mut self.streamed);
        for seq in changed.union(&streamed) {
            if added.contains(seq) {
                continue;
            }
            let Some(r) = self.rows.get(seq) else { continue };
            let lens = (r.data.text.len(), r.data.tool_input.as_ref().map_or(0, String::len));
            let (text_was, input_was) = self.told.insert(*seq, lens).unwrap_or(lens);
            if changed.contains(seq) {
                out.push(RowChange::Changed { seq: *seq, text: TextPatch::Same, input: TextPatch::Same, row: Some(Box::new(r.clone())) });
                continue;
            }
            let patch = |now: &str, was: usize| {
                if now.len() == was {
                    TextPatch::Same
                } else if now.len() > was && now.is_char_boundary(was) {
                    TextPatch::Append(now[was..].to_string())
                } else {
                    TextPatch::Set(now.to_string())
                }
            };
            let text = patch(&r.data.text, text_was);
            let input = patch(r.data.tool_input.as_deref().unwrap_or(""), input_was);
            out.push(RowChange::Changed { seq: *seq, text, input, row: None });
        }
        out
    }

    /// Rows changed since the last call, for saving: (row number, row). A row still being
    /// written is saved as it is and again when it's done. Rows it no longer needs are let go
    /// of after (`take_changes` first, so nothing changed is let go of untold).
    pub fn take_saves(&mut self) -> Vec<(usize, Row)> {
        let mut out = std::mem::take(&mut self.unsaved);
        let dirty = std::mem::take(&mut self.dirty);
        for seq in dirty {
            let Some(r) = self.rows.get(&seq) else { continue };
            let full = if r.live { None } else { self.full_results.remove(&r.data.id) };
            out.push((seq, r.row(full)));
        }
        self.let_go();
        out
    }

    /// Lets go of rows nothing will change: not being written, saved, not one of the turn's
    /// blocks, not a call waiting for its result, not a sub-agent or job still running.
    fn let_go(&mut self) {
        if !self.added.is_empty() || !self.changed.is_empty() || !self.streamed.is_empty() {
            return;
        }
        let in_turn: HashSet<usize> = self.blocks.values().copied().collect();
        let running_tools: HashSet<&str> = self.live.jobs.running.iter().chain(self.live.jobs.agents.iter()).filter_map(|j| j.tool_id.as_deref()).collect();
        let keep = |r: &TranscriptRow| {
            r.live
                || in_turn.contains(&r.seq)
                || (r.data.kind == Kind::Tool && r.data.tool_result.is_none())
                || r.sub.as_ref().is_some_and(|s| s.status == "running")
                || running_tools.contains(r.data.id.as_str())
        };
        let gone: Vec<usize> = self.rows.values().filter(|r| !self.dirty.contains(&r.seq) && !keep(r)).map(|r| r.seq).collect();
        for seq in gone {
            if let Some(r) = self.rows.remove(&seq) {
                if r.data.kind == Kind::Tool && self.tools.get(&r.data.id) == Some(&seq) {
                    self.tools.remove(&r.data.id);
                }
                self.told.remove(&seq);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::golden::{self, Step};

    /// The rows as whoever shows the thread holds them: built only from the changes told.
    #[derive(Default)]
    struct Mirror(BTreeMap<usize, TranscriptRow>);

    impl Mirror {
        fn apply(&mut self, changes: Vec<RowChange>) {
            for change in changes {
                match change {
                    RowChange::Added(r) => {
                        assert!(self.0.insert(r.seq, r).is_none(), "a row added twice");
                    }
                    RowChange::Changed { seq, text, input, row } => match row {
                        Some(r) => {
                            self.0.insert(seq, *r);
                        }
                        None => {
                            let r = self.0.get_mut(&seq).expect("a change to a row never added");
                            text.apply_to(&mut r.data.text);
                            if input != TextPatch::Same {
                                input.apply_to(r.data.tool_input.get_or_insert_with(String::new));
                            }
                        }
                    },
                }
            }
        }
    }

    impl RowSource for Mirror {
        fn tool(&self, id: &str) -> Option<TranscriptRow> {
            self.0.values().find(|r| r.data.kind == Kind::Tool && r.data.id == id).cloned()
        }
        fn by_id(&self, id: &str) -> Option<TranscriptRow> {
            self.0.values().find(|r| r.data.id == id).cloned()
        }
        fn agent_call(&self, task_id: &str) -> Option<String> {
            let named = format!("agentId: {task_id}");
            self.0
                .values()
                .find(|r| r.sub.as_ref().is_some_and(|s| s.task_id.as_deref() == Some(task_id)) || (r.is_agent() && r.data.tool_result.as_deref().is_some_and(|t| t.contains(&named))))
                .map(|r| r.data.id.clone())
        }
        fn has_command(&self, id: &str) -> bool {
            self.0.values().any(|r| r.data.kind == Kind::Command && r.data.id == id)
        }
        fn last_turn_start(&self) -> Option<TranscriptRow> {
            self.0.values().rev().find(|r| r.starts_turn()).cloned()
        }
    }

    /// The same as the app's thread does with a step (`ThreadState`'s golden test), saving and
    /// letting go of rows after each, the mirror standing in for the window's rows.
    fn golden(steps: Vec<Step>) -> String {
        let mut run = ThreadRun { live: LiveState { total: Some(0), ..Default::default() }, ..Default::default() };
        let mut mirror = Mirror::default();
        let mut saved: Vec<(usize, Row)> = vec![];
        for step in steps {
            match step {
                Step::Event(e) => {
                    run.apply(e, &mirror);
                }
                Step::User { id, text, queued } => {
                    let running = run.live.running;
                    run.add_user(text, &[], id, queued && running, None);
                    run.live.running = true;
                }
                Step::Interrupt => run.live.interrupt_requested = true,
                Step::ChangedFiles { message, files } => run.set_changed_files(message, files.into_iter().map(str::to_string).collect(), &mirror),
            }
            mirror.apply(run.take_changes());
            for (seq, row) in run.take_saves() {
                saved.retain(|(s, _)| *s != seq);
                saved.push((seq, row));
            }
        }
        // What the changes told is what was saved (a sub-agent's progress is told, and saved
        // only with its next step, as before).
        for (seq, row) in &saved {
            let shown = mirror.0.get(seq).expect("a saved row never told").row(None);
            assert_eq!((&shown.text, &shown.tool_input, &shown.tool_result, &shown.delivery, &shown.changed_files), (&row.text, &row.tool_input, &row.tool_result, &row.delivery, &row.changed_files), "row {seq}");
        }
        let ids = |v: Vec<String>| v.join(",");
        let state = vec![
            format!("running {} activity {:?} session {:?} meta session {:?} stopped by you {}", run.live.running, run.live.activity, run.live.session_id, None::<String>, run.live.stopped_by_you),
            format!("permissions [{}] elicitations [{}]", ids(run.live.permissions.iter().map(|p| format!("{}:{}", p.id, p.description)).collect()), ids(run.live.elicitations.iter().map(|e| e.id.clone()).collect())),
            format!("queue [{}] restored [{}]", ids(run.live.queue.iter().map(|q| q.id.clone()).collect()), ids(run.live.composer.restored.iter().map(|r| r.0.clone()).collect())),
            format!("context {:?}/{:?} mcp {:?}", run.live.composer.context_tokens, run.live.composer.context_window, run.live.composer.mcp_servers),
            format!("jobs running {} agents {} ended {} total {:?}", run.live.jobs.running.len(), run.live.jobs.agents.len(), run.live.jobs.ended.len(), run.live.total),
        ];
        golden::describe(&saved, &state)
    }

    /// The golden files' rows and state, less what only the app's thread knows (its meta's
    /// session, its list of sub-agents).
    fn expected_without_app_lines(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/expected").join(format!("{name}.txt"));
        std::fs::read_to_string(path).unwrap().lines().filter(|l| !l.starts_with("= subagents")).map(|l| format!("{l}\n")).collect()
    }

    #[test]
    fn saves_what_the_app_saved() {
        let mut cases: Vec<(&str, Vec<Step>)> = golden::SESSIONS.iter().map(|(name, text)| (*name, golden::session_events(text).into_iter().map(Step::Event).collect())).collect();
        cases.push(("synthetic", golden::synthetic()));
        for (name, steps) in cases {
            let actual = golden(steps);
            // The meta's session is the app's: the run's own is said in the same line.
            let expected: String = expected_without_app_lines(name)
                .lines()
                .map(|l| match l.find(" meta session ") {
                    Some(i) => {
                        let rest = &l[i + " meta session ".len()..];
                        let after = rest.find(" stopped by you").map(|j| &rest[j..]).unwrap_or("");
                        format!("{} meta session None{after}\n", &l[..i])
                    }
                    None => format!("{l}\n"),
                })
                .collect();
            assert_eq!(actual, expected, "{name}");
        }
    }

    #[test]
    fn streams_text_as_what_it_gained() {
        let mut run = ThreadRun { live: LiveState { total: Some(0), ..Default::default() }, ..Default::default() };
        run.apply(Event::BlockStart { message: "m".into(), index: 0, parent: None, kind: BlockKind::Text }, &NoRows);
        assert!(matches!(run.take_changes().as_slice(), [RowChange::Added(r)] if r.live && r.data.text.is_empty()));
        for word in ["Hello", ", world"] {
            run.apply(Event::Delta { message: "m".into(), index: 0, parent: None, kind: DeltaKind::Text, text: word.into() }, &NoRows);
            match run.take_changes().as_slice() {
                [RowChange::Changed { seq: 0, text: TextPatch::Append(t), input: TextPatch::Same, row: None }] => assert_eq!(t, word),
                other => panic!("{other:?}"),
            }
        }
        // The whole message: told whole, its text the same.
        run.apply(Event::Assistant { message: "m".into(), parent: None, blocks: vec![Block::Text("Hello, world".into())] }, &NoRows);
        match run.take_changes().as_slice() {
            [RowChange::Changed { row: Some(r), .. }] => assert!(!r.live && r.data.text == "Hello, world"),
            other => panic!("{other:?}"),
        }
        assert!(run.take_changes().is_empty());
    }

    #[test]
    fn holds_only_what_it_may_still_change() {
        let mut run = ThreadRun { live: LiveState { total: Some(0), ..Default::default() }, ..Default::default() };
        run.live.running = true;
        run.add_user("go", &[], "u", false, None);
        run.apply(Event::Assistant { message: "m".into(), parent: None, blocks: vec![Block::Tool { id: "a".into(), name: "Agent".into(), input: "{}".into() }] }, &NoRows);
        run.apply(Event::SubagentStarted { tool_id: "a".into(), description: "bg".into(), task_id: Some("k".into()), background: true }, &NoRows);
        run.apply(Event::ToolResults { parent: None, results: vec![crate::claude::ToolResult { id: "a".into(), text: "started".into(), len: 7, is_error: false, full: None }] }, &NoRows);
        run.apply(Event::Assistant { message: "m2".into(), parent: None, blocks: vec![Block::Text("Started it.".into())] }, &NoRows);
        run.apply(Event::Done { is_error: false, text: String::new() }, &NoRows);
        run.take_changes();
        assert_eq!(run.take_saves().len(), 3);
        // The turn is over: only the background sub-agent's call is held.
        assert_eq!(run.held(), 1);
        run.apply(Event::SubagentFinished { tool_id: "a".into(), status: "completed".into(), summary: Some("7".into()), usage: Default::default() }, &NoRows);
        match run.take_changes().as_slice() {
            [RowChange::Changed { seq: 1, row: Some(r), .. }] => assert_eq!(r.sub.as_ref().map(|s| s.status.as_str()), Some("completed")),
            other => panic!("{other:?}"),
        }
        assert_eq!(run.take_saves().len(), 1);
        assert_eq!(run.held(), 0);
    }

    /// A row it let go of comes back from the source when an event names it.
    #[test]
    fn asks_the_source_for_rows_it_let_go_of() {
        struct One(TranscriptRow);
        impl RowSource for One {
            fn tool(&self, id: &str) -> Option<TranscriptRow> {
                (self.0.data.id == id).then(|| self.0.clone())
            }
            fn by_id(&self, id: &str) -> Option<TranscriptRow> {
                self.tool(id)
            }
            fn agent_call(&self, task_id: &str) -> Option<String> {
                self.0.sub.as_ref().filter(|s| s.task_id.as_deref() == Some(task_id)).map(|_| self.0.data.id.clone())
            }
            fn has_command(&self, _: &str) -> bool {
                false
            }
            fn last_turn_start(&self) -> Option<TranscriptRow> {
                None
            }
        }
        let mut data = EntryData::new("a", Kind::Tool, "");
        data.tool_name = Some("Agent".into());
        data.tool_result = Some("agentId: k".into());
        let mut row = TranscriptRow::saved(data, 4);
        let mut sub = Subagent::new("old".into());
        sub.status = "completed".into();
        sub.task_id = Some("k".into());
        row.sub = Some(sub);
        let src = One(row);
        let mut run = ThreadRun { live: LiveState { total: Some(9), ..Default::default() }, ..Default::default() };
        // Continued under a new call: the run goes on in the old one.
        run.apply(Event::SubagentStarted { tool_id: "b".into(), description: "again".into(), task_id: Some("k".into()), background: true }, &src);
        match run.take_changes().as_slice() {
            [RowChange::Changed { seq: 4, row: Some(r), .. }] => assert_eq!(r.sub.as_ref().map(|s| (s.status.as_str(), s.description.as_str())), Some(("running", "again"))),
            other => panic!("{other:?}"),
        }
        assert_eq!(run.take_saves().iter().map(|(s, _)| *s).collect::<Vec<_>>(), vec![4]);
    }
}
