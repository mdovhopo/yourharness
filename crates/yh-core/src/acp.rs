//! Agents over ACP, the Agent Client Protocol (agentclientprotocol.com): JSON-RPC 2.0, one
//! message a line, on the agent's stdin and stdout. Google's Antigravity is run this way, through
//! its official ACP server (`agy acp`), as editors (Zed, JetBrains, Xcode) and T3 Code run it.
//!
//! `AcpProcess` does what `ClaudeProcess` does for a thread and reports the same `Event`s:
//! `initialize`, then `session/resume` (or `session/load`) with the thread's session, else
//! `session/new`; each message a `session/prompt`, its `session/update`s turned into rows by
//! `Translator`; `session/request_permission` into a permission card. ACP has no way into a turn
//! under way, so a message sent meanwhile waits for its end (or stops it, sent now).

use crate::agents::Agent;
use crate::claude::{Block, BlockKind, Decision, DeltaKind, Event, LaunchOptions, ToolResult};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

/// The ACP version we speak.
const PROTOCOL_VERSION: u64 = 1;

/// A tool result is cut to this for holding; the whole of it is kept for saving.
const RESULT_HEAD: usize = 20_000;

static CHOSEN: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

/// The `agy` chosen in Settings → Agents (`cliPathOverrides.antigravity`), used in place of the one found.
pub fn set_chosen_agy(path: Option<std::path::PathBuf>) {
    *CHOSEN.lock().unwrap() = path;
}

pub const AGY_NOT_FOUND: &str = "Couldn't find Antigravity's `agy` CLI. Install it (antigravity.google/docs/cli/install) or choose it in Settings → Agents.";

pub fn find_agy() -> Option<std::path::PathBuf> {
    CHOSEN.lock().unwrap().clone().filter(|p| p.is_file()).or_else(|| crate::agents::which("agy"))
}

/// What a request we sent was for, so its reply is handled as such.
enum Asked {
    Initialize,
    /// `authenticate`, after a session wanted a sign-in: the session is asked for again.
    Authenticate,
    /// `session/new`, `session/load` or `session/resume`; `replaying`: `session/load` sends the
    /// whole conversation again as updates, which the saved rows already have.
    Session { replaying: bool, retried: bool },
    Prompt,
    /// `session/set_mode`, `session/set_model`: nothing to do with the reply.
    Setting,
}

/// A message waiting for the session, or for the turn under way to end.
struct Waiting {
    id: String,
    text: String,
    images: Vec<(String, String)>,
}

struct Shared {
    next_id: u64,
    asked: HashMap<u64, Asked>,
    session: Option<String>,
    /// A session is up and messages can go.
    ready: bool,
    /// A `session/prompt` is under way.
    busy: bool,
    /// Updates are a `session/load`'s replay: not shown.
    replaying: bool,
    queue: VecDeque<Waiting>,
    /// Permission requests waiting on you: our request id → (theirs, the options offered,
    /// whether it's a question).
    permissions: HashMap<String, (Value, Vec<Value>, bool)>,
    translator: Translator,
    /// What the thread asked for at launch, applied once the session says what it offers.
    model: Option<String>,
    mode: Option<String>,
    cwd: String,
    resume: Option<String>,
    /// The agent's `initialize` answer: what it can do.
    capabilities: Value,
    /// No session to be had (`fail`): the process is ended, the next message starts another.
    dead: bool,
    /// The agent at work with no prompt of ours (woken by a background job's end), and when it
    /// last said something: that turn ends once it's been quiet a while (`woken_turn_over`).
    woke: Option<std::time::Instant>,
    /// Its sign-in methods, from `initialize`.
    auth_methods: Vec<Value>,
}

pub struct AcpProcess {
    shared: Arc<Mutex<Shared>>,
    writer: flume::Sender<Value>,
    events: flume::Sender<Event>,
    child: Arc<Mutex<Child>>,
    pub pid: u32,
}

impl AcpProcess {
    pub fn spawn(options: LaunchOptions, events: flume::Sender<Event>) -> Result<AcpProcess, String> {
        let (executable, args) = match options.agent {
            Agent::Antigravity => (find_agy().ok_or(AGY_NOT_FOUND)?, vec!["acp".to_string()]),
            Agent::Claude => return Err("Claude runs as `claude`, not over ACP.".into()),
        };
        let mut command = Command::new(&executable);
        command.args(&args).current_dir(&options.cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        crate::shell_env::apply(&mut command);
        // The app keeps it running for hours: no updating itself under a thread; its output unbuffered.
        command.env("AGY_CLI_DISABLE_AUTO_UPDATE", "1").env("PYTHONUNBUFFERED", "1");
        for (k, v) in &options.env {
            command.env(k, v);
        }
        let mut child = command.spawn().map_err(|e| format!("{AGY_NOT_FOUND} ({e})"))?;
        let pid = child.id();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let child = Arc::new(Mutex::new(child));

        let (writer, inbox) = flume::unbounded::<Value>();
        std::thread::spawn(move || {
            for message in inbox.iter() {
                let mut line = message.to_string();
                line.push('\n');
                if stdin.write_all(line.as_bytes()).and_then(|_| stdin.flush()).is_err() {
                    break;
                }
            }
        });

        let tail = Arc::new(Mutex::new(String::new()));
        {
            let tail = tail.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stderr);
                let mut buffer = String::new();
                while reader.read_line(&mut buffer).unwrap_or(0) > 0 {
                    let mut t = tail.lock().unwrap();
                    t.push_str(&buffer);
                    if t.len() > 4000 {
                        let cut = t.len() - 2000;
                        let cut = (cut..t.len()).find(|i| t.is_char_boundary(*i)).unwrap_or(t.len());
                        t.replace_range(..cut, "");
                    }
                    buffer.clear();
                }
            });
        }

        let shared = Arc::new(Mutex::new(Shared {
            next_id: 0,
            asked: HashMap::new(),
            session: None,
            ready: false,
            busy: false,
            replaying: false,
            queue: VecDeque::new(),
            permissions: HashMap::new(),
            translator: Translator { nonce: format!("{:x}-", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis())), ..Translator::default() },
            model: options.model.filter(|m| !m.is_empty()),
            mode: options.mode,
            cwd: options.cwd.clone(),
            resume: options.session_id,
            capabilities: Value::Null,
            dead: false,
            woke: None,
            auth_methods: vec![],
        }));
        {
            let shared = shared.clone();
            let writer = writer.clone();
            let events = events.clone();
            let child = child.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::with_capacity(1 << 20, stdout);
                let mut line = Vec::new();
                loop {
                    line.clear();
                    match reader.read_until(b'\n', &mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                    let Ok(message) = serde_json::from_slice::<Value>(&line) else { continue };
                    let out = shared.lock().unwrap().receive(&message, &writer);
                    for event in out {
                        if events.send(event).is_err() {
                            return;
                        }
                    }
                    if shared.lock().unwrap().dead {
                        let _ = child.lock().unwrap().kill();
                        break;
                    }
                }
                // Ended for a failure already told: a quiet exit, not a crash. Either way it's
                // gone, and so is the woken-turn watch.
                let failed = std::mem::replace(&mut shared.lock().unwrap().dead, true);
                let code = child.lock().unwrap().wait().ok().and_then(|s| s.code()).unwrap_or(-1);
                let code = if failed { 0 } else { code };
                let tail = tail.lock().unwrap().clone();
                let _ = events.send(Event::Exit { code, tail });
            });
        }
        // A turn the agent took on its own (woken by a background job's end) has no reply to end
        // it: it ends once the agent has been quiet a while.
        {
            let (shared, events) = (shared.clone(), events.clone());
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(500));
                let out = {
                    let mut s = shared.lock().unwrap();
                    if s.dead {
                        return;
                    }
                    if s.woken_turn_over() { s.end_woken_turn() } else { vec![] }
                };
                for e in out {
                    if events.send(e).is_err() {
                        return;
                    }
                }
            });
        }
        Ok(AcpProcess { shared, writer, events, child, pid })
    }

    pub fn initialize(&mut self) {
        let mut s = self.shared.lock().unwrap();
        // Its session, known from the start: an exit before the agent answers isn't Claude's
        // failed `--resume` (ACP brings a session back itself, or says why not).
        if let Some(session) = s.resume.clone() {
            let _ = self.events.send(Event::Session { id: session });
        }
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            // Files and terminals are the agent's own: it reads, writes and runs in the folder.
            "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false },
            "clientInfo": { "name": "yourharness", "title": "YourHarness", "version": env!("CARGO_PKG_VERSION") },
        });
        s.request("initialize", params, Asked::Initialize, &self.writer);
    }

    /// Your message: now when the session is up and idle, else once it is (`now` stops the turn
    /// under way for it).
    pub fn send(&self, text: &str, images: &[(String, String)], id: &str, priority: Option<&str>) {
        let mut s = self.shared.lock().unwrap();
        let waiting = Waiting { id: id.to_string(), text: text.to_string(), images: images.to_vec() };
        if s.ready && !s.busy {
            let events = s.prompt(waiting, &self.writer);
            drop(s);
            for e in events {
                let _ = self.events.send(e);
            }
            return;
        }
        if priority == Some("now") && s.busy {
            s.queue.push_front(waiting);
            s.cancel(&self.writer);
        } else {
            s.queue.push_back(waiting);
        }
    }

    pub fn interrupt(&mut self) {
        self.shared.lock().unwrap().cancel(&self.writer);
    }

    /// A waiting message sent now: first in line, and the turn under way stopped for it.
    pub fn send_now(&mut self, id: &str) {
        let mut s = self.shared.lock().unwrap();
        if let Some(at) = s.queue.iter().position(|w| w.id == id) {
            let w = s.queue.remove(at).expect("found");
            s.queue.push_front(w);
        }
        s.cancel(&self.writer);
    }

    /// Takes back a message still waiting: its lifecycle ends `cancelled`.
    pub fn cancel_queued(&mut self, id: &str) {
        let mut s = self.shared.lock().unwrap();
        let before = s.queue.len();
        s.queue.retain(|w| w.id != id);
        if s.queue.len() != before {
            let _ = self.events.send(Event::Lifecycle { id: id.to_string(), state: "cancelled".into() });
        }
    }

    pub fn set_mode(&mut self, mode: &str) {
        let mut s = self.shared.lock().unwrap();
        s.mode = Some(mode.to_string());
        s.apply_mode(&self.writer);
    }

    pub fn set_model(&mut self, model: &str) {
        let mut s = self.shared.lock().unwrap();
        s.model = Some(model.to_string()).filter(|m| !m.is_empty() && m != "default");
        s.apply_model(&self.writer);
    }

    pub fn respond(&self, request_id: &str, decision: &Decision) {
        let mut s = self.shared.lock().unwrap();
        let Some((id, options, question)) = s.permissions.remove(request_id) else { return };
        let outcome = if question { question_outcome(&options, decision) } else { permission_outcome(&options, matches!(decision, Decision::Allow { .. })) };
        let _ = self.writer.send(json!({ "jsonrpc": "2.0", "id": id, "result": { "outcome": outcome } }));
    }

    pub fn terminate(&self) {
        unsafe { libc::kill(self.pid as i32, libc::SIGTERM) };
        let _ = self.child.lock().map(|mut c| c.try_wait());
    }

    /// Ends the session, when the agent can (`session/close`): a throwaway one leaves nothing in
    /// its history.
    fn close_session(&self) {
        let mut s = self.shared.lock().unwrap();
        if let (Some(session), false) = (s.session.clone(), s.capabilities["sessionCapabilities"]["close"].is_null()) {
            s.request("session/close", json!({ "sessionId": session }), Asked::Setting, &self.writer);
        }
    }
}

