//! The engine: every thread's run (`yh_core::thread_run::ThreadRun`) and its `claude`, on a
//! thread of its own. The app sends `Command`s and gets `Output`s: each thread's row changes
//! and live state, which it shows, and what else it reacts to (a turn's end, a process gone).
//!
//! The engine starts a thread's `claude` when a message or a request needs one; how to start it
//! (folder, model, tools, prompt) is the app's to say, so it asks (`Output::NeedsLaunch`) and
//! holds what waits until the app answers (`Command::Launch`). It lets an idle `claude` go
//! after ten minutes, starts one over for a new effort once its turn is over, and saves rows
//! as they settle. Rows it let go of are read back from the database when an event names them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use yh_core::claude::{Block, ClaudeProcess, Decision, Event, LaunchOptions};
use yh_core::db::{Database, Row, RowQuery};
use yh_core::library::EntryData;
use yh_core::thread_run::{LiveState, RowChange, RowSource, ThreadRun, TranscriptRow};

/// Thread ids that aren't threads of their own.
pub mod ids {
    /// A side chat's id: this, then its thread's.
    pub const SIDE: &str = "side:";
    /// Settings' hidden thread in Home's folder (Settings → MCP): this, then the project.
    pub const SETTINGS: &str = "settings:";
    /// A memory specialist's id: this, its role (`importer`, `reconciler`), `:`, its project.
    pub const MEMORY_AGENT: &str = "memory:";

    /// Whether its rows are saved: not a thread not made yet, a Claude Code session only
    /// listed, a side chat, a memory specialist or Settings' thread.
    pub fn saved(id: &str) -> bool {
        !["new:", "cc:", SIDE, MEMORY_AGENT, SETTINGS].iter().any(|p| id.starts_with(p))
    }

    /// A side chat or a memory specialist can't be resumed (nothing's saved): its `claude`
    /// lives while it's open, never let go of when idle.
    pub fn aside(id: &str) -> bool {
        id.starts_with(SIDE) || id.starts_with(MEMORY_AGENT)
    }
}

/// A thread's `claude` is let go of after this long without a turn, and resumed on the next.
const IDLE_LIMIT: Duration = Duration::from_secs(600);

/// A message for Claude: its text and images (media type, base64).
#[derive(Clone, Debug)]
pub struct Delivery {
    pub text: String,
    pub blocks: Vec<(String, String)>,
}

pub enum Command {
    /// A thread as the app knows it, before anything else is asked of it (again: ignored).
    Open { thread: String, session_id: Option<String>, context_tokens: Option<u64>, context_window: Option<u64> },
    /// Your message, into the transcript (or queued while Claude works, or stopping the turn
    /// with `now`), and to Claude when `deliver` says what to send; otherwise `Deliver` follows
    /// (files are copied in first).
    Send { thread: String, text: String, attachments: Vec<String>, message_id: String, now: bool, effort: Option<String>, deliver: Option<Delivery> },
    Deliver { thread: String, message_id: String, now: bool, effort: Option<String>, delivery: Delivery },
    /// How to start the thread's `claude`, asked for with `NeedsLaunch`; an error ends the turn
    /// with it. With a `note`, `YH_RUNTIME_LOG` says it started, the note after.
    Launch { thread: String, options: Result<LaunchOptions, String>, effort: Option<String>, note: Option<String> },
    /// Stops the turn for a message sent now (the message carries that on itself).
    Interrupt { thread: String },
    /// Stop: what waited on you is dropped, the turn ends as "Interrupted".
    Stop { thread: String },
    Respond { thread: String, request_id: String, decision: Decision },
    AnswerElicitation { thread: String, request_id: String, action: String, content: Option<serde_json::Value> },
    /// A control request to the thread's `claude` (started if it isn't), its reply sent back.
    Mcp { thread: String, request: serde_json::Value, effort: Option<String>, reply: flume::Sender<Result<serde_json::Value, String>> },
    /// Ends the thread's `claude`; the next message resumes its session. `only_idle`: not while a
    /// turn is under way.
    End { thread: String, only_idle: bool },
    SetModel { thread: String, model: String },
    SetMode { thread: String, mode: String },
    /// The effort it should run with: an idle `claude` with another ends now, a busy one once
    /// its turn is over.
    SetEffort { thread: String, effort: Option<String> },
    Notice { thread: String, text: String, error: bool },
    ChangedFiles { thread: String, message: String, files: Vec<String> },
    CutOffJobs { thread: String, outcome: String },
    /// Takes back a queued message; with `to_composer` it goes back into the composer.
    Withdraw { thread: String, message: String, to_composer: bool },
    /// A queued message, right away: Claude is stopped, the CLI hands it the message.
    SendNow { thread: String, message: String },
    StopJob { thread: String, task_id: String },
    /// Messages the composer took back (`LiveState`'s `composer.restored`).
    TookRestored { thread: String, ids: Vec<String> },
    SetMcpServers { thread: String, servers: Vec<(String, String, String)> },
    /// The thread's session, after a resume or a new folder (`None`).
    SetSession { thread: String, session_id: Option<String> },
    /// The thread is gone: its `claude` ends, its run is dropped.
    Forget { thread: String },
    /// Rows changed and not saved yet go now.
    Save { thread: String },
    /// Events as if from the thread's `claude` (tests, recordings).
    Events { thread: String, events: Vec<Event> },
    /// Quitting: running jobs cut off, everything saved, every `claude` ended; then `done`.
    Quit { done: flume::Sender<()> },
    /// Everything said from now on goes to `to` too (the server); first, each thread's state as
    /// it stands.
    Subscribe { to: flume::Sender<Output> },
    /// The thread's newest `limit` rows as saved now, everything changed saved first, told to
    /// subscribers in order with the changes (`Output::Snapshot`): what's told after it changes
    /// what it holds. `tag` comes back with it (the client that asked).
    Snapshot { thread: String, limit: usize, tag: u64, known: Option<usize> },
}

