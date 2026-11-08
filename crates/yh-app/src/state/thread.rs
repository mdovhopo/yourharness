//! A thread's rows as the app holds them: only the newest page or two of a long history;
//! earlier pages are read when you scroll up to them. A streamed delta changes its entry in
//! place and tells the list to measure just that row again (`ListState::splice`).

use yh_core::claude::{Block, BlockKind, DeltaKind, Event};
use yh_core::db::Row;
use yh_core::library::{EntryData, Kind, ThreadMeta};
use yh_core::markdown::Markdown;
use gpui_kit::{px, Context, ListAlignment, ListOffset, ListState};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

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

const SUBAGENT_LOG: usize = 200;

impl Subagent {
    fn new(description: String) -> Subagent {
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
            started_at: yh_core::library::now_ms() / 1000.0,
            duration_ms: None,
        }
    }

    /// As saved (the Swift app's `SubagentSnapshot`): with what it was asked, from its call.
    fn record(&self, input: Option<&str>) -> yh_core::subagent_record::SubagentRecord {
        use yh_core::subagent_record::{SubagentEntry, SubagentRecord};
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
    fn from_record(r: yh_core::subagent_record::SubagentRecord) -> Subagent {
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
        }
    }

    fn apply(&mut self, usage: yh_core::claude::SubagentUsage) {
        self.tokens = usage.tokens.or(self.tokens);
        self.tool_uses = usage.tool_uses.or(self.tool_uses);
        self.duration_ms = usage.duration_ms.or(self.duration_ms);
    }
}

pub struct Entry {
    pub data: EntryData,
    /// Still being written by Claude.
    pub live: bool,
    pub sub: Option<Subagent>,
    /// Bumped on every change: what's cached from it (markdown) is kept while it matches.
    pub version: u64,
    /// A step opened in its work row.
    pub open: bool,
    /// Its text parsed, for (version, links revision).
    pub markdown: Option<(u64, u64, Rc<Markdown>)>,
    /// Its row number in the whole saved transcript.
    pub seq: usize,
    /// When it was written (seconds since 1970), for rows made here.
    pub date: Option<f64>,


    /// How your message reached Claude: `steered` (joined the turn), `interrupted` (stopped it).
    pub delivery: Option<String>,
}

pub type EntryRef = Rc<RefCell<Entry>>;

