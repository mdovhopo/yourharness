//! A thread's rows as the app holds them: only the newest page or two of a long history;
//! earlier pages are read when you scroll up to them. A streamed delta changes its entry in
//! place and tells the list to measure just that row again (`ListState::splice`).

use crate::claude::{Block, BlockKind, DeltaKind, Event};
use crate::library::{EntryData, Kind, ThreadMeta};
use crate::markdown::Markdown;
use gpui::{px, Context, ListAlignment, ListOffset, ListState};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Clone, Debug)]
pub struct Subagent {
    pub description: String,
    pub status: String,
    pub steps: usize,
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
}

pub type EntryRef = Rc<RefCell<Entry>>;

fn entry(data: EntryData) -> EntryRef {
    Rc::new(RefCell::new(Entry { data, live: false, sub: None, version: 0, open: false, markdown: None }))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemType {
    User,
    Text,
    Work,
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

fn is_work(kind: Kind) -> bool {
    matches!(kind, Kind::Thinking | Kind::Tool)
}

fn item_type(kind: Kind) -> ItemType {
    match kind {
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
    pub permission: Option<Permission>,
    /// Rows of the history before the first one held.
    pub before: usize,
    pub loaded: bool,
    pub loading: bool,
    pub loading_earlier: bool,
    /// Its transcript is made (open or kept for going back): all its rows stay.
    pub in_deck: bool,
    pub session_id: Option<String>,
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
            permission: None,
            before: 0,
            loaded: false,
            loading: false,
            loading_earlier: false,
            in_deck: false,
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
        for data in rows {
            let kind = data.kind;
            let e = entry(data);
            match items.last_mut() {
                Some(last) if is_work(kind) && last.ty == ItemType::Work => last.entries.push(e),
                _ => items.push(Item { pos: 0, ty: item_type(kind), entries: vec![e], open: false }),
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
    }

    /// Puts a new entry at the end, folding work into the work row before it.
    fn append(&mut self, data: EntryData) -> EntryRef {
        let kind = data.kind;
        let e = entry(data);
        let pos = match self.items.last_mut() {
            Some(last) if is_work(kind) && last.ty == ItemType::Work => {
                last.entries.push(e.clone());
                let pos = last.pos;
                self.touch_pos(pos);
                pos
            }
            _ => {
                let pos = self.items.last().map_or(0, |i| i.pos + 1);
                self.items.push(Item { pos, ty: item_type(kind), entries: vec![e.clone()], open: false });
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

    pub fn add_user(&mut self, text: &str) {
        self.next_note += 1;
        let id = format!("u{}", self.next_note);
        self.append(EntryData::new(id, Kind::User, text));
    }

    pub fn notice(&mut self, text: &str) {
        self.next_note += 1;
        let id = format!("n{}", self.next_note);
        self.append(EntryData::new(id, Kind::Notice, text));
    }

    /// Applies one event from this thread's `claude`. True when what the sidebar shows changed.
    pub fn apply(&mut self, event: Event, _cx: &mut Context<Self>) -> bool {
        match event {
            Event::Session { id } => {
                self.session_id = Some(id);
                false
            }
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
                for (index, block) in blocks.into_iter().enumerate() {
                    let key = format!("{message}:{index}");
                    let existing = self.blocks.get(&key).cloned().or_else(|| match &block {
                        Block::Tool { id, .. } => self.tools.get(id).map(|(e, _)| e.clone()),
                        _ => None,
                    });
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
                for r in results {
                    let Some((e, pos)) = self.tools.get(&r.id).cloned() else { continue };
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
                false
            }
            Event::SubagentStarted { tool_id, description } => {
                if let Some((e, pos)) = self.tools.get(&tool_id).cloned() {
                    e.borrow_mut().sub = Some(Subagent { description, status: "running".into(), steps: 0 });
                    e.borrow_mut().version += 1;
                    self.touch_pos(pos);
                }
                false
            }
            Event::SubagentFinished { tool_id, status } => {
                if let Some((e, pos)) = self.tools.get(&tool_id).cloned() {
                    {
                        let mut b = e.borrow_mut();
                        let sub = b.sub.get_or_insert(Subagent { description: String::new(), status: String::new(), steps: 0 });
                        sub.status = status;
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
        {
            let mut b = e.borrow_mut();
            let sub = b.sub.get_or_insert(Subagent { description: "Sub-agent".into(), status: "running".into(), steps: 0 });
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