impl Command {
    fn thread(&self) -> Option<&str> {
        use Command::*;
        match self {
            Open { thread, .. } | Send { thread, .. } | Deliver { thread, .. } | Launch { thread, .. } | Interrupt { thread } | Stop { thread } | Respond { thread, .. } | AnswerElicitation { thread, .. } | Mcp { thread, .. } | End { thread, .. } | SetModel { thread, .. } | SetMode { thread, .. } | SetEffort { thread, .. } | Notice { thread, .. } | ChangedFiles { thread, .. } | CutOffJobs { thread, .. } | Withdraw { thread, .. } | SendNow { thread, .. } | StopJob { thread, .. } | TookRestored { thread, .. } | SetMcpServers { thread, .. } | SetSession { thread, .. } | Forget { thread } | Save { thread } | Events { thread, .. } => Some(thread),
            Snapshot { thread, .. } => Some(thread),
            Quit { .. } | Subscribe { .. } => None,
        }
    }
}

/// A turn is over.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TurnEnd {
    /// You stopped it (or a message sent now did): nothing to tell.
    pub stopped_by_you: bool,
    /// Claude waits on jobs it started: it's back when they report.
    pub waiting_on_jobs: bool,
}

/// What changed in one thread.
#[derive(Clone)]
pub struct ThreadUpdate {
    pub thread: String,
    pub changes: Vec<RowChange>,
    /// Its live state now.
    pub live: LiveState,
    /// What the app keeps from its events: the catalog, limits, the session, the title the CLI
    /// gave, skills, context windows, and tool calls (only their blocks), as they came.
    pub events: Vec<Event>,
    /// What the sidebar or the composer shows changed.
    pub chrome: bool,
    /// The checklist is to be worked out again.
    pub plan: bool,
    /// A shell command touched pull requests.
    pub pulls: bool,
    /// The session couldn't be resumed: forgotten.
    pub session_reset: bool,
    /// Background sub-agents came or went.
    pub agents_changed: bool,
    pub turn_ended: Option<TurnEnd>,
    /// Events applied and how long it took (ms), for the performance run.
    pub batch: Option<(usize, f64)>,
}

impl ThreadUpdate {
    fn new(thread: &str, live: LiveState) -> ThreadUpdate {
        ThreadUpdate { thread: thread.to_string(), changes: vec![], live, events: vec![], chrome: false, plan: false, pulls: false, session_reset: false, agents_changed: false, turn_ended: None, batch: None }
    }

    /// A later update of the same thread folded into this one.
    pub fn merge(&mut self, later: ThreadUpdate) {
        self.changes.extend(later.changes);
        self.live = later.live;
        self.events.extend(later.events);
        self.chrome |= later.chrome;
        self.plan |= later.plan;
        self.pulls |= later.pulls;
        self.session_reset |= later.session_reset;
        self.agents_changed |= later.agents_changed;
        self.turn_ended = later.turn_ended.or(self.turn_ended);
        self.batch = match (self.batch, later.batch) {
            (Some((n, ms)), Some((m, more))) => Some((n + m, ms + more)),
            (a, b) => a.or(b),
        };
    }
}

#[derive(Clone)]
pub enum Output {
    Thread(ThreadUpdate),
    /// The thread's `claude` has to start: the app answers with `Command::Launch`.
    NeedsLaunch { thread: String },
    /// The thread's `claude` started (its process id) or ended.
    Runtime { thread: String, pid: Option<u32> },
    /// What `Command::Snapshot` asked for (to subscribers only): rows `first_seq..count`, and
    /// the live state, as of this point in what's told.
    Snapshot { thread: String, tag: u64, first_seq: usize, count: usize, rows: Vec<TranscriptRow>, live: LiveState },
}

/// Where the app talks to the engine.
#[derive(Clone)]
pub struct EngineHandle {
    inbox: flume::Sender<Input>,
    pub outputs: flume::Receiver<Output>,
}

impl EngineHandle {
    pub fn send(&self, command: Command) {
        let _ = self.inbox.send(Input::Command(command));
    }

    /// A copy of everything the engine says from now on (`Command::Subscribe`).
    pub fn subscribe(&self) -> flume::Receiver<Output> {
        let (to, outputs) = flume::unbounded();
        self.send(Command::Subscribe { to });
        outputs
    }
}

enum Input {
    Command(Command),
    Claude { thread: String, generation: u64, event: Event },
}

/// Starts the engine on a thread of its own. `db`: where rows are saved and read back.
pub fn start(db: Option<Arc<Database>>) -> EngineHandle {
    let (inbox, rx) = flume::unbounded();
    let (out, outputs) = flume::unbounded();
    let engine = Engine { db, inbox: inbox.clone(), out, runs: HashMap::new(), runtimes: HashMap::new(), launching: HashMap::new(), generation: 0, saved_at: HashMap::new(), steering: HashMap::new(), subscribers: Vec::new(), updates: Vec::new() };
    std::thread::Builder::new().name("engine".into()).spawn(move || engine.run(rx)).expect("the engine's thread");
    EngineHandle { inbox, outputs }
}

