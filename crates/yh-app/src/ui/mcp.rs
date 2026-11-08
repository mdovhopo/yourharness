//! `/mcp`: the thread's MCP servers as Claude Code sees them in its folder, without a
//! terminal. Grouped by where each applies, with what each needs: turn on or off, reconnect,
//! sign in (the page opens in your default browser, where your company's sign-in lives;
//! Claude Code's localhost callback finishes it), sign out. A server opens to its tools.
//! Add Server takes a URL, a command or JSON and where it applies, through `claude mcp
//! add-json`, so the terminal sees it too. Everything goes over the thread's `claude`
//! control channel (docs/mcp.md), asked again every 2 s while the sheet is open.

use crate::state::thread::ThreadState;
use crate::state::workspace::Workspace;
use crate::theme::{radius, space, text, Theme, MONO};
use crate::ui::page_icons as icons;
use crate::ui::page_kit::{self, Segment};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::{Disableable, IconName, Sizable};
use gpui_kit::{div, prelude::*, px, Context, Entity, FontWeight, SharedString, Task, Window};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Where Claude Desktop keeps its MCP servers; `YH_CLAUDE_DESKTOP_CONFIG=<file>` reads another
/// (tests: a scratch file, never the real one).
fn claude_desktop_config() -> std::path::PathBuf {
    match std::env::var("YH_CLAUDE_DESKTOP_CONFIG") {
        Ok(p) if !p.is_empty() => p.into(),
        _ => std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Library/Application Support/Claude/claude_desktop_config.json"),
    }
}

/// The servers Claude Desktop has, from its config file: none when it isn't installed.
pub fn claude_desktop_servers(file: &std::path::Path) -> BTreeMap<String, Value> {
    let Ok(text) = std::fs::read_to_string(file) else { return BTreeMap::new() };
    let Ok(v) = serde_json::from_str::<Value>(&text) else { return BTreeMap::new() };
    v["mcpServers"].as_object().map(|o| o.iter().map(|(k, c)| (k.clone(), c.clone())).collect()).unwrap_or_default()
}