/// A group of models sharing limits, as Antigravity's `/usage` lists them ("Gemini Models",
/// "Claude and GPT models"): its windows as Claude's limits are read (`five_hour`, `seven_day`,
/// used 0…1), so they're drawn the same way.
#[derive(Clone, Debug, PartialEq)]
pub struct UsageGroup {
    pub name: String,
    /// Which models count against it ("Models within this group: …").
    pub description: String,
    pub windows: Vec<crate::limits::Window>,
}

/// `/usage`'s groups, from its JSON (`command.data`).
pub fn usage_groups(data: &Value) -> Vec<UsageGroup> {
    data["groups"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|g| UsageGroup {
            name: g["name"].as_str().unwrap_or("Models").to_string(),
            description: g["description"].as_str().unwrap_or("").to_string(),
            windows: g["buckets"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|b| crate::limits::Window {
                    name: match b["window"].as_str().unwrap_or("") {
                        "5h" => "five_hour".to_string(),
                        "weekly" => "seven_day".to_string(),
                        other => other.to_string(),
                    },
                    utilization: (1. - b["remaining_fraction"].as_f64().unwrap_or(1.)).clamp(0., 1.),
                    resets_at: b["reset_time"].as_str().and_then(crate::library::parse_iso),
                })
                .collect::<Vec<_>>(),
        })
        .map(|mut g| {
            // As Claude's are listed: the 5-hour window first, then the weekly.
            g.windows.sort_by_key(|w| crate::limits::PRIMARY.iter().position(|p| *p == w.name).unwrap_or(usize::MAX));
            g
        })
        .filter(|g| !g.windows.is_empty())
        .collect()
}

/// Antigravity's usage limits, from the official `agy` (`agy -p /usage`: a command of its own,
/// no model asked, nothing of its sign-in read): `/usage`'s JSON. Blocks, 30 s at most.
pub fn read_usage() -> Result<Value, String> {
    let agy = find_agy().ok_or(AGY_NOT_FOUND)?;
    let folder = std::env::temp_dir().join("yourharness-agent-catalog");
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let mut command = Command::new(agy);
    command.args(["-p", "/usage", "--output-format", "json"]).current_dir(&folder).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    crate::shell_env::apply(&mut command);
    command.env("AGY_CLI_DISABLE_AUTO_UPDATE", "1");
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let mut stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut out);
        out
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Antigravity didn't answer within 30 seconds.".into());
            }
        }
    }
    let out = reader.join().unwrap_or_default();
    let reply: Value = serde_json::from_str(out.trim()).map_err(|_| "Antigravity's answer wasn't readable.".to_string())?;
    match reply["command"]["data"].get("groups") {
        Some(_) => Ok(reply["command"]["data"].clone()),
        None => Err(reply["response"].as_str().or(reply["error"].as_str()).unwrap_or("Antigravity reported no limits.").trim().to_string()),
    }
}

/// What a throwaway session says of an agent: its models (in `ModelInfo`'s JSON shape), the one
/// it defaults to, and the plan it's signed in with.
pub struct Catalog {
    pub models: Value,
    pub default_model: Option<String>,
    pub plan: Option<String>,
    pub modes: Vec<String>,
}

/// An agent's catalog, from a throwaway session in an empty folder, closed at once: nothing is
/// asked of a model. Blocks, 30 s at most.
pub fn read_catalog(agent: Agent) -> Option<Catalog> {
    let folder = std::env::temp_dir().join("yourharness-agent-catalog");
    std::fs::create_dir_all(&folder).ok()?;
    let options = LaunchOptions {
        agent,
        cwd: folder.to_string_lossy().into_owned(),
        session_id: None,
        model: None,
        mode: None,
        effort: None,
        add_dirs: vec![],
        env: vec![],
        claude_config: None,
        append_system_prompt: None,
        allowed_tools: vec![],
        servers: vec![],
        extra_args: vec![],
        flag_settings: None,
    };
    let (tx, rx) = flume::unbounded();
    let mut process = AcpProcess::spawn(options, tx).ok()?;
    process.initialize();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut found = None;
    while let Ok(event) = rx.recv_deadline(deadline) {
        match event {
            Event::AgentCatalog { models, current_model, plan, modes, .. } if models.as_array().is_some_and(|m| !m.is_empty()) => {
                found = Some(Catalog { models, default_model: current_model, plan, modes });
                break;
            }
            Event::Done { .. } | Event::Exit { .. } => break,
            _ => {}
        }
    }
    process.close_session();
    // Its answer to the close, or a moment: then it's let go of.
    let _ = rx.recv_timeout(std::time::Duration::from_millis(500));
    process.terminate();
    found
}