/// One thread's `claude`, between turns too.
struct Runtime {
    process: ClaudeProcess,
    /// Events from an older process are ignored.
    generation: u64,
    /// Effort is a launch option: a different one starts a new process.
    effort: Option<String>,
    /// When it's let go of, idle.
    idle_at: Option<Instant>,
    /// A launch option (effort) changed while a turn was under way: started over once idle.
    restart_when_idle: bool,
}

/// What waits for a thread's `claude` to start.
#[derive(Default)]
struct Launching {
    effort: Option<String>,
    messages: Vec<(String, bool, Delivery)>,
    requests: Vec<(serde_json::Value, flume::Sender<Result<serde_json::Value, String>>)>,
}

struct Engine {
    db: Option<Arc<Database>>,
    /// For the processes' events.
    inbox: flume::Sender<Input>,
    out: flume::Sender<Output>,
    runs: HashMap<String, ThreadRun>,
    runtimes: HashMap<String, Runtime>,
    launching: HashMap<String, Launching>,
    generation: u64,
    saved_at: HashMap<String, Instant>,
    /// Messages sent without what to tell Claude yet (`Deliver` follows): whether a turn was
    /// under way when each was sent.
    steering: HashMap<String, bool>,
    /// Who else hears everything (`Command::Subscribe`).
    subscribers: Vec<flume::Sender<Output>>,
    /// Updates made since the last were sent, in order, one per thread.
    updates: Vec<ThreadUpdate>,
}

/// `YH_RUNTIME_LOG=1`: each thread's `claude` started and ended, printed as `RUNTIME …`.
fn runtime_log(text: String) {
    if std::env::var("YH_RUNTIME_LOG").as_deref() == Ok("1") {
        eprintln!("RUNTIME {text}");
    }
}

/// Rows the engine let go of, read back from the database.
struct DbRows<'a> {
    db: Option<&'a Database>,
    thread: &'a str,
}

fn from_db(seq: usize, row: Row) -> Option<TranscriptRow> {
    let delivery = row.delivery.clone();
    let mut r = TranscriptRow::saved(EntryData::from_row(row)?, seq);
    r.delivery = delivery;
    Some(r)
}

impl RowSource for DbRows<'_> {
    fn tool(&self, id: &str) -> Option<TranscriptRow> {
        self.by_id(id).filter(|r| r.data.kind == yh_core::library::Kind::Tool)
    }

    fn by_id(&self, id: &str) -> Option<TranscriptRow> {
        let (seq, row) = self.db?.read_row(self.thread, id)?;
        from_db(seq, row)
    }

    fn agent_call(&self, task_id: &str) -> Option<String> {
        self.db?.find_row(self.thread, RowQuery::AgentCallNaming(task_id)).map(|(_, r)| r.id)
    }

    fn has_command(&self, id: &str) -> bool {
        self.db.is_some_and(|db| db.find_row(self.thread, RowQuery::Command(id)).is_some())
    }

    fn last_turn_start(&self) -> Option<TranscriptRow> {
        let (seq, row) = self.db?.find_row(self.thread, RowQuery::LastTurnStart)?;
        from_db(seq, row)
    }
}

impl Engine {
    fn run(mut self, inbox: flume::Receiver<Input>) {
        let log = std::env::var("YH_EVENT_LOG").as_deref() == Ok("1");
        loop {
            let first = match self.next_idle() {
                Some(at) => match inbox.recv_timeout(at.saturating_duration_since(Instant::now())) {
                    Ok(input) => Some(input),
                    Err(flume::RecvTimeoutError::Timeout) => None,
                    Err(flume::RecvTimeoutError::Disconnected) => break,
                },
                None => match inbox.recv() {
                    Ok(input) => Some(input),
                    Err(_) => break,
                },
            };
            let mut inputs: Vec<Input> = first.into_iter().collect();
            inputs.extend(inbox.try_iter());
            // Each thread's events in a batch, as one frame takes them; a command to a thread
            // comes after the events that came before it.
            let mut batches: Vec<(String, u64, Vec<Event>)> = vec![];
            let mut quit = None;
            for input in inputs {
                match input {
                    Input::Claude { thread, generation, event } => {
                        if log {
                            eprintln!("event {thread}: {event:?}");
                        }
                        match batches.iter_mut().find(|(t, g, _)| *t == thread && *g == generation) {
                            Some((_, _, events)) => events.push(event),
                            None => batches.push((thread, generation, vec![event])),
                        }
                    }
                    Input::Command(command) => {
                        if let Some(thread) = command.thread() {
                            if let Some(at) = batches.iter().position(|(t, _, _)| t == thread) {
                                let (t, g, events) = batches.remove(at);
                                self.claude_batch(&t, g, events);
                            }
                        }
                        if let Command::Quit { done } = command {
                            quit = Some(done);
                            continue;
                        }
                        self.command(command);
                    }
                }
            }
            for (thread, generation, events) in batches {
                self.claude_batch(&thread, generation, events);
            }
            self.reap_idle();
            self.flush();
            if let Some(done) = quit {
                self.quit();
                self.flush();
                let _ = done.send(());
            }
        }
    }

    fn next_idle(&self) -> Option<Instant> {
        self.runtimes.values().filter_map(|rt| rt.idle_at).min()
    }

    /// The thread's run, made when first needed, its next row's number known.
    fn run_of(&mut self, thread: &str) -> &mut ThreadRun {
        let db = self.db.clone();
        let run = self.runs.entry(thread.to_string()).or_default();
        run.keep_all = !ids::saved(thread);
        if run.live.total.is_none() {
            run.live.total = Some(db.as_ref().filter(|_| ids::saved(thread)).map_or(0, |db| db.transcript_count(thread)));
        }
        run
    }

