//! The app's server for its clients (the phone app; docs/remote.md, docs/mobile.md): the
//! threads and their rows live, what each thread's run is doing, and actions (send, stop, allow
//! or deny, withdraw, send now), over a WebSocket, in `yh_proto`'s messages. It runs inside
//! YourHarness, off by default (`YH_SERVER`), loopback only, a token required.
//!
//! The thread list comes from the database, as the phone's development server read it: one
//! poller thread looks for changes to it. A watched thread's rows and what runs (working,
//! requests waiting on you, the queue) come from the engine (`yh_engine::Output`), as Claude
//! writes: the engine's snapshot when a client starts watching, told in order with its changes,
//! then each row changed since, whole, at most every `STREAM_EVERY`. Without the engine (a
//! read-only server over a data folder) rows come from the database as it's written. Actions go to the app (`Action`), which takes them as it
//! takes a click. Each connection has a thread of its own, its own read-only connection to the
//! database, and a bounded outbox: a client that falls behind is dropped, and reconnects for a
//! fresh snapshot.

pub mod source;
pub use yh_proto as wire;

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::{Message, WebSocket};

use source::Source;
use wire::{FromApp, Live, Project, Thread, ToApp};

/// How often the database is looked at for new rows; the thread list every `LIST_EVERY`th time.
const POLL: Duration = Duration::from_millis(100);
const LIST_EVERY: u32 = 5;
/// A thread whose rows changed this recently shows as active.
const ACTIVE_FOR: Duration = Duration::from_secs(4);
const OUTBOX: usize = 4096;
/// How often rows changed by the engine go out: often enough to read as streaming.
const STREAM_EVERY: Duration = Duration::from_millis(50);
/// Rows held per watched thread for streaming: changes to earlier ones are left to `page`.
const MIRROR_ROWS: usize = 600;
const DEFAULT_ROWS: i64 = 160;
const MAX_ROWS: i64 = 1000;

pub struct Options {
    /// Required as `?token=` when set.
    pub token: Option<String>,
    pub name: String,
}

/// What a client asks of the app: taken on the app's main thread, as a click would be.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Send { thread: String, text: String, now: bool },
    Stop { thread: String },
    Respond { thread: String, request_id: String, allow: bool },
    Withdraw { thread: String, message: String },
    SendNow { thread: String, message: String },
}

/// Where the server meets the app: the engine (what it says of every thread, its snapshots),
/// and where actions go. Without them it serves only what's saved, as the development server
/// did.
pub struct Links {
    pub engine: yh_engine::EngineHandle,
    pub actions: flume::Sender<Action>,
}

/// A row as the wire carries it, its text and tool output trimmed.
pub fn wire_row(r: &yh_core::thread_run::TranscriptRow, rev: i64) -> wire::Row {
    let saved = r.row(None);
    let d = &r.data;
    wire::Row {
        seq: r.seq as i64,
        rev,
        id: saved.id,
        kind: saved.kind,
        text: saved.text,
        tool_name: saved.tool_name,
        tool_input: saved.tool_input.map(|s| wire::trim(s, wire::INPUT_LIMIT)),
        tool_result: saved.tool_result.map(|s| wire::trim(s, wire::RESULT_LIMIT)),
        tool_result_length: d.tool_result.as_ref().map(|t| d.result_len.max(t.chars().count()) as i64),
        tool_is_error: saved.tool_is_error,
        notice_style: saved.notice_style,
        date: saved.date,
        turn_ended_at: saved.turn_ended_at,
        attachments: d.attachments.clone(),
        changed_files: d.changed_files.clone().unwrap_or_default(),
    }
}

/// A watched thread's newest rows as the engine told them, for streaming whole rows.
#[derive(Default)]
struct Mirror {
    rows: std::collections::BTreeMap<usize, yh_core::thread_run::TranscriptRow>,
    count: usize,
}

