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
/// `name`, `description`, `argumentHint`, `aliases`, `isBuiltin`, `mcpServer`).
#[derive(Clone, Debug, PartialEq)]
pub struct SlashCommand {
    /// What Claude Code takes after the slash: an MCP prompt's is `mcp__server__prompt`.
    pub name: String,
    pub description: String,
    pub hint: String,
    pub aliases: Vec<String>,
    /// One of Claude Code's own (`/compact`), not a skill or a command of yours.
    pub builtin: bool,
    /// An MCP server's prompt: its server, as Claude Code names it.
    pub mcp_server: Option<String>,
}

/// How Claude Code puts a name into a tool's or command's: anything but letters, digits, `_`
/// and `-` becomes `_` ("claude.ai Linear" → "claude_ai_Linear").
pub fn normalized_name(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect()
}

/// "github:review (MCP)" → ("github", "review"): how the catalog lists an MCP prompt.
fn mcp_prompt(listed: &str) -> Option<(String, String)> {
    let bare = listed.strip_suffix(" (MCP)")?;
    let colon = bare.rfind(':')?;
    (colon > 0 && colon + 1 < bare.len()).then(|| (bare[..colon].to_string(), bare[colon + 1..].to_string()))
}

impl SlashCommand {
    pub fn from_json(v: &Value) -> Option<SlashCommand> {
        let listed = v["name"].as_str().filter(|n| !n.is_empty())?;
        // Saved by either app with its server; from the CLI as "server:prompt (MCP)", typed as
        // /mcp__server__prompt.
        let (name, mcp_server) = match (v["mcpServer"].as_str(), mcp_prompt(listed)) {
            (Some(server), _) => (listed.to_string(), Some(server.to_string())),
            (None, Some((server, prompt))) => (format!("mcp__{}__{}", normalized_name(&server), normalized_name(&prompt)), Some(server)),
            (None, None) => (listed.to_string(), None),
        };
        Some(SlashCommand {
            name,
            description: v["description"].as_str().unwrap_or("").to_string(),
            hint: v["argumentHint"].as_str().unwrap_or("").to_string(),
            aliases: v["aliases"].as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_default(),
            // The CLI says `builtin`, the Swift app saved `isBuiltin`.
            builtin: v["builtin"].as_bool().or(v["isBuiltin"].as_bool()).unwrap_or(false),
            mcp_server,
        })
    }

    pub fn new(name: &str, hint: &str, description: &str) -> SlashCommand {
        SlashCommand { name: name.into(), description: description.into(), hint: hint.into(), aliases: vec![], builtin: true, mcp_server: None }
    }

    /// What the menu shows: "server:prompt" for an MCP prompt, as Claude Code lists it.
    pub fn display_name(&self) -> String {
        let Some(server) = &self.mcp_server else { return self.name.clone() };
        match self.name.strip_prefix(&format!("mcp__{}__", normalized_name(server))) {
            Some(prompt) => format!("{server}:{prompt}"),
            None => self.name.clone(),
        }
    }

    /// As the Swift app saves it (`slashCommands`).
    pub fn saved(&self) -> Value {
        let mut v = json!({ "name": self.name, "description": self.description, "argumentHint": self.hint, "aliases": self.aliases, "isBuiltin": self.builtin });
        if let Some(server) = &self.mcp_server {
            v["mcpServer"] = server.clone().into();
        }
        v
    }

    /// Its description, and where it comes from for the menu's chip: the app's own commands,
    /// an MCP server's prompts, else "(project)", "(user)" or "(plugin …)" ending the
    /// description, as Claude Code writes it (taken off the description then).
    pub fn source(&self, app_command: bool) -> (String, Option<String>) {
        if app_command {
            return (self.description.clone(), Some("YourHarness".into()));
        }
        if self.mcp_server.is_some() {
            return (self.description.clone(), Some("MCP".into()));
        }
        let d = self.description.trim_end();
        if let (true, Some(open)) = (d.ends_with(')'), d.rfind('(')) {
            let source = &d[open + 1..d.len() - 1];
            if !source.is_empty() && source.chars().count() <= 40 && (!source.contains(' ') || source.starts_with("plugin")) {
                return (d[..open].trim().to_string(), Some(source.to_string()));
            }
        }
        (self.description.clone(), None)
    }
}