    /// The thread's update so far this round.
    fn update(&mut self, thread: &str) -> &mut ThreadUpdate {
        let live = self.run_of(thread).live.clone();
        match self.updates.iter().position(|u| u.thread == thread) {
            Some(at) => {
                let u = &mut self.updates[at];
                u.live = live;
                u
            }
            None => {
                self.updates.push(ThreadUpdate::new(thread, live));
                self.updates.last_mut().unwrap()
            }
        }
    }

    /// What the run changed goes into the thread's update, with its live state now.
    fn told(&mut self, thread: &str) {
        let changes = self.run_of(thread).take_changes();
        self.update(thread).changes.extend(changes);
    }

    /// To the app, and to every subscriber still listening.
    fn emit(&mut self, output: Output) {
        self.subscribers.retain(|s| s.send(output.clone()).is_ok());
        let _ = self.out.send(output);
    }

    fn flush(&mut self) {
        for u in std::mem::take(&mut self.updates) {
            self.emit(Output::Thread(u));
        }
    }

    /// The thread's changed rows into the database (in the background).
    fn save(&mut self, thread: &str) {
        self.saved_at.insert(thread.to_string(), Instant::now());
        let Some(run) = self.runs.get_mut(thread) else { return };
        let rows = run.take_saves();
        let total = run.live.total;
        if rows.is_empty() || !ids::saved(thread) {
            return;
        }
        if let (Some(db), Some(total)) = (&self.db, total) {
            db.save_rows(thread, total, rows);
        }
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::Open { thread, session_id, context_tokens, context_window } => {
                match self.runs.get_mut(&thread) {
                    // Made before the app told of it (a client's snapshot): what the app knows
                    // fills in, or its next `claude` wouldn't resume the conversation.
                    Some(run) => {
                        let live = &mut run.live;
                        if live.session_id.is_none() && !live.running {
                            live.session_id = session_id;
                        }
                        live.composer.context_tokens = live.composer.context_tokens.or(context_tokens);
                        live.composer.context_window = live.composer.context_window.or(context_window);
                    }
                    None => {
                        self.runs.insert(thread.clone(), ThreadRun::new(session_id, context_tokens, context_window));
                        self.run_of(&thread);
                    }
                }
            }
            Command::Send { thread, text, attachments, message_id, now, effort, deliver } => {
                let run = self.run_of(&thread);
                let running = run.live.running;
                let delivery = running.then_some(if now { "interrupted" } else { "steered" });
                run.add_user(&text, &attachments, &message_id, running && !now, delivery);
                run.live.running = true;
                // A new turn: no word yet of whether you stop it.
                if !running {
                    run.live.stopped_by_you = false;
                }
                self.told(&thread);
                match deliver {
                    Some(d) => self.deliver(&thread, message_id, now, effort, d, running),
                    None => {
                        self.steering.insert(message_id, running);
                    }
                }
                self.save(&thread);
            }
            Command::Deliver { thread, message_id, now, effort, delivery } => {
                let running = self.steering.remove(&message_id).unwrap_or(false);
                self.deliver(&thread, message_id, now, effort, delivery, running);
            }
            Command::Launch { thread, options, effort, note } => self.launch(&thread, options, effort, note),
            Command::Interrupt { thread } => {
                if let Some(rt) = self.runtimes.get_mut(&thread) {
                    rt.process.interrupt();
                }
            }
            Command::Stop { thread } => {
                let run = self.run_of(&thread);
                run.live.interrupt_requested = true;
                run.live.permissions.clear();
                run.live.elicitations.clear();
                self.update(&thread).chrome = true;
                if let Some(rt) = self.runtimes.get_mut(&thread) {
                    rt.process.interrupt();
                }
            }
            Command::Respond { thread, request_id, decision } => {
                let Some(permission) = self.run_of(&thread).answered(&request_id) else { return };
                if let Some(rt) = self.runtimes.get(&thread) {
                    rt.process.respond(&permission.id, &permission.input, &decision);
                }
                self.update(&thread).chrome = true;
            }
            Command::AnswerElicitation { thread, request_id, action, content } => self.answer_elicitation(&thread, &request_id, &action, content),
            Command::Mcp { thread, request, effort, reply } => {
                let running = self.run_of(&thread).live.running;
                if !self.ready(&thread, &effort, running) {
                    self.launching.get_mut(&thread).expect("launching").requests.push((request, reply));
                    return;
                }
                self.request(&thread, request, reply);
            }
            Command::End { thread, only_idle } => {
                if only_idle && self.runs.get(&thread).is_some_and(|r| r.live.running) {
                    return;
                }
                self.end_runtime(&thread);
            }
            Command::SetModel { thread, model } => {
                if let Some(rt) = self.runtimes.get_mut(&thread) {
                    rt.process.set_model(&model);
                }
            }
            Command::SetMode { thread, mode } => {
                if let Some(rt) = self.runtimes.get_mut(&thread) {
                    rt.process.set_mode(&mode);
                }
            }
            Command::SetEffort { thread, effort } => {
                let busy = self.runs.get(&thread).is_some_and(|r| r.live.running || r.live.jobs.any_running());
                if let Some(rt) = self.runtimes.get_mut(&thread).filter(|rt| rt.effort != effort) {
                    if busy {
                        rt.restart_when_idle = true;
                        runtime_log(format!("{thread}: effort now {effort:?} while working, its claude starts over once the turn ends"));
                    } else {
                        runtime_log(format!("{thread}: effort now {effort:?}, its idle claude ends"));
                        self.end_runtime(&thread);
                    }
                }
            }
            Command::Notice { thread, text, error } => {
                let run = self.run_of(&thread);
                if error {
                    run.error_notice(&text);
                } else {
                    run.notice(&text);
                }
                self.told(&thread);
                self.save(&thread);
            }
            Command::ChangedFiles { thread, message, files } => {
                let db = self.db.clone();
                let src = DbRows { db: db.as_deref(), thread: &thread };
                self.run_of(&thread).set_changed_files(&message, files, &src);
                self.told(&thread);
                self.save(&thread);
            }
            Command::CutOffJobs { thread, outcome } => {
                if self.runs.get(&thread).is_some_and(|r| r.live.jobs.any_running()) {
                    self.cut_off_jobs(&thread, &outcome);
                    self.save(&thread);
                }
            }
            Command::Withdraw { thread, message, to_composer } => {
                if to_composer {
                    self.run_of(&thread).live.composer.restore_on_withdraw.insert(message.clone());
                }
                match self.runtimes.get_mut(&thread) {
                    Some(rt) => rt.process.cancel_queued(&message),
                    // No `claude` to hold it any more: it's simply gone.
                    None => {
                        let db = self.db.clone();
                        let src = DbRows { db: db.as_deref(), thread: &thread };
                        self.run_of(&thread).apply(Event::Lifecycle { id: message, state: "cancelled".into() }, &src);
                        self.told(&thread);
                        self.save(&thread);
                    }
                }
                self.update(&thread);
            }
            Command::SendNow { thread, message } => {
                self.run_of(&thread).live.composer.sending_now.insert(message);
                self.update(&thread);
                if let Some(rt) = self.runtimes.get_mut(&thread) {
                    rt.process.interrupt();
                }
            }
            Command::StopJob { thread, task_id } => {
                if let Some(rt) = self.runtimes.get_mut(&thread) {
                    rt.process.stop_task(&task_id);
                }
            }
            Command::TookRestored { thread, ids } => {
                self.run_of(&thread).live.composer.restored.retain(|(id, ..)| !ids.contains(id));
                self.update(&thread);
            }
            Command::SetMcpServers { thread, servers } => {
                let run = self.run_of(&thread);
                if run.live.composer.mcp_servers != servers {
                    run.live.composer.mcp_servers = servers;
                    self.update(&thread);
                }
            }
            Command::SetSession { thread, session_id } => {
                self.run_of(&thread).live.session_id = session_id;
                self.update(&thread);
            }
            Command::Forget { thread } => {
                if let Some(rt) = self.runtimes.remove(&thread) {
                    rt.process.terminate();
                    self.emit(Output::Runtime { thread: thread.clone(), pid: None });
                }
                self.launching.remove(&thread);
                self.runs.remove(&thread);
                self.updates.retain(|u| u.thread != thread);
            }
            Command::Save { thread } => self.save(&thread),
            Command::Events { thread, events } => {
                let generation = self.runtimes.get(&thread).map_or(0, |rt| rt.generation);
                self.claude_batch(&thread, generation, events);
            }
            Command::Subscribe { to } => {
                for (thread, run) in &self.runs {
                    let _ = to.send(Output::Thread(ThreadUpdate::new(thread, run.live.clone())));
                }
                for (thread, rt) in &self.runtimes {
                    let _ = to.send(Output::Runtime { thread: thread.clone(), pid: Some(rt.process.pid) });
                }
                self.subscribers.push(to);
            }
            Command::Snapshot { thread, limit, tag, known } => {
                // Changes made so far go out first, so the snapshot comes after them.
                self.told(&thread);
                self.save(&thread);
                self.flush();
                let count = self.run_of(&thread).live.total.unwrap_or(0);
                // Reconnecting: from a little before what the client holds (which it keeps), but
                // never more than a page: far behind, it gets the newest page and starts over.
                let first_seq = match known {
                    Some(k) if k <= count => k.saturating_sub(40).max(count.saturating_sub(limit)),
                    _ => count.saturating_sub(limit),
                };
                let mut rows: Vec<TranscriptRow> = match &self.db {
                    Some(db) if ids::saved(&thread) => db.read_rows(&thread, first_seq, count - first_seq).into_iter().enumerate().filter_map(|(i, r)| from_db(first_seq + i, r)).collect(),
                    _ => vec![],
                };
                // Rows it holds that aren't saved (a side chat's, a thread not made yet's) are
                // as it holds them.
                for (seq, r) in self.runs.get(&thread).map(|run| run.held_rows()).unwrap_or_default() {
                    if seq >= first_seq {
                        match rows.iter_mut().find(|x| x.seq == seq) {
                            Some(x) => *x = r,
                            None => rows.push(r),
                        }
                    }
                }
                rows.sort_by_key(|r| r.seq);
                let live = self.run_of(&thread).live.clone();
                let snapshot = Output::Snapshot { thread, tag, first_seq, count, rows, live };
                self.subscribers.retain(|s| s.send(snapshot.clone()).is_ok());
            }
            Command::Quit { .. } => {}
        }
    }

    /// Whether the thread's `claude` can take a message now; if not, it's being started (asked
    /// of the app) and what waits goes into `launching`. A running turn is never ended for a
    /// changed effort: the process starts over once it's idle.
    fn ready(&mut self, thread: &str, effort: &Option<String>, running: bool) -> bool {
        if self.launching.contains_key(thread) {
            return false;
        }
        if let Some(rt) = self.runtimes.get_mut(thread) {
            if rt.effort == *effort {
                return true;
            }
            if running {
                rt.restart_when_idle = true;
                runtime_log(format!("{thread}: effort now {effort:?} while working, its claude starts over once the turn ends"));
                return true;
            }
            runtime_log(format!("{thread}: effort now {effort:?}, its claude starts over"));
        }
        self.end_runtime(thread);
        self.launching.insert(thread.to_string(), Launching { effort: effort.clone(), ..Default::default() });
        self.emit(Output::NeedsLaunch { thread: thread.to_string() });
        false
    }

    /// To Claude; `running`: a turn was under way when it was sent (it steers in).
    fn deliver(&mut self, thread: &str, message_id: String, now: bool, effort: Option<String>, delivery: Delivery, running: bool) {
        if !self.ready(thread, &effort, running) {
            self.launching.get_mut(thread).expect("launching").messages.push((message_id, now, delivery));
            return;
        }
        let rt = self.runtimes.get_mut(thread).expect("ready");
        rt.idle_at = None;
        rt.process.send(&delivery.text, &delivery.blocks, &message_id, now.then_some("now"));
    }

    fn request(&mut self, thread: &str, request: serde_json::Value, reply: flume::Sender<Result<serde_json::Value, String>>) {
        let Some(rt) = self.runtimes.get_mut(thread) else {
            let _ = reply.send(Err("No Claude Code process.".into()));
            return;
        };
        let rx = rt.process.request(request);
        std::thread::spawn(move || {
            let answer = rx.recv_timeout(Duration::from_secs(30)).map_err(|_| "Claude Code didn't answer.".to_string()).and_then(|r| r);
            let _ = reply.send(answer);
        });
    }

    fn launch(&mut self, thread: &str, options: Result<LaunchOptions, String>, effort: Option<String>, note: Option<String>) {
        let Some(waiting) = self.launching.remove(thread) else { return };
        let effort = if effort.is_some() { effort } else { waiting.effort };
        let (tx, rx) = flume::unbounded();
        let spawned = options.and_then(|options| ClaudeProcess::spawn(options, tx));
        let mut process = match spawned {
            Ok(p) => p,
            Err(error) => {
                for (_, reply) in waiting.requests {
                    let _ = reply.send(Err(error.clone()));
                }
                // The reason itself, in error colour; the turn is over.
                let run = self.run_of(thread);
                run.error_notice(&error);
                let was_running = std::mem::replace(&mut run.live.running, false);
                self.told(thread);
                if was_running || !waiting.messages.is_empty() {
                    self.turn_end(thread);
                }
                return;
            }
        };
        process.initialize();
        self.run_of(thread).live.received_init = false;
        self.generation += 1;
        let generation = self.generation;
        let pid = process.pid;
        if let Some(note) = note {
            runtime_log(format!("{thread}: claude started, pid {pid}, effort {effort:?}{note}"));
        }
        self.runtimes.insert(thread.to_string(), Runtime { process, generation, effort, idle_at: None, restart_when_idle: false });
        // Its events, tagged, into the engine's inbox.
        let inbox = self.inbox.clone();
        let id = thread.to_string();
        std::thread::Builder::new()
            .name(format!("claude events {id}"))
            .spawn(move || {
                for event in rx {
                    if inbox.send(Input::Claude { thread: id.clone(), generation, event }).is_err() {
                        break;
                    }
                }
            })
            .expect("a thread for its events");
        self.emit(Output::Runtime { thread: thread.to_string(), pid: Some(pid) });
        let rt = self.runtimes.get_mut(thread).expect("just started");
        for (message_id, now, d) in waiting.messages {
            rt.process.send(&d.text, &d.blocks, &message_id, now.then_some("now"));
        }
        for (request, reply) in waiting.requests {
            self.request(thread, request, reply);
        }
        self.update(thread);
    }

    /// Ends a thread's `claude`; the next message resumes its session.
    fn end_runtime(&mut self, thread: &str) {
        let Some(rt) = self.runtimes.remove(thread) else { return };
        rt.process.terminate();
        self.emit(Output::Runtime { thread: thread.to_string(), pid: None });
        if self.runs.get(thread).is_some_and(|r| r.live.jobs.any_running()) {
            self.cut_off_jobs(thread, "stopped when Claude was restarted");
        }
        if self.runs.get(thread).is_some_and(|r| r.live.running) {
            let db = self.db.clone();
            let src = DbRows { db: db.as_deref(), thread };
            self.run_of(thread).apply(Event::Exit { code: 0, tail: String::new() }, &src);
            self.told(thread);
            self.update(thread).chrome = true;
            self.turn_end(thread);
        }
        self.save(thread);
    }

    fn cut_off_jobs(&mut self, thread: &str, outcome: &str) {
        let db = self.db.clone();
        let src = DbRows { db: db.as_deref(), thread };
        self.run_of(thread).cut_off_jobs(outcome, &src);
        self.told(thread);
        self.update(thread).chrome = true;
    }

    fn answer_elicitation(&mut self, thread: &str, request_id: &str, action: &str, content: Option<serde_json::Value>) {
        if let Some(rt) = self.runtimes.get(thread) {
            rt.process.respond_elicitation(request_id, action, content);
        }
        self.run_of(thread).live.elicitations.retain(|e| e.id != request_id);
        self.update(thread).chrome = true;
    }

    /// Claude's events into the thread's run: what's arrived, deltas joined.
    fn claude_batch(&mut self, thread: &str, generation: u64, events: Vec<Event>) {
        let current = self.runtimes.get(thread).map_or(generation == 0, |rt| rt.generation == generation);
        if !current {
            return;
        }
        let started = Instant::now();
        let mut batch: Vec<Event> = vec![];
        for e in events {
            if !batch.last_mut().is_some_and(|last| last.merge(&e)) {
                batch.push(e);
            }
        }
        let count = batch.len();
        // A turn ends with its result, or with `claude` exiting mid-turn; an exit after a
        // finished turn (the idle process going) doesn't end it again.
        let had_done = batch.iter().any(|e| matches!(e, Event::Done { .. }));
        let was_running = self.run_of(thread).live.running;
        let exited = batch.iter().any(|e| matches!(e, Event::Exit { .. }));
        let ended = had_done || (was_running && exited);
        let agents_before = self.run_of(thread).live.jobs.agents_running();
        let mut kept: Vec<Event> = vec![];
        let (mut chrome, mut plan, mut pulls, mut session_reset) = (false, false, false, false);
        let db = self.db.clone();
        for e in batch {
            match &e {
                Event::Catalog { .. } | Event::Limits(_) | Event::Session { .. } | Event::TitleChanged(_) | Event::Skills(_) | Event::ContextWindows(_) => kept.push(e.clone()),
                Event::Assistant { message, parent, blocks } => {
                    let tools: Vec<Block> = blocks.iter().filter(|b| matches!(b, Block::Tool { .. })).cloned().collect();
                    if !tools.is_empty() {
                        kept.push(Event::Assistant { message: message.clone(), parent: parent.clone(), blocks: tools });
                    }
                }
                // A page-mode question answered on its page: the server has its answer, so the
                // card goes.
                Event::ElicitationComplete { server, elicitation_id } => {
                    let done: Vec<String> = self.run_of(thread).live.elicitations.iter().filter(|q| q.server == *server && q.elicitation_id.as_deref() == Some(elicitation_id.as_str())).map(|q| q.id.clone()).collect();
                    for request in done {
                        self.answer_elicitation(thread, &request, "accept", None);
                    }
                }
                _ => {}
            }
            let src = DbRows { db: db.as_deref(), thread };
            let outcome = self.run_of(thread).apply(e, &src);
            chrome |= outcome.chrome;
            plan |= outcome.plan;
            pulls |= outcome.pulls;
            session_reset |= outcome.session_reset;
        }
        let agents_changed = self.run_of(thread).live.jobs.agents_running() != agents_before;
        self.told(thread);
        {
            let u = self.update(thread);
            u.events.extend(kept);
            u.chrome |= chrome;
            u.plan |= plan;
            u.pulls |= pulls;
            u.session_reset |= session_reset;
            u.agents_changed |= agents_changed;
            u.batch = Some((count, started.elapsed().as_secs_f64() * 1000.0));
        }
        // Saved at most once a second while it streams, and when anything settles.
        if chrome || ended || self.saved_at.get(thread).is_none_or(|at| at.elapsed() > Duration::from_secs(1)) {
            self.save(thread);
        }
        if exited {
            if self.runtimes.remove(thread).is_some() {
                self.emit(Output::Runtime { thread: thread.to_string(), pid: None });
            }
        }
        // A message sent now (after a stop) starts its own turn right after the stop's end.
        let restarted = !exited && self.run_of(thread).live.running;
        if ended && !restarted {
            self.turn_end(thread);
        }
    }

    /// The turn is over: saved, its `claude` let go of later (or started over now for a new
    /// effort).
    fn turn_end(&mut self, thread: &str) {
        let run = self.run_of(thread);
        run.live.running = false;
        let waiting_on_jobs = run.live.jobs.waiting() > 0;
        let stopped_by_you = std::mem::take(&mut run.live.stopped_by_you);
        let any_running = run.live.jobs.any_running();
        self.update(thread).turn_ended = Some(TurnEnd { stopped_by_you, waiting_on_jobs });
        self.save(thread);
        // Effort changed mid-turn: the process starts over now, unless jobs it left run on.
        let restart = self.runtimes.get(thread).is_some_and(|rt| rt.restart_when_idle) && !any_running && !waiting_on_jobs;
        if restart {
            runtime_log(format!("{thread}: turn over, its claude ends for the new effort"));
            self.end_runtime(thread);
        }
        if let Some(rt) = self.runtimes.get_mut(thread).filter(|_| !ids::aside(thread)) {
            rt.idle_at = Some(Instant::now() + IDLE_LIMIT);
        }
    }

    /// Idle `claude`s go: unless jobs and sub-agents it left run (they'd end with it: looked
    /// at again later), or a turn or a request to you is under way.
    fn reap_idle(&mut self) {
        let now = Instant::now();
        let due: Vec<String> = self.runtimes.iter().filter(|(_, rt)| rt.idle_at.is_some_and(|at| at <= now)).map(|(id, _)| id.clone()).collect();
        for thread in due {
            let (jobs, busy) = self.runs.get(&thread).map_or((false, false), |r| (r.live.jobs.any_running(), r.live.running || r.live.waiting_for_you()));
            let rt = self.runtimes.get_mut(&thread).expect("due");
            if jobs {
                rt.idle_at = Some(now + IDLE_LIMIT);
                continue;
            }
            rt.idle_at = None;
            if !busy {
                self.end_runtime(&thread);
            }
        }
    }

    fn quit(&mut self) {
        let ids: Vec<String> = self.runs.keys().cloned().collect();
        for thread in &ids {
            if self.runs.get(thread).is_some_and(|r| r.live.jobs.any_running()) {
                self.cut_off_jobs(thread, "stopped when YourHarness quit");
            }
            self.save(thread);
        }
        for (_, rt) in self.runtimes.drain() {
            rt.process.terminate();
        }
        if let Some(db) = &self.db {
            db.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yh_core::golden::{self, Step};

    /// The steps as commands, as the app sends them, into a database of its own; what's in the
    /// database at the end, and the live state last told.
    fn golden(name: &str, steps: Vec<Step>) -> String {
        let dir = std::env::temp_dir().join(format!("yh-engine-golden-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = Arc::new(Database::open(&dir).unwrap());
        let engine = start(Some(db.clone()));
        let thread = "t".to_string();
        engine.send(Command::Open { thread: thread.clone(), session_id: None, context_tokens: None, context_window: None });
        for step in steps {
            match step {
                Step::Event(e) => engine.send(Command::Events { thread: thread.clone(), events: vec![e] }),
                Step::User { id, text, .. } => engine.send(Command::Send { thread: thread.clone(), text: text.into(), attachments: vec![], message_id: id.into(), now: false, effort: None, deliver: None }),
                Step::Interrupt => engine.send(Command::Stop { thread: thread.clone() }),
                Step::ChangedFiles { message, files } => engine.send(Command::ChangedFiles { thread: thread.clone(), message: message.into(), files: files.into_iter().map(str::to_string).collect() }),
            }
            // Saved after every step, as the golden files were made (the app saves at most once
            // a second while a turn streams).
            engine.send(Command::Save { thread: thread.clone() });
        }
        let (done, finished) = flume::bounded(1);
        engine.send(Command::Quit { done });
        finished.recv_timeout(Duration::from_secs(10)).expect("the engine quits");
        let mut live = None;
        let mut asked_to_launch = false;
        for output in engine.outputs.try_iter() {
            match output {
                Output::Thread(u) => live = Some(u.live),
                Output::NeedsLaunch { .. } => asked_to_launch = true,
                Output::Runtime { .. } | Output::Snapshot { .. } => {}
            }
        }
        assert!(!asked_to_launch, "nothing was to go to Claude");
        let live = live.expect("told");
        let saved: Vec<(usize, Row)> = db.read_rows(&thread, 0, 100_000).into_iter().enumerate().collect();
        let _ = std::fs::remove_dir_all(&dir);
        let ids = |v: Vec<String>| v.join(",");
        let state = vec![
            format!("running {} activity {:?} session {:?} meta session {:?} stopped by you {}", live.running, live.activity, live.session_id, None::<String>, live.stopped_by_you),
            format!("permissions [{}] elicitations [{}]", ids(live.permissions.iter().map(|p| format!("{}:{}", p.id, p.description)).collect()), ids(live.elicitations.iter().map(|e| e.id.clone()).collect())),
            format!("queue [{}] restored [{}]", ids(live.queue.iter().map(|q| q.id.clone()).collect()), ids(live.composer.restored.iter().map(|r| r.0.clone()).collect())),
            format!("context {:?}/{:?} mcp {:?}", live.composer.context_tokens, live.composer.context_window, live.composer.mcp_servers),
            format!("jobs running {} agents {} ended {} total {:?}", live.jobs.running.len(), live.jobs.agents.len(), live.jobs.ended.len(), live.total),
        ];
        golden::describe(&saved, &state)
    }

    /// The golden file less what only the app knows (its meta's session, its sub-agent list).
    fn expected(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../yh-core/fixtures/expected").join(format!("{name}.txt"));
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .filter(|l| !l.starts_with("= subagents"))
            .map(|l| match l.find(" meta session ") {
                Some(i) => {
                    let rest = &l[i + " meta session ".len()..];
                    let after = rest.find(" stopped by you").map(|j| &rest[j..]).unwrap_or("");
                    format!("{} meta session None{after}\n", &l[..i])
                }
                None => format!("{l}\n"),
            })
            .collect()
    }

    /// A phone watching a thread before the app opened it doesn't cost the thread its session.
    #[test]
    fn a_snapshot_first_keeps_the_session_the_app_tells() {
        let engine = start(None);
        let outputs = engine.subscribe();
        let snapshot = |engine: &EngineHandle| {
            engine.send(Command::Snapshot { thread: "t".into(), limit: 10, tag: 1, known: None });
            loop {
                if let Output::Snapshot { live, .. } = outputs.recv_timeout(Duration::from_secs(5)).expect("a snapshot") {
                    return live;
                }
            }
        };
        assert_eq!(snapshot(&engine).session_id, None);
        engine.send(Command::Open { thread: "t".into(), session_id: Some("s1".into()), context_tokens: Some(9), context_window: None });
        let live = snapshot(&engine);
        assert_eq!((live.session_id.as_deref(), live.composer.context_tokens), (Some("s1"), Some(9)));
    }

    #[test]
    fn saves_what_the_app_saved() {
        let mut cases: Vec<(&str, Vec<Step>)> = golden::SESSIONS.iter().map(|(name, text)| (*name, golden::session_events(text).into_iter().map(Step::Event).collect())).collect();
        cases.push(("synthetic", golden::synthetic()));
        for (name, steps) in cases {
            assert_eq!(golden(name, steps), expected(name), "{name}");
        }
    }
}
