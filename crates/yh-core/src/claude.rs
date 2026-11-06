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

#[derive(Clone, Debug)]
pub enum Event {
    Session { id: String },
    BlockStart { message: String, index: u64, parent: Option<String>, kind: BlockKind },
    Delta { message: String, index: u64, parent: Option<String>, kind: DeltaKind, text: String },
    Assistant { message: String, parent: Option<String>, blocks: Vec<Block> },
    ToolResults { parent: Option<String>, results: Vec<ToolResult> },
    SubagentStarted { tool_id: String, description: String },
    SubagentFinished { tool_id: String, status: String },
    Permission { request_id: String, tool: String, description: String, input: String },
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
    pub mode: String,
}

pub struct ClaudeProcess {
    child: Arc<Mutex<Child>>,
    writer: flume::Sender<Value>,
    counter: u64,
    pub pid: u32,
}

/// `claude` on `YH_PATH` (as the app's test runs pass it), else `PATH` and the usual places.
fn find_claude() -> Option<std::path::PathBuf> {
    let path = std::env::var("YH_PATH").or_else(|_| std::env::var("PATH")).unwrap_or_default();
    let home = std::env::var("HOME").unwrap_or_default();
    let extra = [format!("{home}/.local/bin"), format!("{home}/.claude/local"), "/opt/homebrew/bin".into(), "/usr/local/bin".into()];
    path.split(':')
        .map(str::to_string)
        .chain(extra)
        .map(|d| std::path::Path::new(&d).join("claude"))
        .find(|p| p.is_file())
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
        args.extend(["--permission-mode".into(), options.mode]);
        if let Some(session) = options.session_id {
            args.extend(["--resume".into(), session]);
        }
        let mut command = Command::new(&executable);
        command.args(&args).current_dir(&options.cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        if let Ok(path) = std::env::var("YH_PATH") {
            command.env("PATH", path);
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
        {
            let child = child.clone();
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
        Ok(ClaudeProcess { child, writer, counter: 0, pid })
    }

    pub fn send(&self, text: &str) {
        let _ = self.writer.send(json!({
            "type": "user",
            "message": { "role": "user", "content": [{ "type": "text", "text": text }] },
        }));
    }

    fn control(&mut self, request: Value) {
        self.counter += 1;
        let _ = self.writer.send(json!({ "type": "control_request", "request_id": format!("yh-{}", self.counter), "request": request }));
    }

    pub fn interrupt(&mut self) {
        self.control(json!({ "subtype": "interrupt" }));
    }

    pub fn set_mode(&mut self, mode: &str) {
        self.control(json!({ "subtype": "set_permission_mode", "mode": mode }));
    }

    pub fn set_model(&mut self, model: &str) {
        self.control(json!({ "subtype": "set_model", "model": if model.is_empty() { "default" } else { model } }));
    }

    pub fn respond(&self, request_id: &str, input: &str, allow: bool) {
        let body = if allow {
            let input: Value = serde_json::from_str(input).unwrap_or(json!({}));
            json!({ "behavior": "allow", "updatedInput": input })
        } else {
            json!({ "behavior": "deny", "message": "The user denied this action." })
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

/// stream-json lines into events. Unknown types and fields are ignored: the protocol changes.
#[derive(Default)]
struct Parser {
    /// The message being streamed, per sub-agent (`None`: the main conversation).
    current: HashMap<Option<String>, String>,
}

impl Parser {
    fn parse(&mut self, object: &Value) -> Vec<Event> {
        let parent = object["parent_tool_use_id"].as_str().map(str::to_string);
        match object["type"].as_str().unwrap_or("") {
            "system" => match object["subtype"].as_str().unwrap_or("") {
                "init" => object["session_id"].as_str().map(|id| vec![Event::Session { id: id.into() }]).unwrap_or_default(),
                "task_started" => vec![Event::SubagentStarted {
                    tool_id: object["tool_use_id"].as_str().unwrap_or("").into(),
                    description: object["description"].as_str().unwrap_or("Sub-agent").into(),
                }],
                "task_notification" => vec![Event::SubagentFinished {
                    tool_id: object["tool_use_id"].as_str().unwrap_or("").into(),
                    status: object["status"].as_str().unwrap_or("completed").into(),
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
