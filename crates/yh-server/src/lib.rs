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

pub mod diagram;
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
    /// `images`: (media type, bytes).
    Send { thread: String, text: String, now: bool, images: Vec<(String, Vec<u8>)> },
    Stop { thread: String },
    Respond { thread: String, request_id: String, allow: bool },
    Withdraw { thread: String, message: String },
    SendNow { thread: String, message: String },
    Answer { thread: String, request_id: String, answers: std::collections::BTreeMap<String, String> },
    SkipQuestion { thread: String, request_id: String },
    ApprovePlan { thread: String, request_id: String, mode: String, feedback: Option<String> },
    KeepPlanning { thread: String, request_id: String, feedback: Option<String> },
    AnswerQuestion { thread: String, request_id: String, accept: bool },
    NewThread { project: String, text: Option<String> },
    SetModel { thread: String, model: Option<String> },
    SetEffort { thread: String, effort: Option<String> },
    SetMode { thread: String, mode: String },
    /// A question on the side of `thread`, in its side chat (as `/btw`).
    AskAside { thread: String, text: String },
    Rename { thread: String, title: String },
    Pin { thread: String, on: bool },
    Settle { thread: String, on: bool },
    Delete { thread: String },
}

/// An action and where the app says how it went: the thread it made, or why it couldn't.
pub struct Asked {
    pub action: Action,
    pub reply: flume::Sender<Result<Option<String>, String>>,
}

impl Shared {
    fn options(&self) -> ToApp {
        ToApp::Options { models: self.models.lock().unwrap().clone(), modes: MODES.iter().map(|m| m.to_string()).collect(), commands: self.commands.lock().unwrap().clone() }
    }
}

/// The permission modes a project can have, as the Mac's picker lists them.
pub const MODES: &[&str] = &["auto", "ask", "acceptEdits", "plan", "bypass"];

/// Where the server meets the app: the engine (what it says of every thread, its snapshots),
/// and where actions go. Without them it serves only what's saved, as the development server
/// did.
pub struct Links {
    pub engine: yh_engine::EngineHandle,
    pub actions: flume::Sender<Asked>,
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
        subagent: r.sub.as_ref().map(|sub| subagent_info(sub, d.tool_input.as_deref())),
    }
}

/// A live sub-agent run as its card shows it; what it was asked comes from its call's input.
fn subagent_info(sub: &yh_core::thread_run::Subagent, input: Option<&str>) -> wire::SubagentInfo {
    let input: serde_json::Value = input.and_then(|i| serde_json::from_str(i).ok()).unwrap_or_default();
    wire::SubagentInfo {
        description: sub.description.clone(),
        status: sub.status.clone(),
        agent_type: input["subagent_type"].as_str().map(str::to_string),
        tool_uses: sub.tool_uses.or((sub.steps > 0).then_some(sub.steps as u64)),
        tokens: sub.tokens,
        duration_ms: sub.duration_ms,
        activity: sub.activity.clone(),
        background: input["run_in_background"] == true,
    }
}

/// A sub-agent's whole run for its sheet: from what it is now (live) or as it was saved.
fn subagent_run(sub: &yh_core::thread_run::Subagent, input: Option<&str>) -> wire::SubagentRun {
    let prompt = input.and_then(|i| serde_json::from_str::<serde_json::Value>(i).ok()).and_then(|v| v["prompt"].as_str().map(str::to_string));
    wire::SubagentRun {
        info: subagent_info(sub, input),
        prompt,
        steps: sub
            .log
            .iter()
            .map(|s| wire::SubagentStep { tool: s.tool.clone(), text: wire::trim(s.text.clone(), wire::INPUT_LIMIT), result: s.result.clone().map(|r| wire::trim(r, wire::RESULT_LIMIT)), is_error: s.is_error })
            .collect(),
        report: sub.report.clone(),
    }
}

/// Claude's `show_media` tool (the app's `harness` server): its pictures are the row's.
const MEDIA_TOOL: &str = "mcp__harness__show_media";
const PICTURE_TYPES: &[(&str, &str)] = &[("png", "image/png"), ("jpg", "image/jpeg"), ("jpeg", "image/jpeg"), ("gif", "image/gif"), ("webp", "image/webp"), ("heic", "image/heic")];
const MAX_PICTURE: u64 = 20 * 1024 * 1024;

fn picture_type(name: &str) -> Option<&'static str> {
    let ext = name.rsplit('.').next()?.to_lowercase();
    PICTURE_TYPES.iter().find(|(e, _)| *e == ext).map(|(_, t)| *t)
}