/// The option to pick for an answer: one of its kind that applies once, else any of its kind;
/// none of either is cancelling.
fn permission_outcome(options: &[Value], allow: bool) -> Value {
    let (once, always) = if allow { ("allow_once", "allow_always") } else { ("reject_once", "reject_always") };
    let pick = options.iter().find(|o| o["kind"] == once).or_else(|| options.iter().find(|o| o["kind"] == always));
    match pick.and_then(|o| o["optionId"].as_str()) {
        Some(option) => json!({ "outcome": "selected", "optionId": option }),
        None => json!({ "outcome": "cancelled" }),
    }
}

/// A question with fixed choices, which Antigravity asks as a permission request whose call's
/// id starts `interaction_`: its title the question, each option a choice.
fn is_question(tool_call: &Value) -> bool {
    tool_call["toolCallId"].as_str().is_some_and(|id| id.starts_with("interaction_"))
}

/// The question as our question card reads it (Claude's `AskUserQuestion` input).
fn question_input(tool_call: &Value, options: &[Value]) -> String {
    let choices: Vec<Value> = options.iter().map(|o| json!({ "label": o["name"].as_str().unwrap_or(""), "description": "" })).collect();
    json!({ "questions": [{ "question": tool_call["title"].as_str().unwrap_or("Antigravity asks"), "header": "Antigravity", "options": choices, "multiSelect": false }] }).to_string()
}

/// The answer to a question: the option whose name was picked; anything else (your own words,
/// a dismissal) is cancelled, as Antigravity takes only its own choices.
fn question_outcome(options: &[Value], decision: &Decision) -> Value {
    let Decision::Allow { updated_input: Some(input), .. } = decision else { return json!({ "outcome": "cancelled" }) };
    let answers: Value = serde_json::from_str(input).unwrap_or_default();
    let picked = answers["answers"].as_object().and_then(|a| a.values().next()).and_then(Value::as_str).unwrap_or("");
    match options.iter().find(|o| o["name"] == picked).and_then(|o| o["optionId"].as_str()) {
        Some(option) => json!({ "outcome": "selected", "optionId": option }),
        None => json!({ "outcome": "cancelled" }),
    }
}

/// The agent's mode for our permission mode, when it offers one (Antigravity 1.5: `default`,
/// `accept-edits`, and for some accounts `auto`, its own review); else `default`, Accept Edits
/// and Bypass then answered by the app (`answers_itself`).
fn agent_mode(mode: &str, offered: &[String]) -> &'static str {
    let wanted = match mode {
        "auto" => "auto",
        "acceptEdits" => "accept-edits",
        _ => "default",
    };
    if offered.iter().any(|m| m == wanted) {
        wanted
    } else {
        "default"
    }
}

/// The app's answer to a call without asking you, in this permission mode: allow (Bypass, an
/// edit in Accept Edits), decline (Plan: it may look but not change anything), or ask.
fn answers_itself(mode: Option<&str>, kind: &str) -> Option<bool> {
    match mode {
        Some("bypassPermissions") => Some(true),
        Some("acceptEdits") if kind == "edit" => Some(true),
        Some("plan") if matches!(kind, "edit" | "delete" | "move" | "execute") => Some(false),
        _ => None,
    }
}

/// What a thread says when Antigravity wants a sign-in.
pub const NOT_SIGNED_IN: &str = "Antigravity isn't signed in. Run `agy` once in Terminal to sign in, then send again.";

impl Shared {
    /// No session to be had: the turn ends with why, and what waited for it is withdrawn.
    fn fail(&mut self, error: String) -> Vec<Event> {
        self.dead = true;
        let mut out = vec![Event::Done { is_error: true, text: error }];
        for w in self.queue.drain(..) {
            out.push(Event::Lifecycle { id: w.id, state: "cancelled".into() });
        }
        out
    }

    fn request(&mut self, method: &str, params: Value, asked: Asked, writer: &flume::Sender<Value>) {
        self.next_id += 1;
        let id = self.next_id;
        self.asked.insert(id, asked);
        let _ = writer.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
    }

    /// A woken turn that's been quiet long enough (or one a message of yours comes after) ends.
    fn end_woken_turn(&mut self) -> Vec<Event> {
        if self.woke.take().is_none() {
            return vec![];
        }
        let mut out = self.translator.end_turn(false);
        out.push(Event::Done { is_error: false, text: String::new() });
        out
    }

    /// Whether a woken turn is over: quiet for a few seconds, nothing waiting on you, no call of
    /// its own still under way.
    fn woken_turn_over(&self) -> bool {
        self.woke.is_some_and(|at| at.elapsed() > std::time::Duration::from_secs(3)) && !self.busy && self.permissions.is_empty() && !self.translator.calls_open()
    }

    fn prompt(&mut self, w: Waiting, writer: &flume::Sender<Value>) -> Vec<Event> {
        let Some(session) = self.session.clone() else {
            self.queue.push_front(w);
            return vec![];
        };
        let mut before = self.end_woken_turn();
        let mut prompt: Vec<Value> = w.images.iter().map(|(media, data)| json!({ "type": "image", "mimeType": media, "data": data })).collect();
        if !w.text.is_empty() || prompt.is_empty() {
            prompt.push(json!({ "type": "text", "text": w.text }));
        }
        self.busy = true;
        self.translator.start_turn();
        self.request("session/prompt", json!({ "sessionId": session, "prompt": prompt }), Asked::Prompt, writer);
        before.extend([Event::Lifecycle { id: w.id, state: "started".into() }, Event::Status("requesting".into())]);
        before
    }

