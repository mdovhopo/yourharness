//! A thread's rows as the app holds them: only the newest page or two of a long history;
//! earlier pages are read when you scroll up to them. A streamed delta changes its entry in
//! place and tells the list to measure just that row again (`ListState::splice`).

use yh_core::library::{EntryData, Kind, ThreadMeta};
use yh_core::thread_run::{ComposerState, LiveState, RowChange, TextPatch, TranscriptRow};
pub use yh_core::plan::is_plan_tool;
pub use yh_core::thread_run::{Elicitation, Permission, SubStep, Subagent, AGENT_TOOLS};
use yh_core::markdown::Markdown;
use gpui_kit::{px, ListAlignment, ListOffset, ListState};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

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

/// A row as the run told it, to show.
fn entry_of(r: TranscriptRow) -> EntryRef {
    Rc::new(RefCell::new(Entry { data: r.data, live: r.live, sub: r.sub, version: 0, open: false, markdown: None, seq: r.seq, date: r.date, delivery: r.delivery }))
}

/// A row read from storage: a sub-agent's call with its run as it was saved.
fn entry(data: EntryData, seq: usize) -> EntryRef {
    entry_of(TranscriptRow::saved(data, seq))
}

impl Entry {
    /// As the run holds it.
    #[cfg(test)]
    fn transcript_row(&self) -> TranscriptRow {
        TranscriptRow { seq: self.seq, data: self.data.clone(), live: self.live, sub: self.sub.clone(), date: self.date, delivery: self.delivery.clone() }
    }
}

/// A sub-agent call that isn't held, as the Sub-agents panel lists it: what it was asked
/// (only what the list shows) and its run summarized, no steps.
fn summary_entry(call: yh_core::db::SubagentCall) -> EntryRef {
    let mut data = EntryData::new(call.id, Kind::Tool, "");
    data.tool_name = call.tool_name;
    let mut input = serde_json::Map::new();
    if let Some(d) = call.description {
        input.insert("description".into(), d.into());
    }
    if let Some(t) = call.agent_type {
        input.insert("subagent_type".into(), t.into());
    }
    if call.background {
        input.insert("run_in_background".into(), true.into());
    }
    data.tool_input = Some(serde_json::Value::Object(input).to_string());
    data.tool_result = call.has_result.then(String::new);
    data.is_error = call.is_error;
    let e = entry(data, call.seq);
    // A call with neither a run nor a result was cut off with the process it ran in.
    e.borrow_mut().sub = Some(call.run.map(Subagent::from_summary).unwrap_or_else(|| {
        let mut s = Subagent::new(String::new());
        s.status = if !call.has_result { "stopped".into() } else if call.is_error { "failed".into() } else { "completed".into() };
        s.started_at = 0.0;
        s
    }));
    e
}