/// Typed as the menu shows an MCP prompt ("/github:review fix"): what Claude Code takes
/// ("/mcp__github__review fix"). Anything else comes back as it was.
pub fn translate_mcp_prompt(text: &str, commands: &[SlashCommand]) -> String {
    let Some((name, args)) = parse_command(text) else { return text.to_string() };
    match commands.iter().find(|c| c.mcp_server.is_some() && c.display_name() == name) {
        Some(prompt) if args.is_empty() => format!("/{}", prompt.name),
        Some(prompt) => format!("/{} {args}", prompt.name),
        None => text.to_string(),
    }
}

/// "/name the rest" → ("name", "the rest"); None when the text isn't a command.
pub fn parse_command(text: &str) -> Option<(String, String)> {
    let body = text.trim().strip_prefix('/')?;
    let name: String = body.chars().take_while(|c| !c.is_whitespace()).collect();
    if name.is_empty() || name.contains('/') {
        return None;
    }
    Some((name.clone(), body[name.len()..].trim().to_string()))
}

/// The `/name` being typed at the end of `text`: at its start or after a space, with nothing
/// after it yet. (where its slash is, what follows it)
pub fn trailing_command(text: &str) -> Option<(usize, String)> {
    let slash = text.rfind('/')?;
    if slash > 0 && !text[..slash].ends_with(char::is_whitespace) {
        return None;
    }
    let query = &text[slash + 1..];
    if query.contains(|c: char| c.is_whitespace() || c == '/') {
        return None;
    }
    Some((slash, query.to_string()))
}

/// `/name` words after the start of a message that name one of `known`, in order, once each:
/// skills to run along the way. The command a message starts with is Claude Code's to run.
pub fn chained_commands(text: &str, known: &std::collections::HashSet<String>) -> Vec<String> {
    let mut found: Vec<String> = vec![];
    for (slash, _) in text.match_indices('/') {
        // A word of its own, not the start of the message, not part of a path or `code`.
        if slash == 0 || !text[..slash].ends_with(char::is_whitespace) {
            continue;
        }
        let rest = &text[slash + 1..];
        let name: String = rest.chars().take_while(|c| !c.is_whitespace() && !",.;!?)\"'`".contains(*c)).collect();
        if name.is_empty() || name.contains('/') || rest[name.len()..].starts_with('/') {
            continue;
        }
        if known.contains(&name) && !found.contains(&name) {
            found.push(name);
        }
    }
    found
}