    fn cancel(&mut self, writer: &flume::Sender<Value>) {
        // Every question waiting on you is answered as cancelled, as ACP asks of a cancel.
        for (_, (id, _, _)) in self.permissions.drain() {
            let _ = writer.send(json!({ "jsonrpc": "2.0", "id": id, "result": { "outcome": { "outcome": "cancelled" } } }));
        }
        if let (Some(session), true) = (&self.session, self.busy) {
            let _ = writer.send(json!({ "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": session } }));
        }
    }

    fn apply_mode(&mut self, writer: &flume::Sender<Value>) {
        let Some(session) = self.session.clone() else { return };
        let id = agent_mode(self.mode.as_deref().unwrap_or("default"), &self.translator.modes);
        if self.translator.current_mode.as_deref().unwrap_or("default") == id {
            return;
        }
        self.translator.current_mode = Some(id.to_string());
        match self.translator.option_id("mode") {
            Some(config) => self.request("session/set_config_option", json!({ "sessionId": session, "configId": config, "value": id }), Asked::Setting, writer),
            None => self.request("session/set_mode", json!({ "sessionId": session, "modeId": id }), Asked::Setting, writer),
        }
    }

    fn apply_model(&mut self, writer: &flume::Sender<Value>) {
        let (Some(session), Some(model)) = (self.session.clone(), self.model.clone()) else { return };
        if self.translator.current_model.as_deref() == Some(model.as_str()) {
            return;
        }
        self.translator.current_model = Some(model.clone());
        match self.translator.option_id("model") {
            Some(config) => self.request("session/set_config_option", json!({ "sessionId": session, "configId": config, "value": model }), Asked::Setting, writer),
            None => self.request("session/set_model", json!({ "sessionId": session, "modelId": model }), Asked::Setting, writer),
        }
    }

    /// `session/new`, or the thread's session brought back (`session/resume`: `session/load`
    /// would replay every tool call the saved rows already show).
    fn open_session(&mut self, retried: bool, writer: &flume::Sender<Value>) {
        let mcp: Vec<Value> = vec![];
        match self.resume.clone() {
            Some(session) if !self.capabilities["sessionCapabilities"]["resume"].is_null() => {
                self.session = Some(session.clone());
                self.request("session/resume", json!({ "sessionId": session, "cwd": self.cwd, "mcpServers": mcp }), Asked::Session { replaying: false, retried }, writer);
            }
            Some(session) if self.capabilities["loadSession"] == true => {
                self.session = Some(session.clone());
                self.replaying = true;
                self.request("session/load", json!({ "sessionId": session, "cwd": self.cwd, "mcpServers": mcp }), Asked::Session { replaying: true, retried }, writer);
            }
            _ => self.request("session/new", json!({ "cwd": self.cwd, "mcpServers": mcp }), Asked::Session { replaying: false, retried }, writer),
        }
    }

    /// One line from the agent: a reply to us, a request of us, or a notification.
    fn receive(&mut self, message: &Value, writer: &flume::Sender<Value>) -> Vec<Event> {
        let method = message["method"].as_str();
        match (method, message.get("id")) {
            // A reply to one of ours.
            (None, Some(id)) => {
                let Some(asked) = id.as_u64().and_then(|id| self.asked.remove(&id)) else { return vec![] };
                self.reply(asked, message, writer)
            }
            (Some("session/update"), _) => {
                if self.replaying {
                    return vec![];
                }
                let update = &message["params"]["update"];
                // Outside a prompt: the agent woken on its own is a turn of its own.
                let mut out = vec![];
                if !self.busy && self.ready && self.translator.is_activity(update) {
                    if self.woke.is_none() {
                        self.translator.start_turn();
                    }
                    self.woke = Some(std::time::Instant::now());
                }
                out.extend(self.translator.update(update));
                out
            }
            (Some("session/request_permission"), Some(id)) => {
                let params = &message["params"];
                let call = &params["toolCall"];
                let request_id = format!("acp-{}", id);
                let options = params["options"].as_array().cloned().unwrap_or_default();
                let description = call["title"].as_str().unwrap_or("").to_string();
                if is_question(call) {
                    let input = question_input(call, &options);
                    self.permissions.insert(request_id.clone(), (id.clone(), options, true));
                    return vec![Event::Permission { request_id, tool: "AskUserQuestion".into(), description, input }];
                }
                let kind = call["kind"].as_str().map(str::to_string).or_else(|| call["toolCallId"].as_str().and_then(|c| self.translator.kind_of(c))).unwrap_or_default();
                if let Some(allow) = answers_itself(self.mode.as_deref(), &kind) {
                    let _ = writer.send(json!({ "jsonrpc": "2.0", "id": id, "result": { "outcome": permission_outcome(&options, allow) } }));
                    return vec![];
                }
                let (tool, input) = self.translator.permission_tool(call);
                self.permissions.insert(request_id.clone(), (id.clone(), options, false));
                vec![Event::Permission { request_id, tool, description, input }]
            }
            // Something we didn't offer (files, terminals) or don't know.
            (Some(method), Some(id)) => {
                let _ = writer.send(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("{method} isn't offered") } }));
                vec![]
            }
            _ => vec![],
        }
    }

    fn reply(&mut self, asked: Asked, message: &Value, writer: &flume::Sender<Value>) -> Vec<Event> {
        let error = message.get("error").map(|e| e["message"].as_str().unwrap_or("The agent reported an error.").to_string());
        let result = &message["result"];
        match asked {
            Asked::Initialize => {
                if let Some(error) = error {
                    return vec![Event::Done { is_error: true, text: error }];
                }
                self.capabilities = result["agentCapabilities"].clone();
                self.auth_methods = result["authMethods"].as_array().cloned().unwrap_or_default();
                self.open_session(false, writer);
                vec![]
            }
            Asked::Authenticate => {
                if let Some(error) = error {
                    return self.fail(format!("{NOT_SIGNED_IN} ({error})"));
                }
                self.open_session(true, writer);
                vec![]
            }
            Asked::Session { replaying, retried } => {
                if replaying {
                    self.replaying = false;
                }
                if let Some(error) = error {
                    let code = message["error"]["code"].as_i64().unwrap_or(0);
                    // Not signed in (`auth_required`): its cached sign-in is picked up once asked
                    // for by method, then the session again; still not, you're told how.
                    if code == -32000 || error.starts_with("No browser is available here") {
                        if retried {
                            return self.fail(NOT_SIGNED_IN.to_string());
                        }
                        let method = self.auth_methods.iter().find(|m| m["id"] == "oauth-personal").or(self.auth_methods.first()).and_then(|m| m["id"].as_str()).unwrap_or("oauth-personal").to_string();
                        self.session = None;
                        self.request("authenticate", json!({ "methodId": method }), Asked::Authenticate, writer);
                        return vec![];
                    }
                    // A session it can't bring back: a new one, the transcript kept.
                    if self.resume.take().is_some() {
                        self.session = None;
                        self.open_session(retried, writer);
                        return vec![Event::Notice(format!("Antigravity couldn't resume its session ({error}); this is a new one."))];
                    }
                    return self.fail(error);
                }
                if let Some(id) = result["sessionId"].as_str() {
                    self.session = Some(id.to_string());
                }
                let Some(session) = self.session.clone() else { return vec![] };
                self.translator.session_state(result);
                self.ready = true;
                let mut out = vec![Event::Session { id: session }];
                out.extend(self.translator.catalog());
                self.apply_mode(writer);
                self.apply_model(writer);
                if let Some(w) = self.queue.pop_front() {
                    out.extend(self.prompt(w, writer));
                }
                out
            }
            Asked::Prompt => {
                self.busy = false;
                let cancelled = result["stopReason"] == "cancelled";
                let mut out = self.translator.end_turn(cancelled);
                match (error, result["stopReason"].as_str()) {
                    (Some(error), _) => out.push(Event::Done { is_error: true, text: error }),
                    (None, Some("cancelled")) => out.push(Event::Done { is_error: true, text: String::new() }),
                    (None, Some("refusal")) => out.push(Event::Done { is_error: true, text: "The model refused to go on.".into() }),
                    (None, Some("max_tokens")) => out.push(Event::Done { is_error: true, text: "The reply hit the model's length limit.".into() }),
                    _ => out.push(Event::Done { is_error: false, text: String::new() }),
                }
                if let Some(usage) = self.translator.usage.take() {
                    out.push(Event::ContextUsage(usage));
                }
                if let Some(w) = self.queue.pop_front() {
                    out.extend(self.prompt(w, writer));
                }
                out
            }
            Asked::Setting => error.map(|e| vec![Event::Notice(format!("Antigravity: {e}"))]).unwrap_or_default(),
        }
    }
}

/// A tool call as it's known so far: ACP sends it, then updates with only what changed.
#[derive(Clone, Debug, Default)]
struct Call {
    title: String,
    kind: String,
    raw_input: Value,
    content: Vec<Value>,
    locations: Vec<Value>,
    raw_output: Value,
    /// What it was last shown as: (name, input), so an unchanged update shows nothing new.
    shown: Option<(String, String)>,
    finished: bool,
}

/// `session/update`s into the transcript's events, one turn at a time. Pure: tested on its own.
#[derive(Default)]
pub struct Translator {
    /// This process's own mark in its rows' ids: turns count from 1 in each process, and a
    /// resumed session's rows mustn't take an earlier one's.
    nonce: String,
    turn: u64,
    blocks: u64,
    /// The text or thinking being written: (its message id, kind, what it says so far).
    open: Option<(String, DeltaKind, String)>,
    calls: HashMap<String, Call>,
    /// Commands still running when their turn ended: background jobs, finished by a later
    /// update (Antigravity reports them done outside any prompt, then wakes the agent).
    jobs: HashMap<String, Call>,
    /// What the session offers: its modes' ids and the one in effect, its models.
    pub modes: Vec<String>,
    pub current_mode: Option<String>,
    pub current_model: Option<String>,
    models: Vec<Value>,
    commands: Vec<Value>,
    /// The session's settings (`configOptions`): its model and mode among them, when it has them.
    config: Vec<Value>,
    /// Tokens in the context, as the agent last said, and its model's window.
    pub usage: Option<u64>,
    window: Option<u64>,
    /// The plan it's signed in with, as its session says (`_meta.antigravity.tier`).
    plan: Option<String>,
}