/// A thread's live state as the wire says it.
pub fn live_of(s: &yh_core::thread_run::LiveState) -> Live {
    Live {
        running: s.running,
        activity: s.activity.clone(),
        requests: s.permissions.iter().map(|p| wire::Request { id: p.id.clone(), tool: p.tool.clone(), description: p.description.clone(), input: wire::trim(p.input.clone(), wire::INPUT_LIMIT) }).collect(),
        questions: s.elicitations.iter().map(|e| wire::Question { id: e.id.clone(), server: e.server.clone(), message: e.message.clone() }).collect(),
        queue: s.queue.iter().map(|q| wire::Queued { id: q.id.clone(), text: q.text.clone() }).collect(),
        context_tokens: s.composer.context_tokens,
        context_window: s.composer.context_window,
    }
}

struct Client {
    outbox: flume::Sender<String>,
    app: bool,
    thread: Option<String>,
    /// Its thread's snapshot was sent: changes go to it from now on.
    ready: bool,
}

#[derive(Default)]
struct Shared {
    clients: Mutex<HashMap<u64, Client>>,
    listing: Mutex<(Vec<Project>, Vec<Thread>)>,
    /// Each thread's live state, as the engine last said it.
    live: Mutex<HashMap<String, Live>>,
    actions: Option<flume::Sender<Action>>,
    engine: Option<yh_engine::EngineHandle>,
    /// The `rev` streamed rows take: above anything saved, so a client keeps the newest copy.
    rev: std::sync::atomic::AtomicI64,
}

impl Shared {
    /// Queues `messages` for one client, after `then` changes what it's sent from now on.
    fn send_to(&self, id: u64, messages: &[ToApp], then: impl FnOnce(&mut Client)) {
        let mut clients = self.clients.lock().unwrap();
        let Some(c) = clients.get_mut(&id) else { return };
        let ok = messages.iter().all(|m| c.outbox.try_send(serde_json::to_string(m).expect("messages serialize")).is_ok());
        then(c);
        if !ok {
            eprintln!("client {id}: fell behind, dropped");
            clients.remove(&id);
        }
    }

    /// Queues `message` for the clients `wants` picks; a client whose outbox is full is dropped.
    fn send(&self, message: &ToApp, wants: impl Fn(&Client) -> bool) {
        let text = serde_json::to_string(message).expect("messages serialize");
        self.clients.lock().unwrap().retain(|id, c| {
            if !wants(c) {
                return true;
            }
            let ok = c.outbox.try_send(text.clone()).is_ok();
            if !ok {
                eprintln!("client {id}: fell behind, dropped");
            }
            ok
        });
    }
}

/// Serves `directory` on `listener` until the process ends.
pub fn serve(listener: TcpListener, directory: PathBuf, options: Options, links: Option<Links>) -> Result<(), String> {
    let (engine, actions) = match links {
        Some(l) => (Some(l.engine), Some(l.actions)),
        None => (None, None),
    };
    // Subscribed before anyone connects, so no change is missed.
    let told = engine.as_ref().map(|e| e.subscribe());
    let shared = Arc::new(Shared { actions, engine, ..Default::default() });
    let source = Source::open(&directory)?;
    shared.rev.store(source.max_rev().unwrap_or(0), std::sync::atomic::Ordering::SeqCst);
    // The list is read before anyone connects, so the first `threads` is never empty.
    refresh_listing(&source, &shared, &HashMap::new(), true);
    let options = Arc::new(options);
    std::thread::Builder::new()
        .name("poller".into())
        .spawn({
            let shared = shared.clone();
            move || poll(source, &shared)
        })
        .map_err(|e| e.to_string())?;
    if let Some(told) = told {
        let source = Source::open(&directory)?;
        std::thread::Builder::new()
            .name("live".into())
            .spawn({
                let shared = shared.clone();
                move || follow(told, &shared, &source)
            })
            .map_err(|e| e.to_string())?;
    }
    let mut next = 0u64;
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        next += 1;
        let (id, shared, options, directory) = (next, shared.clone(), options.clone(), directory.clone());
        let _ = std::thread::Builder::new().name(format!("client {id}")).spawn(move || {
            if let Err(e) = connection(id, stream, &shared, &options, &directory) {
                eprintln!("client {id}: {e}");
            }
            shared.clients.lock().unwrap().remove(&id);
        });
    }
    Ok(())
}