/// What's added to a message that chains skills, so the agent runs each where it comes up.
pub fn chain_note(names: &[String]) -> Option<String> {
    let list: Vec<String> = names.iter().map(|n| format!("/{n}")).collect();
    match list.len() {
        0 => None,
        1 => Some(format!("(Run the {} skill where it comes up above, with the Skill tool.)", list[0])),
        n => Some(format!(
            "(Run the {} and {} skills where they come up above, in that order, with the Skill tool.)",
            list[..n - 1].join(", "),
            list[n - 1]
        )),
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
    /// Whether `--permission-mode auto` works with it (the CLI quietly asks instead when it
    /// doesn't). The CLI leaves the key out for models without it.
    pub supports_auto_mode: bool,
}

impl ModelInfo {
    /// Aliases that follow the latest model (`opus`), as against pinned versions (`claude-…`).
    pub fn is_alias(&self) -> bool {
        !self.value.starts_with("claude-")
    }

    pub fn from_json(v: &Value) -> Option<ModelInfo> {
        Some(ModelInfo {
            value: v["value"].as_str()?.to_string(),
            display_name: v["displayName"].as_str()?.to_string(),
            description: v["description"].as_str().map(str::to_string),
            efforts: v["supportedEffortLevels"]
                .as_array()
                .map(|a| a.iter().filter_map(|e| e.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
            supports_auto_mode: v["supportsAutoMode"].as_bool().unwrap_or(false),
        })
    }

    /// The CLI's model list as the Swift app saves it (`claudeModels`, `[ClaudeModel]`), so
    /// 0.0.16 reads it after a downgrade: `supportedEffortLevels` is required there, and a
    /// model without it (Haiku) cost the whole list.
    pub fn saved_list(models: &Value) -> Value {
        let list = models.as_array().map(Vec::as_slice).unwrap_or_default();
        Value::Array(
            list.iter()
                .filter_map(|m| {
                    let info = ModelInfo::from_json(m)?;
                    let mut o = serde_json::Map::new();
                    o.insert("value".into(), info.value.into());
                    if let Some(r) = m["resolvedModel"].as_str() {
                        o.insert("resolvedModel".into(), r.into());
                    }
                    o.insert("displayName".into(), info.display_name.into());
                    if let Some(d) = info.description {
                        o.insert("description".into(), d.into());
                    }
                    o.insert("supportedEffortLevels".into(), info.efforts.into());
                    o.insert("supportsAutoMode".into(), info.supports_auto_mode.into());
                    Some(Value::Object(o))
                })
                .collect(),
        )
    }
}

/// What a sub-agent has used: its `usage` in `task_progress` and `task_notification`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SubagentUsage {
    pub tokens: Option<u64>,
    pub tool_uses: Option<u64>,
    pub duration_ms: Option<u64>,
}

impl SubagentUsage {
    fn from_json(v: &Value) -> SubagentUsage {
        SubagentUsage { tokens: v["total_tokens"].as_u64(), tool_uses: v["tool_uses"].as_u64(), duration_ms: v["duration_ms"].as_u64() }
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
    /// `task_id` is what `stop_task` takes to stop it; `background`: started with
    /// `run_in_background`, it outlives the turn that started it.
    SubagentStarted { tool_id: String, description: String, task_id: Option<String>, background: bool },
    /// What a sub-agent is doing now ("Reading a.txt") and what it has used so far.
    SubagentProgress { tool_id: String, activity: Option<String>, usage: SubagentUsage },
    /// A sub-agent's end, with its summary (a background one's report comes only here).
    SubagentFinished { tool_id: String, status: String, summary: Option<String>, usage: SubagentUsage },
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
    /// Every background sub-agent still running (their task IDs), sent with `JobsRunning`.
    AgentsRunning(Vec<String>),
    Permission { request_id: String, tool: String, description: String, input: String },
    /// An MCP server asks something mid-call: a form (`schema`) or a page to visit (`url`).
    Elicitation { request_id: String, server: String, message: String, mode: String, schema: Value, url: Option<String>, elicitation_id: Option<String> },
    /// A page-mode question answered on the page: the server has its answer.
    ElicitationComplete { server: String, elicitation_id: String },
    /// What the CLI is doing between messages: `requesting`, `compacting`.
    Status(String),
    Notice(String),
    /// What a built-in command printed (`/context`, `/usage`): markdown, not a reply.
    /// `id` is the message's `uuid`: after a `/compact` the CLI sends kept ones again.
    CommandOutput { text: String, id: Option<String> },
    /// Tokens in the context after Claude's latest message (everything it saw plus what it wrote).
    ContextUsage(u64),
    /// Each model's context window, from a turn's result.
    ContextWindows(Vec<(String, u64)>),
    /// `/clear`: the CLI starts a new session with empty context (its init brings the new ID).
    ConversationReset,
    /// The session's title changed (`/rename`).
    TitleChanged(String),
    /// What the agent can run itself with its Skill tool, from the session's start.
    Skills(Vec<String>),
    /// MCP servers as the session's start found them: (name, status, scope).
    McpServers(Vec<(String, String, String)>),
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

/// What a thread says when there's no `claude` to run (the Swift app's `claudeNotFound`).
pub const CLAUDE_NOT_FOUND: &str = "Couldn't find a working `claude` CLI. Check Settings → Agents.";

/// The chosen `claude`, else the one on `YH_PATH` (as the app's test runs pass it), `PATH` and the usual places.
pub fn find_claude() -> Option<std::path::PathBuf> {
    CHOSEN.lock().unwrap().clone().filter(|p| p.is_file()).or_else(|| crate::agents::which("claude"))
}

impl ClaudeProcess {
    pub fn spawn(options: LaunchOptions, events: flume::Sender<Event>) -> Result<ClaudeProcess, String> {
        let executable = find_claude().ok_or(CLAUDE_NOT_FOUND)?;
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
        // The login shell's environment (`YH_PATH`'s in test runs), never an outer Claude Code
        // session's variables.
        crate::shell_env::apply(&mut command);
        for (k, v) in &options.env {
            command.env(k, v);
        }
        let mut child = command.spawn().map_err(|e| format!("{CLAUDE_NOT_FOUND} ({e})"))?;
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

    /// Takes back a message still waiting for the current step: its lifecycle ends `cancelled`.
    pub fn cancel_queued(&mut self, id: &str) {
        self.control(json!({ "subtype": "cancel_async_message", "message_uuid": id }));
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
                "init" => {
                    let Some(id) = object["session_id"].as_str() else { return vec![] };
                    let mut events = vec![Event::Session { id: id.into() }];
                    if let Some(skills) = object["skills"].as_array() {
                        events.push(Event::Skills(skills.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()));
                    }
                    if let Some(servers) = object["mcp_servers"].as_array() {
                        let s = |v: &Value, k: &str| v[k].as_str().unwrap_or("").to_string();
                        events.push(Event::McpServers(
                            servers.iter().map(|v| (s(v, "name"), s(v, "status"), v["scope"].as_str().or(v["source"].as_str()).unwrap_or("").to_string())).collect(),
                        ));
                    }
                    events
                }
                "session_title_changed" => object["title"].as_str().map(|t| vec![Event::TitleChanged(t.into())]).unwrap_or_default(),
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
                    task_id: object["task_id"].as_str().map(str::to_string),
                    background: object["is_backgrounded"] == true,
                }],
                "task_notification" if object["task_id"].as_str().is_some_and(|id| self.jobs.remove(id)) => vec![Event::JobFinished {
                    task_id: object["task_id"].as_str().unwrap_or("").into(),
                    status: object["status"].as_str().unwrap_or("completed").into(),
                    summary: object["summary"].as_str().map(str::to_string),
                    output_file: object["output_file"].as_str().filter(|f| !f.is_empty()).map(str::to_string),
                }],
                "background_tasks_changed" => match object["tasks"].as_array() {
                    Some(tasks) => {
                        let ids = |agents: bool| {
                            tasks.iter().filter(|t| (t["task_type"] == "local_agent") == agents).filter_map(|t| t["task_id"].as_str().map(str::to_string)).collect()
                        };
                        vec![Event::JobsRunning(ids(false)), Event::AgentsRunning(ids(true))]
                    }
                    None => vec![],
                },
                "task_notification" => vec![Event::SubagentFinished {
                    tool_id: object["tool_use_id"].as_str().unwrap_or("").into(),
                    status: object["status"].as_str().unwrap_or("completed").into(),
                    summary: object["summary"].as_str().map(str::to_string),
                    usage: SubagentUsage::from_json(&object["usage"]),
                }],
                "task_progress" => match object["tool_use_id"].as_str() {
                    Some(id) => vec![Event::SubagentProgress {
                        tool_id: id.into(),
                        activity: object["description"].as_str().filter(|d| !d.is_empty()).map(str::to_string),
                        usage: SubagentUsage::from_json(&object["usage"]),
                    }],
                    None => vec![],
                },
                "status" => object["status"].as_str().map(|s| vec![Event::Status(s.into())]).unwrap_or_default(),
                "elicitation_complete" => match (object["mcp_server_name"].as_str(), object["elicitation_id"].as_str()) {
                    (Some(server), Some(id)) => vec![Event::ElicitationComplete { server: server.into(), elicitation_id: id.into() }],
                    _ => vec![],
                },
                "compact_boundary" => {
                    let meta = &object["compact_metadata"];
                    let mut notice =
                        if meta["trigger"] == "manual" { "Conversation compacted".to_string() } else { "Conversation compacted automatically".to_string() };
                    if let (Some(before), Some(after)) = (meta["pre_tokens"].as_u64(), meta["post_tokens"].as_u64()) {
                        notice += &format!(" · {} → {} tokens", compact_tokens(before), compact_tokens(after));
                    }
                    let mut events = vec![Event::Notice(notice)];
                    if let Some(after) = meta["post_tokens"].as_u64() {
                        events.push(Event::ContextUsage(after));
                    }
                    events
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
                    // A message's final usage: everything the model saw plus what it wrote.
                    "message_delta" if parent.is_none() => {
                        let usage = &event["usage"];
                        let tokens: u64 = ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens", "output_tokens"]
                            .iter()
                            .filter_map(|k| usage[*k].as_u64())
                            .sum();
                        if tokens > 0 {
                            vec![Event::ContextUsage(tokens)]
                        } else {
                            vec![]
                        }
                    }
                    _ => vec![],
                }
            }
            "assistant" => {
                let message = &object["message"];
                if !object["local_command_source"].is_null() {
                    let text: Vec<&str> = message["content"].as_array().into_iter().flatten().filter_map(|b| b["text"].as_str()).collect();
                    let text = strip_command_tags(&text.join("\n"));
                    return if text.is_empty() { vec![] } else { vec![Event::CommandOutput { text, id: object["uuid"].as_str().map(str::to_string) }] };
                }
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
                        elicitation_id: request["elicitation_id"].as_str().map(str::to_string),
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
            "conversation_reset" => vec![Event::ConversationReset],
            "result" => {
                let mut events = vec![];
                let windows: Vec<(String, u64)> = object["modelUsage"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter_map(|(model, u)| Some((model.clone(), u["contextWindow"].as_u64()?)))
                    .collect();
                if !windows.is_empty() {
                    events.push(Event::ContextWindows(windows));
                }
                events.push(Event::Done {
                    is_error: object["is_error"] == true,
                    text: object["result"].as_str().or(object["subtype"].as_str()).unwrap_or("").into(),
                });
                events
            }
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

/// What a built-in command printed, without the tags Claude Code wraps it in.
fn strip_command_tags(text: &str) -> String {
    let mut out = text.to_string();
    for tag in ["local-command-stdout", "local-command-stderr", "local-command-caveat"] {
        out = out.replace(&format!("<{tag}>"), "").replace(&format!("</{tag}>"), "");
    }
    out.trim().to_string()
}

/// 26823 → "27k", 1000000 → "1M", as the context ring shows them.
pub fn compact_tokens(value: u64) -> String {
    if value >= 1_000_000 {
        let m = value as f64 / 1_000_000.0;
        if m.fract() == 0.0 {
            format!("{}M", m as u64)
        } else {
            format!("{m:.1}M")
        }
    } else if value >= 1_000 {
        format!("{}k", (value as f64 / 1000.0).round() as u64)
    } else {
        value.to_string()
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

    /// The Swift app's capture, on Haiku: a sub-agent in the background, the turn over before
    /// it reports, Claude woken by its end.
    const BACKGROUND_AGENT: &str = include_str!("../fixtures/subagent-background.jsonl");

    #[test]
    fn a_background_sub_agent_is_known_from_start_to_end() {
        let events = events(BACKGROUND_AGENT);
        let starts: Vec<(&str, Option<&str>, bool)> = events
            .iter()
            .filter_map(|e| match e {
                Event::SubagentStarted { tool_id, task_id, background, .. } => Some((tool_id.as_str(), task_id.as_deref(), *background)),
                _ => None,
            })
            .collect();
        assert_eq!(starts, [("toolu_012pyhNsq5VtNVxpEfvbwiHz", Some("a36269b96faba4efe"), true)]);
        let lists: Vec<Vec<String>> = events.iter().filter_map(|e| if let Event::AgentsRunning(ids) = e { Some(ids.clone()) } else { None }).collect();
        assert_eq!(lists, [vec!["a36269b96faba4efe".to_string()], vec![]]);
        // Never a job.
        assert!(events.iter().all(|e| !matches!(e, Event::JobStarted { .. } | Event::JobFinished { .. })));
        assert!(events.iter().all(|e| !matches!(e, Event::JobsRunning(ids) if !ids.is_empty())));
        // The turn ends first; its end comes after.
        let done = events.iter().position(|e| matches!(e, Event::Done { .. })).unwrap();
        let end = events.iter().position(|e| matches!(e, Event::SubagentFinished { .. })).unwrap();
        assert!(done < end);
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

    fn parse(line: &str) -> Vec<Event> {
        Parser::default().parse(&serde_json::from_str(line).unwrap())
    }

    #[test]
    fn finds_the_command_being_typed_anywhere() {
        let q = |t: &str| trailing_command(t).map(|(_, q)| q);
        assert_eq!(q("/rev").as_deref(), Some("rev"));
        assert_eq!(q("fix the bug then /rev").as_deref(), Some("rev"));
        assert_eq!(q("then /").as_deref(), Some(""));
        assert_eq!(q("then /review "), None);
        assert_eq!(q("see src/main"), None);
        assert_eq!(q("open /usr/bin"), None);
        assert_eq!(trailing_command("a /re").map(|(s, _)| s), Some(2));
    }

    #[test]
    fn lists_skills_chained_in_a_message() {
        let known: std::collections::HashSet<String> = ["review", "commit", "deploy:staging", "plan"].iter().map(|s| s.to_string()).collect();
        assert_eq!(chained_commands("/plan the work, then /review and /commit.", &known), vec!["review", "commit"]);
        assert_eq!(chained_commands("do it then /deploy:staging", &known), vec!["deploy:staging"]);
        assert_eq!(chained_commands("run /review twice: /review", &known), vec!["review"]);
        assert!(chained_commands("look at /review/notes.md and src/commit", &known).is_empty());
        assert!(chained_commands("unknown /nope here", &known).is_empty());
        assert!(chained_commands("/review only", &known).is_empty());
        assert!(chained_commands("then (/review)", &known).is_empty());
    }

    #[test]
    fn notes_the_chain() {
        assert_eq!(chain_note(&[]), None);
        assert_eq!(chain_note(&["review".into()]).unwrap(), "(Run the /review skill where it comes up above, with the Skill tool.)");
        assert_eq!(
            chain_note(&["review".into(), "commit".into(), "deploy".into()]).unwrap(),
            "(Run the /review, /commit and /deploy skills where they come up above, in that order, with the Skill tool.)"
        );
    }

    #[test]
    fn mcp_prompts_are_typed_as_claude_code_takes_them() {
        let c = SlashCommand::from_json(&serde_json::json!({"name": "claude.ai Linear:triage (MCP)", "description": "Triage issues"})).unwrap();
        assert_eq!(c.name, "mcp__claude_ai_Linear__triage");
        assert_eq!(c.display_name(), "claude.ai Linear:triage");
        assert_eq!(SlashCommand::from_json(&c.saved()), Some(c.clone()));
        let review = SlashCommand::from_json(&serde_json::json!({"name": "github:review (MCP)"})).unwrap();
        let all = vec![c, review];
        assert_eq!(translate_mcp_prompt("/github:review the PR", &all), "/mcp__github__review the PR");
        assert_eq!(translate_mcp_prompt("/github:review", &all), "/mcp__github__review");
        assert_eq!(translate_mcp_prompt("/compact", &all), "/compact");
        assert_eq!(SlashCommand::from_json(&serde_json::json!({"name": "x (MCP)"})).unwrap().mcp_server, None);
    }

    #[test]
    fn says_where_a_command_comes_from() {
        let c = |d: &str| SlashCommand { description: d.into(), ..SlashCommand::new("x", "", "") };
        assert_eq!(c("Say hello (project)").source(false), ("Say hello".to_string(), Some("project".to_string())));
        assert_eq!(c("Deploy (plugin acme)").source(false).1.as_deref(), Some("plugin acme"));
        assert_eq!(c("Compact the conversation (keeps a summary)").source(false).1, None);
        assert_eq!(c("Ask aside").source(true).1.as_deref(), Some("YourHarness"));
    }

    #[test]
    fn parses_commands() {
        assert_eq!(parse_command("/rename  My title "), Some(("rename".into(), "My title".into())));
        assert_eq!(parse_command("/clear"), Some(("clear".into(), String::new())));
        assert_eq!(parse_command("hello /clear"), None);
        assert_eq!(parse_command("/usr/bin"), None);
    }

    #[test]
    fn reads_what_built_in_commands_say() {
        let out = parse(r#"{"type":"assistant","local_command_source":"x","message":{"id":"m","model":"<synthetic>","content":[{"type":"text","text":"<local-command-stdout>## Context Usage</local-command-stdout>"}]}}"#);
        assert!(matches!(&out[..], [Event::CommandOutput { text: t, .. }] if t == "## Context Usage"));
        // Its uuid comes along, so the copy sent again after `/compact` is known.
        let out = parse(r#"{"type":"assistant","uuid":"e8ec","local_command_source":"x","message":{"content":[{"type":"text","text":"Session renamed to: Zed"}]}}"#);
        assert!(matches!(&out[..], [Event::CommandOutput { id: Some(id), .. }] if id == "e8ec"));
        assert!(matches!(&parse(r#"{"type":"conversation_reset","trigger":"clear"}"#)[..], [Event::ConversationReset]));
        assert!(matches!(&parse(r#"{"type":"system","subtype":"session_title_changed","title":"T"}"#)[..], [Event::TitleChanged(t)] if t == "T"));
        let compacted = parse(r#"{"type":"system","subtype":"compact_boundary","compact_metadata":{"trigger":"manual","pre_tokens":26823,"post_tokens":8253}}"#);
        assert!(matches!(&compacted[..], [Event::Notice(n), Event::ContextUsage(8253)] if n == "Conversation compacted · 27k → 8k tokens"));
    }

    #[test]
    fn reads_how_full_the_context_is() {
        let delta = parse(r#"{"type":"stream_event","event":{"type":"message_delta","usage":{"input_tokens":2,"cache_creation_input_tokens":15164,"cache_read_input_tokens":11384,"output_tokens":7}}}"#);
        assert!(matches!(&delta[..], [Event::ContextUsage(26557)]));
        let sub = parse(r#"{"type":"stream_event","parent_tool_use_id":"t","event":{"type":"message_delta","usage":{"input_tokens":2}}}"#);
        assert!(sub.is_empty());
        let result = parse(r#"{"type":"result","is_error":false,"result":"ok","modelUsage":{"claude-haiku":{"contextWindow":1000000}}}"#);
        assert!(matches!(&result[..], [Event::ContextWindows(w), Event::Done { .. }] if w == &vec![("claude-haiku".to_string(), 1_000_000)]));
        assert_eq!(compact_tokens(1_000_000), "1M");
        assert_eq!(compact_tokens(200_000), "200k");
        assert_eq!(compact_tokens(512), "512");
    }

    /// The Swift app's capture: a foreground sub-agent reading two files.
    const SUBAGENT: &str = include_str!("../../../legacy/Tests/HarnessCoreTests/Fixtures/subagent-foreground.jsonl");

    #[test]
    fn reads_sub_agent_progress_and_status() {
        let events = events(SUBAGENT);
        let progress: Vec<(String, Option<String>, Option<u64>)> = events
            .iter()
            .filter_map(|e| match e {
                Event::SubagentProgress { tool_id, activity, usage } => Some((tool_id.clone(), activity.clone(), usage.tokens)),
                _ => None,
            })
            .collect();
        assert_eq!(progress.first(), Some(&("toolu_01VNvV6dcB9RqSfqqTfFRccv".to_string(), Some("Reading a.txt".to_string()), Some(15684))));
        assert!(events.iter().any(|e| matches!(e, Event::SubagentFinished { usage, .. } if usage.tokens.is_some())));
        assert!(matches!(&parse(r#"{"type":"system","subtype":"status","status":"compacting"}"#)[..], [Event::Status(s)] if s == "compacting"));
        assert!(parse(r#"{"type":"system","subtype":"status","status":null}"#).is_empty());
        assert!(parse(r#"{"type":"system","subtype":"task_progress","description":"x"}"#).is_empty());
    }

    #[test]
    fn reads_elicitations_and_their_end() {
        let asked = parse(r#"{"type":"control_request","request_id":"r1","request":{"subtype":"elicitation","mcp_server_name":"s","message":"Sign in","mode":"url","url":"https://x","elicitation_id":"e1"}}"#);
        assert!(matches!(&asked[..], [Event::Elicitation { elicitation_id: Some(id), mode, .. }] if id == "e1" && mode == "url"));
        let done = parse(r#"{"type":"system","subtype":"elicitation_complete","mcp_server_name":"s","elicitation_id":"e1"}"#);
        assert!(matches!(&done[..], [Event::ElicitationComplete { server, elicitation_id }] if server == "s" && elicitation_id == "e1"));
        assert!(parse(r#"{"type":"system","subtype":"elicitation_complete","mcp_server_name":"s"}"#).is_empty());
    }

    #[test]
    fn reads_the_session_start() {
        let init = parse(r#"{"type":"system","subtype":"init","session_id":"s","skills":["review"],"mcp_servers":[{"name":"linear","status":"needs-auth","source":"user"}]}"#);
        assert!(matches!(&init[..], [Event::Session { .. }, Event::Skills(s), Event::McpServers(m)] if s == &vec!["review".to_string()] && m[0].1 == "needs-auth"));
    }
}