impl Translator {
    pub fn start_turn(&mut self) {
        self.turn += 1;
        self.blocks = 0;
        self.open = None;
    }

    /// The turn's end: what's still being written settles. A command still running carries on
    /// as a background job (a turn that ended on its own: Antigravity moved it aside); anything
    /// else unfinished, or everything in a cancelled turn, is stopped.
    pub fn end_turn(&mut self, cancelled: bool) -> Vec<Event> {
        let mut out = self.close();
        let mut started = false;
        for (id, mut call) in std::mem::take(&mut self.calls).into_iter().filter(|(_, c)| !c.finished) {
            if !cancelled && call.kind == "execute" {
                out.push(Event::ToolResults { parent: None, results: vec![ToolResult { id: id.clone(), text: "Running in the background…".into(), len: 26, is_error: false, full: None }] });
                out.push(Event::JobStarted { task_id: id.clone(), tool_id: Some(id.clone()), description: call.title.clone(), background: true, parent_task: None });
                self.jobs.insert(id, call);
                started = true;
            } else {
                call.finished = true;
                out.push(Event::ToolResults { parent: None, results: vec![ToolResult { id, text: "Stopped.".into(), len: 8, is_error: true, full: None }] });
            }
        }
        if started {
            out.push(Event::JobsRunning(self.jobs.keys().cloned().collect()));
        }
        out
    }

    /// Whether an update is the agent at work (writing, thinking, calling a tool), rather than a
    /// background job's end or bookkeeping: outside a prompt, it's the agent woken on its own.
    pub fn is_activity(&self, u: &Value) -> bool {
        match u["sessionUpdate"].as_str().unwrap_or("") {
            "agent_message_chunk" | "agent_thought_chunk" | "tool_call" | "plan" => true,
            "tool_call_update" => !u["toolCallId"].as_str().is_some_and(|id| self.jobs.contains_key(id)),
            _ => false,
        }
    }

    /// A call of the turn still under way (not a background job): a woken turn isn't over yet.
    pub fn calls_open(&self) -> bool {
        self.calls.values().any(|c| !c.finished)
    }

    fn close(&mut self) -> Vec<Event> {
        match self.open.take() {
            Some((message, kind, text)) => {
                let block = if kind == DeltaKind::Thinking { Block::Thinking(text) } else { Block::Text(text) };
                vec![Event::Assistant { message, parent: None, blocks: vec![block] }]
            }
            None => vec![],
        }
    }

    /// The id of the session setting for `model` or `mode`, when it has one.
    pub fn option_id(&self, what: &str) -> Option<String> {
        self.config.iter().find(|o| o["category"] == what || o["id"] == what).and_then(|o| o["id"].as_str().map(str::to_string))
    }

    pub fn kind_of(&self, call: &str) -> Option<String> {
        self.calls.get(call).map(|c| c.kind.clone()).filter(|k| !k.is_empty())
    }

    /// Settings as the session lists them: the model setting's choices become its models
    /// (choices may come in groups), its current value the model in effect.
    fn note_config(&mut self, options: &Value) {
        let Some(options) = options.as_array() else { return };
        self.config = options.clone();
        if let Some(model) = options.iter().find(|o| o["category"] == "model" || o["id"] == "model") {
            let mut models = vec![];
            for o in model["options"].as_array().into_iter().flatten() {
                match o["options"].as_array() {
                    Some(group) => models.extend(group.iter().cloned()),
                    None => models.push(o.clone()),
                }
            }
            if !models.is_empty() {
                self.models = models.into_iter().map(|m| json!({ "modelId": m["value"], "name": m["name"], "description": m["description"] })).collect();
            }
            if let Some(current) = model["currentValue"].as_str() {
                self.current_model = Some(current.to_string());
            }
        }
        if let Some(mode) = options.iter().find(|o| o["category"] == "mode") {
            if let Some(current) = mode["currentValue"].as_str() {
                self.current_mode = Some(current.to_string());
            }
            let ids: Vec<String> = mode["options"].as_array().into_iter().flatten().filter_map(|o| o["value"].as_str().map(str::to_string)).collect();
            if !ids.is_empty() {
                self.modes = ids;
            }
        }
    }

    /// What `session/new` (or `load`, `resume`) said: its modes and models.
    pub fn session_state(&mut self, result: &Value) {
        if let Some(modes) = result["modes"]["availableModes"].as_array() {
            self.modes = modes.iter().filter_map(|m| m["id"].as_str().map(str::to_string)).collect();
        }
        self.current_mode = result["modes"]["currentModeId"].as_str().map(str::to_string);
        if let Some(models) = result["models"]["availableModels"].as_array() {
            self.models = models.clone();
        }
        self.current_model = result["models"]["currentModelId"].as_str().map(str::to_string);
        self.note_config(&result["configOptions"]);
        if let Some(plan) = result["_meta"]["antigravity"]["tier"]["name"].as_str() {
            self.plan = Some(plan.to_string());
        }
    }

    /// The models and commands, as an agent catalog.
    pub fn catalog(&self) -> Vec<Event> {
        let models: Vec<Value> = self
            .models
            .iter()
            .filter_map(|m| {
                let id = m["modelId"].as_str()?;
                Some(json!({ "value": id, "displayName": m["name"].as_str().unwrap_or(id), "description": m["description"], "supportedEffortLevels": [] }))
            })
            .collect();
        let commands: Vec<Value> = self
            .commands
            .iter()
            .filter_map(|c| Some(json!({ "name": c["name"].as_str()?, "description": c["description"], "argumentHint": c["input"]["hint"] })))
            .collect();
        vec![Event::AgentCatalog { agent: Agent::Antigravity, models: Value::Array(models), commands: Value::Array(commands), current_model: self.current_model.clone(), plan: self.plan.clone(), modes: self.modes.clone() }]
    }