fn entry(mut data: EntryData, seq: usize) -> EntryRef {
    let date = data.date;
    // A sub-agent's call read from history: its run as it was saved.
    let sub = data.subagent.take().and_then(|b| yh_core::subagent_record::SubagentRecord::decode(&b)).map(Subagent::from_record);
    Rc::new(RefCell::new(Entry {
        data,
        live: false,
        sub,
        version: 0,
        open: false,
        markdown: None,
        seq,
        date,

        delivery: None,
    }))
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

impl Entry {
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemType {
    User,
    Text,
    Work,
    /// A sub-agent's call (`Agent`, `Task`): a card of its own, never folded into work.
    Agent,
    Notice,
    /// Images and videos Claude showed you (`show_media`): never folded away.
    Media,
}

/// One row on screen: a message, or a run of thinking and tool calls folded into one.
pub struct Item {
    /// Rows are numbered without gaps, so a row's index is its number less the first one's.
    pub pos: i64,
    pub ty: ItemType,
    pub entries: Vec<EntryRef>,
    pub open: bool,
}

/// A call that changes or reads the plan, or loads only the task tools (`ToolSearch`
/// `select:TaskCreate,TaskUpdate`): shown as the plan it leaves.
pub fn is_plan_tool(name: &str, input: &str) -> bool {
    if yh_core::plan::TOOLS.contains(&name) {
        return true;
    }
    if name != "ToolSearch" || !input.contains("select:") {
        return false;
    }
    let query = serde_json::from_str::<serde_json::Value>(input).ok().and_then(|v| v["query"].as_str().map(str::to_string)).unwrap_or_default();
    let Some(names) = query.strip_prefix("select:") else { return false };
    let names: Vec<&str> = names.split(',').map(str::trim).filter(|n| !n.is_empty()).collect();
    !names.is_empty() && names.iter().all(|n| yh_core::plan::TOOLS.contains(n))
}

/// Tools whose call is a sub-agent: shown as a card of its own.
pub const AGENT_TOOLS: &[&str] = &["Agent", "Task"];

fn item_type(data: &EntryData) -> ItemType {
    match data.kind {
        Kind::Tool if data.tool_name.as_deref() == Some(crate::state::harness_tools::MEDIA_TOOL) => ItemType::Media,
        Kind::Tool if data.tool_name.as_deref().is_some_and(|n| AGENT_TOOLS.contains(&n)) => ItemType::Agent,
        Kind::Thinking | Kind::Tool => ItemType::Work,
        Kind::User => ItemType::User,
        Kind::Assistant => ItemType::Text,
        Kind::Notice | Kind::Command => ItemType::Notice,
    }
}

/// A row's place in its turn. A finished turn's work (thinking, tool calls, sub-agents and
/// what Claude said along the way) folds into one summary row above its answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fold {
    Shown,
    /// Folded into its turn's summary: drawn as nothing.
    Hidden,
    /// The first row of a turn's work: draws the summary (`turns[turn]`), then itself when open.
    Summary { turn: usize, open: bool },
    /// Work of a turn shown open.
    Open,
}

/// A finished turn's work, for its summary row: "Worked for 2m 14s · 9 tool calls · edited 3 files".
pub struct TurnWork {
    /// The rows folded, by number; the first draws the summary.
    pub items: Vec<i64>,
    /// When your message went (seconds since 1970) and when the turn ended, when known.
    pub started: Option<f64>,
    pub ended: Option<f64>,
    /// Its details, worked out the first time the row is drawn.
    pub details: RefCell<Option<String>>,
    /// The files git saw change in it (shell commands too), once known.
    pub changed: Vec<String>,
}

/// Whether your message starts a turn: one that joined Claude's turn (steered) doesn't.
fn starts_turn(e: &Entry) -> bool {
    e.data.kind == Kind::User && e.delivery.as_deref() != Some("steered")
}

/// `YH_EXPAND_WORK=1`: every finished turn's work drawn open, for snapshots.
fn expand_all() -> bool {
    static EXPAND: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *EXPAND.get_or_init(|| std::env::var("YH_EXPAND_WORK").as_deref() == Ok("1"))
}

/// Your message sent while Claude works, waiting for its current step.
#[derive(Clone, Debug)]
pub struct Queued {
    pub id: String,
    pub text: String,
    /// Images (names in `attachments/`), then files.
    pub attachments: Vec<String>,
}

/// Claude asks you to sign in to an MCP server (`mcp__harness__sign_in_to_mcp_server`).
#[derive(Clone, Debug)]
pub struct SignInAsk {
    pub server: String,
    pub reason: Option<String>,
    /// Your answer goes here; None once given.
    pub decision: Option<flume::Sender<bool>>,
    /// Set by Cancel while the browser page is open.
    pub cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
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

impl ThreadState {
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

    /// Your queued messages go back into the composer: when `claude` is gone with them unread.
    pub fn restore_queued(&mut self) {
        for q in std::mem::take(&mut self.queue) {
            self.composer.restore_on_withdraw.remove(&q.id);
            self.composer.restored.push((q.id, q.text, q.attachments));
        }
    }

    /// Takes a request off the list once answered.
    pub fn answered(&mut self, request_id: &str) -> Option<Permission> {
        let i = self.permissions.iter().position(|p| p.id == request_id)?;
        Some(self.permissions.remove(i))
    }
}

pub struct ThreadState {
    pub meta: ThreadMeta,
    pub title: String,
    pub items: Vec<Item>,
    pub list: ListState,
    pub running: bool,
    /// A turn ended while you were elsewhere.
    pub unread: bool,
    /// The checklist Claude keeps with its task tools, from the rows held.
    pub plan: yh_core::plan::Plan,
    /// The row Find is on, by its number.
    pub find_hit: Option<i64>,
    /// What Find looks for while open (folded: no case, no accents); None draws no marks.
    pub find_query: Option<String>,
    /// Requests waiting for you, oldest first: parallel sub-agents can each ask at once. The
    /// composer shows the first; each is answered by its own request id.
    pub permissions: Vec<Permission>,
    /// Your messages sent while Claude works, oldest first, until it picks them up: shown above
    /// the composer, not in the transcript.
    pub queue: Vec<Queued>,
    /// MCP servers' questions mid-call, oldest first: the composer shows the first.
    pub elicitations: Vec<Elicitation>,
    pub sign_in: Option<SignInAsk>,
    /// You pressed Stop: the turn's end is "Interrupted", not an error, and tells nobody.
    pub interrupt_requested: bool,
    /// The turn that just ended was stopped by you (read by the workspace as it ends).
    pub stopped_by_you: bool,
    /// What the CLI is doing between messages, when it says ("Compacting the conversation…").
    pub activity: Option<String>,
    /// The session's start (`init`) came from this `claude`: a resume that never got one failed.
    pub received_init: bool,
    /// Its first title was a guess (the subject wasn't known yet): written again from the
    /// conversation once the first turn is over.
    pub title_needs_refinement: bool,
    /// Rows of the history before the first one held.
    pub before: usize,
    pub loaded: bool,
    pub loading: bool,
    pub loading_earlier: bool,
    /// Its transcript is made (open or kept for going back): all its rows stay.
    pub in_deck: bool,
    pub session_id: Option<String>,
    /// Rows in the whole transcript, saved and held: the next row's number. `None` until known
    /// (read from the database before the first new row).
    pub total: Option<usize>,
    /// Rows changed since the last save, by number.
    dirty: std::collections::BTreeSet<usize>,
    /// Whole tool results the rows hold only a head of, until saved.
    full_results: HashMap<String, String>,
    /// Changed rows let go of before they were saved: saved with the next changes.
    unsaved: Vec<(usize, Row)>,
    blocks: HashMap<String, EntryRef>,
    tools: HashMap<String, (EntryRef, i64)>,
    /// Goes up whenever a sub-agent's log changes, for the right sidebar.
    pub subagents_version: u64,
    /// The sub-agent you picked in the right sidebar (its Agent tool call's id).
    pub inspected_subagent: Option<String>,
    next_note: u64,
    /// Each held row's place in its turn, as `items`.
    pub folds: Vec<Fold>,
    /// The finished turns' work, as `Fold::Summary` numbers them.
    pub turns: Vec<TurnWork>,
    /// Turns whose work you opened, by their first folded row.
    open_turns: std::collections::HashSet<i64>,
    /// Rows came or went, or a turn ended: `folds` is worked out again.
    folds_dirty: bool,
    /// What replies link against (the thread's files and repo), once read.
    pub linker: Option<std::sync::Arc<yh_core::links::Linker>>,
    /// What Claude left running (background commands, watches): never saved.
    pub jobs: crate::state::jobs::Jobs,
    /// The plan as of each run of task tool calls, by the run's last call (the transcript shows
    /// the newest as the whole checklist, earlier ones as a line).
    pub plan_steps: HashMap<String, yh_core::plan::Plan>,
    /// Task tool calls followed by another: their run shows once, at its end.
    pub folded_plan_calls: std::collections::HashSet<String>,
    pub latest_plan_step: Option<String>,
    /// What each running foreground command printed last, by its tool call: the transcript
    /// reads it once a second.
    pub job_lines: HashMap<String, String>,
    /// What an open job step shows of its output (its last 40 lines), by its tool call.
    pub job_outputs: HashMap<String, String>,
    /// The composer's: how full the context is, queued messages, MCP servers needing you.
    pub composer: ComposerState,
}

/// What the composer shows of a thread beyond its rows (see `ui::composer`).
#[derive(Default)]
pub struct ComposerState {
    /// Tokens in the context after Claude's latest message, and the model's window.
    pub context_tokens: Option<u64>,
    pub context_window: Option<u64>,
    /// Queued messages you took back that go back into the composer when the CLI lets go of
    /// them: (id, text, attachments).
    pub restore_on_withdraw: std::collections::HashSet<String>,
    pub restored: Vec<(String, String, Vec<String>)>,
    /// Queued messages you asked to go now (Claude is being stopped for them).
    pub sending_now: std::collections::HashSet<String>,
    /// MCP servers as last heard: (name, status, scope).
    pub mcp_servers: Vec<(String, String, String)>,
}

impl ThreadState {
    pub fn new(meta: ThreadMeta) -> Self {
        let composer = ComposerState { context_tokens: meta.context_tokens, context_window: meta.context_window, ..Default::default() };
        ThreadState {
            title: meta.title.clone(),
            session_id: meta.session_id.clone(),
            meta,
            items: vec![],
            list: ListState::new(0, ListAlignment::Bottom, px(800.)),
            running: false,
            unread: false,
            plan: Default::default(),
            find_hit: None,
            find_query: None,
            permissions: vec![],
            elicitations: vec![],
            queue: vec![],
            sign_in: None,
            interrupt_requested: false,
            stopped_by_you: false,
            activity: None,
            received_init: false,
            title_needs_refinement: false,
            before: 0,
            loaded: false,
            loading: false,
            loading_earlier: false,
            in_deck: false,
            total: None,
            dirty: Default::default(),
            full_results: HashMap::new(),
            unsaved: vec![],
            blocks: HashMap::new(),
            tools: HashMap::new(),
            subagents_version: 0,
            inspected_subagent: None,
            next_note: 0,
            folds: vec![],
            turns: vec![],
            open_turns: Default::default(),
            folds_dirty: false,
            linker: None,
            jobs: Default::default(),
            plan_steps: HashMap::new(),
            folded_plan_calls: Default::default(),
            latest_plan_step: None,
            job_lines: HashMap::new(),
            job_outputs: HashMap::new(),
            composer,
        }
    }

    pub fn id(&self) -> &str {
        &self.meta.id
    }

    fn index(&self, pos: i64) -> Option<usize> {
        let first = self.items.first()?.pos;
        let ix = (pos - first) as usize;
        (pos >= first && ix < self.items.len()).then_some(ix)
    }

    pub fn item_at(&self, ix: usize) -> Option<&Item> {
        self.items.get(ix)
    }

    /// History rows, oldest first, put in front of what's held; what you're reading stays put.
    pub fn prepend(&mut self, rows: Vec<EntryData>, before: usize) {
        let mut items: Vec<Item> = vec![];
        for (n, data) in rows.into_iter().enumerate() {
            let ty = item_type(&data);
            let e = entry(data, before + n);
            match items.last_mut() {
                Some(last) if ty == ItemType::Work && last.ty == ItemType::Work => last.entries.push(e),
                _ => items.push(Item { pos: 0, ty, entries: vec![e], open: false }),
            }
        }
        let first = self.items.first().map_or(0, |i| i.pos);
        let count = items.len() as i64;
        for (n, item) in items.iter_mut().enumerate() {
            item.pos = first - count + n as i64;
            for e in &item.entries {
                let b = e.borrow();
                if b.data.kind == Kind::Tool {
                    self.tools.insert(b.data.id.clone(), (e.clone(), item.pos));
                }
            }
        }
        let was_empty = self.items.is_empty();
        let top = self.list.logical_scroll_top();
        self.items.splice(0..0, items);
        self.before = before;
        if was_empty {
            self.list.reset(self.items.len());
        } else {
            self.list.splice(0..0, count as usize);
            self.list.scroll_to(ListOffset { item_ix: top.item_ix + count as usize, offset_in_item: top.offset_in_item });
        }
        // After the list knows the rows: it may measure some of them again.
        self.refresh_plan();
        self.refresh_folds();
    }

    /// Puts a new entry at the end, folding work into the work row before it.
    fn append(&mut self, data: EntryData) -> EntryRef {
        let kind = data.kind;
        let ty = item_type(&data);
        let seq = self.total.unwrap_or(0);
        self.total = Some(seq + 1);
        self.dirty.insert(seq);
        let e = entry(data, seq);
        e.borrow_mut().date = Some(yh_core::library::now_ms() / 1000.0);
        let pos = match self.items.last_mut() {
            Some(last) if ty == ItemType::Work && last.ty == ItemType::Work => {
                last.entries.push(e.clone());
                let pos = last.pos;
                self.touch_pos(pos);
                pos
            }
            _ => {
                let pos = self.items.last().map_or(0, |i| i.pos + 1);
                self.items.push(Item { pos, ty, entries: vec![e.clone()], open: false });
                self.folds_dirty = true;
                let n = self.items.len();
                self.list.splice(n - 1..n - 1, 1);
                // Nobody is looking: keep the newest rows, the rest go back to "earlier".
                if !self.in_deck && self.items.len() > 300 {
                    self.trim(200);
                }
                pos
            }
        };
        if kind == Kind::Tool {
            let id = e.borrow().data.id.clone();
            self.tools.insert(id, (e.clone(), pos));
        }
        e
    }

    /// The row has to be measured again.
    pub fn touch_pos(&mut self, pos: i64) {
        if let Some(ix) = self.index(pos) {
            self.list.splice(ix..ix + 1, 1);
        }
    }

    fn touch(&mut self, e: &EntryRef) {
        e.borrow_mut().version += 1;
        self.dirty.insert(e.borrow().seq);
        let id = e.borrow().data.id.clone();
        // Tool entries know their row; anything else streaming is in the last rows.
        if let Some((_, pos)) = self.tools.get(&id) {
            let pos = *pos;
            self.touch_pos(pos);
            return;
        }
        let found = self.items.iter().rev().take(8).find(|i| i.entries.iter().any(|x| Rc::ptr_eq(x, e))).map(|i| i.pos);
        if let Some(pos) = found {
            self.touch_pos(pos);
        }
    }

    /// Rows changed since the last call, for saving: (row number, row). A row still being
    /// written is saved as it is and again when it's done.
    pub fn take_changes(&mut self) -> Vec<(usize, Row)> {
        let mut out = std::mem::take(&mut self.unsaved);
        if self.dirty.is_empty() {
            return out;
        }
        let dirty = std::mem::take(&mut self.dirty);
        for item in &self.items {
            for e in &item.entries {
                let b = e.borrow();
                if dirty.contains(&b.seq) {
                    let full = if b.live { None } else { self.full_results.remove(&b.data.id) };
                    out.push((b.seq, b.row(full)));
                }
            }
        }
        out
    }

    /// The first held row's number, if any: history is read from before it.
    pub fn first_seq(&self) -> Option<usize> {
        self.items.first()?.entries.first().map(|e| e.borrow().seq)
    }

    /// Keeps the newest `keep` rows; the ones before go back to "earlier".
    pub fn trim(&mut self, keep: usize) {
        if self.items.len() <= keep {
            return;
        }
        let cut = self.items.len() - keep;
        let mut count = 0;
        for item in self.items.drain(..cut) {
            count += item.entries.len();
            for e in item.entries {
                let b = e.borrow();
                if self.dirty.remove(&b.seq) {
                    let full = self.full_results.remove(&b.data.id);
                    self.unsaved.push((b.seq, b.row(full)));
                }
                if b.data.kind == Kind::Tool {
                    self.tools.remove(&b.data.id);
                }
            }
        }
        self.before += count;
        self.list.splice(0..cut, 0);
        self.refresh_folds();
    }

    /// Lets go of the rows; they're read again on the next visit.
    pub fn unload(&mut self) {
        self.items.clear();
        self.list.reset(0);
        self.before = 0;
        self.blocks.clear();
        self.tools.clear();
        self.loaded = false;
        self.loading = false;
        self.folds.clear();
        self.turns.clear();
    }

    pub fn toggle_item(&mut self, pos: i64) {
        if let Some(ix) = self.index(pos) {
            self.items[ix].open = !self.items[ix].open;
            self.list.splice(ix..ix + 1, 1);
        }
    }

    pub fn toggle_step(&mut self, pos: i64, e: &EntryRef) {
        let open = e.borrow().open;
        e.borrow_mut().open = !open;
        self.touch_pos(pos);
    }

    /// Your message, as sent: `id` is what the CLI's lifecycle events name it by. Sent while
    /// Claude works (`queued`), it waits above the composer and joins the transcript where
    /// Claude picks it up, as in the Swift app: never in both places.
    pub fn add_user(&mut self, text: &str, images: &[String], id: &str, queued: bool, delivery: Option<&str>) {
        if queued {
            self.queue.push(Queued { id: id.to_string(), text: text.to_string(), attachments: images.to_vec() });
            return;
        }
        let mut data = EntryData::new(id, Kind::User, text);
        data.attachments = images.to_vec();
        let e = self.append(data);
        e.borrow_mut().delivery = delivery.map(str::to_string);
        self.refresh_folds();
        // Jobs started from here on are what Claude waits on.
        self.jobs.turn = Some(id.to_string());
    }

    /// The CLI says where your message is. True when the composer has to show it (a queued
    /// message picked up or taken back).
    fn lifecycle(&mut self, id: &str, state: &str) -> bool {
        let Some(at) = self.queue.iter().position(|q| q.id == id) else { return false };
        match state {
            "started" | "completed" => {
                // Claude has it now: into the transcript, where it entered the conversation.
                let q = self.queue.remove(at);
                let delivery = if self.composer.sending_now.remove(id) {
                    Some("interrupted")
                } else if self.running {
                    Some("steered")
                } else {
                    // Picked up after the turn ended or was stopped for it: a turn of its own.
                    None
                };
                self.running = true;
                self.add_user(&q.text, &q.attachments, &q.id, false, delivery);
                true
            }
            "cancelled" => {
                let q = self.queue.remove(at);
                self.composer.sending_now.remove(id);
                if self.composer.restore_on_withdraw.remove(id) {
                    self.composer.restored.push((q.id, q.text, q.attachments));
                }
                true
            }
            _ => false,
        }
    }

    /// Queued messages still waiting for Claude, oldest first: (id, text, attachments).
    pub fn queued_messages(&self) -> Vec<(String, String, usize)> {
        self.queue.iter().map(|q| (q.id.clone(), q.text.clone(), q.attachments.len())).collect()
    }

    pub fn notice(&mut self, text: &str) {
        self.next_note += 1;
        let id = format!("n{}", self.next_note);
        self.append(EntryData::new(id, Kind::Notice, text));
        self.refresh_folds();
    }

    /// What running foreground commands printed last, by tool call: rows whose line changed
    /// are measured again. True when any did.
    pub fn set_job_lines(&mut self, lines: Vec<(String, String)>) -> bool {
        let mut changed = false;
        for (tool, line) in lines {
            if self.job_lines.get(&tool) == Some(&line) {
                continue;
            }
            if let Some((_, pos)) = self.tools.get(&tool) {
                let pos = *pos;
                self.touch_pos(pos);
            }
            self.job_lines.insert(tool, line);
            changed = true;
        }
        changed
    }

    /// Tool calls whose job's output shows (their step is open, the job running or in the
    /// background), with the file it writes to: what the ticker reads.
    pub fn open_job_files(&self) -> Vec<(String, String)> {
        self.jobs
            .running
            .iter()
            .chain(self.jobs.ended.values().filter(|j| j.background))
            .filter_map(|j| {
                let tool = j.tool_id.clone()?;
                let (e, _) = self.tools.get(&tool)?;
                (e.borrow().open || expand_all()).then_some(())?;
                Some((tool, j.output_file.clone()?))
            })
            .collect()
    }

    /// What open job steps show of their output (the last 40 lines): rows whose text changed
    /// are measured again. True when any did.
    pub fn set_job_outputs(&mut self, outputs: Vec<(String, String)>) -> bool {
        let mut changed = false;
        for (tool, text) in outputs {
            if self.job_outputs.get(&tool) == Some(&text) {
                continue;
            }
            if let Some((_, pos)) = self.tools.get(&tool) {
                let pos = *pos;
                self.touch_pos(pos);
            }
            self.job_outputs.insert(tool, text);
            changed = true;
        }
        changed
    }

    /// Rows with a running job whose clock shows: drawn again (one row each, nothing measured
    /// but them). Only those laid out in the list, so a clock off screen costs nothing.
    pub fn tick_job_clocks(&mut self) -> bool {
        let rows: Vec<i64> = self
            .jobs
            .running
            .iter()
            .filter_map(|j| self.tools.get(j.tool_id.as_deref()?).map(|(_, pos)| *pos))
            .filter(|pos| self.index(*pos).is_some_and(|ix| self.list.bounds_for_item(ix).is_some()))
            .collect();
        for pos in &rows {
            self.touch_pos(*pos);
        }
        !rows.is_empty()
    }

    /// The files git saw a turn change, on the message that started it (by its id): saved, and
    /// its summary says them.
    pub fn set_changed_files(&mut self, message: &str, files: Vec<String>) {
        let found = self.items.iter().flat_map(|i| i.entries.iter()).find(|e| e.borrow().data.id == message).cloned();
        let Some(e) = found else { return };
        {
            let mut b = e.borrow_mut();
            b.data.changed_files = Some(files);
            self.dirty.insert(b.seq);
        }
        // Its turn's summary counts them: worked out again.
        let summaries: Vec<i64> = self.turns.iter().map(|t| t.items[0]).collect();
        self.turns.clear();
        self.refresh_folds();
        for pos in summaries {
            self.touch_pos(pos);
        }
    }

    /// The message that started the turn going now, and its tool calls' inputs so far.
    pub fn turn_start_and_inputs(&self) -> Option<(String, String)> {
        let start = self.items.iter().rposition(|i| i.ty == ItemType::User && starts_turn(&i.entries[0].borrow()))?;
        let id = self.items[start].entries[0].borrow().data.id.clone();
        let inputs: Vec<String> = self.items[start..].iter().flat_map(|i| i.entries.iter()).filter_map(|e| e.borrow().data.tool_input.clone()).collect();
        Some((id, inputs.join("\n")))
    }

    /// A notice in the error colour (saved as such: `notice_style` "error").
    pub fn error_notice(&mut self, text: &str) {
        self.next_note += 1;
        let id = format!("n{}", self.next_note);
        let mut data = EntryData::new(id, Kind::Notice, text);
        data.is_error = true;
        self.append(data);
        self.refresh_folds();
    }

    /// Opens or closes a finished turn's work, by its first folded row.
    pub fn toggle_turn(&mut self, first: i64) {
        if !self.open_turns.remove(&first) {
            self.open_turns.insert(first);
        }
        self.refresh_folds();
    }

    /// When the turn going now started: your message that started it.
    pub fn turn_started_at(&self) -> Option<f64> {
        self.items.iter().rev().filter(|i| i.ty == ItemType::User).find_map(|i| {
            let b = i.entries[0].borrow();
            if starts_turn(&b) { Some(b.date) } else { None }
        })?
    }

    /// Works out each row's place in its turn again, and has the rows whose place changed
    /// measured again.
    pub fn refresh_folds(&mut self) {
        self.folds_dirty = false;
        let old: HashMap<i64, Fold> = self.items.iter().zip(&self.folds).map(|(i, f)| (i.pos, *f)).collect();
        let mut old_details: HashMap<(i64, usize), Option<String>> =
            self.turns.drain(..).map(|t| ((t.items[0], t.items.len()), t.details.into_inner())).collect();
        let n = self.items.len();
        let mut folds = vec![Fold::Shown; n];
        let starts: Vec<usize> =
            (0..n).filter(|&ix| self.items[ix].ty == ItemType::User && starts_turn(&self.items[ix].entries[0].borrow())).collect();
        // Turns: from each message of yours that starts one to the next; rows held from before
        // the first are the end of a turn whose start isn't held.
        let mut bounds: Vec<(usize, usize)> = vec![];
        let mut from = 0;
        for &s in &starts {
            if s > from {
                bounds.push((from, s));
            }
            from = s;
        }
        if from < n {
            bounds.push((from, n));
        }
        let count = bounds.len();
        for (t, (s, e)) in bounds.into_iter().enumerate() {
            // The turn going now stays as it comes.
            if t + 1 == count && self.running {
                continue;
            }
            let user = starts.contains(&s).then_some(s);
            let body = if user.is_some() { s + 1 } else { s };
            let answer = (body..e).rev().find(|&ix| self.items[ix].ty == ItemType::Text);
            let folded: Vec<usize> = (body..e)
                .filter(|&ix| Some(ix) != answer && matches!(self.items[ix].ty, ItemType::Work | ItemType::Agent | ItemType::Text))
                .collect();
            let Some(&first) = folded.first() else { continue };
            let first_pos = self.items[first].pos;
            let open = expand_all() || self.open_turns.contains(&first_pos);
            for &ix in &folded {
                folds[ix] = if open { Fold::Open } else { Fold::Hidden };
            }
            folds[first] = Fold::Summary { turn: self.turns.len(), open };
            let (started, mut ended, changed) = match user {
                Some(u) => {
                    let b = self.items[u].entries[0].borrow();
                    (b.date, b.data.turn_ended_at, b.data.changed_files.clone().unwrap_or_default())
                }
                None => (None, None, vec![]),
            };
            if ended.is_none() {
                ended = self.items[s..e].iter().filter_map(|i| i.entries.last().and_then(|x| x.borrow().date)).reduce(f64::max);
            }
            let items: Vec<i64> = folded.iter().map(|&ix| self.items[ix].pos).collect();
            let details = old_details.remove(&(first_pos, items.len())).flatten();
            self.turns.push(TurnWork { items, started, ended, details: RefCell::new(details), changed });
        }
        for (ix, fold) in folds.iter().enumerate() {
            if old.get(&self.items[ix].pos).is_some_and(|was| was != fold) {
                self.list.splice(ix..ix + 1, 1);
            }
        }
        self.folds = folds;
    }

    /// A row's place in its turn.
    pub fn fold_at(&self, ix: usize) -> Fold {
        self.folds.get(ix).copied().unwrap_or(Fold::Shown)
    }

    /// The held rows at these numbers.
    pub fn items_at<'a>(&'a self, positions: &'a [i64]) -> impl Iterator<Item = &'a Item> + 'a {
        positions.iter().filter_map(|p| self.index(*p).map(|ix| &self.items[ix]))
    }

    /// Rows whose text, tool input or result has `query` (`yh_core::find::query`: case and
    /// accents don't count), oldest first, by number.
    pub fn find(&self, query: &str) -> Vec<i64> {
        use yh_core::find::contains;
        self.items
            .iter()
            .filter(|item| {
                item.entries.iter().any(|e| {
                    let b = e.borrow();
                    let has = |s: &str| contains(s, query);
                    has(&b.data.text) || b.data.tool_input.as_deref().is_some_and(has) || b.data.tool_result.as_deref().is_some_and(has)
                })
            })
            .map(|i| i.pos)
            .collect()
    }

    /// What Find looks for, while it's open: the rows draw its matches marked. Nothing is
    /// measured again (a mark is a background); the rows on screen are drawn again with the
    /// thread's notify, the list drawing what's in view each frame.
    pub fn set_find_query(&mut self, query: Option<String>) {
        self.find_query = query.filter(|q| !q.is_empty());
    }

    /// Find moves to a row: what's folded over it opens (its turn, its work row, the step whose
    /// input or output has the match), and it's shown and marked.
    pub fn show_hit(&mut self, pos: Option<i64>) {
        let old = self.find_hit;
        self.find_hit = pos;
        if let (Some(p), Some(ix)) = (pos, pos.and_then(|p| self.index(p))) {
            // A finished turn folded away: opened by its first folded row.
            if self.fold_at(ix) == Fold::Hidden {
                if let Some(turn) = self.turns.iter().find(|t| t.items.contains(&p)) {
                    self.open_turns.insert(turn.items[0]);
                    self.refresh_folds();
                }
            }
            if let Some(query) = self.find_query.clone() {
                let item = &mut self.items[ix];
                if item.ty == ItemType::Work && item.entries.len() > 1 && !item.open {
                    item.open = true;
                }
                for e in &item.entries {
                    let mut b = e.borrow_mut();
                    let hidden = b.data.tool_input.as_deref().is_some_and(|s| yh_core::find::contains(s, &query))
                        || b.data.tool_result.as_deref().is_some_and(|s| yh_core::find::contains(s, &query))
                        || (b.data.kind == Kind::Thinking && yh_core::find::contains(&b.data.text, &query));
                    if hidden && !b.open {
                        b.open = true;
                        b.version += 1;
                    }
                }
            }
        }
        for p in [old, pos].into_iter().flatten() {
            self.touch_pos(p);
        }
        if let Some(ix) = pos.and_then(|p| self.index(p)) {
            self.list.scroll_to_reveal_item(ix);
        }
    }

    /// Rebuilds the checklist from the task tool calls held, in order.
    pub fn refresh_plan(&mut self) {
        let mut plan = yh_core::plan::Plan::default();
        let mut steps: HashMap<String, yh_core::plan::Plan> = HashMap::new();
        let mut folded = std::collections::HashSet::new();
        let mut previous: Option<String> = None;
        for item in &self.items {
            for e in &item.entries {
                let b = e.borrow();
                let input = b.data.tool_input.as_deref().unwrap_or("");
                let Some(name) = b.data.tool_name.as_deref().filter(|n| b.data.kind == Kind::Tool && is_plan_tool(n, input)) else {
                    previous = None;
                    continue;
                };
                if yh_core::plan::TOOLS.contains(&name) {
                    plan.apply(name, input, b.data.tool_result.as_deref(), &b.data.id);
                }
                if let Some(previous) = previous.take() {
                    steps.remove(&previous);
                    folded.insert(previous);
                }
                steps.insert(b.data.id.clone(), plan.clone());
                previous = Some(b.data.id.clone());
            }
        }
        let latest = self.items.iter().rev().flat_map(|i| i.entries.iter().rev()).map(|e| e.borrow().data.id.clone()).find(|id| steps.contains_key(id));
        // Calls whose plan line changed are measured again.
        let changed: Vec<i64> = self
            .tools
            .iter()
            .filter(|(id, _)| {
                steps.get(*id) != self.plan_steps.get(*id)
                    || folded.contains(*id) != self.folded_plan_calls.contains(*id)
                    || (latest.as_ref() == Some(*id)) != (self.latest_plan_step.as_ref() == Some(*id))
            })
            .map(|(_, (_, pos))| *pos)
            .collect();
        self.plan_steps = steps;
        self.folded_plan_calls = folded;
        self.latest_plan_step = latest;
        for pos in changed {
            self.touch_pos(pos);
        }
        if std::env::var("YH_EVENT_LOG").as_deref() == Ok("1") {
            eprintln!("plan: {:?}", plan.items.iter().map(|i| (&i.id, &i.subject, i.status)).collect::<Vec<_>>());
        }
        self.plan = plan;
    }

    /// Applies one event from this thread's `claude`. True when what the sidebar shows changed.
    pub fn apply(&mut self, event: Event, cx: &mut Context<Self>) -> bool {
        let changed = self.apply_event(event, cx);
        if self.folds_dirty {
            self.refresh_folds();
        }
        changed
    }

    fn apply_event(&mut self, event: Event, _cx: &mut Context<Self>) -> bool {
        match event {
            Event::Session { id } => {
                self.session_id = Some(id);
                self.received_init = true;
                false
            }
            Event::Status(status) => {
                let activity = (status == "compacting").then(|| "Compacting the conversation…".to_string());
                let changed = activity != self.activity;
                self.activity = activity;
                changed
            }
            Event::SubagentProgress { tool_id, activity, usage } => {
                if let Some((e, pos)) = self.tools.get(&tool_id).cloned() {
                    {
                        let mut b = e.borrow_mut();
                        let is_agent = b.sub.is_some() || b.data.tool_name.as_deref().is_some_and(|n| AGENT_TOOLS.contains(&n));
                        if !is_agent {
                            return false;
                        }
                        let sub = b.sub.get_or_insert_with(|| Subagent::new(String::new()));
                        if activity.is_some() {
                            sub.activity = activity;
                        }
                        sub.apply(usage);
                        b.version += 1;
                    }
                    self.subagents_version += 1;
                    self.touch_pos(pos);
                }
                false
            }
            // Answered for you by the workspace (it holds the process), which takes it off the list.
            Event::ElicitationComplete { .. } => false,
            Event::Lifecycle { id, state } => self.lifecycle(&id, &state),
            Event::CommandOutput { text, id } => {
                // After `/compact` the CLI sends the outputs it kept again, same `uuid`: shown once.
                let seen = id.as_deref().is_some_and(|id| self.items.iter().rev().flat_map(|i| i.entries.iter()).any(|e| {
                    let b = e.borrow();
                    b.data.kind == Kind::Command && b.data.id == id
                }));
                if seen {
                    return false;
                }
                let id = id.unwrap_or_else(|| {
                    self.next_note += 1;
                    format!("c{}", self.next_note)
                });
                self.append(EntryData::new(id, Kind::Command, &text));
                false
            }
            Event::ContextUsage(tokens) => {
                self.composer.context_tokens = Some(tokens);
                false
            }
            Event::ContextWindows(windows) => {
                self.composer.context_window = windows.iter().map(|(_, w)| *w).max();
                false
            }
            Event::ConversationReset => {
                self.notice("Context cleared. Claude starts fresh from here.");
                self.composer.context_tokens = None;
                true
            }
            Event::McpServers(servers) => {
                let changed = self.composer.mcp_servers != servers;
                self.composer.mcp_servers = servers;
                changed
            }
            Event::TitleChanged(_) | Event::Skills(_) => false,
            Event::Catalog { .. } | Event::Limits(_) => false,
            Event::BlockStart { message, index, parent, kind } => {
                if let Some(parent) = parent {
                    self.subagent_step(&parent);
                    return false;
                }
                let key = format!("{message}:{index}");
                if self.blocks.contains_key(&key) {
                    return false;
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
                let e = self.append(data);
                e.borrow_mut().live = true;
                self.blocks.insert(key, e);
                // Claude woken by a job's end: a turn with no message of yours.
                let woke = !self.running && self.jobs.version > 0;
                if woke {
                    self.running = true;
                }
                woke
            }
            Event::Delta { message, index, parent, kind, text } => {
                if parent.is_some() {
                    return false;
                }
                let key = format!("{message}:{index}");
                let e = match self.blocks.get(&key) {
                    Some(e) => e.clone(),
                    None => {
                        let k = if kind == DeltaKind::Thinking { Kind::Thinking } else { Kind::Assistant };
                        let e = self.append(EntryData::new(key.clone(), k, ""));
                        e.borrow_mut().live = true;
                        self.blocks.insert(key, e.clone());
                        e
                    }
                };
                {
                    let mut b = e.borrow_mut();
                    if kind == DeltaKind::Input {
                        b.data.tool_input.get_or_insert_with(String::new).push_str(&text);
                    } else {
                        b.data.text.push_str(&text);
                    }
                }
                self.touch(&e);
                false
            }
            Event::Assistant { message, parent, blocks } => {
                if let Some(parent) = parent {
                    for block in &blocks {
                        if matches!(block, Block::Tool { .. }) {
                            self.subagent_step(&parent);
                        }
                    }
                    self.subagent_log(&parent, |log| {
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
                    return false;
                }
                // The CLI sends a message's blocks one `assistant` event each, all under the
                // message's ID: a tool block is its tool call; a text or thinking block is the
                // message's streamed block of that kind still being written, else one that
                // already says the same, else a new row.
                for (index, block) in blocks.into_iter().enumerate() {
                    let mut key = format!("{message}:{index}");
                    let existing = match &block {
                        Block::Tool { id, .. } => self.tools.get(id).map(|(e, _)| e.clone()),
                        Block::Text(text) | Block::Thinking(text) => {
                            let kind = if matches!(block, Block::Text(_)) { Kind::Assistant } else { Kind::Thinking };
                            let prefix = format!("{message}:");
                            let mut candidates: Vec<(&String, &EntryRef)> = self
                                .blocks
                                .iter()
                                .filter(|(k, e)| k.starts_with(&prefix) && e.borrow().data.kind == kind)
                                .collect();
                            candidates.sort_by_key(|(k, _)| k.rsplit(':').next().and_then(|n| n.parse::<u64>().ok()).unwrap_or(0));
                            let found = candidates
                                .iter()
                                .find(|(_, e)| e.borrow().live)
                                .or_else(|| candidates.iter().find(|(_, e)| e.borrow().data.text == *text))
                                .map(|(k, e)| ((*k).clone(), (*e).clone()));
                            match found {
                                Some((k, e)) => {
                                    key = k;
                                    Some(e)
                                }
                                // A new block of this message: a key no stream block has.
                                None => {
                                    key = format!("{message}:a{}", candidates.len() + index);
                                    None
                                }
                            }
                        }
                    };
                    let e = match existing {
                        Some(e) => e,
                        None => {
                            let data = match &block {
                                Block::Text(_) => EntryData::new(key.clone(), Kind::Assistant, ""),
                                Block::Thinking(_) => EntryData::new(key.clone(), Kind::Thinking, ""),
                                Block::Tool { id, .. } => EntryData::new(id.clone(), Kind::Tool, ""),
                            };
                            let e = self.append(data);
                            self.blocks.insert(key, e.clone());
                            e
                        }
                    };
                    {
                        let mut b = e.borrow_mut();
                        match block {
                            Block::Text(t) | Block::Thinking(t) => b.data.text = t,
                            Block::Tool { name, input, .. } => {
                                b.data.tool_name = Some(name);
                                b.data.tool_input = Some(input);
                            }
                        }
                        b.live = false;
                    }
                    self.touch(&e);
                }
                false
            }
            Event::ToolResults { parent, results } => {
                crate::state::pulls::note_results(self, parent.as_deref(), &results, |id| self.tools.get(id).map(|(e, _)| e.clone()));
                if let Some(parent) = parent {
                    self.subagent_log(&parent, |log| {
                        for r in results {
                            if let Some(step) = log.iter_mut().rev().find(|s| s.tool.is_some() && s.id == r.id) {
                                step.result = Some(r.text);
                                step.is_error = r.is_error;
                            }
                        }
                    });
                    return false;
                }
                let touches_plan = results.iter().any(|r| {
                    self.tools.get(&r.id).is_some_and(|(e, _)| {
                        {
                            let b = e.borrow();
                            b.data.tool_name.as_deref().is_some_and(|n| is_plan_tool(n, b.data.tool_input.as_deref().unwrap_or("")))
                        }
                    })
                });
                let jobs_before = self.jobs.running.len();
                for r in results {
                    self.jobs.tool_result(&r.id, &r.text);
                    let Some((e, pos)) = self.tools.get(&r.id).cloned() else { continue };
                    if let Some(full) = r.full {
                        self.full_results.insert(r.id.clone(), full);
                    }
                    self.dirty.insert(e.borrow().seq);
                    {
                        let mut b = e.borrow_mut();
                        b.data.tool_result = Some(r.text);
                        b.data.result_len = r.len;
                        b.data.is_error = r.is_error;
                        b.live = false;
                        b.version += 1;
                    }
                    self.touch_pos(pos);
                }
                if touches_plan {
                    self.refresh_plan();
                    return true;
                }
                self.jobs.running.len() != jobs_before
            }
            Event::SubagentStarted { tool_id, description, task_id } => {
                if let Some((e, pos)) = self.tools.get(&tool_id).cloned() {
                    {
                        let mut b = e.borrow_mut();
                        // Keep what it already did when the start comes after its first steps.
                        match &mut b.sub {
                            Some(sub) => sub.description = description,
                            None => b.sub = Some(Subagent::new(description)),
                        }
                        if let Some(sub) = &mut b.sub {
                            sub.task_id = task_id.or(sub.task_id.take());
                        }
                    }
                    e.borrow_mut().version += 1;
                    self.touch_pos(pos);
                }
                false
            }
            Event::SubagentFinished { tool_id, status, summary, usage } => {
                if let Some((e, pos)) = self.tools.get(&tool_id).cloned() {
                    {
                        let mut b = e.borrow_mut();
                        let sub = b.sub.get_or_insert_with(|| Subagent::new(String::new()));
                        sub.status = status;
                        sub.activity = None;
                        sub.apply(usage);
                        if summary.is_some() {
                            sub.report = summary;
                        }
                        b.version += 1;
                        self.dirty.insert(b.seq);
                    }
                    self.touch_pos(pos);
                }
                false
            }
            Event::Elicitation { request_id, server, message, mode, schema, url, elicitation_id } => {
                // Several servers (or calls) can ask at once: each waits its turn.
                let asked = Elicitation { id: request_id, server, message, mode, schema, url, elicitation_id };
                match self.elicitations.iter_mut().find(|e| e.id == asked.id) {
                    Some(e) => *e = asked,
                    None => self.elicitations.push(asked),
                }
                true
            }
            Event::Permission { request_id, tool, description, input } => {
                // The same request again (a resumed stream) replaces itself; a new one waits its turn.
                let asked = Permission { id: request_id, tool, description, input };
                match self.permissions.iter_mut().find(|p| p.id == asked.id) {
                    Some(p) => *p = asked,
                    None => self.permissions.push(asked),
                }
                true
            }
            Event::Notice(text) => {
                self.notice(&text);
                false
            }
            Event::Done { is_error, text } => {
                self.end_turn();
                // Stopped for a message sent now, which carries that on itself; stopped by you,
                // a plain note (the CLI ends it as `error_during_execution`); anything else, the
                // CLI's error.
                let for_message = !self.composer.sending_now.is_empty();
                if is_error && !for_message {
                    if self.interrupt_requested {
                        self.notice("Interrupted");
                    } else if text.is_empty() || (text.starts_with("error_") && !text.contains(' ')) {
                        let why = if text.is_empty() { String::new() } else { format!(" ({text})") };
                        self.error_notice(&format!("Claude stopped with an error{why}."));
                    } else {
                        self.error_notice(&text);
                    }
                }
                self.stopped_by_you = self.interrupt_requested || for_message;
                self.interrupt_requested = false;
                true
            }
            Event::JobStarted { task_id, tool_id, description, background } => {
                let tool = tool_id.as_ref().and_then(|id| self.tools.get(id)).and_then(|(e, _)| e.borrow().data.tool_name.clone());
                self.jobs.start(task_id, tool_id, tool.as_deref(), description, background);
                true
            }
            Event::JobFinished { task_id, status, summary, output_file } => {
                let code = summary.as_deref().and_then(yh_core::claude::job_exit_code);
                match self.jobs.finish(&task_id, &status, code, output_file) {
                    Some((note, true)) => self.error_notice(&note),
                    Some((note, false)) => self.notice(&note),
                    None => {}
                }
                true
            }
            Event::JobOutput { task_id, text } => {
                self.jobs.saw(&task_id, &text);
                false
            }
            Event::JobsRunning(ids) => {
                let notes = self.jobs.keep_only(&ids);
                for note in &notes {
                    self.notice(note);
                }
                !notes.is_empty()
            }
            Event::Exit { code, tail } => {
                for note in self.jobs.cut_off("stopped when Claude exited") {
                    self.notice(&note);
                }
                self.end_turn();
                // Whatever still waited never reached Claude: back to the composer.
                self.restore_queued();
                self.composer.sending_now.clear();
                self.interrupt_requested = false;
                self.elicitations.clear();
                if code != 0 && !self.received_init && self.session_id.is_some() {
                    // Its session couldn't be resumed (gone, or another folder's): start afresh.
                    self.session_id = None;
                    self.meta.session_id = None;
                    self.error_notice("Couldn't resume the previous session. Your next message starts a new one.");
                } else if code != 0 {
                    let detail: Vec<&str> = tail.lines().filter(|l| !l.trim().is_empty()).collect();
                    let detail = detail[detail.len().saturating_sub(3)..].join("\n");
                    self.error_notice(&format!("Claude exited (code {code}). {detail}").trim_end().to_string());
                }
                self.received_init = false;
                true
            }
        }
    }

    fn subagent_step(&mut self, parent: &str) {
        let Some((e, pos)) = self.tools.get(parent).cloned() else { return };
        self.dirty.insert(e.borrow().seq);
        {
            let mut b = e.borrow_mut();
            let sub = b.sub.get_or_insert_with(|| Subagent::new("Sub-agent".into()));
            sub.steps += 1;
            b.version += 1;
        }
        self.touch_pos(pos);
    }

    /// Changes what a sub-agent did (its log), and tells whoever follows it.
    fn subagent_log(&mut self, parent: &str, change: impl FnOnce(&mut Vec<SubStep>)) {
        let Some((e, pos)) = self.tools.get(parent).cloned() else { return };
        // Saved with its call (the `subagent` column).
        self.dirty.insert(e.borrow().seq);
        {
            let mut b = e.borrow_mut();
            let sub = b.sub.get_or_insert_with(|| Subagent::new("Sub-agent".into()));
            change(&mut sub.log);
            if sub.log.len() > SUBAGENT_LOG {
                let extra = sub.log.len() - SUBAGENT_LOG;
                sub.log.drain(..extra);
            }
            b.version += 1;
        }
        self.subagents_version += 1;
        self.touch_pos(pos);
    }

    /// The thread's sub-agents still held, oldest first: (Agent tool call id, its entry).
    pub fn subagents(&self) -> Vec<(String, EntryRef)> {
        let mut found: Vec<(usize, String, EntryRef)> = self
            .tools
            .iter()
            .filter(|(_, (e, _))| e.borrow().sub.is_some() || e.borrow().data.tool_name.as_deref().is_some_and(|n| AGENT_TOOLS.contains(&n)))
            .map(|(id, (e, _))| (e.borrow().seq, id.clone(), e.clone()))
            .collect();
        found.sort_by_key(|(seq, _, _)| *seq);
        found.into_iter().map(|(_, id, e)| (id, e)).collect()
    }

    fn end_turn(&mut self) {
        let live: Vec<EntryRef> = self.blocks.values().filter(|e| e.borrow().live).cloned().collect();
        for e in live {
            e.borrow_mut().live = false;
            self.touch(&e);
        }
        self.blocks.clear();
        self.running = false;
        self.activity = None;
        self.permissions.clear();
        // How long the turn took goes on the message that started it.
        let start = self.items.iter().rev().filter(|i| i.ty == ItemType::User).map(|i| i.entries[0].clone()).find(|e| starts_turn(&e.borrow()));
        if let Some(e) = start {
            let mut b = e.borrow_mut();
            b.data.turn_ended_at = Some(yh_core::library::now_ms() / 1000.0);
            self.dirty.insert(b.seq);
        }
        self.folds_dirty = true;
    }
}