/// A server's config in a line: its URL, or its command and arguments (never env or headers).
fn config_summary(config: &Value) -> String {
    config["url"].as_str().map(str::to_string).unwrap_or_else(|| {
        let args: Vec<&str> = config["args"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        format!("{} {}", config["command"].as_str().unwrap_or(""), args.join(" ")).trim().to_string()
    })
}

/// Sign-in pages opened for servers, by name, for Open Again and Copy Link (here and on the
/// card Claude puts up to sign in).
static SIGN_IN_PAGES: std::sync::Mutex<Option<HashMap<String, String>>> = std::sync::Mutex::new(None);

pub fn note_sign_in_page(server: &str, url: &str) {
    SIGN_IN_PAGES.lock().unwrap().get_or_insert_with(HashMap::new).insert(server.to_string(), url.to_string());
}

pub fn sign_in_page(server: &str) -> Option<String> {
    SIGN_IN_PAGES.lock().unwrap().as_ref().and_then(|m| m.get(server).cloned())
}

/// Opens a sign-in page in the default browser (`YH_MCP_BROWSER=log` prints it instead).
pub fn open_sign_in_page(url: &str, cx: &mut gpui_kit::App) {
    if std::env::var("YH_MCP_BROWSER").as_deref() == Ok("log") {
        eprintln!("MCP sign-in page: {url}");
    } else {
        cx.open_url(url);
    }
}

/// Add Server's ways: a URL, a command, or JSON.
#[derive(Clone, Copy, PartialEq)]
enum Way {
    Url,
    Command,
    Json,
}

/// A name and value typed in a row (headers, environment variables).
type Pair = (Entity<InputState>, Entity<InputState>);

#[derive(Clone, Debug)]
pub struct Server {
    pub name: String,
    /// `connected`, `failed`, `needs-auth`, `disabled`, `pending`…
    pub status: String,
    pub scope: String,
    pub transport: String,
    pub error: Option<String>,
    /// (name, read only, destructive)
    pub tools: Vec<(String, bool, bool)>,
    /// The command or URL it runs, without env values or headers (they can hold secrets).
    pub summary: String,
}

pub fn parse_status(v: &Value) -> Vec<Server> {
    v["mcpServers"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| {
            let config = &s["config"];
            let summary = config["url"].as_str().map(str::to_string).unwrap_or_else(|| {
                let args: Vec<&str> = config["args"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                format!("{} {}", config["command"].as_str().unwrap_or(""), args.join(" ")).trim().to_string()
            });
            Server {
                name: s["name"].as_str().unwrap_or("").into(),
                status: s["status"].as_str().unwrap_or("unknown").into(),
                scope: s["scope"].as_str().unwrap_or("").into(),
                transport: s["transport"].as_str().or(config["type"].as_str()).unwrap_or("stdio").into(),
                error: s["error"].as_str().map(str::to_string),
                tools: s["tools"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|t| (t["name"].as_str().unwrap_or("").to_string(), t["annotations"]["readOnly"].as_bool().unwrap_or(false), t["annotations"]["destructive"].as_bool().unwrap_or(false)))
                    .collect(),
                summary,
            }
        })
        .collect()
}

fn scope_label(scope: &str) -> &'static str {
    match scope {
        "project" => "This project, for everyone (.mcp.json)",
        "local" => "This project, just you",
        "user" => "Everywhere",
        "claudeai" => "claude.ai connectors",
        "dynamic" => "This session",
        "enterprise" | "managed" => "Your organization",
        _ => "Other",
    }
}

/// Where Add Server puts it, as `claude mcp add-json --scope` takes it.
const SCOPES: [(&str, &str); 3] = [("local", "This project, just you"), ("project", "This project, for everyone"), ("user", "Everywhere")];

pub struct McpSheet {
    workspace: Entity<Workspace>,
    pub open: bool,
    thread: Option<Entity<ThreadState>>,
    pub servers: Vec<Server>,
    loaded: bool,
    error: Option<String>,
    /// A server's page.
    page: Option<String>,
    adding: bool,
    add_source: Entity<InputState>,
    add_name: Entity<InputState>,
    add_scope: &'static str,
    busy: Option<String>,
    /// Settings → MCP: only what applies everywhere, asked in Home's folder; no on/off
    /// (Claude Code keeps that per folder), and Add adds everywhere.
    everywhere: bool,
    /// Drawn inside Settings' MCP tab rather than over the window (`new_embedded`).
    embedded: bool,
    /// Add Server: the way, JSON, the URL's advanced fields (headers, OAuth client ID, secret,
    /// callback port), a command's environment, and Claude Desktop's servers once read.
    way: Way,
    add_json: Entity<TextareaState>,
    advanced: bool,
    headers: Vec<Pair>,
    env: Vec<Pair>,
    client_id: Entity<InputState>,
    client_secret: Entity<InputState>,
    callback_port: Entity<InputState>,
    desktop: Option<BTreeMap<String, Value>>,
    desktop_picked: HashSet<String>,
    /// Remove Server… asked; the server page shows the question.
    confirm_remove: Option<String>,
    /// Signing in: the browser's page is out for these servers, until they connect.
    signing_in: HashSet<String>,
    /// "Browser didn't come back?": the address it ended on, pasted.
    callback: Entity<InputState>,
    callback_open: bool,
    _poll: Option<Task<()>>,
}

impl McpSheet {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        McpSheet {
            workspace,
            open: false,
            thread: None,
            servers: vec![],
            loaded: false,
            error: None,
            page: None,
            adding: false,
            add_source: cx.new(|cx| InputState::new(window, cx).placeholder("https://…/mcp")),
            add_name: cx.new(|cx| InputState::new(window, cx).placeholder("From the URL or command")),
            add_scope: "local",
            busy: None,
            everywhere: false,
            embedded: false,
            way: Way::Url,
            add_json: cx.new(|cx| {
                let mut s = TextareaState::new(window, cx).placeholder("{\"mcpServers\": {…}}, or one server's config");
                s.set_auto_grow(5, 14, cx);
                s
            }),
            advanced: false,
            headers: vec![],
            env: vec![],
            client_id: cx.new(|cx| InputState::new(window, cx).placeholder("OAuth client ID")),
            client_secret: cx.new(|cx| InputState::new(window, cx).placeholder("OAuth client secret")),
            callback_port: cx.new(|cx| InputState::new(window, cx).placeholder("Callback port")),
            desktop: None,
            desktop_picked: HashSet::new(),
            confirm_remove: None,
            signing_in: HashSet::new(),
            callback: cx.new(|cx| InputState::new(window, cx).placeholder("http://localhost:…/callback?code=…")),
            callback_open: false,
            _poll: None,
        }
    }

    /// Settings → MCP's list, drawn inside the tab: what applies everywhere, kept current
    /// while it's alive (it stops when it's dropped). Embed it with `.child(entity)`.
    #[allow(dead_code)] // Settings' own window embeds it (the app agent's), not yet on this branch.
    pub fn new_embedded(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut sheet = McpSheet::new(workspace, window, cx);
        sheet.embedded = true;
        sheet.show_everywhere(cx);
        sheet
    }

    fn pair(&self, name: &str, value: &str, window: &mut Window, cx: &mut Context<Self>) -> Pair {
        let (name, value) = (name.to_string(), value.to_string());
        (cx.new(|cx| InputState::new(window, cx).placeholder(name)), cx.new(|cx| InputState::new(window, cx).placeholder(value)))
    }

    /// Opens for the selected thread, and asks again every 2 s while open.
    pub fn show(&mut self, cx: &mut Context<Self>) {
        self.everywhere = false;
        self.add_scope = "local";
        self.thread = self.workspace.read(cx).selected_thread();
        self.open = true;
        self.page = None;
        self.adding = false;
        self.loaded = false;
        self.error = None;
        self.refresh(cx);
        self._poll = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
            if this.update(cx, |this, cx| if this.open { this.refresh(cx) }).is_err() {
                break;
            }
        }));
        cx.notify();
    }

    /// Settings → MCP: the servers that apply everywhere, through a session in Home's folder.
    pub fn show_everywhere(&mut self, cx: &mut Context<Self>) {
        let Some(thread) = self.workspace.update(cx, |ws, cx| ws.settings_thread(cx)) else { return };
        self.show(cx);
        self.everywhere = true;
        self.add_scope = "user";
        self.thread = Some(thread);
        self.loaded = false;
        self.refresh(cx);
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        self._poll = None;
        cx.notify();
    }

    fn request(&mut self, request: Value, cx: &mut Context<Self>) -> Option<Task<Result<Value, String>>> {
        let thread = self.thread.clone()?;
        Some(self.workspace.update(cx, |ws, cx| ws.mcp(&thread, request, cx)))
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(task) = self.request(json!({ "subtype": "mcp_status" }), cx) else { return };
        cx.spawn(async move |this, cx| {
            let reply = task.await;
            let _ = this.update(cx, |this, cx| {
                match reply {
                    Ok(v) => {
                        let first = !this.loaded;
                        this.servers = parse_status(&v);
                        // Signed in: the browser came back and the server connected.
                        let servers = &this.servers;
                        this.signing_in.retain(|name| servers.iter().find(|s| &s.name == name).is_none_or(|s| s.status != "connected"));
                        // The composer's chip for servers needing you follows what the sheet hears.
                        if let (Some(t), false) = (this.thread.clone(), this.everywhere) {
                            let servers = this.servers.iter().map(|s| (s.name.clone(), s.status.clone(), s.scope.clone())).collect();
                            this.workspace.update(cx, |ws, cx| ws.note_mcp_servers(&t, servers, cx));
                        }
                        this.loaded = true;
                        this.error = None;
                        if first && std::env::var("YH_OPEN_MCP").is_ok() {
                            eprintln!("MCP servers: {}", this.describe());
                            // `YH_MCP_SELECT=<server>`: that server's page, for a snapshot of it.
                            if let Ok(name) = std::env::var("YH_MCP_SELECT") {
                                this.page = Some(name);
                            }
                        }
                    }
                    Err(e) => this.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn describe(&self) -> String {
        if self.servers.is_empty() {
            return "none".into();
        }
        self.servers.iter().map(|s| format!("{} ({}, {}, {} tools)", s.name, s.status, s.scope, s.tools.len())).collect::<Vec<_>>().join(", ")
    }

    /// A request about one server, then the list again.
    fn act(&mut self, subtype: &str, server: &str, extra: Value, cx: &mut Context<Self>) {
        let mut request = json!({ "subtype": subtype, "serverName": server });
        if let (Some(r), Some(e)) = (request.as_object_mut(), extra.as_object()) {
            r.extend(e.clone());
        }
        let Some(task) = self.request(request, cx) else { return };
        self.busy = Some(format!("{server}…"));
        let sign_in = subtype == "mcp_authenticate";
        let name = server.to_string();
        cx.spawn(async move |this, cx| {
            let reply = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                match reply {
                    Ok(v) if sign_in => match v["authUrl"].as_str() {
                        // The default browser: your SSO session, passkeys and device checks live
                        // there. The sheet waits with Open Again, Copy Link and a pasted address.
                        Some(url) => {
                            note_sign_in_page(&name, url);
                            this.signing_in.insert(name.clone());
                            this.callback_open = false;
                            open_sign_in_page(url, cx);
                        }
                        None => this.error = Some("The server didn't give a page to sign in on.".into()),
                    },
                    Ok(_) => {}
                    Err(e) => this.error = Some(e),
                }
                this.refresh(cx);
            });
        })
        .detach();
    }

    /// Each filled-in row of pairs as an object (headers, environment variables).
    fn pairs_object(&self, pairs: &[Pair], cx: &gpui_kit::App) -> serde_json::Map<String, Value> {
        pairs
            .iter()
            .filter_map(|(k, v)| {
                let k = k.read(cx).value().trim().to_string();
                (!k.is_empty()).then(|| (k, Value::from(v.read(cx).value().to_string())))
            })
            .collect()
    }

    /// The servers Add Server would add, as (name, config), from the way picked and what's
    /// typed, or why not.
    fn typed_servers(&self, cx: &gpui_kit::App) -> Result<Vec<(String, Value)>, String> {
        let typed_name = self.add_name.read(cx).value().trim().to_string();
        let source = self.add_source.read(cx).value().trim().to_string();
        match self.way {
            Way::Json => {
                let text = self.add_json.read(cx).value().trim().to_string();
                let v = serde_json::from_str::<Value>(&text).map_err(|_| "That JSON doesn't parse.".to_string())?;
                // One config, {"name": {…}}, or a whole mcpServers block.
                let block = v.get("mcpServers").cloned().unwrap_or(v.clone());
                if block.get("command").is_some() || block.get("url").is_some() {
                    Ok(vec![(if typed_name.is_empty() { "server".into() } else { typed_name }, block)])
                } else {
                    let all: Vec<(String, Value)> = block.as_object().map(|o| o.iter().map(|(k, c)| (k.clone(), c.clone())).collect()).unwrap_or_default();
                    if all.is_empty() { Err("That JSON has no servers in it.".into()) } else { Ok(all) }
                }
            }
            Way::Url => {
                if !(source.starts_with("http://") || source.starts_with("https://")) {
                    return Err("Type the server's address, starting with https://.".into());
                }
                let mut config = json!({ "type": "http", "url": source });
                let headers = self.pairs_object(&self.headers, cx);
                if !headers.is_empty() {
                    config["headers"] = Value::Object(headers);
                }
                let mut oauth = serde_json::Map::new();
                let id = self.client_id.read(cx).value().trim().to_string();
                if !id.is_empty() {
                    oauth.insert("clientId".into(), id.into());
                }
                if let Ok(port) = self.callback_port.read(cx).value().trim().parse::<u64>() {
                    oauth.insert("callbackPort".into(), port.into());
                }
                if !oauth.is_empty() {
                    config["oauth"] = Value::Object(oauth);
                }
                Ok(vec![(if typed_name.is_empty() { suggested_name_for_url(&source) } else { typed_name }, config)])
            }
            Way::Command => {
                let words = yh_core::forge::shell_words(&source);
                let Some((command, args)) = words.split_first() else { return Err("Type the command that runs the server.".into()) };
                let mut config = json!({ "type": "stdio", "command": command, "args": args });
                let env = self.pairs_object(&self.env, cx);
                if !env.is_empty() {
                    config["env"] = Value::Object(env);
                }
                Ok(vec![(if typed_name.is_empty() { suggested_name_for_command(&source) } else { typed_name }, config)])
            }
        }
    }

    /// Adds what's typed (a URL, a command line, or JSON) where chosen, with Claude Code's own
    /// `claude mcp add-json`, then gets the running `claude` to see it.
    pub fn add(&mut self, cx: &mut Context<Self>) {
        match self.typed_servers(cx) {
            Ok(servers) => {
                let secret = (self.way == Way::Url).then(|| self.client_secret.read(cx).value().trim().to_string()).filter(|s| !s.is_empty());
                self.add_servers(servers, secret, cx);
            }
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }

    /// Import from Claude Desktop: reads its servers to pick from.
    fn load_desktop(&mut self, cx: &mut Context<Self>) {
        let file = claude_desktop_config();
        cx.spawn(async move |this, cx| {
            let servers = cx.background_executor().spawn(async move { claude_desktop_servers(&file) }).await;
            let _ = this.update(cx, |this, cx| {
                if std::env::var("YH_OPEN_MCP").is_ok() {
                    eprintln!("MCP Claude Desktop servers: {}", servers.keys().cloned().collect::<Vec<_>>().join(", "));
                }
                this.desktop_picked = servers.keys().cloned().collect();
                this.desktop = Some(servers);
                cx.notify();
            });
        })
        .detach();
    }

    fn import_desktop(&mut self, cx: &mut Context<Self>) {
        let picked: Vec<(String, Value)> = self.desktop.iter().flatten().filter(|(k, _)| self.desktop_picked.contains(*k)).map(|(k, v)| (k.clone(), v.clone())).collect();
        if !picked.is_empty() {
            self.add_servers(picked, None, cx);
        }
    }

    fn add_servers(&mut self, entries: Vec<(String, Value)>, client_secret: Option<String>, cx: &mut Context<Self>) {
        let Some(thread) = self.thread.clone() else { return };
        // A name Claude Code already has (any case) would take its place.
        if let Some((name, existing)) = entries.iter().find_map(|(n, _)| self.servers.iter().find(|s| s.name.eq_ignore_ascii_case(n)).map(|s| (n.clone(), s.clone()))) {
            self.error = Some(format!("There's already a server named {} ({}). Pick another name than {name}.", existing.name, scope_label(&existing.scope).to_lowercase()));
            cx.notify();
            return;
        }
        let folder = self.workspace.read(cx).folder_of(thread.read(cx));
        let scope = self.add_scope;
        self.busy = Some("Adding…".into());
        self.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let names: Vec<String> = entries.iter().map(|(n, _)| n.clone()).collect();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut failures = vec![];
                    let one = entries.len() == 1;
                    for (name, config) in entries {
                        let json = config.to_string();
                        let mut args = vec!["mcp", "add-json", &name, &json, "--scope", scope];
                        if client_secret.is_some() {
                            args.push("--client-secret");
                        }
                        if let Err(e) = run_claude(&folder, &args, client_secret.as_deref()) {
                            failures.push(if one { e } else { format!("{name}: {e}") });
                        }
                    }
                    if failures.is_empty() { Ok(()) } else { Err(failures.join("\n")) }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                match result {
                    Err(e) => this.error = Some(e),
                    Ok(()) => {
                        if std::env::var("YH_OPEN_MCP").is_ok() {
                            eprintln!("MCP added {} ({scope})", names.join(", "));
                        }
                        this.adding = false;
                        this.desktop = None;
                        // One server: its page, where Sign In is.
                        if names.len() == 1 {
                            this.page = names.first().cloned();
                        }
                        this.after_change(scope == "local", names, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The address the browser ended on, for when it couldn't reach Claude Code.
    fn finish_sign_in(&mut self, server: &str, cx: &mut Context<Self>) {
        let address = self.callback.read(cx).value().trim().to_string();
        if address.is_empty() {
            return;
        }
        self.act("mcp_oauth_callback_url", server, json!({ "callbackUrl": address }), cx);
    }

    pub fn remove(&mut self, server: &Server, cx: &mut Context<Self>) {
        let Some(thread) = self.thread.clone() else { return };
        let folder = self.workspace.read(cx).folder_of(thread.read(cx));
        let (name, scope) = (server.name.clone(), server.scope.clone());
        self.busy = Some(format!("Removing {name}…"));
        cx.spawn(async move |this, cx| {
            let n = name.clone();
            let result = cx.background_executor().spawn(async move { run_claude(&folder, &["mcp", "remove", &n, "--scope", &scope], None) }).await;
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                this.page = None;
                match result {
                    Err(e) => this.error = Some(e),
                    // A removed server stays connected until the session starts over.
                    Ok(()) => this.after_change(true, vec![], cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A running `claude` notices a user or project server about a second after it's added,
    /// and connects it when asked; a local one only when it starts again.
    fn after_change(&mut self, restart: bool, added: Vec<String>, cx: &mut Context<Self>) {
        let Some(thread) = self.thread.clone() else { return };
        if restart {
            self.workspace.update(cx, |ws, cx| ws.restart_runtime(&thread, cx));
            self.refresh(cx);
            return;
        }
        cx.spawn(async move |this, cx| {
            for _ in 0..3 {
                cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
                for name in &added {
                    let _ = this.update(cx, |this, cx| this.act("mcp_reconnect", name, json!({}), cx));
                }
            }
        })
        .detach();
    }
}

/// `claude <args>` in a folder, for `claude mcp add-json` and `remove`; an OAuth client
/// secret goes in through the environment, as Claude Code reads it.
fn run_claude(folder: &str, args: &[&str], client_secret: Option<&str>) -> Result<(), String> {
    let claude = yh_core::claude::find_claude().ok_or("Claude Code isn't installed.")?;
    let mut command = std::process::Command::new(claude);
    command.args(args).current_dir(folder);
    if let Ok(p) = std::env::var("YH_PATH") {
        command.env("PATH", p);
    }
    if let Some(secret) = client_secret {
        command.env("MCP_CLIENT_SECRET", secret);
    }
    let out = command.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(if out.stderr.is_empty() { &out.stdout } else { &out.stderr }).trim().to_string())
    }
}

/// A name from a URL's host: "mcp.linear.app" → "linear".
fn suggested_name_for_url(url: &str) -> String {
    let host = url.split('/').nth(2).unwrap_or("").split(':').next().unwrap_or("");
    let generic = ["mcp", "api", "www", "app", "server", "com", "io", "dev", "ai", "net", "org", "co"];
    let parts: Vec<&str> = host.split('.').filter(|p| !p.is_empty()).collect();
    parts.iter().find(|p| !generic.contains(&p.to_lowercase().as_str())).or(parts.first()).map(|s| s.to_string()).unwrap_or_else(|| "server".into())
}

/// A name from a command: `npx -y @modelcontextprotocol/server-github` → "github".
fn suggested_name_for_command(line: &str) -> String {
    let words: Vec<String> = yh_core::forge::shell_words(line).into_iter().filter(|w| !w.starts_with('-')).collect();
    let runners = ["npx", "uvx", "bunx", "pnpx", "docker", "node", "python", "python3", "deno"];
    let skip = usize::from(words.first().is_some_and(|w| runners.contains(&w.as_str())));
    let package = words.get(skip).cloned().unwrap_or_default();
    let mut name = package.rsplit('/').next().unwrap_or("").to_string();
    if let Some(at) = name.find('@').filter(|a| *a > 0) {
        name.truncate(at);
    }
    for prefix in ["mcp-server-", "server-", "mcp-"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            name = rest.to_string();
        }
    }
    let name = name.trim_end_matches(".py").trim_end_matches(".js").to_string();
    if name.is_empty() { "server".into() } else { name }
}

fn status_color(status: &str, theme: &Theme) -> gpui_kit::Hsla {
    match status {
        "connected" => theme.success,
        "failed" => theme.error,
        "needs-auth" => theme.warning,
        "disabled" => theme.tertiary,
        _ => theme.accent,
    }
}

fn dot(status: &str, theme: &Theme) -> gpui_kit::Div {
    page_kit::dot(status_color(status, theme), 8.)
}

fn status_text(status: &str) -> &str {
    match status {
        "connected" => "Connected",
        "failed" => "Failed",
        "needs-auth" => "Needs sign-in",
        "disabled" => "Off",
        "pending" => "Connecting…",
        other => other,
    }
}

fn tools_count(n: usize) -> String {
    if n == 1 { "1 tool".into() } else { format!("{n} tools") }
}

impl McpSheet {
    fn list(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        if !self.loaded {
            return div()
                .py(px(space::XL))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(space::S))
                .text_size(px(text::FOOTNOTE))
                .text_color(if self.error.is_some() { theme.error } else { theme.secondary })
                .child(self.error.clone().unwrap_or_else(|| "Asking Claude Code…".into()))
                .into_any_element();
        }
        let everywhere = self.everywhere;
        let shown: Vec<&Server> = self.servers.iter().filter(|s| !everywhere || ["user", "claudeai", "enterprise", "managed"].contains(&s.scope.as_str())).collect();
        if shown.is_empty() {
            return div()
                .h(px(220.))
                .child(page_kit::empty_state(icons::server(), "No MCP Servers", "Add one with its URL, the command that runs it, or its JSON. Claude Code in a terminal sees it too.", None, theme))
                .into_any_element();
        }
        let mut column = div().flex().flex_col().gap(px(space::S));
        let mut scopes: Vec<String> = vec![];
        for s in &shown {
            if !scopes.contains(&s.scope) {
                scopes.push(s.scope.clone());
            }
        }
        for scope in scopes {
            column = column.child(div().pt(px(space::XS)).child(page_kit::group_label(scope_label(&scope), theme)));
            let mut card = page_kit::card(theme).overflow_hidden().flex().flex_col();
            for (n, s) in shown.iter().copied().filter(|s| s.scope == scope).enumerate() {
                let (open, name) = (s.name.clone(), s.name.clone());
                let action: Option<(&str, &str, gpui_kit::component::Icon)> = match s.status.as_str() {
                    "needs-auth" => Some(("Sign In", "mcp_authenticate", icons::key_round())),
                    "failed" => Some(("Reconnect", "mcp_reconnect", icons::rotate_ccw())),
                    _ => None,
                };
                let on = s.status != "disabled";
                let (n1, n2) = (name.clone(), name.clone());
                card = card.child(
                    div()
                        .id(SharedString::from(format!("mcp-{name}")))
                        .flex()
                        .items_center()
                        .gap(px(space::S + 2.))
                        .pl(px(space::M))
                        .pr(px(space::S))
                        .py(px(space::S))
                        .when(n > 0, |d| d.border_t_1().border_color(theme.separator))
                        .cursor_pointer()
                        .hover(|d| d.bg(theme.hover))
                        .child(dot(&s.status, theme))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(1.))
                                .child(div().truncate().font_weight(FontWeight::MEDIUM).text_color(theme.text).child(s.name.clone()))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(5.))
                                        .text_size(px(text::CAPTION))
                                        .text_color(theme.secondary)
                                        .child(div().text_color(if s.status == "connected" || s.status == "disabled" || s.status == "pending" { theme.secondary } else { status_color(&s.status, theme) }).child(status_text(&s.status).to_string()))
                                        .child(div().text_color(theme.tertiary).child(format!("· {} · {}", s.transport, tools_count(s.tools.len())))),
                                )
                                .when_some(s.error.clone(), |d, e| d.child(div().text_size(px(text::CAPTION)).text_color(theme.error).line_clamp(2).child(e))),
                        )
                        .when_some(action, |d, (label, subtype, icon)| {
                            let subtype = subtype.to_string();
                            d.child(Button::new(SharedString::from(format!("mcp-act-{name}"))).small().outline().icon(icon).label(label).on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.act(&subtype, &n1, json!({}), cx)
                            })))
                        })
                        // The app's own servers (browser, memory, threads) aren't Claude Code's to turn off.
                        .when(!everywhere && s.scope != "claudeai" && s.scope != "dynamic", |d| {
                            d.child(Button::new(SharedString::from(format!("mcp-toggle-{name}"))).small().ghost().icon(icons::power()).tooltip(if on { "Turn Off" } else { "Turn On" }).on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.act("mcp_toggle", &n2, json!({ "enabled": !on }), cx)
                            })))
                        })
                        .child(gpui_kit::component::Icon::new(IconName::ChevronRight).size(px(14.)).text_color(theme.tertiary))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.page = Some(open.clone());
                            cx.notify();
                        })),
                );
            }
            column = column.child(card);
        }
        column.into_any_element()
    }

    fn server_page(&self, s: &Server, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let mut tools = page_kit::card(theme).overflow_hidden().flex().flex_col();
        if s.tools.is_empty() {
            tools = tools.child(div().px(px(space::M)).py(px(space::S + 2.)).text_size(px(text::FOOTNOTE)).text_color(theme.tertiary).child(if s.status == "connected" { "This server offers no tools." } else { "Its tools show once it's connected." }));
        }
        for (i, (name, read_only, destructive)) in s.tools.iter().enumerate() {
            tools = tools.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S))
                    .px(px(space::M))
                    .py(px(6.))
                    .when(i > 0, |d| d.border_t_1().border_color(theme.separator))
                    .child(icons::wrench().size(px(12.)).text_color(theme.tertiary))
                    .child(div().flex_1().min_w_0().truncate().font_family(MONO).text_size(px(text::CODE)).child(name.clone()))
                    .when(*read_only, |d| d.child(page_kit::quiet_chip("Read only", theme)))
                    .when(*destructive, |d| d.child(page_kit::chip("Destructive", theme.error))),
            );
        }
        let (n1, n2, n3) = (s.name.clone(), s.name.clone(), s.name.clone());
        let removable = ["local", "project", "user"].contains(&s.scope.as_str());
        let waiting = self.signing_in.contains(&s.name);
        let box_ = |tint: gpui_kit::Hsla| div().p(px(space::M)).rounded(px(radius::LARGE)).border_1().border_color(tint.opacity(0.35)).bg(tint.opacity(0.07)).flex().flex_col().gap(px(space::S));
        // Signing in: waiting for the browser, with what to do when it doesn't come back.
        let sign_in = (s.status == "needs-auth" || waiting).then(|| {
            let host = s.summary.split('/').nth(2).map(str::to_string);
            let name = s.name.clone();
            if waiting {
                let page = sign_in_page(&s.name);
                let (open, copy, cancel, finish) = (page.clone(), page, name.clone(), name.clone());
                box_(theme.warning)
                    .child(div().flex().items_center().gap(px(space::S)).child(gpui_kit::component::Icon::new(IconName::LoaderCircle).size(px(14.)).text_color(theme.warning)).child(div().font_weight(FontWeight::MEDIUM).child("Waiting for you to sign in in your browser…")))
                    .child(div().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child("This connects by itself once you're done there."))
                    .child(
                        div()
                            .flex()
                            .gap(px(space::S))
                            .child(Button::new("mcp-open-again").small().outline().label("Open Again").disabled(open.is_none()).on_click(move |_, _, cx| {
                                if let Some(u) = &open {
                                    open_sign_in_page(u, cx);
                                }
                            }))
                            .child(Button::new("mcp-copy-link").small().outline().icon(IconName::Copy).label("Copy Link").disabled(copy.is_none()).on_click(move |_, _, cx| {
                                if let Some(u) = &copy {
                                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(u.clone()));
                                }
                            }))
                            .child(Button::new("mcp-cancel-sign-in").small().ghost().label("Cancel").on_click(cx.listener(move |this, _, _, cx| {
                                this.signing_in.remove(&cancel);
                                cx.notify();
                            }))),
                    )
                    .child(
                        div()
                            .id("mcp-callback-toggle")
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .text_size(px(text::FOOTNOTE))
                            .text_color(theme.secondary)
                            .cursor_pointer()
                            .child(gpui_kit::component::Icon::new(if self.callback_open { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(12.)))
                            .child("Browser didn't come back?")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.callback_open = !this.callback_open;
                                cx.notify();
                            })),
                    )
                    .when(self.callback_open, |d| {
                        d.child(div().text_size(px(text::CAPTION)).text_color(theme.secondary).child("Paste the address the browser ended on, even if the page didn't load."))
                            .child(
                                div()
                                    .flex()
                                    .gap(px(space::S))
                                    .child(div().flex_1().child(Input::new(&self.callback).small()))
                                    .child(Button::new("mcp-finish-sign-in").small().primary().label("Finish").on_click(cx.listener(move |this, _, _, cx| this.finish_sign_in(&finish, cx)))),
                            )
                    })
                    .into_any_element()
            } else {
                box_(theme.warning)
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("Sign in to use {}", s.name)))
                    .child(div().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).whitespace_normal().child(format!(
                        "Opens {} in your browser, with your company's sign-in if it uses one. Once you've approved it there, it connects here by itself.",
                        host.unwrap_or_else(|| "the server's sign-in page".into())
                    )))
                    .child(div().flex().child(Button::new("mcp-sign-in").small().primary().icon(icons::key_round()).label("Sign In in Browser…").on_click(cx.listener(move |this, _, _, cx| this.act("mcp_authenticate", &name, json!({}), cx)))))
                    .into_any_element()
            }
        });
        // Couldn't start: Retry, or hand it to Claude in the thread.
        let failure = (s.status == "failed").then(|| {
            let (retry, fix) = (s.name.clone(), s.clone());
            box_(theme.error)
                .child(div().flex().items_center().gap(px(space::S)).text_color(theme.error).child(gpui_kit::component::Icon::new(IconName::CircleX).size(px(14.))).child(div().font_weight(FontWeight::SEMIBOLD).child("Couldn't start")))
                .when_some(s.error.clone(), |d, e| d.child(div().font_family(MONO).text_size(px(text::CODE)).whitespace_normal().child(crate::ui::rows::selectable(e, theme))))
                .child(
                    div()
                        .flex()
                        .gap(px(space::S))
                        .child(Button::new("mcp-retry").small().outline().label("Retry").on_click(cx.listener(move |this, _, _, cx| this.act("mcp_reconnect", &retry, json!({}), cx))))
                        .when(!self.everywhere, |d| {
                            d.child(Button::new("mcp-ask-fix").small().outline().icon(icons::sparkles()).label("Ask Claude to Fix It").on_click(cx.listener(move |this, _, _, cx| this.ask_to_fix(&fix, cx))))
                        }),
                )
                .into_any_element()
        });
        let confirm = (self.confirm_remove.as_deref() == Some(s.name.as_str())).then(|| {
            let server = s.clone();
            box_(theme.error)
                .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("Remove {}?", s.name)))
                .child(div().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(if s.scope == "project" {
                    "It's taken out of this project's .mcp.json, for everyone who uses it."
                } else {
                    "Claude Code forgets it, in the terminal too."
                }))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(space::S))
                        .child(Button::new("mcp-remove-cancel").small().outline().label("Cancel").on_click(cx.listener(|this, _, _, cx| {
                            this.confirm_remove = None;
                            cx.notify();
                        })))
                        .child(Button::new("mcp-remove-confirm").small().danger().label("Remove").on_click(cx.listener(move |this, _, _, cx| {
                            this.confirm_remove = None;
                            this.remove(&server, cx);
                        }))),
                )
                .into_any_element()
        });
        div()
            .flex()
            .flex_col()
            .gap(px(space::M))
            .child(div().ml(px(-6.)).flex().child(Button::new("mcp-back").small().ghost().icon(icons::chevron_left()).label("All Servers").on_click(cx.listener(|this, _, _, cx| {
                this.page = None;
                this.confirm_remove = None;
                cx.notify();
            }))))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::M))
                    .child(page_kit::icon_tile(icons::server(), 36., theme))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(div().flex().items_center().gap(px(space::S)).child(div().truncate().text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).child(s.name.clone())).child(page_kit::chip(status_text(&s.status).to_string(), status_color(&s.status, theme))))
                            .child(div().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(format!("{} · {}", scope_label(&s.scope), s.transport))),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(space::XS))
                            .when(s.status == "connected" && s.transport != "stdio", |d| d.child(Button::new("mcp-sign-out").small().outline().icon(icons::log_out()).label("Sign Out").on_click(cx.listener(move |this, _, _, cx| this.act("mcp_clear_auth", &n2, json!({}), cx)))))
                            .when(s.status == "connected", |d| d.child(Button::new("mcp-reconnect").small().ghost().icon(icons::rotate_ccw()).tooltip("Reconnect").on_click(cx.listener(move |this, _, _, cx| this.act("mcp_reconnect", &n3, json!({}), cx)))))
                            .when(removable, |d| {
                                d.child(Button::new("mcp-remove").small().ghost().icon(icons::trash()).tooltip("Remove Server…").on_click(cx.listener(move |this, _, _, cx| {
                                    this.confirm_remove = Some(n1.clone());
                                    cx.notify();
                                })))
                            }),
                    ),
            )
            .children(confirm)
            .children(sign_in)
            .children(failure)
            .when(!s.summary.is_empty(), |d| d.child(div().px(px(space::M)).py(px(space::S)).rounded(px(radius::MEDIUM)).bg(theme.code).border_1().border_color(theme.separator).font_family(MONO).text_size(px(text::CODE)).text_color(theme.secondary).child(s.summary.clone())))
            .when_some(s.error.clone().filter(|_| s.status != "failed"), |d, e| {
                d.child(div().flex().items_start().gap(px(space::S)).text_size(px(text::FOOTNOTE)).text_color(theme.error).child(div().mt(px(1.)).child(gpui_kit::component::Icon::new(IconName::CircleAlert).size(px(14.)))).child(div().flex_1().child(e)))
            })
            .child(div().pt(px(space::XS)).child(page_kit::group_label(format!("Tools · {}", s.tools.len()), theme)))
            .child(tools)
            .into_any_element()
    }

    /// Ask Claude to Fix It: what's wrong goes in the thread's composer, and the sheet closes.
    fn ask_to_fix(&mut self, s: &Server, cx: &mut Context<Self>) {
        let text = format!(
            "The MCP server \"{}\" won't start{}{} Find out why and fix it.",
            s.name,
            if s.summary.is_empty() { String::new() } else { format!(" (`{}`)", s.summary) },
            s.error.as_ref().map(|e| format!(": {e}")).unwrap_or_else(|| ".".into())
        );
        self.workspace.update(cx, |_, cx| cx.emit(crate::state::workspace::WorkspaceEvent::AppendToComposer(text)));
        self.close(cx);
    }

    fn pairs_view(&self, pairs: &[Pair], which: &'static str, add: &'static str, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let mut column = div().flex().flex_col().gap(px(space::XS));
        for (i, (k, v)) in pairs.iter().enumerate() {
            column = column.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S))
                    .child(div().w(px(160.)).child(Input::new(k).small()))
                    .child(div().flex_1().child(if which == "env" { Input::new(v).small().mask_toggle().into_any_element() } else { Input::new(v).small().into_any_element() }))
                    .child(crate::ui::thread_toolbar::icon_button(SharedString::from(format!("{which}-remove-{i}")), gpui_kit::component::Icon::new(IconName::Minus), "Remove", theme).on_click(cx.listener(move |this, _, _, cx| {
                        let list = if which == "env" { &mut this.env } else { &mut this.headers };
                        if i < list.len() {
                            list.remove(i);
                        }
                        cx.notify();
                    }))),
            );
        }
        column
            .child(div().flex().child(Button::new(SharedString::from(format!("{which}-add"))).small().ghost().icon(IconName::Plus).label(add).on_click(cx.listener(move |this, _, window, cx| {
                let pair = if which == "env" { this.pair("NAME", "Value", window, cx) } else { this.pair("Header", "Value", window, cx) };
                if which == "env" { this.env.push(pair) } else { this.headers.push(pair) }
                cx.notify();
            }))))
            .into_any_element()
    }

    fn add_page(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let page = cx.entity().downgrade();
        let scopes: Vec<Segment> = SCOPES
            .into_iter()
            .filter(|(s, _)| !self.everywhere || *s == "user")
            .map(|(scope, label)| {
                let page = page.clone();
                Segment::new(SharedString::from(format!("scope-{scope}")), label, self.add_scope == scope, move |_, _, cx| {
                    let _ = page.update(cx, |this, cx| {
                        this.add_scope = scope;
                        cx.notify();
                    });
                })
            })
            .collect();
        let ways: Vec<Segment> = [(Way::Url, "URL"), (Way::Command, "Command"), (Way::Json, "JSON")]
            .into_iter()
            .map(|(way, label)| {
                let page = page.clone();
                Segment::new(SharedString::from(format!("way-{label}")), label, self.way == way, move |_, window, cx| {
                    let _ = page.update(cx, |this, cx| {
                        if this.way != way {
                            this.way = way;
                            this.add_source.update(cx, |i, cx| {
                                i.set_value("", window, cx);
                                i.set_placeholder(if way == Way::Url { "https://…/mcp" } else { "npx -y @modelcontextprotocol/server-github" }, window, cx);
                            });
                        }
                        cx.notify();
                    });
                })
            })
            .collect();
        let about = match self.way {
            Way::Url => "Paste the server's URL. If it needs you to sign in, your browser opens for it, with your company's sign-in if it uses one.",
            Way::Command => "A program on your Mac that Claude Code starts.",
            Way::Json => "A server's config, or a whole mcpServers block from a README.",
        };
        let mut form = div().flex().flex_col().gap(px(space::M)).child(div().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).whitespace_normal().child(about)).child(div().flex().child(page_kit::segmented(ways, theme)));
        match self.way {
            Way::Url => {
                form = form
                    .child(page_kit::form_row("URL", Input::new(&self.add_source).into_any_element(), theme))
                    .child(page_kit::form_row("Name", Input::new(&self.add_name).into_any_element(), theme))
                    .child(
                        div()
                            .id("mcp-advanced")
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .text_size(px(text::FOOTNOTE))
                            .text_color(theme.secondary)
                            .cursor_pointer()
                            .child(gpui_kit::component::Icon::new(if self.advanced { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(12.)))
                            .child("Advanced")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.advanced = !this.advanced;
                                cx.notify();
                            })),
                    );
                if self.advanced {
                    form = form
                        .child(div().text_size(px(text::CAPTION)).text_color(theme.tertiary).child("Only if the server's docs ask for them."))
                        .child(page_kit::form_row("Headers", self.pairs_view(&self.headers, "headers", "Add Header", theme, cx), theme))
                        .child(page_kit::form_row("Client ID", Input::new(&self.client_id).into_any_element(), theme))
                        .child(page_kit::form_row("Client secret", Input::new(&self.client_secret).mask_toggle().into_any_element(), theme))
                        .child(page_kit::form_row("Callback port", div().w(px(140.)).child(Input::new(&self.callback_port)).into_any_element(), theme));
                }
            }
            Way::Command => {
                form = form
                    .child(page_kit::form_row("Command", Input::new(&self.add_source).into_any_element(), theme))
                    .child(page_kit::form_row("Name", Input::new(&self.add_name).into_any_element(), theme))
                    .child(page_kit::form_row("Environment", self.pairs_view(&self.env, "env", "Add Variable", theme, cx), theme));
            }
            Way::Json => {
                form = form.child(page_kit::card(theme).px(px(space::M)).py(px(space::S)).font_family(MONO).text_size(px(text::CODE)).child(Textarea::new(&self.add_json).appearance(false))).child(page_kit::form_row("Name", Input::new(&self.add_name).into_any_element(), theme));
            }
        }
        if !self.everywhere {
            form = form.child(page_kit::form_row("Applies to", div().flex().child(page_kit::segmented(scopes, theme)).into_any_element(), theme));
        }
        // From Claude Desktop: its servers, ticked, added in one go.
        if let Some(desktop) = &self.desktop {
            let mut card = page_kit::card(theme).p(px(space::M)).flex().flex_col().gap(px(space::S)).child(page_kit::group_label("From Claude Desktop", theme));
            if desktop.is_empty() {
                card = card.child(div().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child("Claude Desktop has no MCP servers, or isn't installed."));
            } else {
                for (name, config) in desktop {
                    let picked = self.desktop_picked.contains(name);
                    let key = name.clone();
                    card = card.child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(space::S))
                            .child(gpui_kit::component::checkbox::Checkbox::new(SharedString::from(format!("desktop-{name}"))).checked(picked).on_click(cx.listener(move |this, on: &bool, _, cx| {
                                if *on {
                                    this.desktop_picked.insert(key.clone());
                                } else {
                                    this.desktop_picked.remove(&key);
                                }
                                cx.notify();
                            })))
                            .child(div().flex_1().min_w_0().flex().flex_col().child(div().font_weight(FontWeight::MEDIUM).child(name.clone())).child(div().truncate().font_family(MONO).text_size(px(text::CAPTION)).text_color(theme.secondary).child(config_summary(config)))),
                    );
                }
                let n = self.desktop_picked.len();
                card = card.child(div().flex().child(Button::new("desktop-add").small().outline().label(format!("Add {n} Server{}", if n == 1 { "" } else { "s" })).disabled(n == 0 || self.busy.is_some()).on_click(cx.listener(|this, _, _, cx| this.import_desktop(cx)))));
            }
            form = form.child(card);
        }
        form.child(
            div()
                .pt(px(space::XS))
                .flex()
                .items_center()
                .gap(px(space::S))
                .child(Button::new("add-desktop").small().ghost().label("Import from Claude Desktop…").on_click(cx.listener(|this, _, _, cx| this.load_desktop(cx))))
                .child(div().flex_1())
                .child(Button::new("add-cancel").small().outline().label("Cancel").on_click(cx.listener(|this, _, _, cx| {
                    this.adding = false;
                    this.desktop = None;
                    cx.notify();
                })))
                .child(Button::new("add-submit").small().primary().label("Add Server").disabled(self.busy.is_some()).on_click(cx.listener(|this, _, _, cx| this.add(cx)))),
        )
        .into_any_element()
    }

    /// For test runs: what's typed in Add Server (a URL, or a command line).
    pub fn fill_add(&mut self, source: &str, scope: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        self.adding = true;
        self.add_scope = scope;
        self.way = if source.starts_with("http://") || source.starts_with("https://") {
            Way::Url
        } else if source.starts_with('{') {
            Way::Json
        } else {
            Way::Command
        };
        if self.way == Way::Json {
            self.add_json.update(cx, |i, cx| i.set_value(source.to_string(), window, cx));
        } else {
            self.add_source.update(cx, |i, cx| i.set_value(source.to_string(), window, cx));
        }
        // `YH_MCP_ADD_ENV=NAME=value`: an environment variable for a command (Add Variable).
        if let Ok(kv) = std::env::var("YH_MCP_ADD_ENV") {
            if let Some((k, v)) = kv.split_once('=') {
                let pair = self.pair("NAME", "Value", window, cx);
                pair.0.update(cx, |i, cx| i.set_value(k.to_string(), window, cx));
                pair.1.update(cx, |i, cx| i.set_value(v.to_string(), window, cx));
                self.env.push(pair);
            }
        }
        // `YH_MCP_ADD_DESKTOP=1`: Import from Claude Desktop pressed (with `YH_CLAUDE_DESKTOP_CONFIG`).
        if std::env::var("YH_MCP_ADD_DESKTOP").is_ok() {
            self.load_desktop(cx);
        }
        cx.notify();
    }

    pub fn remove_named(&mut self, name: &str, cx: &mut Context<Self>) {
        if let Some(s) = self.servers.iter().find(|s| s.name == name).cloned() {
            self.remove(&s, cx);
        }
    }
}

