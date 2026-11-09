//! The phone app's development server (docs/mobile.md): a data folder's `harness.sqlite`,
//! opened read-only, served live over a WebSocket. Temporary: `yh-server` (docs/remote.md)
//! replaces it, speaking the same messages.
//!
//! One poller thread looks for rows written since the last `rev` and for changes to the
//! thread list, and queues messages for the clients that asked. Each connection has a thread
//! of its own, its own read-only connection to the database, and a bounded outbox: a client
//! that falls behind is dropped, and reconnects for a fresh snapshot.

pub mod source;
pub mod wire;

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::{Message, WebSocket};

use source::Source;
use wire::{FromApp, Project, Thread, ToApp};

/// How often the database is looked at for new rows; the thread list every `LIST_EVERY`th time.
const POLL: Duration = Duration::from_millis(100);
const LIST_EVERY: u32 = 5;
/// A thread whose rows changed this recently shows as active.
const ACTIVE_FOR: Duration = Duration::from_secs(4);
const OUTBOX: usize = 4096;
const DEFAULT_ROWS: i64 = 160;
const MAX_ROWS: i64 = 1000;

pub struct Options {
    /// Required as `?token=` when set.
    pub token: Option<String>,
    pub name: String,
}

struct Client {
    outbox: flume::Sender<String>,
    app: bool,
    thread: Option<String>,
}

#[derive(Default)]
struct Shared {
    clients: Mutex<HashMap<u64, Client>>,
    listing: Mutex<(Vec<Project>, Vec<Thread>)>,
}

impl Shared {
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
pub fn serve(listener: TcpListener, directory: PathBuf, options: Options) -> Result<(), String> {
    let shared = Arc::new(Shared::default());
    let source = Source::open(&directory)?;
    let options = Arc::new(options);
    std::thread::Builder::new()
        .name("poller".into())
        .spawn({
            let shared = shared.clone();
            move || poll(source, &shared)
        })
        .map_err(|e| e.to_string())?;
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

fn poll(source: Source, shared: &Shared) {
    let mut rev = source.max_rev().unwrap_or(0);
    let mut changed_at: HashMap<String, Instant> = HashMap::new();
    let mut tick = 0u32;
    refresh_listing(&source, shared, &changed_at, true);
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
            let watched = shared.clients.lock().unwrap().values().any(|c| c.thread.as_deref() == Some(&thread));
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
    for t in &mut threads {
        t.active = changed_at.get(&t.id).is_some_and(|at| now - *at <= ACTIVE_FOR);
    }
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
    shared.clients.lock().unwrap().insert(id, Client { outbox, app: false, thread: None });
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
            // Watch first, then read: a row written in between comes twice, and `rev` sorts it out.
            set(&|c| c.thread = Some(thread.clone()));
            let limit = limit.unwrap_or(DEFAULT_ROWS).clamp(1, MAX_ROWS);
            let count = match source.count(&thread) {
                Ok(n) => n,
                Err(e) => return failed(e),
            };
            let first_seq = (count - limit).max(0);
            match source.rows(&thread, first_seq, count) {
                Ok(rows) => vec![ToApp::Snapshot { thread, first_seq, count, rows }],
                Err(e) => failed(e),
            }
        }
        FromApp::UnsubscribeThread => {
            set(&|c| c.thread = None);
            vec![]
        }
        FromApp::Page { thread, before_seq, limit } => {
            let limit = limit.unwrap_or(DEFAULT_ROWS).clamp(1, MAX_ROWS);
            match source.rows(&thread, (before_seq - limit).max(0), before_seq) {
                Ok(rows) => vec![ToApp::PageRows { thread, rows }],
                Err(e) => failed(e),
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