/// A held sub-agent call let go of, kept for the panel as `summary_entry` would read it back.
fn summary_of(e: &Entry) -> EntryRef {
    let mut data = EntryData::new(e.data.id.clone(), Kind::Tool, "");
    data.tool_name = e.data.tool_name.clone();
    data.tool_input = e.data.tool_input.clone();
    data.tool_result = e.data.tool_result.as_ref().map(|_| String::new());
    data.is_error = e.data.is_error;
    let summary = entry(data, e.seq);
    summary.borrow_mut().sub = e.sub.as_ref().map(Subagent::summarized);
    summary
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

/// A call that runs a sub-agent: an Agent (or Task) call, or one whose tool isn't known yet
/// but already has a run.
fn is_agent_entry(e: &Entry) -> bool {
    match e.data.tool_name.as_deref() {
        Some(name) => AGENT_TOOLS.contains(&name),
        None => e.sub.is_some(),
    }
}

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

impl ThreadState {
    /// The request the composer shows: the oldest waiting.
    pub fn permission(&self) -> Option<&Permission> {
        self.run.permission()
    }

    pub fn waiting_for_you(&self) -> bool {
        self.run.waiting_for_you()
    }

    /// The MCP question the composer shows: the oldest waiting.
    pub fn elicitation(&self) -> Option<&Elicitation> {
        self.run.elicitation()
    }

}

pub struct ThreadState {
    pub meta: ThreadMeta,
    pub title: String,
    pub items: Vec<Item>,
    pub list: ListState,
    /// Its run's live state (requests, the queue, jobs, context) as the engine last told it;
    /// the rows here are built from the changes it tells (`apply_update`).
    pub run: LiveState,
    /// A turn ended while you were elsewhere.
    pub unread: bool,
    /// The checklist Claude keeps with its task tools, from the rows held.
    pub plan: yh_core::plan::Plan,
    /// The row Find is on, by its number.
    pub find_hit: Option<i64>,
    /// What Find looks for while open (folded: no case, no accents); None draws no marks.
    pub find_query: Option<String>,
    pub sign_in: Option<SignInAsk>,
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
    /// The held rows' tool calls by id, with their row's number.
    tools: HashMap<String, (EntryRef, i64)>,
    /// The sub-agent you picked in the right sidebar (its Agent tool call's id).
    pub inspected_subagent: Option<String>,
    /// Sub-agent calls in rows that aren't held (earlier pages, or let go of), for the panel,
    /// by call id: (row number, the call summarized, `summary_entry`). The one in
    /// `run_detail` is read whole.
    saved_runs: HashMap<String, (usize, EntryRef)>,
    /// `saved_runs` was asked of the database for this thread.
    saved_runs_read: bool,
    /// The saved run read whole for the panel (its steps, report), by call id; `true` once in.
    run_detail: Option<(String, bool)>,
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
}

impl ThreadState {
    pub fn new(meta: ThreadMeta) -> Self {
        let composer = ComposerState { context_tokens: meta.context_tokens, context_window: meta.context_window, ..Default::default() };
        let run = LiveState { session_id: meta.session_id.clone(), composer, ..Default::default() };
        ThreadState {
            title: meta.title.clone(),
            meta,
            run,
            items: vec![],
            list: ListState::new(0, ListAlignment::Bottom, px(800.)),
            unread: false,
            plan: Default::default(),
            find_hit: None,
            find_query: None,
            sign_in: None,
            title_needs_refinement: false,
            before: 0,
            loaded: false,
            loading: false,
            loading_earlier: false,
            in_deck: false,
            tools: HashMap::new(),
            inspected_subagent: None,
            saved_runs: HashMap::new(),
            saved_runs_read: false,
            run_detail: None,
            folds: vec![],
            turns: vec![],
            open_turns: Default::default(),
            folds_dirty: false,
            linker: None,
            plan_steps: HashMap::new(),
            folded_plan_calls: Default::default(),
            latest_plan_step: None,
            job_lines: HashMap::new(),
            job_outputs: HashMap::new(),
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
                    // Held again: the row is the run, its summary goes.
                    if self.saved_runs.remove(&b.data.id).is_some() && self.run_detail.as_ref().is_some_and(|(id, _)| *id == b.data.id) {
                        self.run_detail = None;
                    }
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

    /// Puts a row the run added at the end, folding work into the work row before it.
    fn push(&mut self, row: TranscriptRow) {
        let kind = row.data.kind;
        let ty = item_type(&row.data);
        let e = entry_of(row);
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
    }

    /// A held row by its number, with its item's.
    fn locate(&self, seq: usize) -> Option<(EntryRef, i64)> {
        let first = |i: &Item| i.entries.first().map_or(0, |e| e.borrow().seq);
        let ix = self.items.partition_point(|i| first(i) <= seq).checked_sub(1)?;
        let item = &self.items[ix];
        item.entries.iter().find(|e| e.borrow().seq == seq).map(|e| (e.clone(), item.pos))
    }

    /// Applies what the run changed to the rows held: new rows at the end, changed ones in
    /// place, each measured again. A change to a row not held (let go of, or in an earlier
    /// page) is nothing to show.
    fn apply_changes(&mut self, changes: Vec<RowChange>) {
        // Your messages changed (a turn's end, the files it changed): their turns' summaries
        // are worked out again.
        let mut turns_changed = false;
        for change in changes {
            match change {
                RowChange::Added(row) => self.push(row),
                RowChange::Changed { seq, text, input, row } => {
                    let Some((e, pos)) = self.locate(seq) else { continue };
                    {
                        let mut b = e.borrow_mut();
                        match row {
                            Some(r) => {
                                turns_changed |= r.data.kind == Kind::User;
                                b.data = r.data;
                                b.live = r.live;
                                b.sub = r.sub;
                                b.date = r.date;
                                b.delivery = r.delivery;
                            }
                            None => {
                                text.apply_to(&mut b.data.text);
                                if input != TextPatch::Same {
                                    input.apply_to(b.data.tool_input.get_or_insert_with(String::new));
                                }
                            }
                        }
                        b.version += 1;
                    }
                    self.touch_pos(pos);
                }
            }
        }
        if turns_changed {
            let summaries: Vec<i64> = self.turns.iter().map(|t| t.items[0]).collect();
            self.turns.clear();
            self.refresh_folds();
            for pos in summaries {
                self.touch_pos(pos);
            }
        }
    }

    /// What the engine told of this thread: its live state now, and its rows' changes.
    /// `plan`: the checklist is worked out again; `ended`: a turn ended or its `claude` went.
    pub fn apply_update(&mut self, changes: Vec<RowChange>, live: LiveState, plan: bool, session_reset: bool, ended: bool) {
        let was_running = self.run.running;
        // A session the engine never heard of (its run made for a phone's snapshot before the
        // app told it) isn't forgotten here; `/clear` clears it on purpose first.
        let session = self.run.session_id.take();
        self.run = live;
        if self.run.session_id.is_none() && !session_reset {
            self.run.session_id = session;
        }
        self.apply_changes(changes);
        if session_reset {
            self.meta.session_id = None;
        }
        if plan {
            self.refresh_plan();
        }
        if ended || was_running != self.run.running {
            self.folds_dirty = true;
        }
        if self.folds_dirty {
            self.refresh_folds();
        }
    }

    /// The row has to be measured again.
    pub fn touch_pos(&mut self, pos: i64) {
        if let Some(ix) = self.index(pos) {
            self.list.splice(ix..ix + 1, 1);
        }
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
                if b.data.kind == Kind::Tool {
                    self.tools.remove(&b.data.id);
                    if is_agent_entry(&b) {
                        self.saved_runs.insert(b.data.id.clone(), (b.seq, summary_of(&b)));
                    }
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
        // Its sub-agents stay listed, summarized; the one read whole goes back to a summary.
        for (id, (e, _)) in self.tools.drain() {
            let b = e.borrow();
            if is_agent_entry(&b) {
                self.saved_runs.insert(id, (b.seq, summary_of(&b)));
            }
        }
        if let Some((id, _)) = self.run_detail.take() {
            if let Some((_, e)) = self.saved_runs.get_mut(&id) {
                let summary = summary_of(&e.borrow());
                *e = summary;
            }
        }
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

    /// Queued messages still waiting for Claude, oldest first: (id, text, attachments).
    pub fn queued_messages(&self) -> Vec<(String, String, usize)> {
        self.run.queued_messages()
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
        self.run.jobs
            .running
            .iter()
            .chain(self.run.jobs.ended.values().filter(|j| j.background))
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
            .run.jobs
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

    /// The message that started the turn going now, and its tool calls' inputs so far.
    pub fn turn_start_and_inputs(&self) -> Option<(String, String)> {
        let start = self.items.iter().rposition(|i| i.ty == ItemType::User && starts_turn(&i.entries[0].borrow()))?;
        let id = self.items[start].entries[0].borrow().data.id.clone();
        let inputs: Vec<String> = self.items[start..].iter().flat_map(|i| i.entries.iter()).filter_map(|e| e.borrow().data.tool_input.clone()).collect();
        Some((id, inputs.join("\n")))
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
            if t + 1 == count && self.run.running {
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

    /// The thread's sub-agents, oldest first: (Agent tool call id, its entry). The held rows'
    /// as they are (live, running ones among them), and the rest's as saved, summarized
    /// (`saved_runs`) unless read whole (`run_detail`).
    pub fn subagents(&self) -> Vec<(String, EntryRef)> {
        let mut found: Vec<(usize, String, EntryRef)> = self
            .tools
            .iter()
            // Agent calls only: a watch or command the CLI once reported as a task, or a call
            // that continued an agent, may carry a run from older versions.
            .filter(|(_, (e, _))| is_agent_entry(&e.borrow()))
            .map(|(id, (e, _))| (e.borrow().seq, id.clone(), e.clone()))
            .collect();
        found.extend(self.saved_runs.iter().filter(|(id, _)| !self.tools.contains_key(*id)).map(|(id, (seq, e))| (*seq, id.clone(), e.clone())));
        found.sort_by_key(|(seq, _, _)| *seq);
        found.into_iter().map(|(_, id, e)| (id, e)).collect()
    }

    /// The panel wants every sub-agent and the saved ones haven't been asked for yet.
    pub fn needs_saved_runs(&self) -> bool {
        !self.saved_runs_read && !self.id().starts_with("new:")
    }

    pub fn mark_saved_runs_read(&mut self) {
        self.saved_runs_read = true;
    }

    /// The saved sub-agent calls, as `Library::subagent_calls` read them: those not held join
    /// the list. One let go of meanwhile is kept as it was let go of, newer than what was read.
    pub fn set_saved_runs(&mut self, calls: Vec<yh_core::db::SubagentCall>) {
        for call in calls {
            if self.tools.contains_key(&call.id) || self.saved_runs.contains_key(&call.id) {
                continue;
            }
            let (id, seq) = (call.id.clone(), call.seq);
            self.saved_runs.insert(id, (seq, summary_entry(call)));
        }
        self.run.subagents_version += 1;
    }

    /// A run listed from its summary that the panel shows: its steps and report are to be read.
    pub fn wants_run_detail(&self, id: &str) -> bool {
        self.saved_runs.contains_key(id) && !self.tools.contains_key(id) && self.run_detail.as_ref().is_none_or(|(d, _)| d != id)
    }

    pub fn mark_run_detail(&mut self, id: &str) {
        self.run_detail = Some((id.to_string(), false));
    }

    /// The run's whole row, read for the panel: it replaces its summary, and the one read
    /// before goes back to a summary (one run's steps are held at a time).
    pub fn set_run_detail(&mut self, id: &str, row: Option<(usize, EntryData)>) {
        if self.run_detail.as_ref().is_none_or(|(d, _)| d != id) {
            return; // another was picked meanwhile
        }
        self.run_detail = Some((id.to_string(), true));
        for (other, (_, e)) in self.saved_runs.iter_mut() {
            if other != id && e.borrow().sub.as_ref().is_some_and(|s| !s.log.is_empty() || s.report.is_some()) {
                let summary = summary_of(&e.borrow());
                *e = summary;
            }
        }
        if let (Some((seq, data)), Some(slot)) = (row, self.saved_runs.get_mut(id)) {
            if !self.tools.contains_key(id) && data.id == id {
                let whole = entry(data, seq);
                if whole.borrow().sub.is_none() {
                    // No run saved with it: keep what the summary made of its call.
                    whole.borrow_mut().sub = slot.1.borrow().sub.clone();
                }
                *slot = (seq, whole);
            }
        }
        self.run.subagents_version += 1;
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use yh_core::db::Row;
    use yh_core::db::SubagentCall;
    use yh_core::subagent_record::{SubagentEntry, SubagentRecord, SubagentSummary};

    fn agent(id: &str, description: &str, run: Option<&SubagentRecord>) -> EntryData {
        let mut d = EntryData::new(id, Kind::Tool, "");
        d.tool_name = Some("Agent".into());
        d.tool_input = Some(format!(r#"{{"description":"{description}","prompt":"go"}}"#));
        d.tool_result = Some("done".into());
        d.subagent = run.map(|r| r.encode());
        d
    }

    fn call(seq: usize, id: &str, description: &str) -> SubagentCall {
        SubagentCall {
            seq,
            id: id.into(),
            tool_name: Some("Agent".into()),
            description: Some(description.into()),
            has_result: true,
            run: Some(SubagentSummary { description: description.into(), status: "completed".into(), tool_uses: Some(3), ..Default::default() }),
            ..Default::default()
        }
    }

    /// The recorded sessions and the synthetic one through the engine, as the app sends them,
    /// into a database of its own, the thread built from what the engine tells
    /// (`yh_core::golden`): what's saved must be exactly what the golden files say.
    fn golden(name: &str, steps: Vec<yh_core::golden::Step>) -> String {
        use yh_core::golden::Step;
        use yh_engine::{Command, Output};
        let dir = std::env::temp_dir().join(format!("yh-app-golden-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = std::sync::Arc::new(yh_core::db::Database::open(&dir).unwrap());
        let engine = yh_engine::start(Some(db.clone()));
        let mut t = ThreadState::new(ThreadMeta::new("T".into(), "P".into(), "t".into()));
        t.in_deck = true;
        t.loaded = true;
        let id = t.id().to_string();
        engine.send(Command::Open { thread: id.clone(), session_id: None, context_tokens: None, context_window: None });
        for step in steps {
            match step {
                Step::Event(e) => engine.send(Command::Events { thread: id.clone(), events: vec![e] }),
                Step::User { id: message, text, .. } => engine.send(Command::Send { thread: id.clone(), text: text.into(), attachments: vec![], message_id: message.into(), now: false, effort: None, deliver: None }),
                Step::Interrupt => engine.send(Command::Stop { thread: id.clone() }),
                Step::ChangedFiles { message, files } => engine.send(Command::ChangedFiles { thread: id.clone(), message: message.into(), files: files.into_iter().map(str::to_string).collect() }),
            }
            // Saved after every step, as the golden files were made.
            engine.send(Command::Save { thread: id.clone() });
        }
        let (done, finished) = flume::bounded(1);
        engine.send(Command::Quit { done });
        finished.recv_timeout(std::time::Duration::from_secs(10)).expect("the engine quits");
        for output in engine.outputs.try_iter() {
            if let Output::Thread(u) = output {
                t.apply_update(u.changes, u.live, u.plan, u.session_reset, u.turn_ended.is_some());
            }
        }
        let saved: Vec<(usize, Row)> = db.read_rows(&id, 0, 100_000).into_iter().enumerate().collect();
        let _ = std::fs::remove_dir_all(&dir);
        // The rows shown, built from what the engine told, are what was saved.
        for (seq, row) in &saved {
            let (e, _) = t.locate(*seq).expect("a saved row not shown");
            let shown = e.borrow().transcript_row().row(None);
            assert_eq!((&shown.text, &shown.tool_input, &shown.tool_result, &shown.delivery, &shown.changed_files), (&row.text, &row.tool_input, &row.tool_result, &row.delivery, &row.changed_files), "{name} row {seq}");
        }
        let ids = |v: Vec<String>| v.join(",");
        let state = vec![
            format!("running {} activity {:?} session {:?} meta session {:?} stopped by you {}", t.run.running, t.run.activity, t.run.session_id, t.meta.session_id, t.run.stopped_by_you),
            format!("permissions [{}] elicitations [{}]", ids(t.run.permissions.iter().map(|p| format!("{}:{}", p.id, p.description)).collect()), ids(t.run.elicitations.iter().map(|e| e.id.clone()).collect())),
            format!("queue [{}] restored [{}]", ids(t.run.queue.iter().map(|q| q.id.clone()).collect()), ids(t.run.composer.restored.iter().map(|r| r.0.clone()).collect())),
            format!("context {:?}/{:?} mcp {:?}", t.run.composer.context_tokens, t.run.composer.context_window, t.run.composer.mcp_servers),
            format!("jobs running {} agents {} ended {} total {:?}", t.run.jobs.running.len(), t.run.jobs.agents.len(), t.run.jobs.ended.len(), t.run.total),
            format!("subagents [{}]", ids(t.subagents().iter().map(|(id, e)| format!("{id}:{}", e.borrow().sub.as_ref().map_or("-".to_string(), |s| format!("{}/{}/{:?}", s.status, s.steps, s.activity)))).collect())),
        ];
        yh_core::golden::describe(&saved, &state)
    }

    #[test]
    fn saves_what_the_golden_files_say() {
        for (name, text) in yh_core::golden::SESSIONS {
            let steps = yh_core::golden::session_events(text).into_iter().map(yh_core::golden::Step::Event).collect();
            yh_core::golden::check(name, &golden(name, steps));
        }
        yh_core::golden::check("synthetic", &golden("synthetic", yh_core::golden::synthetic()));
    }

    #[test]
    fn the_panel_lists_saved_runs_with_the_held_ones_and_reads_one_whole_on_demand() {
        let mut t = ThreadState::new(ThreadMeta::new("T".into(), "P".into(), "t".into()));
        // The newest page holds rows 10.. with one run; rows 0..10 aren't held.
        t.prepend(vec![EntryData::new("u", Kind::User, "hi"), agent("a3", "Held", None)], 10);
        assert_eq!(t.subagents().len(), 1);
        assert!(t.needs_saved_runs());
        t.mark_saved_runs_read();
        // What the database lists: two older runs, and the held one (held wins).
        t.set_saved_runs(vec![call(2, "a1", "First"), call(6, "a2", "Second"), call(11, "a3", "Saved copy")]);
        let runs = t.subagents();
        assert_eq!(runs.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), vec!["a1", "a2", "a3"]);
        assert_eq!(runs[2].1.borrow().data.tool_result.as_deref(), Some("done"));
        assert_eq!(runs[0].1.borrow().sub.as_ref().map(|s| (s.description.as_str(), s.steps, s.log.len())), Some(("First", 3, 0)));
        // Picked: read whole, steps and all.
        assert!(t.wants_run_detail("a1") && !t.wants_run_detail("a3"));
        t.mark_run_detail("a1");
        assert!(!t.wants_run_detail("a1"));
        let run = SubagentRecord {
            description: "First".into(),
            status: "completed".into(),
            report: Some("Found it".into()),
            entries: vec![SubagentEntry { id: "x".into(), kind: "tool".into(), tool_name: Some("Read".into()), tool_input: Some("{}".into()), ..Default::default() }],
            ..Default::default()
        };
        t.set_run_detail("a1", Some((2, agent("a1", "First", Some(&run)))));
        let a1 = t.subagents()[0].1.clone();
        assert_eq!(a1.borrow().sub.as_ref().map(|s| (s.log.len(), s.report.clone())), Some((1, Some("Found it".into()))));
        // Another picked: the first goes back to a summary.
        t.mark_run_detail("a2");
        t.set_run_detail("a2", Some((6, agent("a2", "Second", Some(&run)))));
        assert!(t.subagents()[0].1.borrow().sub.as_ref().is_some_and(|s| s.log.is_empty()));
        // Let go of: the held run stays listed, summarized.
        t.unload();
        let runs = t.subagents();
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[2].1.borrow().data.tool_input.as_deref(), Some(r#"{"description":"Held","prompt":"go"}"#));
        // Read back: it's held again, its summary gone.
        t.prepend(vec![agent("a3", "Held", None)], 11);
        assert_eq!(t.subagents().len(), 3);
        assert!(!t.saved_runs.contains_key("a3"));
    }
}