impl Render for McpSheet {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        if !self.open {
            return div();
        }
        let body = if self.adding {
            self.add_page(&theme, cx)
        } else if let Some(s) = self.page.as_ref().and_then(|p| self.servers.iter().find(|s| &s.name == p)).cloned() {
            self.server_page(&s, &theme, cx)
        } else {
            self.list(&theme, cx)
        };
        let (title, subtitle) = if self.adding {
            ("Add Server", if self.everywhere { "For every folder you run Claude Code in" } else { "Through claude mcp add-json, so the terminal sees it too" })
        } else if self.everywhere {
            ("MCP Servers Everywhere", "What every Claude Code session gets, in any folder")
        } else {
            ("MCP Servers", "What Claude Code connects to in this thread's folder")
        };
        let error = self.error.clone().filter(|_| self.loaded).map(|e| {
            div()
                .flex()
                .items_start()
                .gap(px(space::S))
                .px(px(space::M))
                .py(px(space::S))
                .rounded(px(radius::MEDIUM))
                .bg(theme.error.opacity(0.08))
                .text_size(px(text::FOOTNOTE))
                .text_color(theme.error)
                .child(div().mt(px(1.)).child(gpui_kit::component::Icon::new(IconName::CircleAlert).size(px(14.))))
                .child(div().flex_1().child(e))
        });
        // Inside Settings: the list and its pages in the tab, Add Server above them.
        if self.embedded {
            return div()
                .flex()
                .flex_col()
                .gap(px(space::M))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S))
                        .child(div().flex_1().min_w_0().truncate().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(self.busy.clone().unwrap_or_else(|| subtitle.to_string())))
                        .when(!self.adding, |d| {
                            d.child(Button::new("mcp-add").small().outline().icon(IconName::Plus).label("Add Server…").on_click(cx.listener(|this, _, _, cx| {
                                this.adding = true;
                                this.page = None;
                                cx.notify();
                            })))
                        }),
                )
                .children(error)
                .child(body);
        }
        div()
            .absolute()
            .size_full()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(60.))
            .bg(gpui_kit::black().opacity(0.15))
            .on_mouse_down(gpui_kit::MouseButton::Left, cx.listener(|this, _, _, cx| this.close(cx)))
            .child(
                div()
                    .id("mcp-sheet")
                    .w(px(600.))
                    .max_h(px(640.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(space::M + 2.))
                    .px(px(space::XL - 4.))
                    .pt(px(space::L + 2.))
                    .pb(px(space::XL - 4.))
                    .rounded(px(radius::LARGE + 2.))
                    .bg(theme.canvas)
                    .border_1()
                    .border_color(theme.separator)
                    .shadow_lg()
                    .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::M))
                            .child(page_kit::icon_tile(icons::plug(), 34., &theme))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(1.))
                                    .child(div().text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).child(title))
                                    .child(div().truncate().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(self.busy.clone().unwrap_or_else(|| subtitle.to_string()))),
                            )
                            .when(!self.adding, |d| {
                                d.child(Button::new("mcp-add").small().outline().icon(IconName::Plus).label("Add Server…").on_click(cx.listener(|this, _, _, cx| {
                                    this.adding = true;
                                    this.page = None;
                                    cx.notify();
                                })))
                            })
                            .child(Button::new("mcp-close").small().when(self.adding, |b| b.ghost()).when(!self.adding, |b| b.primary()).label("Done").on_click(cx.listener(|this, _, _, cx| this.close(cx)))),
                    )
                    .children(error)
                    .child(body),
            )
    }
}