/// What the engine says: snapshots to the clients that asked, each watched thread's rows as they
/// change (gathered, then sent whole at most every `STREAM_EVERY`), a thread's live state to the
/// clients watching it, and the thread list's `running` and `waiting` when those change.
fn follow(told: flume::Receiver<yh_engine::Output>, shared: &Shared, source: &Source) {
    let mut mirrors: HashMap<String, Mirror> = HashMap::new();
    let mut changed: HashMap<String, std::collections::BTreeSet<usize>> = HashMap::new();
    let mut sent_at = Instant::now();
    loop {
        let first = match told.recv_timeout(STREAM_EVERY) {
            Ok(o) => Some(o),
            Err(flume::RecvTimeoutError::Timeout) => None,
            Err(flume::RecvTimeoutError::Disconnected) => return,
        };
        for output in first.into_iter().chain(told.try_iter()) {
            match output {
                yh_engine::Output::Thread(u) => {
                    if let Some(m) = mirrors.get_mut(&u.thread) {
                        let seqs = changed.entry(u.thread.clone()).or_default();
                        for change in u.changes {
                            apply(m, change, seqs);
                        }
                        m.count = u.live.total.unwrap_or(m.count).max(m.count);
                        while m.rows.len() > MIRROR_ROWS {
                            m.rows.pop_first();
                        }
                    }
                    tell_live(shared, &u.thread, live_of(&u.live));
                }
                yh_engine::Output::Snapshot { thread, tag, first_seq, count, rows, live } => {
                    // Above anything saved by now, so what streams after it wins.
                    let rev = shared.rev.fetch_max(source.max_rev().unwrap_or(0), std::sync::atomic::Ordering::SeqCst).max(source.max_rev().unwrap_or(0));
                    let wire_rows: Vec<wire::Row> = rows.iter().map(|r| wire_row(r, rev)).collect();
                    mirrors.insert(thread.clone(), Mirror { rows: rows.into_iter().map(|r| (r.seq, r)).collect(), count });
                    let snapshot = ToApp::Snapshot { thread: thread.clone(), first_seq: first_seq as i64, count: count as i64, rows: wire_rows };
                    let live = live_of(&live);
                    shared.live.lock().unwrap().insert(thread.clone(), live.clone());
                    shared.send_to(tag, &[snapshot, ToApp::Live { thread: thread.clone(), live }], |c| {
                        if c.thread.as_deref() == Some(&thread) {
                            c.ready = true;
                        }
                    });
                }
                _ => {}
            }
        }
        if sent_at.elapsed() < STREAM_EVERY {
            continue;
        }
        sent_at = Instant::now();
        for (thread, seqs) in changed.drain() {
            let Some(m) = mirrors.get(&thread) else { continue };
            let rows: Vec<wire::Row> = seqs
                .iter()
                .filter_map(|seq| m.rows.get(seq))
                .map(|r| wire_row(r, shared.rev.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1))
                .collect();
            if rows.is_empty() {
                continue;
            }
            let message = ToApp::RowsChanged { thread: thread.clone(), count: m.count as i64, rows };
            shared.send(&message, |c| c.ready && c.thread.as_deref() == Some(&thread));
        }
        // Threads nobody watches any more aren't held.
        let watched: std::collections::HashSet<String> = shared.clients.lock().unwrap().values().filter_map(|c| c.thread.clone()).collect();
        mirrors.retain(|t, _| watched.contains(t));
    }
}

