//! A `claude` process per thread that's been talked to, speaking stream-json as HarnessCore's
//! `ClaudeSession` does (same arguments, permission prompts as `control_request`s). Its output
//! is parsed on a reader thread into `Event`s; the UI only takes what's ready, once per wakeup.

use crate::history::tool_result_text;
use crate::library::{head, LIVE_RESULT_LIMIT, TOOL_TEXT_LIMIT};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub enum BlockKind {
    Text,
    Thinking,
    Tool { id: String, name: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeltaKind {
    Text,
    Thinking,
    Input,
}

#[derive(Clone, Debug)]
pub enum Block {
    Text(String),
    Thinking(String),
    Tool { id: String, name: String, input: String },
}

#[derive(Clone, Debug)]
pub struct ToolResult {
    pub id: String,
    pub text: String,
    pub len: usize,
    pub is_error: bool,
    /// The whole result when `text` is only its head: for saving, not for holding.
    pub full: Option<String>,
}

/// How a permission request is settled.
#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    /// Run the tool, with changed input (answers to questions) and a permission mode for the
    /// rest of the session (approving a plan into Auto) when given.
    Allow { updated_input: Option<String>, mode: Option<String> },
    /// Don't; the message goes back to Claude (feedback on a plan).
    Deny { message: String },
}

impl Decision {
    pub fn allow() -> Decision {
        Decision::Allow { updated_input: None, mode: None }
    }

    pub fn deny() -> Decision {
        Decision::Deny { message: "The user denied this action.".into() }
    }
}

/// A slash command or skill, as the CLI's catalog lists it (saved in the Swift app's shape:
/// `name`, `description`, `argumentHint`, `aliases`, `isBuiltin`).
#[derive(Clone, Debug, PartialEq)]
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    pub hint: String,
}

impl SlashCommand {
    pub fn from_json(v: &Value) -> Option<SlashCommand> {
        let name = v["name"].as_str().filter(|n| !n.is_empty())?;
        Some(SlashCommand {
            name: name.to_string(),
            description: v["description"].as_str().unwrap_or("").to_string(),
            hint: v["argumentHint"].as_str().unwrap_or("").to_string(),
        })
    }
}

/// A model as the CLI's catalog lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelInfo {
    /// What `--model` takes: an alias (`opus`) or a full ID.
    pub value: String,
    pub display_name: String,
    pub description: Option<String>,
    pub efforts: Vec<String>,
}