    pub fn update(&mut self, u: &Value) -> Vec<Event> {
        match u["sessionUpdate"].as_str().unwrap_or("") {
            kind @ ("agent_message_chunk" | "agent_thought_chunk") => {
                let Some(text) = u["content"]["text"].as_str().filter(|_| u["content"]["type"] == "text") else { return vec![] };
                let kind = if kind == "agent_thought_chunk" { DeltaKind::Thinking } else { DeltaKind::Text };
                let mut out = vec![];
                if self.open.as_ref().is_some_and(|(_, k, _)| *k != kind) {
                    out.extend(self.close());
                }
                let open = self.open.get_or_insert_with(|| {
                    self.blocks += 1;
                    (format!("acp-{}{}-{}", self.nonce, self.turn, self.blocks), kind, String::new())
                });
                open.2.push_str(text);
                let message = open.0.clone();
                if out.is_empty() && open.2.len() == text.len() {
                    out.push(Event::BlockStart { message: message.clone(), index: 0, parent: None, kind: if kind == DeltaKind::Thinking { BlockKind::Thinking } else { BlockKind::Text } });
                }
                out.push(Event::Delta { message, index: 0, parent: None, kind, text: text.to_string() });
                out
            }
            "tool_call" | "tool_call_update" => {
                let Some(id) = u["toolCallId"].as_str().map(str::to_string) else { return vec![] };
                // A background job's update: its row is updated where it is, and its end ends the job.
                if let Some(mut call) = self.jobs.remove(&id) {
                    let status = u["status"].as_str().unwrap_or("");
                    if !u["rawOutput"].is_null() {
                        call.raw_output = u["rawOutput"].clone();
                    }
                    if let Some(c) = u["content"].as_array() {
                        call.content = c.clone();
                    }
                    if status != "completed" && status != "failed" {
                        self.jobs.insert(id, call);
                        return vec![];
                    }
                    let full = result_text(&call);
                    let len = full.len();
                    let code = call.raw_output["exit_code"].as_i64();
                    let state = if status == "failed" { "failed" } else { "completed" };
                    let summary = match code {
                        Some(c) => format!("Background command \"{}\" {state} (exit code {c})", call.title),
                        None => format!("Background command \"{}\" {state}", call.title),
                    };
                    let (text, full) = if len > RESULT_HEAD {
                        let cut = (0..=RESULT_HEAD).rev().find(|i| full.is_char_boundary(*i)).unwrap_or(0);
                        (full[..cut].to_string(), Some(full))
                    } else {
                        (full, None)
                    };
                    return vec![
                        Event::ToolResults { parent: None, results: vec![ToolResult { id: id.clone(), text, len, is_error: status == "failed" || code.is_some_and(|c| c != 0), full }] },
                        Event::JobFinished { task_id: id, status: state.into(), summary: Some(summary), output_file: None },
                        Event::JobsRunning(self.jobs.keys().cloned().collect()),
                    ];
                }
                let mut out = self.close();
                let call = self.calls.entry(id.clone()).or_default();
                if let Some(t) = u["title"].as_str() {
                    call.title = t.to_string();
                }
                if let Some(k) = u["kind"].as_str() {
                    call.kind = k.to_string();
                }
                if !u["rawInput"].is_null() {
                    call.raw_input = u["rawInput"].clone();
                }
                if let Some(c) = u["content"].as_array() {
                    call.content = c.clone();
                }
                if let Some(l) = u["locations"].as_array() {
                    call.locations = l.clone();
                }
                if !u["rawOutput"].is_null() {
                    call.raw_output = u["rawOutput"].clone();
                }
                let shown = shown_tool(call);
                if call.shown.as_ref() != Some(&shown) {
                    call.shown = Some(shown.clone());
                    out.push(Event::Assistant { message: format!("acp-tool-{id}"), parent: None, blocks: vec![Block::Tool { id: id.clone(), name: shown.0, input: shown.1 }] });
                }
                let status = u["status"].as_str().unwrap_or("");
                if (status == "completed" || status == "failed") && !call.finished {
                    call.finished = true;
                    let full = result_text(call);
                    let len = full.len();
                    let (text, full) = if len > RESULT_HEAD {
                        let cut = (0..=RESULT_HEAD).rev().find(|i| full.is_char_boundary(*i)).unwrap_or(0);
                        (full[..cut].to_string(), Some(full))
                    } else {
                        (full, None)
                    };
                    out.push(Event::ToolResults { parent: None, results: vec![ToolResult { id, text, len, is_error: status == "failed", full }] });
                }
                out
            }
            // The agent's plan, the whole of it each time: our checklist, as Claude's TodoWrite.
            "plan" => {
                let todos: Vec<Value> = u["entries"]
                    .as_array()
                    .map(|e| e.iter().map(|e| json!({ "content": e["content"], "status": e["status"], "activeForm": e["content"] })).collect())
                    .unwrap_or_default();
                let mut out = self.close();
                self.blocks += 1;
                let id = format!("acp-plan-{}{}-{}", self.nonce, self.turn, self.blocks);
                out.push(Event::Assistant { message: id.clone(), parent: None, blocks: vec![Block::Tool { id: id.clone(), name: "TodoWrite".into(), input: json!({ "todos": todos }).to_string() }] });
                out.push(Event::ToolResults { parent: None, results: vec![ToolResult { id, text: "Plan updated.".into(), len: 13, is_error: false, full: None }] });
                out
            }
            "available_commands_update" => {
                self.commands = u["availableCommands"].as_array().cloned().unwrap_or_default();
                self.catalog()
            }
            "config_option_update" => {
                self.note_config(&u["configOptions"]);
                self.catalog()
            }
            "current_mode_update" => {
                self.current_mode = u["currentModeId"].as_str().map(str::to_string);
                vec![]
            }
            // Tokens in the context and the window's size: the composer's context ring.
            "usage_update" => {
                let mut out = vec![];
                if let Some(size) = u["size"].as_u64().filter(|s| *s > 0 && self.window != Some(*s)) {
                    self.window = Some(size);
                    out.push(Event::ContextWindows(vec![(self.current_model.clone().unwrap_or_default(), size)]));
                }
                // As it goes, as Claude's does; 0 is the session's start, before anything's in it.
                if let Some(used) = u["used"].as_u64().filter(|u| *u > 0) {
                    self.usage = Some(used);
                    out.push(Event::ContextUsage(used));
                }
                out
            }
            _ => vec![],
        }
    }

    /// A permission request's tool, shown as its call is.
    pub fn permission_tool(&mut self, tool_call: &Value) -> (String, String) {
        let id = tool_call["toolCallId"].as_str().unwrap_or("");
        let mut call = self.calls.get(id).cloned().unwrap_or_default();
        if let Some(t) = tool_call["title"].as_str() {
            call.title = t.to_string();
        }
        if let Some(k) = tool_call["kind"].as_str() {
            call.kind = k.to_string();
        }
        if !tool_call["rawInput"].is_null() {
            call.raw_input = tool_call["rawInput"].clone();
        }
        if let Some(c) = tool_call["content"].as_array() {
            call.content = c.clone();
        }
        if let Some(l) = tool_call["locations"].as_array() {
            call.locations = l.clone();
        }
        shown_tool(&call)
    }
}

/// The first string among these keys of the call's input.
fn input_str(input: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| input[*k].as_str().filter(|s| !s.is_empty()).map(str::to_string))
}

fn first_path(call: &Call) -> Option<String> {
    call.locations
        .iter()
        .find_map(|l| l["path"].as_str().map(str::to_string))
        .or_else(|| call.content.iter().find_map(|c| (c["type"] == "diff").then(|| c["path"].as_str().map(str::to_string)).flatten()))
        .or_else(|| input_str(&call.raw_input, &["path", "file_path", "filePath", "AbsolutePath", "TargetFile", "file"]))
}

/// How a call shows: as the Claude Code tool it is when it is one (so its row reads as Claude's
/// do: a command, a file read, an edit's diff), else under its own title with its input.
fn shown_tool(call: &Call) -> (String, String) {
    let input = &call.raw_input;
    match call.kind.as_str() {
        "execute" => {
            let command = input_str(input, &["command", "CommandLine", "cmd", "commandLine"]).unwrap_or_else(|| call.title.clone());
            ("Bash".into(), json!({ "command": command, "description": call.title }).to_string())
        }
        "read" => match first_path(call) {
            Some(path) => ("Read".into(), json!({ "file_path": path }).to_string()),
            None => (title_name(call), input.to_string()),
        },
        "edit" => {
            let diff = call.content.iter().find(|c| c["type"] == "diff");
            // Antigravity's `edit_file` names the exact text it replaced; its diff is the whole file.
            if let (Some(path), Some(old), Some(new)) = (first_path(call), input_str(input, &["Target", "TargetContent"]), input["Replacement"].as_str().or(input["ReplacementContent"].as_str())) {
                return ("Edit".into(), json!({ "file_path": path, "old_string": old, "new_string": new }).to_string());
            }
            match (diff, first_path(call)) {
                (Some(d), Some(path)) if d["oldText"].as_str().is_none_or(str::is_empty) => {
                    ("Write".into(), json!({ "file_path": path, "content": d["newText"].as_str().unwrap_or("") }).to_string())
                }
                (Some(d), Some(path)) => (
                    "Edit".into(),
                    json!({ "file_path": path, "old_string": d["oldText"].as_str().unwrap_or(""), "new_string": d["newText"].as_str().unwrap_or("") }).to_string(),
                ),
                (None, Some(path)) => ("Edit".into(), json!({ "file_path": path }).to_string()),
                _ => (title_name(call), input.to_string()),
            }
        }
        "fetch" => match input_str(input, &["url", "Url"]) {
            Some(url) => ("WebFetch".into(), json!({ "url": url }).to_string()),
            None => (title_name(call), input.to_string()),
        },
        _ => (title_name(call), if input.is_null() { "{}".into() } else { input.to_string() }),
    }
}