/// One change the engine told into a mirror; the row's number goes into `seqs`.
fn apply(m: &mut Mirror, change: yh_core::thread_run::RowChange, seqs: &mut std::collections::BTreeSet<usize>) {
    use yh_core::thread_run::{RowChange, TextPatch};
    match change {
        RowChange::Added(r) => {
            m.count = m.count.max(r.seq + 1);
            seqs.insert(r.seq);
            m.rows.insert(r.seq, r);
        }
        RowChange::Changed { seq, text, input, row } => match row {
            Some(r) => {
                seqs.insert(seq);
                m.rows.insert(seq, *r);
            }
            None => {
                // A row older than what's held: its earlier text isn't here to add to.
                let Some(r) = m.rows.get_mut(&seq) else { return };
                text.apply_to(&mut r.data.text);
                if input != TextPatch::Same {
                    input.apply_to(r.data.tool_input.get_or_insert_with(String::new));
                }
                seqs.insert(seq);
            }
        },
    }
}

/// A thread's live state to the clients watching it when it changed, and the list's `running`
/// and `waiting` to the app subscribers when those did.
fn tell_live(shared: &Shared, thread: &str, live: Live) {
    let was = shared.live.lock().unwrap().insert(thread.to_string(), live.clone());
    if was.as_ref() == Some(&live) {
        return;
    }
    shared.send(&ToApp::Live { thread: thread.to_string(), live: live.clone() }, |c| c.thread.as_deref() == Some(thread));
    let flags = |l: &Live| (l.running, !l.requests.is_empty() || !l.questions.is_empty());
    if was.as_ref().map(flags).unwrap_or((false, false)) != flags(&live) {
        let changed = {
            let mut listing = shared.listing.lock().unwrap();
            listing.1.iter_mut().find(|t| t.id == thread).map(|t| {
                (t.running, t.waiting) = flags(&live);
                t.active |= t.running;
                t.clone()
            })
        };
        if let Some(t) = changed {
            shared.send(&ToApp::ThreadUpserted { thread: t }, |c| c.app);
        }
    }
}

fn poll(source: Source, shared: &Shared) {
    let mut rev = source.max_rev().unwrap_or(0);
    let mut changed_at: HashMap<String, Instant> = HashMap::new();
    let mut tick = 0u32;
    loop {
        std::thread::sleep(POLL);
        tick += 1;
        let changes = match source.changes_since(rev, 5000) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("poller: {e}");
                continue;
            }
        };
        let mut threads: Vec<(String, Vec<wire::Row>)> = vec![];
        for change in changes {
            rev = rev.max(change.row.rev);
            match threads.iter_mut().find(|(t, _)| *t == change.thread) {
                Some((_, rows)) => rows.push(change.row),
                None => threads.push((change.thread, vec![change.row])),
            }
        }
        let now = Instant::now();
        let mut became_active = false;
        for (thread, mut rows) in threads {
            became_active |= changed_at.insert(thread.clone(), now).is_none_or(|t| now - t > ACTIVE_FOR);
            // With the engine, its changes stream instead (`follow`).
            let watched = shared.engine.is_none() && shared.clients.lock().unwrap().values().any(|c| c.thread.as_deref() == Some(&thread));
            if !watched {
                continue;
            }
            // A row written twice since the last look is sent once, its newest copy.
            rows.sort_by_key(|r| (r.seq, std::cmp::Reverse(r.rev)));
            rows.dedup_by_key(|r| r.seq);
            let count = source.count(&thread).unwrap_or(0);
            let message = ToApp::RowsChanged { thread: thread.clone(), count, rows };
            shared.send(&message, |c| c.thread.as_deref() == Some(&thread));
        }
        let went_quiet = changed_at.values().any(|t| (now - *t > ACTIVE_FOR) && (now - *t <= ACTIVE_FOR + POLL * 2));
        if became_active || went_quiet || tick % LIST_EVERY == 0 {
            refresh_listing(&source, shared, &changed_at, false);
        }
        changed_at.retain(|_, t| now - *t < ACTIVE_FOR * 4);
    }
}