/// A row's pictures, in order, as files on the Mac: your attachments (pictures kept in the
/// app's attachments folder, by name) or what Claude showed you (`show_media`'s paths). Only
/// pictures, and only what the row names: nothing else on the Mac can be asked for.
pub fn pictures(data_dir: &std::path::Path, kind: &str, tool: Option<&str>, input: Option<&str>, attachments: &[String]) -> Vec<std::path::PathBuf> {
    match kind {
        "user" => attachments
            .iter()
            .filter(|n| picture_type(n).is_some() && !n.contains('/') && !n.contains(".."))
            .map(|n| yh_core::attachments::path(data_dir, n))
            .collect(),
        "tool" if tool == Some(MEDIA_TOOL) => {
            let v: serde_json::Value = input.and_then(|i| serde_json::from_str(i).ok()).unwrap_or_default();
            let mut paths: Vec<String> = v["paths"].as_array().map(|a| a.iter().filter_map(|p| p.as_str().map(str::to_string)).collect()).unwrap_or_default();
            if let Some(p) = v["path"].as_str() {
                paths.push(p.to_string());
            }
            paths.into_iter().filter(|p| picture_type(p).is_some()).map(std::path::PathBuf::from).collect()
        }
        _ => vec![],
    }
}

/// A watched thread's newest rows as the engine told them, for streaming whole rows.
#[derive(Default, Clone)]
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
        questions: s.elicitations.iter().map(|e| wire::Question { id: e.id.clone(), server: e.server.clone(), message: e.message.clone(), mode: e.mode.clone(), url: e.url.clone() }).collect(),
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
    /// The models to choose from and each project's slash commands, as last read.
    models: Mutex<Vec<wire::ModelOption>>,
    commands: Mutex<std::collections::BTreeMap<String, Vec<wire::SlashCommand>>>,
    /// Usage limits as last heard (from the engine, or as saved).
    limits: Mutex<Vec<wire::LimitWindow>>,
    /// Folders' files for `@`, kept 30 seconds.
    files: Mutex<HashMap<String, (Instant, Arc<Vec<String>>)>>,
    actions: Option<flume::Sender<Asked>>,
    engine: Option<yh_engine::EngineHandle>,
    /// Each watched thread's newest rows as the engine told them.
    mirrors: Mutex<HashMap<String, Mirror>>,
    /// The data folder: where your attachments are.
    data_dir: PathBuf,
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
    let shared = Arc::new(Shared { actions, engine, data_dir: directory.clone(), ..Default::default() });
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
                    for e in &u.events {
                        if let yh_core::claude::Event::Limits(info) = e {
                            let windows = source::limit_windows(info);
                            let mut held = shared.limits.lock().unwrap();
                            if !windows.is_empty() && *held != windows {
                                *held = windows.clone();
                                drop(held);
                                shared.send(&ToApp::Limits { windows }, |c| c.app);
                            }
                        }
                    }
                    if let Some(m) = shared.mirrors.lock().unwrap().get_mut(&u.thread) {
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
                    shared.mirrors.lock().unwrap().insert(thread.clone(), Mirror { rows: rows.into_iter().map(|r| (r.seq, r)).collect(), count });
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
            let Some(m) = shared.mirrors.lock().unwrap().get(&thread).cloned() else { continue };
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
        shared.mirrors.lock().unwrap().retain(|t, _| watched.contains(t));
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
    let mut options_changed = false;
    if let Ok(models) = source.models() {
        let mut held = shared.models.lock().unwrap();
        options_changed |= *held != models;
        *held = models;
    }
    if let Ok(commands) = source.commands() {
        let mut held = shared.commands.lock().unwrap();
        options_changed |= *held != commands;
        *held = commands;
    }
    if options_changed && !first {
        shared.send(&shared.options(), |c| c.app);
    }
    // Saved limits, until the engine says newer.
    if let Ok(limits) = source.limits() {
        let mut held = shared.limits.lock().unwrap();
        if held.is_empty() && !limits.is_empty() {
            *held = limits.clone();
            drop(held);
            if !first {
                shared.send(&ToApp::Limits { windows: limits }, |c| c.app);
            }
        }
    }
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
            let limits = shared.limits.lock().unwrap().clone();
            vec![ToApp::Threads { projects, threads }, shared.options(), ToApp::Limits { windows: limits }]
        }
        FromApp::ToolResult { thread, seq } => match source.whole(&thread, seq) {
            Ok((input, result)) => vec![ToApp::ToolResult { thread, seq, input, result }],
            Err(e) => failed(e),
        },
        FromApp::Subagent { thread, seq } => {
            // Live (with what it's doing now) when the thread is watched, else as saved.
            let live = shared.mirrors.lock().unwrap().get(&thread).and_then(|m| m.rows.get(&(seq as usize)).and_then(|r| r.sub.as_ref().map(|s| subagent_run(s, r.data.tool_input.as_deref()))));
            let run = match live {
                Some(run) => Some(run),
                None => match source.subagent_run(&thread, seq) {
                    Ok(found) => found.map(|(record, input)| subagent_run(&yh_core::thread_run::Subagent::from_record(record), input.as_deref())),
                    Err(e) => return failed(e),
                },
            };
            match run {
                Some(run) => vec![ToApp::Subagent { thread, seq, run }],
                None => vec![ToApp::Error { message: "That row has no sub-agent.".into() }],
            }
        }
        FromApp::Media { thread, seq, index } => {
            let held = shared.mirrors.lock().unwrap().get(&thread).and_then(|m| m.rows.get(&(seq as usize)).cloned());
            let row = match held {
                Some(r) => Some((r.row(None).kind, r.data.tool_name.clone(), r.data.tool_input.clone(), r.data.attachments.clone())),
                None => match source.media_of(&thread, seq) {
                    Ok(found) => found,
                    Err(e) => return failed(e),
                },
            };
            let answer = row
                .and_then(|(kind, tool, input, attachments)| pictures(&shared.data_dir, &kind, tool.as_deref(), input.as_deref(), &attachments).into_iter().nth(index))
                .ok_or_else(|| "No such picture.".to_string())
                .and_then(|path| {
                    let size = std::fs::metadata(&path).map_err(|_| "The picture isn't on the Mac any more.".to_string())?.len();
                    if size > MAX_PICTURE {
                        return Err("The picture is too big to send.".into());
                    }
                    let bytes = std::fs::read(&path).map_err(|_| "The picture couldn't be read.".to_string())?;
                    Ok((picture_type(&path.to_string_lossy()).unwrap_or("image/png").to_string(), yh_core::attachments::base64(&bytes)))
                });
            match answer {
                Ok((media_type, data)) => vec![ToApp::Media { thread, seq, index, media_type: Some(media_type), data: Some(data), error: None }],
                Err(error) => vec![ToApp::Media { thread, seq, index, media_type: None, data: None, error: Some(error) }],
            }
        }
        FromApp::Search { query } => match source.search(&query, 40) {
            Ok(hits) => vec![ToApp::SearchResults { query, hits }],
            Err(e) => failed(e),
        },
        FromApp::Files { thread, query } => {
            let folder = match source.folder_of(&thread) {
                Ok(Some(f)) => f,
                Ok(None) => return vec![ToApp::Files { thread, query, files: vec![] }],
                Err(e) => return failed(e),
            };
            let cached = shared.files.lock().unwrap().get(&folder).filter(|(at, _)| at.elapsed() < Duration::from_secs(30)).map(|(_, f)| f.clone());
            let all = cached.unwrap_or_else(|| {
                let listed = Arc::new(yh_core::files::list(std::path::Path::new(&folder)));
                shared.files.lock().unwrap().insert(folder.clone(), (Instant::now(), listed.clone()));
                listed
            });
            vec![ToApp::Files { thread, query: query.clone(), files: yh_core::files::matches(&all, &query, 20) }]
        }
        FromApp::Diagram { key, source } => match diagram::draw(&source) {
            Ok(drawing) => vec![ToApp::Diagram { key, drawing: Some(drawing), error: None }],
            Err(error) => vec![ToApp::Diagram { key, drawing: None, error: Some(error) }],
        },
        FromApp::SubscribeThread { thread, limit, known } => {
            let limit = limit.unwrap_or(DEFAULT_ROWS).clamp(1, MAX_ROWS);
            // With the engine: its snapshot, in order with its changes, comes from `follow`.
            if let Some(engine) = &shared.engine {
                set(&|c| {
                    c.thread = Some(thread.clone());
                    c.ready = false;
                });
                engine.send(yh_engine::Command::Snapshot { thread, limit: limit as usize, tag: id, known: known.map(|k| k.max(0) as usize) });
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
        action => {
            let id = action.action_id().map(str::to_string);
            let (error, made) = match to_action(action) {
                Err(e) => (Some(e), None),
                Ok(action) => match &shared.actions {
                    None => (Some("This server only reads.".to_string()), None),
                    Some(actions) => {
                        let (reply, answer) = flume::bounded(1);
                        if actions.send(Asked { action, reply }).is_err() {
                            (Some("The app is gone.".to_string()), None)
                        } else {
                            // Taken on the app's main thread, as a click: answered within a frame or two.
                            match answer.recv_timeout(Duration::from_secs(10)) {
                                Ok(Ok(made)) => (None, made),
                                Ok(Err(e)) => (Some(e), None),
                                Err(_) => (Some("The app didn't answer.".to_string()), None),
                            }
                        }
                    }
                },
            };
            match id {
                Some(id) => vec![ToApp::Done { id, error, thread: made }],
                None => error.map(|message| vec![ToApp::Error { message }]).unwrap_or_default(),
            }
        }
    }
}

