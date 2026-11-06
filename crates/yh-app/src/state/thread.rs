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
    pub markdown: Option<(u64, Rc<Markdown>)>,
    /// Its row number in the whole saved transcript.
    pub seq: usize,
    /// When it was written (seconds since 1970), for rows made here.
    pub date: Option<f64>,
    /// Your message, sent while Claude works, waiting for the current step to end.
    pub queued: bool,
    /// How your message reached Claude: `steered` (joined the turn), `interrupted` (stopped it).
    pub delivery: Option<String>,
}

pub type EntryRef = Rc<RefCell<Entry>>;

fn entry(data: EntryData, seq: usize) -> EntryRef {
    Rc::new(RefCell::new(Entry {
        data,
        live: false,
        sub: None,
        version: 0,
        open: false,
        markdown: None,
        seq,
        date: None,
        queued: false,
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
            delivery: self.delivery.clone(),
            attachments: (!self.data.attachments.is_empty()).then(|| serde_json::to_string(&self.data.attachments).unwrap_or_default()),
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
}

/// One row on screen: a message, or a run of thinking and tool calls folded into one.
pub struct Item {
    /// Rows are numbered without gaps, so a row's index is its number less the first one's.
    pub pos: i64,
    pub ty: ItemType,
    pub entries: Vec<EntryRef>,
    pub open: bool,
}

/// Tools whose call is a sub-agent: shown as a card of its own.
pub const AGENT_TOOLS: &[&str] = &["Agent", "Task"];

fn item_type(data: &EntryData) -> ItemType {
    match data.kind {
        Kind::Tool if data.tool_name.as_deref().is_some_and(|n| AGENT_TOOLS.contains(&n)) => ItemType::Agent,
        Kind::Thinking | Kind::Tool => ItemType::Work,
        Kind::User => ItemType::User,
        Kind::Assistant => ItemType::Text,
        Kind::Notice | Kind::Command => ItemType::Notice,
    }
}

#[derive(Clone, Debug)]
pub struct Permission {
    pub id: String,
    pub tool: String,
    pub description: String,
    pub input: String,
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
    pub permission: Option<Permission>,
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
    next_note: u64,
}

impl ThreadState {
    pub fn new(meta: ThreadMeta) -> Self {
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
            permission: None,
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
            next_note: 0,
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
        self.refresh_plan();
        self.before = before;
        if was_empty {
            self.list.reset(self.items.len());
        } else {
            self.list.splice(0..0, count as usize);
            self.list.scroll_to(ListOffset { item_ix: top.item_ix + count as usize, offset_in_item: top.offset_in_item });
        }
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

    /// Your message, as sent: `id` is what the CLI's lifecycle events name it by.
    pub fn add_user(&mut self, text: &str, images: &[String], id: &str, queued: bool, delivery: Option<&str>) {
        let mut data = EntryData::new(id, Kind::User, text);
        data.attachments = images.to_vec();
        let e = self.append(data);
        let mut b = e.borrow_mut();
        b.queued = queued;
        b.delivery = delivery.map(str::to_string);
    }

    /// The CLI says where your message is.
    fn lifecycle(&mut self, id: &str, state: &str) {
        let found = self.items.iter().rev().take(40).flat_map(|i| i.entries.iter()).find(|e| e.borrow().data.id == id).cloned();
        let Some(e) = found else { return };
        let queued = state == "queued";
        if e.borrow().queued != queued {
            e.borrow_mut().queued = queued;
            self.touch(&e);
        }
        if state == "cancelled" {
            self.notice("That message was withdrawn before Claude read it.");
        }
    }

    pub fn notice(&mut self, text: &str) {
        self.next_note += 1;
        let id = format!("n{}", self.next_note);
        self.append(EntryData::new(id, Kind::Notice, text));
    }

    /// Rows whose text, tool input or result has `query` (lower case), oldest first, by number.
    pub fn find(&self, query: &str) -> Vec<i64> {
        self.items
            .iter()
            .filter(|item| {
                item.entries.iter().any(|e| {
                    let b = e.borrow();
                    let has = |s: &str| s.to_lowercase().contains(query);
                    has(&b.data.text) || b.data.tool_input.as_deref().is_some_and(has) || b.data.tool_result.as_deref().is_some_and(has)
                })
            })
            .map(|i| i.pos)
            .collect()
    }

    /// Find moves to a row: it's shown and marked.
    pub fn show_hit(&mut self, pos: Option<i64>) {
        let old = self.find_hit;
        self.find_hit = pos;
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
        for item in &self.items {
            for e in &item.entries {
                let b = e.borrow();
                if let Some(name) = b.data.tool_name.as_deref().filter(|n| yh_core::plan::TOOLS.contains(n)) {
                    plan.apply(name, b.data.tool_input.as_deref().unwrap_or(""), b.data.tool_result.as_deref(), &b.data.id);
                }
            }
        }
        if std::env::var("YH_EVENT_LOG").as_deref() == Ok("1") {
            eprintln!("plan: {:?}", plan.items.iter().map(|i| (&i.id, &i.subject, i.status)).collect::<Vec<_>>());
        }
        self.plan = plan;
    }

    /// Applies one event from this thread's `claude`. True when what the sidebar shows changed.
    pub fn apply(&mut self, event: Event, _cx: &mut Context<Self>) -> bool {
        match event {
            Event::Session { id } => {
                self.session_id = Some(id);
                false
            }
            Event::Lifecycle { id, state } => {
                self.lifecycle(&id, &state);
                false
            }
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
                false
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
                if parent.is_some() {
                    return false;
                }
                let touches_plan = results.iter().any(|r| {
                    self.tools.get(&r.id).is_some_and(|(e, _)| {
                        e.borrow().data.tool_name.as_deref().is_some_and(|n| yh_core::plan::TOOLS.contains(&n))
                    })
                });
                for r in results {
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
                false
            }
            Event::SubagentStarted { tool_id, description } => {
                if let Some((e, pos)) = self.tools.get(&tool_id).cloned() {
                    e.borrow_mut().sub = Some(Subagent { description, status: "running".into(), steps: 0, report: None });
                    e.borrow_mut().version += 1;
                    self.touch_pos(pos);
                }
                false
            }
            Event::SubagentFinished { tool_id, status, summary } => {
                if let Some((e, pos)) = self.tools.get(&tool_id).cloned() {
                    {
                        let mut b = e.borrow_mut();
                        let sub = b.sub.get_or_insert(Subagent { description: String::new(), status: String::new(), steps: 0, report: None });
                        sub.status = status;
                        if summary.is_some() {
                            sub.report = summary;
                        }
                        b.version += 1;
                    }
                    self.touch_pos(pos);
                }
                false
            }
            Event::Permission { request_id, tool, description, input } => {
                self.permission = Some(Permission { id: request_id, tool, description, input });
                true
            }
            Event::Notice(text) => {
                self.notice(&text);
                false
            }
            Event::Done { is_error, text } => {
                self.end_turn();
                if is_error && !text.is_empty() {
                    self.notice(&text);
                }
                true
            }
            Event::Exit { code, tail } => {
                self.end_turn();
                if code != 0 {
                    self.notice(&format!("claude exited ({code}) {tail}"));
                }
                true
            }
        }
    }

    fn subagent_step(&mut self, parent: &str) {
        let Some((e, pos)) = self.tools.get(parent).cloned() else { return };
        self.dirty.insert(e.borrow().seq);
        {
            let mut b = e.borrow_mut();
            let sub = b.sub.get_or_insert(Subagent { description: "Sub-agent".into(), status: "running".into(), steps: 0, report: None });
            sub.steps += 1;
            b.version += 1;
        }
        self.touch_pos(pos);
    }

    fn end_turn(&mut self) {
        let live: Vec<EntryRef> = self.blocks.values().filter(|e| e.borrow().live).cloned().collect();
        for e in live {
            e.borrow_mut().live = false;
            self.touch(&e);
        }
        self.blocks.clear();
        self.running = false;
        self.permission = None;
    }
}