/// Reads projects and threads again and tells app subscribers what changed: the whole list
/// when projects or the order changed, else each thread that did.
fn refresh_listing(source: &Source, shared: &Shared, changed_at: &HashMap<String, Instant>, first: bool) {
    let (Ok(projects), Ok(mut threads)) = (source.projects(), source.threads()) else { return };
    let now = Instant::now();
    let live = shared.live.lock().unwrap();
    for t in &mut threads {
        if let Some(l) = live.get(&t.id) {
            t.running = l.running;
            t.waiting = !l.requests.is_empty() || !l.questions.is_empty();
        }
        t.active = t.running || changed_at.get(&t.id).is_some_and(|at| now - *at <= ACTIVE_FOR);
    }
    drop(live);
    let mut listing = shared.listing.lock().unwrap();
    let (old_projects, old_threads) = std::mem::replace(&mut *listing, (projects.clone(), threads.clone()));
    drop(listing);
    if first {
        return;
    }
    let same_order = old_threads.len() == threads.len() && old_threads.iter().zip(&threads).all(|(a, b)| a.id == b.id);
    if old_projects != projects || !same_order {
        let removed: Vec<String> = old_threads.iter().filter(|o| !threads.iter().any(|t| t.id == o.id)).map(|o| o.id.clone()).collect();
        for id in removed {
            shared.send(&ToApp::ThreadRemoved { id }, |c| c.app);
        }
        shared.send(&ToApp::Threads { projects, threads }, |c| c.app);
        return;
    }
    for (old, new) in old_threads.iter().zip(threads) {
        if *old != new {
            shared.send(&ToApp::ThreadUpserted { thread: new }, |c| c.app);
        }
    }
}