/// A client's action as the app takes it; its pictures decoded.
fn to_action(m: FromApp) -> Result<Action, String> {
    use FromApp as F;
    Ok(match m {
        F::Send { thread, text, now, images, .. } => {
            let images = images.into_iter().map(|i| decode_base64(&i.data).map(|bytes| (i.media_type, bytes)).ok_or_else(|| "A picture couldn't be read.".to_string())).collect::<Result<_, _>>()?;
            Action::Send { thread, text, now, images }
        }
        F::Stop { thread, .. } => Action::Stop { thread },
        F::Respond { thread, request_id, allow, .. } => Action::Respond { thread, request_id, allow },
        F::Withdraw { thread, message, .. } => Action::Withdraw { thread, message },
        F::SendNow { thread, message, .. } => Action::SendNow { thread, message },
        F::Answer { thread, request_id, answers, .. } => Action::Answer { thread, request_id, answers },
        F::SkipQuestion { thread, request_id, .. } => Action::SkipQuestion { thread, request_id },
        F::ApprovePlan { thread, request_id, mode, feedback, .. } => Action::ApprovePlan { thread, request_id, mode, feedback },
        F::KeepPlanning { thread, request_id, feedback, .. } => Action::KeepPlanning { thread, request_id, feedback },
        F::AnswerQuestion { thread, request_id, accept, .. } => Action::AnswerQuestion { thread, request_id, accept },
        F::NewThread { project, text, .. } => Action::NewThread { project, text },
        F::SetModel { thread, model, .. } => Action::SetModel { thread, model },
        F::SetEffort { thread, effort, .. } => Action::SetEffort { thread, effort },
        F::AskAside { thread, text, .. } => Action::AskAside { thread, text },
        F::SetMode { thread, mode, .. } => {
            if !MODES.contains(&mode.as_str()) {
                return Err(format!("No permission mode named {mode}."));
            }
            Action::SetMode { thread, mode }
        }
        F::Rename { thread, title, .. } => Action::Rename { thread, title },
        F::Pin { thread, on, .. } => Action::Pin { thread, on },
        F::Settle { thread, on, .. } => Action::Settle { thread, on },
        F::Delete { thread, .. } => Action::Delete { thread },
        F::SubscribeApp | F::SubscribeThread { .. } | F::UnsubscribeThread | F::Page { .. } | F::ToolResult { .. } | F::Subagent { .. } | F::Media { .. } | F::Diagram { .. } | F::Search { .. } | F::Files { .. } | F::Unknown => return Err("Not an action.".into()),
    })
}

/// Standard base64 (padding optional, whitespace skipped); None if it isn't.
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        } as u32)
    };
    let digits: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace() && *b != b'=').collect();
    let mut out = Vec::with_capacity(digits.len() * 3 / 4);
    for chunk in digits.chunks(4) {
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            n |= value(*c)? << (18 - 6 * i);
        }
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        match chunk.len() {
            4 => out.extend_from_slice(&bytes),
            3 => out.extend_from_slice(&bytes[..2]),
            2 => out.push(bytes[0]),
            _ => return None,
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn decodes_base64() {
        assert_eq!(super::decode_base64("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(super::decode_base64("aGVsbG8gd29ybGQ").unwrap(), b"hello world");
        assert!(super::decode_base64("a$b").is_none());
    }
}

fn write(ws: &mut WebSocket<TcpStream>, message: &ToApp) -> Result<(), String> {
    let text = serde_json::to_string(message).expect("messages serialize");
    ws.send(Message::text(text)).map_err(|e| e.to_string())
}

fn name_of(directory: &Path) -> String {
    directory.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}