fn title_name(call: &Call) -> String {
    if call.title.is_empty() {
        "Tool".into()
    } else {
        call.title.clone()
    }
}

/// What a finished call says: its text content, diffs as such, else its raw output.
fn result_text(call: &Call) -> String {
    let mut parts: Vec<String> = vec![];
    for c in &call.content {
        match c["type"].as_str() {
            Some("content") => {
                if let Some(t) = c["content"]["text"].as_str() {
                    parts.push(t.to_string());
                }
            }
            Some("diff") => parts.push(format!("Edited {}", c["path"].as_str().unwrap_or("a file"))),
            _ => {}
        }
    }
    if parts.is_empty() {
        return match &call.raw_output {
            Value::Null => String::new(),
            Value::String(s) => s.clone(),
            other => input_str(other, &["output", "stdout", "result", "text"]).unwrap_or_else(|| other.to_string()),
        };
    }
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(kind: &str, text: &str) -> Value {
        json!({ "sessionUpdate": kind, "content": { "type": "text", "text": text } })
    }

    #[test]
    fn text_streams_into_one_block_per_kind() {
        let mut t = Translator::default();
        t.start_turn();
        let a = t.update(&chunk("agent_thought_chunk", "Hmm"));
        assert!(matches!(&a[0], Event::BlockStart { kind: BlockKind::Thinking, .. }));
        assert!(matches!(&a[1], Event::Delta { kind: DeltaKind::Thinking, text, .. } if text == "Hmm"));
        let b = t.update(&chunk("agent_message_chunk", "Hel"));
        // The thought settles, the reply starts.
        assert!(matches!(&b[0], Event::Assistant { blocks, .. } if matches!(&blocks[0], Block::Thinking(t) if t == "Hmm")));
        assert!(matches!(&b[1], Event::Delta { kind: DeltaKind::Text, .. }));
        let c = t.update(&chunk("agent_message_chunk", "lo"));
        assert_eq!(c.len(), 1);
        let end = t.end_turn(false);
        assert!(matches!(&end[0], Event::Assistant { blocks, .. } if matches!(&blocks[0], Block::Text(t) if t == "Hello")));
    }

    #[test]
    fn tool_calls_show_as_claude_tools() {
        let mut t = Translator::default();
        t.start_turn();
        let a = t.update(&json!({ "sessionUpdate": "tool_call", "toolCallId": "c1", "title": "Run ls", "kind": "execute", "status": "pending", "rawInput": { "CommandLine": "ls -la" } }));
        match &a[0] {
            Event::Assistant { blocks, .. } => match &blocks[0] {
                Block::Tool { id, name, input } => {
                    assert_eq!((id.as_str(), name.as_str()), ("c1", "Bash"));
                    assert!(input.contains("ls -la"));
                }
                b => panic!("{b:?}"),
            },
            e => panic!("{e:?}"),
        }
        // Only the status changes: nothing new to show until it ends.
        assert!(t.update(&json!({ "sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "in_progress" })).is_empty());
        let done = t.update(&json!({ "sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed", "content": [{ "type": "content", "content": { "type": "text", "text": "a.txt" } }] }));
        assert!(matches!(&done[0], Event::ToolResults { results, .. } if results[0].text == "a.txt" && !results[0].is_error));
        assert!(t.end_turn(false).is_empty());
    }

    #[test]
    fn diffs_are_edits_or_writes() {
        let mut t = Translator::default();
        t.start_turn();
        let new = t.update(&json!({ "sessionUpdate": "tool_call", "toolCallId": "w", "title": "Create a.rs", "kind": "edit", "content": [{ "type": "diff", "path": "/p/a.rs", "oldText": null, "newText": "fn a() {}" }] }));
        assert!(matches!(&new[0], Event::Assistant { blocks, .. } if matches!(&blocks[0], Block::Tool { name, .. } if name == "Write")));
        let edit = t.update(&json!({ "sessionUpdate": "tool_call", "toolCallId": "e", "title": "Edit a.rs", "kind": "edit", "content": [{ "type": "diff", "path": "/p/a.rs", "oldText": "a", "newText": "b" }] }));
        assert!(matches!(&edit[0], Event::Assistant { blocks, .. } if matches!(&blocks[0], Block::Tool { name, input, .. } if name == "Edit" && input.contains("old_string"))));
        // Never finished: the turn's end settles both as stopped.
        assert_eq!(t.end_turn(false).len(), 2);
    }

    #[test]
    fn antigravity_edits_show_only_what_changed() {
        let mut t = Translator::default();
        t.start_turn();
        // As agy 1.5.14 sends it: the exact text replaced in rawInput, the whole file in the diff.
        let out = t.update(&json!({ "sessionUpdate": "tool_call", "toolCallId": "e", "kind": "edit", "title": "File main.rs", "locations": [{ "path": "/p/main.rs" }],
            "rawInput": { "Path": "/p/main.rs", "Target": "    println!(\"hello\");", "Replacement": "    println!(\"hello world\");" },
            "content": [{ "type": "diff", "path": "/p/main.rs", "oldText": "fn main() {\n    println!(\"hello\");\n}\n", "newText": "fn main() {\n    println!(\"hello world\");\n}\n" }] }));
        match &out[0] {
            Event::Assistant { blocks, .. } => match &blocks[0] {
                Block::Tool { name, input, .. } => {
                    let v: Value = serde_json::from_str(input).unwrap();
                    assert_eq!(name, "Edit");
                    assert_eq!(v["old_string"], "    println!(\"hello\");");
                }
                b => panic!("{b:?}"),
            },
            e => panic!("{e:?}"),
        }
        assert_eq!(agent_mode("acceptEdits", &["default".into(), "accept-edits".into()]), "accept-edits");
        assert_eq!(agent_mode("acceptEdits", &["default".into()]), "default");
    }

    /// A real session with Antigravity 1.5.14 in Accept Edits (sanitized): the agent's side
    /// replayed, ours checked as we'd send it.
    #[test]
    fn replays_a_real_antigravity_session() {
        let recorded = include_str!("../fixtures/antigravity-acp-1.5.14.jsonl");
        let (w, sent) = flume::unbounded();
        let mut s = Shared {
            next_id: 0,
            asked: HashMap::new(),
            session: None,
            ready: false,
            busy: false,
            replaying: false,
            queue: VecDeque::new(),
            permissions: HashMap::new(),
            translator: Translator::default(),
            model: None,
            mode: Some("acceptEdits".into()),
            cwd: "/work/rproj".into(),
            resume: None,
            capabilities: Value::Null,
            dead: false,
            woke: None,
            auth_methods: vec![],
        };
        s.request("initialize", json!({}), Asked::Initialize, &w);
        s.queue.push_back(Waiting { id: "m1".into(), text: "Change main.rs so it prints hi instead of hello.".into(), images: vec![] });
        let mut events = vec![];
        for line in recorded.lines() {
            let (dir, body) = line.split_at(2);
            let message: Value = serde_json::from_str(body).unwrap();
            if dir.starts_with('>') {
                events.extend(s.receive(&message, &w));
            }
        }
        let ours: Vec<String> = sent.try_iter().map(|m| m["method"].as_str().unwrap_or("").to_string()).collect();
        assert_eq!(ours, ["initialize", "session/new", "session/set_config_option", "session/prompt"]);
        assert!(matches!(&events[0], Event::Session { .. }));
        // Its models come from the model setting's choices; the one in effect with them.
        assert!(events.iter().any(|e| matches!(e, Event::AgentCatalog { models, current_model: Some(c), .. } if c == "gemini-4-neon" && models.as_array().is_some_and(|m| m.len() > 3))));
        let edit = events.iter().find_map(|e| match e {
            Event::Assistant { blocks, .. } => blocks.iter().find_map(|b| match b {
                Block::Tool { name, input, .. } if name == "Edit" => Some(input.clone()),
                _ => None,
            }),
            _ => None,
        });
        assert!(edit.is_some_and(|i| i.contains("old_string") && i.contains("hi")));
        let reply: String = events.iter().filter_map(|e| match e {
            Event::Delta { kind: DeltaKind::Text, text, .. } => Some(text.as_str()),
            _ => None,
        }).collect();
        assert!(reply.contains("hi"), "{reply}");
        assert!(matches!(events.iter().rev().find(|e| matches!(e, Event::Done { .. })), Some(Event::Done { is_error: false, .. })));
        assert!(s.permissions.is_empty() && !s.busy);
    }

    #[test]
    fn usage_reads_as_claude_limits() {
        // As `agy -p /usage --output-format json` (1.5.14) says it, its `command.data`.
        let data = json!({ "groups": [
            { "name": "Gemini Models", "description": "Models within this group: Gemini 4 Neon", "buckets": [
                { "id": "gemini-weekly", "window": "weekly", "remaining_fraction": 0.75, "reset_time": "2026-11-28T12:02:36Z" },
                { "id": "gemini-5h", "window": "5h", "remaining_fraction": 0.9882404804229736, "reset_time": "2026-11-21T17:02:36Z" } ] },
            { "name": "Empty", "buckets": [] } ] });
        let groups = usage_groups(&data);
        assert_eq!(groups.len(), 1);
        let w = &groups[0].windows;
        assert_eq!((w[0].name.as_str(), w[1].name.as_str()), ("five_hour", "seven_day"));
        assert!((w[1].utilization - 0.25).abs() < 1e-9);
        assert_eq!(w[0].resets_at, crate::library::parse_iso("2026-11-21T17:02:36Z"));
    }

    /// A real session with Antigravity 1.5.14 (sanitized): a command it moved to the background,
    /// the prompt's end while it ran, then its result and the agent's reply with no prompt open.
    #[test]
    fn background_commands_are_jobs_and_a_woken_turn_ends_when_quiet() {
        let recorded = include_str!("../fixtures/antigravity-acp-background.jsonl");
        let (w, _sent) = flume::unbounded();
        let mut s = Shared {
            next_id: 0,
            asked: HashMap::new(),
            session: None,
            ready: false,
            busy: false,
            replaying: false,
            queue: VecDeque::new(),
            permissions: HashMap::new(),
            translator: Translator::default(),
            model: None,
            mode: Some("bypassPermissions".into()),
            cwd: "/work".into(),
            resume: None,
            capabilities: Value::Null,
            dead: false,
            woke: None,
            auth_methods: vec![],
        };
        s.request("initialize", json!({}), Asked::Initialize, &w);
        s.queue.push_back(Waiting { id: "m1".into(), text: "run it in the background".into(), images: vec![] });
        let mut events = vec![];
        for line in recorded.lines().filter(|l| l.starts_with('>')) {
            events.extend(s.receive(&serde_json::from_str(&line[2..]).unwrap(), &w));
        }
        let at = |pred: &dyn Fn(&Event) -> bool| events.iter().position(|e| pred(e)).expect("event");
        let done = at(&|e| matches!(e, Event::Done { is_error: false, .. }));
        let started = at(&|e| matches!(e, Event::JobStarted { background: true, .. }));
        let finished = at(&|e| matches!(e, Event::JobFinished { status, summary: Some(sm), .. } if status == "completed" && sm.contains("exit code 0")));
        // The job starts as the prompt's turn ends, and ends after it.
        assert!(started < done && done < finished);
        assert!(!events.iter().any(|e| matches!(e, Event::ToolResults { results, .. } if results[0].text == "Stopped.")));
        // Its reply came with no prompt: a woken turn, over once quiet.
        assert!(s.woke.is_some());
        s.woke = Some(std::time::Instant::now() - std::time::Duration::from_secs(5));
        assert!(s.woken_turn_over());
        let end = s.end_woken_turn();
        assert!(matches!(end.last(), Some(Event::Done { is_error: false, .. })));
        assert!(s.woke.is_none());
    }

    #[test]
    fn plans_become_the_checklist() {
        let mut t = Translator::default();
        t.start_turn();
        let out = t.update(&json!({ "sessionUpdate": "plan", "entries": [{ "content": "Read", "priority": "high", "status": "completed" }, { "content": "Write", "priority": "high", "status": "in_progress" }] }));
        match &out[0] {
            Event::Assistant { blocks, .. } => match &blocks[0] {
                Block::Tool { name, input, .. } => {
                    assert!(crate::plan::is_plan_tool(name, input));
                    assert!(input.contains("in_progress"));
                }
                b => panic!("{b:?}"),
            },
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn permission_answers_pick_the_matching_option() {
        let options = vec![
            json!({ "optionId": "a1", "kind": "allow_always", "name": "Always" }),
            json!({ "optionId": "a0", "kind": "allow_once", "name": "Allow" }),
            json!({ "optionId": "r0", "kind": "reject_once", "name": "Reject" }),
        ];
        assert_eq!(permission_outcome(&options, true)["optionId"], "a0");
        assert_eq!(permission_outcome(&options, false)["optionId"], "r0");
        assert_eq!(permission_outcome(&options[..1], false)["outcome"], "cancelled");
    }

    #[test]
    fn a_session_resumes_or_starts_and_prompts_wait_for_it() {
        let (w, sent) = flume::unbounded();
        let mut s = Shared {
            next_id: 0,
            asked: HashMap::new(),
            session: None,
            ready: false,
            busy: false,
            replaying: false,
            queue: VecDeque::new(),
            permissions: HashMap::new(),
            translator: Translator::default(),
            model: Some("gemini-pro".into()),
            mode: Some("plan".into()),
            cwd: "/p".into(),
            resume: Some("old".into()),
            capabilities: Value::Null,
            dead: false,
            woke: None,
            auth_methods: vec![],
        };
        s.request("initialize", json!({}), Asked::Initialize, &w);
        let _ = sent.try_recv();
        s.queue.push_back(Waiting { id: "m1".into(), text: "hi".into(), images: vec![] });
        // It can load sessions: the replay isn't shown.
        s.receive(&json!({ "jsonrpc": "2.0", "id": 1, "result": { "protocolVersion": 1, "agentCapabilities": { "loadSession": true } } }), &w);
        let load = sent.try_recv().unwrap();
        assert_eq!(load["method"], "session/load");
        assert!(s.receive(&json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": "old", "update": chunk("agent_message_chunk", "old reply") } }), &w).is_empty());
        let out = s.receive(&json!({ "jsonrpc": "2.0", "id": 2, "result": { "modes": { "currentModeId": "default", "availableModes": [{ "id": "default" }, { "id": "plan" }] }, "models": { "currentModelId": "gemini-flash", "availableModels": [{ "modelId": "gemini-flash", "name": "Flash" }, { "modelId": "gemini-pro", "name": "Pro" }] } } }), &w);
        assert!(matches!(&out[0], Event::Session { id } if id == "old"));
        assert!(out.iter().any(|e| matches!(e, Event::Lifecycle { id, state } if id == "m1" && state == "started")));
        let methods: Vec<String> = sent.try_iter().map(|m| m["method"].as_str().unwrap_or("").to_string()).collect();
        assert_eq!(methods, ["session/set_model", "session/prompt"]);
        // Its end: done, and nothing left waiting.
        let end = s.receive(&json!({ "jsonrpc": "2.0", "id": 4, "result": { "stopReason": "end_turn" } }), &w);
        assert!(matches!(end.last(), Some(Event::Done { is_error: false, .. })));
        assert!(!s.busy);
    }
}