fn connection(id: u64, stream: TcpStream, shared: &Shared, options: &Options, directory: &Path) -> Result<(), String> {
    let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
    let token = options.token.clone();
    let check = move |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
        let Some(token) = &token else { return Ok(response) };
        let given = request.uri().query().unwrap_or("").split('&').find_map(|kv| kv.strip_prefix("token="));
        if given == Some(token.as_str()) {
            Ok(response)
        } else {
            Err(tungstenite::http::Response::builder().status(401).body(Some("wrong or missing token".into())).unwrap())
        }
    };
    let mut ws = tungstenite::accept_hdr(stream, check).map_err(|e| format!("handshake from {peer}: {e}"))?;
    ws.get_ref().set_read_timeout(Some(Duration::from_millis(20))).map_err(|e| e.to_string())?;
    ws.get_ref().set_nodelay(true).ok();
    eprintln!("client {id}: connected from {peer}");
    let source = Source::open(directory)?;
    let (outbox, inbox) = flume::bounded(OUTBOX);
    shared.clients.lock().unwrap().insert(id, Client { outbox, app: false, thread: None, ready: false });
    let hello = ToApp::Hello { protocol: wire::PROTOCOL, server: options.name.clone(), data: name_of(directory) };
    write(&mut ws, &hello)?;
    loop {
        match ws.read() {
            Ok(Message::Text(text)) => {
                let message = serde_json::from_str::<FromApp>(&text).unwrap_or(FromApp::Unknown);
                for reply in answer(id, message, shared, &source) {
                    write(&mut ws, &reply)?;
                }
            }
            Ok(Message::Close(_)) | Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                eprintln!("client {id}: closed");
                return Ok(());
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => return Err(e.to_string()),
        }
        loop {
            match inbox.try_recv() {
                Ok(text) => ws.write(Message::text(text)).map_err(|e| e.to_string())?,
                Err(flume::TryRecvError::Empty) => break,
                // Dropped by `Shared::send` for falling behind.
                Err(flume::TryRecvError::Disconnected) => {
                    let _ = ws.close(None);
                    return Err("dropped".into());
                }
            }
        }
        match ws.flush() {
            Ok(()) => {}
            Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// What a client's message asks for: the replies to send it now. Subscribing also changes
/// what the poller sends it.
fn answer(id: u64, message: FromApp, shared: &Shared, source: &Source) -> Vec<ToApp> {
    let set = |f: &dyn Fn(&mut Client)| {
        if let Some(c) = shared.clients.lock().unwrap().get_mut(&id) {
            f(c);
        }
    };
    let failed = |e: rusqlite::Error| vec![ToApp::Error { message: e.to_string() }];
    match message {
        FromApp::SubscribeApp => {
            set(&|c| c.app = true);
            let (projects, threads) = shared.listing.lock().unwrap().clone();
            vec![ToApp::Threads { projects, threads }]
        }
        FromApp::SubscribeThread { thread, limit } => {
            let limit = limit.unwrap_or(DEFAULT_ROWS).clamp(1, MAX_ROWS);
            // With the engine: its snapshot, in order with its changes, comes from `follow`.
            if let Some(engine) = &shared.engine {
                set(&|c| {
                    c.thread = Some(thread.clone());
                    c.ready = false;
                });
                engine.send(yh_engine::Command::Snapshot { thread, limit: limit as usize, tag: id });
                return vec![];
            }
            // Watch first, then read: a row written in between comes twice, and `rev` sorts it out.
            set(&|c| {
                c.thread = Some(thread.clone());
                c.ready = true;
            });
            let count = match source.count(&thread) {
                Ok(n) => n,
                Err(e) => return failed(e),
            };
            let first_seq = (count - limit).max(0);
            let live = shared.live.lock().unwrap().get(&thread).cloned().unwrap_or_default();
            match source.rows(&thread, first_seq, count) {
                Ok(rows) => vec![ToApp::Snapshot { thread: thread.clone(), first_seq, count, rows }, ToApp::Live { thread, live }],
                Err(e) => failed(e),
            }
        }
        FromApp::UnsubscribeThread => {
            set(&|c| {
                c.thread = None;
                c.ready = false;
            });
            vec![]
        }
        FromApp::Page { thread, before_seq, limit } => {
            let limit = limit.unwrap_or(DEFAULT_ROWS).clamp(1, MAX_ROWS);
            match source.rows(&thread, (before_seq - limit).max(0), before_seq) {
                Ok(rows) => vec![ToApp::PageRows { thread, rows }],
                Err(e) => failed(e),
            }
        }
        FromApp::Send { .. } | FromApp::Stop { .. } | FromApp::Respond { .. } | FromApp::Withdraw { .. } | FromApp::SendNow { .. } => {
            let id = message.action_id().map(str::to_string);
            let action = match message {
                FromApp::Send { thread, text, now, .. } => Action::Send { thread, text, now },
                FromApp::Stop { thread, .. } => Action::Stop { thread },
                FromApp::Respond { thread, request_id, allow, .. } => Action::Respond { thread, request_id, allow },
                FromApp::Withdraw { thread, message, .. } => Action::Withdraw { thread, message },
                FromApp::SendNow { thread, message, .. } => Action::SendNow { thread, message },
                _ => unreachable!("an action"),
            };
            let error = match &shared.actions {
                Some(actions) => actions.send(action).err().map(|_| "The app is gone.".to_string()),
                None => Some("This server only reads.".to_string()),
            };
            match id {
                Some(id) => vec![ToApp::Done { id, error }],
                None => error.map(|message| vec![ToApp::Error { message }]).unwrap_or_default(),
            }
        }
        FromApp::Unknown => vec![],
    }
}

fn write(ws: &mut WebSocket<TcpStream>, message: &ToApp) -> Result<(), String> {
    let text = serde_json::to_string(message).expect("messages serialize");
    ws.send(Message::text(text)).map_err(|e| e.to_string())
}

fn name_of(directory: &Path) -> String {
    directory.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}