impl ModelInfo {
    pub fn from_json(v: &Value) -> Option<ModelInfo> {
        Some(ModelInfo {
            value: v["value"].as_str()?.to_string(),
            display_name: v["displayName"].as_str()?.to_string(),
            description: v["description"].as_str().map(str::to_string),
            efforts: v["supportedEffortLevels"]
                .as_array()
                .map(|a| a.iter().filter_map(|e| e.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
        })
    }
}

#[derive(Clone, Debug)]
pub enum Event {
    /// The CLI's catalog, as JSON: `models` (see `ModelInfo`) and `commands`.
    Catalog { models: Value, commands: Value },
    /// Where your message is: `queued` (waiting for the current step), `started`,
    /// `completed`, `cancelled`.
    Lifecycle { id: String, state: String },
    /// Usage limits as the CLI reported them (`rate_limit_info`).
    Limits(Value),
    Session { id: String },
    BlockStart { message: String, index: u64, parent: Option<String>, kind: BlockKind },
    Delta { message: String, index: u64, parent: Option<String>, kind: DeltaKind, text: String },
    Assistant { message: String, parent: Option<String>, blocks: Vec<Block> },
    ToolResults { parent: Option<String>, results: Vec<ToolResult> },
    SubagentStarted { tool_id: String, description: String },
    /// A sub-agent's end, with its summary (a background one's report comes only here).
    SubagentFinished { tool_id: String, status: String, summary: Option<String> },
    /// A job Claude left running: a command in the background (`run_in_background`), a
    /// foreground one that's taking a while, or a watch (Monitor). `task_id` is what
    /// `stop_task` takes; `tool_id` the call that started it.
    JobStarted { task_id: String, tool_id: Option<String>, description: String, background: bool },
    /// Its end: `status` `completed`, `failed`, `killed`, `stopped`…; `summary` says the exit
    /// code (`job_exit_code`), `output_file` holds everything it printed.
    JobFinished { task_id: String, status: String, summary: Option<String>, output_file: Option<String> },
    /// What a watch saw, as it wakes Claude with it (a `<task-notification>`'s `<event>`).
    JobOutput { task_id: String, text: String },
    /// Every job still running, sent whenever that changes: the list to trust over the ends.
    JobsRunning(Vec<String>),
    Permission { request_id: String, tool: String, description: String, input: String },
    /// An MCP server asks something mid-call: a form (`schema`) or a page to visit (`url`).
    Elicitation { request_id: String, server: String, message: String, mode: String, schema: Value, url: Option<String> },
    Notice(String),
    Done { is_error: bool, text: String },
    Exit { code: i32, tail: String },
}

impl Event {
    /// Consecutive deltas of the same block become one: a burst of them costs one change.
    pub fn merge(&mut self, next: &Event) -> bool {
        match (self, next) {
            (
                Event::Delta { message: m, index: i, parent: p, kind: k, text },
                Event::Delta { message: m2, index: i2, parent: p2, kind: k2, text: more },
            ) if m == m2 && i == i2 && p == p2 && k == k2 => {
                text.push_str(more);
                true
            }
            _ => false,
        }
    }
}

pub struct LaunchOptions {
    pub cwd: String,
    pub session_id: Option<String>,
    pub model: Option<String>,
    /// The CLI's `--permission-mode`, none for its default (asking).
    pub mode: Option<String>,
    pub effort: Option<String>,
    /// More folders it may read without asking (the app's attachments).
    pub add_dirs: Vec<String>,
    pub env: Vec<(String, String)>,
    /// What the agent is told on top of Claude Code's own prompt (the memory index…).
    pub append_system_prompt: Option<String>,
    /// Tools that never ask (`mcp__memory`: they touch only the app's own database).
    pub allowed_tools: Vec<String>,
    /// Tools served from inside the app.
    pub servers: Vec<std::sync::Arc<dyn crate::mcp::ToolServer>>,
    /// More arguments for `claude` (a side chat's `--fork-session`, `--tools`…).
    pub extra_args: Vec<String>,
}

type Waiting = Arc<Mutex<std::collections::HashMap<String, flume::Sender<Result<Value, String>>>>>;

pub struct ClaudeProcess {
    /// Control requests waiting for their reply, by `request_id`.
    waiting: Waiting,
    child: Arc<Mutex<Child>>,
    writer: flume::Sender<Value>,
    counter: u64,
    pub pid: u32,
    /// The app's own tool servers, named in `initialize`.
    server_names: Vec<String>,
}

static CHOSEN: std::sync::Mutex<Option<std::path::PathBuf>> = std::sync::Mutex::new(None);

/// The `claude` chosen in Settings → Agents (`cliPathOverrides.claude`), used in place of the one found.
pub fn set_chosen_claude(path: Option<std::path::PathBuf>) {
    *CHOSEN.lock().unwrap() = path;
}

/// The chosen `claude`, else the one on `YH_PATH` (as the app's test runs pass it), `PATH` and the usual places.
pub fn find_claude() -> Option<std::path::PathBuf> {
    CHOSEN.lock().unwrap().clone().filter(|p| p.is_file()).or_else(|| crate::agents::which("claude"))
}

impl ClaudeProcess {
    pub fn spawn(options: LaunchOptions, events: flume::Sender<Event>) -> Result<ClaudeProcess, String> {
        let executable = find_claude().ok_or("claude isn't on PATH")?;
        let mut args: Vec<String> = [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--replay-user-messages",
            "--permission-prompt-tool",
            "stdio",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        if let Some(model) = options.model.filter(|m| !m.is_empty()) {
            args.extend(["--model".into(), model]);
        }
        if let Some(mode) = options.mode {
            args.extend(["--permission-mode".into(), mode]);
        }
        if let Some(effort) = options.effort.filter(|e| !e.is_empty()) {
            args.extend(["--effort".into(), effort]);
        }
        for dir in options.add_dirs {
            args.extend(["--add-dir".into(), dir]);
        }
        if let Some(prompt) = options.append_system_prompt.filter(|p| !p.is_empty()) {
            args.extend(["--append-system-prompt".into(), prompt]);
        }
        if !options.allowed_tools.is_empty() {
            args.extend(["--allowedTools".into(), options.allowed_tools.join(",")]);
        }
        let servers: std::collections::HashMap<String, std::sync::Arc<dyn crate::mcp::ToolServer>> =
            options.servers.into_iter().map(|s| (s.name().to_string(), s)).collect();
        let mut server_names: Vec<String> = servers.keys().cloned().collect();
        server_names.sort();
        if let Some(session) = options.session_id {
            args.extend(["--resume".into(), session]);
        }
        args.extend(options.extra_args);
        let mut command = Command::new(&executable);
        command.args(&args).current_dir(&options.cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        if let Ok(path) = std::env::var("YH_PATH") {
            command.env("PATH", path);
        }
        for (k, v) in &options.env {
            command.env(k, v);
        }
        let mut child = command.spawn().map_err(|e| format!("couldn't start claude: {e}"))?;
        let pid = child.id();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let child = Arc::new(Mutex::new(child));

        // Writes go through a thread: a full pipe must never hold up the UI.
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
        let waiting: Waiting = Arc::default();
        {
            let child = child.clone();
            let mcp_writer = writer.clone();
            let waiting = waiting.clone();
            std::thread::spawn(move || {
                let mut parser = Parser::default();
                let mut reader = BufReader::with_capacity(1 << 20, stdout);
                let mut line = Vec::new();
                loop {
                    line.clear();
                    match reader.by_ref().read_until(b'\n', &mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                    let Ok(object) = serde_json::from_slice::<Value>(&line) else { continue };
                    // A call to one of the app's own tools: answered on a thread of its own, so a
                    // slow one doesn't hold up the rest.
                    if object["type"] == "control_request" && object["request"]["subtype"] == "mcp_message" {
                        let request_id = object["request_id"].clone();
                        let server = object["request"]["server_name"].as_str().unwrap_or("").to_string();
                        let message = object["request"]["message"].clone();
                        let (servers, w) = (servers.clone(), mcp_writer.clone());
                        std::thread::spawn(move || {
                            let reply = match servers.get(&server) {
                                Some(s) => crate::mcp::handle(s.as_ref(), &message).unwrap_or(json!({ "jsonrpc": "2.0", "result": {} })),
                                None => json!({ "jsonrpc": "2.0", "id": message["id"], "error": { "code": -32601, "message": format!("No server {server}") } }),
                            };
                            let _ = w.send(json!({
                                "type": "control_response",
                                "response": { "subtype": "success", "request_id": request_id, "response": { "mcp_response": reply } },
                            }));
                        });
                        continue;
                    }
                    // The reply to a request someone waits on (`request`).
                    if object["type"] == "control_response" {
                        let response = &object["response"];
                        if let Some(id) = response["request_id"].as_str() {
                            if let Some(reply) = waiting.lock().unwrap().remove(id) {
                                let _ = reply.send(if response["subtype"] == "error" {
                                    Err(response["error"].as_str().unwrap_or("The request failed.").to_string())
                                } else {
                                    Ok(response["response"].clone())
                                });
                            }
                        }
                    }
                    for event in parser.parse(&object) {
                        if events.send(event).is_err() {
                            return;
                        }
                    }
                }
                let code = child.lock().unwrap().wait().ok().and_then(|s| s.code()).unwrap_or(-1);
                let tail = tail.lock().unwrap().clone();
                let _ = events.send(Event::Exit { code, tail });
            });
        }
        Ok(ClaudeProcess { waiting, child, writer, counter: 0, pid, server_names })
    }

    /// Your message. `id` comes back in the CLI's message lifecycle; `priority` `now`
    /// interrupts the turn for it, none waits for the current step (steering).
    pub fn send(&self, text: &str, images: &[(String, String)], id: &str, priority: Option<&str>) {
        // Images first, as the Swift app sends them: (media type, base64).
        let mut content: Vec<Value> = images
            .iter()
            .map(|(media, data)| json!({ "type": "image", "source": { "type": "base64", "media_type": media, "data": data } }))
            .collect();
        if !text.is_empty() || content.is_empty() {
            content.push(json!({ "type": "text", "text": text }));
        }
        let mut message = json!({
            "type": "user",
            "uuid": id,
            "message": { "role": "user", "content": content },
        });
        if let Some(priority) = priority {
            message["priority"] = priority.into();
        }
        let _ = self.writer.send(message);
    }

    /// Asks for the catalog (models, efforts, slash commands): answered with `Event::Catalog`.
    pub fn initialize(&mut self) {
        let mut request = json!({ "subtype": "initialize" });
        if !self.server_names.is_empty() {
            request["sdkMcpServers"] = json!(self.server_names);
        }
        self.control(request);
    }

    /// A control request whose reply is waited for: `mcp_status`, `mcp_toggle`…
    pub fn request(&mut self, request: Value) -> flume::Receiver<Result<Value, String>> {
        self.counter += 1;
        let id = format!("yh-{}", self.counter);
        let (tx, rx) = flume::bounded(1);
        self.waiting.lock().unwrap().insert(id.clone(), tx);
        let _ = self.writer.send(json!({ "type": "control_request", "request_id": id, "request": request }));
        rx
    }

    /// An MCP server's question mid-call (elicitation): `accept` (with `content` for a
    /// form), `decline` or `cancel`.
    pub fn respond_elicitation(&self, request_id: &str, action: &str, content: Option<Value>) {
        let mut body = json!({ "action": action });
        if let Some(c) = content {
            body["content"] = c;
        }
        let _ = self.writer.send(json!({
            "type": "control_response",
            "response": { "subtype": "success", "request_id": request_id, "response": body },
        }));
    }

    fn control(&mut self, request: Value) {
        self.counter += 1;
        let _ = self.writer.send(json!({ "type": "control_request", "request_id": format!("yh-{}", self.counter), "request": request }));
    }

    pub fn interrupt(&mut self) {
        self.control(json!({ "subtype": "interrupt" }));
    }

    /// Stops a job Claude left running (a background command, a watch), as the Swift app's
    /// Stop does; the CLI reports its end as for any other.
    pub fn stop_task(&mut self, task_id: &str) {
        self.control(json!({ "subtype": "stop_task", "task_id": task_id }));
    }

    pub fn set_mode(&mut self, mode: &str) {
        self.control(json!({ "subtype": "set_permission_mode", "mode": mode }));
    }

    pub fn set_model(&mut self, model: &str) {
        self.control(json!({ "subtype": "set_model", "model": if model.is_empty() { "default" } else { model } }));
    }

    pub fn respond(&self, request_id: &str, input: &str, decision: &Decision) {
        let body = match decision {
            Decision::Allow { updated_input, mode } => {
                let input: Value = serde_json::from_str(updated_input.as_deref().unwrap_or(input)).unwrap_or(json!({}));
                let mut allow = json!({ "behavior": "allow", "updatedInput": input });
                if let Some(mode) = mode {
                    allow["updatedPermissions"] = json!([{ "type": "setMode", "mode": mode, "destination": "session" }]);
                }
                allow
            }
            Decision::Deny { message } => json!({ "behavior": "deny", "message": message }),
        };
        let _ = self.writer.send(json!({
            "type": "control_response",
            "response": { "subtype": "success", "request_id": request_id, "response": body },
        }));
    }

    pub fn terminate(&self) {
        unsafe { libc::kill(self.pid as i32, libc::SIGTERM) };
        let _ = self.child.lock().map(|mut c| c.try_wait());
    }
}

/// A job's exit code, from its end's summary: `Background command "Build" failed with exit
/// code 127`, `… completed (exit code 0)`. A watch's (`Monitor "x" stream ended`) has none.
pub fn job_exit_code(summary: &str) -> Option<i32> {
    let rest = &summary[summary.find("exit code ")? + "exit code ".len()..];
    let end = rest.char_indices().find(|(i, c)| !(c.is_ascii_digit() || (*i == 0 && *c == '-'))).map_or(rest.len(), |(i, _)| i);
    rest[..end].parse().ok()
}

/// `<task-notification><task-id>b1</task-id>…<event>sample 3: 2.64</event></task-notification>`:
/// what a watch saw, as the CLI wakes Claude with it. (task id, text).
fn job_event(text: &str) -> Option<(String, String)> {
    let text = text.strip_prefix("<task-notification>")?;
    let id = &text[text.find("<task-id>")? + "<task-id>".len()..];
    let id = &id[..id.find("</task-id>")?];
    let start = text.find("<event>")? + "<event>".len();
    let end = text.rfind("</event>")?;
    (start <= end).then(|| (id.to_string(), text[start..end].to_string()))
}

/// stream-json lines into events. Unknown types and fields are ignored: the protocol changes.
#[derive(Default)]
struct Parser {
    /// The message being streamed, per sub-agent (`None`: the main conversation).
    current: HashMap<Option<String>, String>,
    /// Tasks that are jobs rather than sub-agents: their end says only which task it was.
    jobs: std::collections::HashSet<String>,
}

impl Parser {
    fn parse(&mut self, object: &Value) -> Vec<Event> {
        let parent = object["parent_tool_use_id"].as_str().map(str::to_string);
        match object["type"].as_str().unwrap_or("") {
            "system" => match object["subtype"].as_str().unwrap_or("") {
                "init" => object["session_id"].as_str().map(|id| vec![Event::Session { id: id.into() }]).unwrap_or_default(),
                // A command or a watch (`local_bash`, `monitor`…) rather than a sub-agent (`local_agent`).
                "task_started" if object["task_type"].as_str().is_some_and(|t| t != "local_agent") && object["task_id"].is_string() => {
                    let task_id = object["task_id"].as_str().unwrap_or("").to_string();
                    self.jobs.insert(task_id.clone());
                    vec![Event::JobStarted {
                        task_id,
                        tool_id: object["tool_use_id"].as_str().map(str::to_string),
                        description: object["description"].as_str().unwrap_or("").into(),
                        background: object["is_backgrounded"] == true,
                    }]
                }
                "task_started" => vec![Event::SubagentStarted {
                    tool_id: object["tool_use_id"].as_str().unwrap_or("").into(),
                    description: object["description"].as_str().unwrap_or("Sub-agent").into(),
                }],
                "task_notification" if object["task_id"].as_str().is_some_and(|id| self.jobs.remove(id)) => vec![Event::JobFinished {
                    task_id: object["task_id"].as_str().unwrap_or("").into(),
                    status: object["status"].as_str().unwrap_or("completed").into(),
                    summary: object["summary"].as_str().map(str::to_string),
                    output_file: object["output_file"].as_str().filter(|f| !f.is_empty()).map(str::to_string),
                }],
                "background_tasks_changed" => match object["tasks"].as_array() {
                    Some(tasks) => vec![Event::JobsRunning(
                        tasks.iter().filter(|t| t["task_type"] != "local_agent").filter_map(|t| t["task_id"].as_str().map(str::to_string)).collect(),
                    )],
                    None => vec![],
                },
                "task_notification" => vec![Event::SubagentFinished {
                    tool_id: object["tool_use_id"].as_str().unwrap_or("").into(),
                    status: object["status"].as_str().unwrap_or("completed").into(),
                    summary: object["summary"].as_str().map(str::to_string),
                }],
                "compact_boundary" => {
                    let trigger = object["compact_metadata"]["trigger"].as_str().unwrap_or("auto");
                    vec![Event::Notice(format!("Conversation compacted ({trigger})"))]
                }
                _ => vec![],
            },
            "stream_event" => {
                let event = &object["event"];
                match event["type"].as_str().unwrap_or("") {
                    "message_start" => {
                        if let Some(id) = event["message"]["id"].as_str() {
                            self.current.insert(parent, id.to_string());
                        }
                        vec![]
                    }
                    "content_block_start" => {
                        let message = self.message(object, &parent);
                        let index = event["index"].as_u64().unwrap_or(0);
                        let block = &event["content_block"];
                        let kind = match block["type"].as_str().unwrap_or("") {
                            "text" => BlockKind::Text,
                            "thinking" => BlockKind::Thinking,
                            "tool_use" => BlockKind::Tool {
                                id: block["id"].as_str().unwrap_or("").into(),
                                name: block["name"].as_str().unwrap_or("Tool").into(),
                            },
                            _ => return vec![],
                        };
                        vec![Event::BlockStart { message, index, parent, kind }]
                    }
                    "content_block_delta" => {
                        let message = self.message(object, &parent);
                        let index = event["index"].as_u64().unwrap_or(0);
                        let delta = &event["delta"];
                        let (kind, text) = match delta["type"].as_str().unwrap_or("") {
                            "text_delta" => (DeltaKind::Text, delta["text"].as_str()),
                            "thinking_delta" => (DeltaKind::Thinking, delta["thinking"].as_str()),
                            "input_json_delta" => (DeltaKind::Input, delta["partial_json"].as_str()),
                            _ => return vec![],
                        };
                        vec![Event::Delta { message, index, parent, kind, text: text.unwrap_or("").into() }]
                    }
                    _ => vec![],
                }
            }
            "assistant" => {
                let message = &object["message"];
                let blocks = message["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|b| match b["type"].as_str()? {
                        "text" => Some(Block::Text(b["text"].as_str().unwrap_or("").into())),
                        "thinking" => Some(Block::Thinking(b["thinking"].as_str().unwrap_or("").into())),
                        "tool_use" => Some(Block::Tool {
                            id: b["id"].as_str().unwrap_or("").into(),
                            name: b["name"].as_str().unwrap_or("Tool").into(),
                            input: head(&b["input"].to_string(), TOOL_TEXT_LIMIT),
                        }),
                        _ => None,
                    })
                    .collect();
                vec![Event::Assistant { message: message["id"].as_str().unwrap_or("").into(), parent, blocks }]
            }
            "user" => {
                // What wakes Claude for a job, replayed: never a message of yours.
                if let Some(text) = object["message"]["content"].as_str() {
                    return job_event(text).map(|(task_id, text)| vec![Event::JobOutput { task_id, text }]).unwrap_or_default();
                }
                let results: Vec<ToolResult> = object["message"]["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|b| b["type"] == "tool_result")
                    .map(|b| {
                        let text = tool_result_text(&b["content"]);
                        ToolResult {
                            id: b["tool_use_id"].as_str().unwrap_or("").into(),
                            len: text.len(),
                            full: (text.len() > LIVE_RESULT_LIMIT).then(|| text.clone()),
                            text: head(&text, LIVE_RESULT_LIMIT),
                            is_error: b["is_error"] == true,
                        }
                    })
                    .collect();
                if results.is_empty() {
                    vec![]
                } else {
                    vec![Event::ToolResults { parent, results }]
                }
            }
            "control_request" => {
                let request = &object["request"];
                if request["subtype"] == "elicitation" {
                    return vec![Event::Elicitation {
                        request_id: object["request_id"].as_str().unwrap_or("").into(),
                        server: request["mcp_server_name"].as_str().unwrap_or("").into(),
                        message: request["message"].as_str().unwrap_or("").into(),
                        mode: request["mode"].as_str().unwrap_or("form").into(),
                        schema: request["requested_schema"].clone(),
                        url: request["url"].as_str().map(str::to_string),
                    }];
                }
                if request["subtype"] != "can_use_tool" {
                    return vec![];
                }
                vec![Event::Permission {
                    request_id: object["request_id"].as_str().unwrap_or("").into(),
                    tool: request["tool_name"].as_str().unwrap_or("Tool").into(),
                    description: request["description"].as_str().unwrap_or("").into(),
                    input: head(&request["input"].to_string(), TOOL_TEXT_LIMIT),
                }]
            }
            "command_lifecycle" => match (object["command_uuid"].as_str(), object["state"].as_str()) {
                (Some(id), Some(state)) => vec![Event::Lifecycle { id: id.into(), state: state.into() }],
                _ => vec![],
            },
            "control_response" => {
                let payload = &object["response"]["response"];
                if payload["models"].is_array() {
                    vec![Event::Catalog { models: payload["models"].clone(), commands: payload["commands"].clone() }]
                } else {
                    vec![]
                }
            }
            "rate_limit_event" => vec![Event::Limits(object["rate_limit_info"].clone())],
            "result" => vec![Event::Done {
                is_error: object["is_error"] == true,
                text: object["result"].as_str().or(object["subtype"].as_str()).unwrap_or("").into(),
            }],
            _ => vec![],
        }
    }

    fn message(&self, object: &Value, parent: &Option<String>) -> String {
        object["api_message_id"]
            .as_str()
            .map(str::to_string)
            .or_else(|| self.current.get(parent).cloned())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(fixture: &str) -> Vec<Event> {
        let mut parser = Parser::default();
        fixture.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).flat_map(|o| parser.parse(&o)).collect()
    }

    /// A real capture (sanitized), on Haiku: a 20-second foreground command, then two in the
    /// background, one failing with exit code 3, Claude woken by each end in a turn of its own.
    const JOBS: &str = include_str!("../fixtures/background-jobs.jsonl");
    /// The Swift app's capture: a background command, a Monitor watching it.
    const MONITOR: &str = include_str!("../fixtures/background-job-monitor.jsonl");

    #[test]
    fn jobs_start_and_end_by_task() {
        let events = events(JOBS);
        let starts: Vec<(&str, Option<&str>, bool)> = events
            .iter()
            .filter_map(|e| match e {
                Event::JobStarted { task_id, tool_id, background, .. } => Some((task_id.as_str(), tool_id.as_deref(), *background)),
                _ => None,
            })
            .collect();
        assert_eq!(
            starts,
            [
                ("bgk54klt5", Some("toolu_01EeWLHRgvJ8xirFi9r6KjGf"), false),
                ("bjr80s33k", Some("toolu_011qVMdMTJUenacvBFC3kZT9"), true),
                ("bac339jo3", Some("toolu_0173CFzuEXafWCtcudftBpkD"), true),
            ]
        );
        let ends: Vec<(&str, &str, Option<i32>, bool)> = events
            .iter()
            .filter_map(|e| match e {
                Event::JobFinished { task_id, status, summary, output_file } => {
                    Some((task_id.as_str(), status.as_str(), summary.as_deref().and_then(job_exit_code), output_file.is_some()))
                }
                _ => None,
            })
            .collect();
        // The foreground one's end has an empty output file: none.
        assert_eq!(ends, [("bgk54klt5", "completed", None, false), ("bac339jo3", "failed", Some(3), true), ("bjr80s33k", "completed", Some(0), true)]);
        // None is taken for a sub-agent.
        assert!(!events.iter().any(|e| matches!(e, Event::SubagentStarted { .. } | Event::SubagentFinished { .. })));
        // Woken by each end: three turns, the first one yours.
        assert_eq!(events.iter().filter(|e| matches!(e, Event::Done { .. })).count(), 3);
    }

    #[test]
    fn the_running_list_follows_every_change() {
        let lists: Vec<Vec<String>> = events(JOBS).into_iter().filter_map(|e| if let Event::JobsRunning(ids) = e { Some(ids) } else { None }).collect();
        assert_eq!(lists, [vec!["bjr80s33k".to_string()], vec!["bjr80s33k".into(), "bac339jo3".into()], vec!["bjr80s33k".into()], vec![]]);
    }

    #[test]
    fn what_a_watch_saw_is_an_event_not_a_message() {
        let events = events(MONITOR);
        let seen: Vec<String> = events.iter().filter_map(|e| if let Event::JobOutput { task_id, text } = e { Some(format!("{task_id}: {text}")) } else { None }).collect();
        assert_eq!(seen.first().map(String::as_str), Some("bfyo780nc: sample 1: 2.58\nsample 2: 2.69"));
        assert_eq!(seen.last().map(String::as_str), Some("bfyo780nc: sample 8: 3.05"));
        let ends: Vec<(String, String)> = events
            .iter()
            .filter_map(|e| if let Event::JobFinished { task_id, status, .. } = e { Some((task_id.clone(), status.clone())) } else { None })
            .collect();
        assert_eq!(ends, [("b7xkqnlhn".to_string(), "failed".to_string()), ("bfyo780nc".into(), "completed".into())]);
    }

    #[test]
    fn exit_code_comes_from_the_summary() {
        assert_eq!(job_exit_code(r#"Background command "Build" failed with exit code 127"#), Some(127));
        assert_eq!(job_exit_code(r#"Background command "x" completed (exit code 0)"#), Some(0));
        assert_eq!(job_exit_code(r#"Background command "x" failed with exit code -1"#), Some(-1));
        assert_eq!(job_exit_code(r#"Monitor "x" stream ended"#), None);
        assert_eq!(job_exit_code("exit code "), None);
    }

    #[test]
    fn job_lines_with_missing_or_odd_fields_never_crash() {
        let lines = [
            r#"{"type":"system","subtype":"task_started"}"#,
            r#"{"type":"system","subtype":"task_started","task_type":"local_bash"}"#,
            r#"{"type":"system","subtype":"task_started","task_type":"local_bash","task_id":7}"#,
            r#"{"type":"system","subtype":"task_notification","task_id":"nope"}"#,
            r#"{"type":"system","subtype":"background_tasks_changed","tasks":"x"}"#,
            r#"{"type":"system","subtype":"background_tasks_changed","tasks":[1,{"task_id":2},{"task_id":"a","task_type":"local_agent"}]}"#,
            r#"{"type":"user","message":{"content":"<task-notification><task-id>b"}}"#,
            r#"{"type":"user","message":{"content":"<task-notification><event>x</event><task-id>b</task-id>"}}"#,
            r#"{"type":"system","subtype":"task_brand_new","task_id":"b"}"#,
        ];
        let events = events(&lines.join("\n"));
        assert!(events.iter().all(|e| !matches!(e, Event::JobStarted { .. } | Event::JobFinished { .. })));
        assert!(events.iter().any(|e| matches!(e, Event::JobsRunning(ids) if ids.is_empty())));
    }
}
