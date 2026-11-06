//! `/mcp`: the thread's MCP servers as Claude Code sees them in its folder, without a
//! terminal. Grouped by where each applies, with what each needs: turn on or off, reconnect,
//! sign in (the page opens in your default browser, where your company's sign-in lives;
//! Claude Code's localhost callback finishes it), sign out. A server opens to its tools.
//! Add Server takes a URL, a command or JSON and where it applies, through `claude mcp
//! add-json`, so the terminal sees it too. Everything goes over the thread's `claude`
//! control channel (docs/mcp.md), asked again every 2 s while the sheet is open.

use crate::state::thread::ThreadState;
use crate::state::workspace::Workspace;
use crate::theme::{Theme, MONO};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::Sizable;
use gpui_kit::{div, prelude::*, px, Context, Entity, FontWeight, SharedString, Task, Window};
use serde_json::{json, Value};

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
            add_source: cx.new(|cx| InputState::new(window, cx).placeholder("https://mcp.example.com/mcp, a command, or JSON")),
            add_name: cx.new(|cx| InputState::new(window, cx).placeholder("Name (from the URL or command)")),
            add_scope: "local",
            busy: None,
            _poll: None,
        }
    }

    /// Opens for the selected thread, and asks again every 2 s while open.
    pub fn show(&mut self, cx: &mut Context<Self>) {
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
                        this.loaded = true;
                        this.error = None;
                        if first && std::env::var("YH_OPEN_MCP").is_ok() {
                            eprintln!("MCP servers: {}", this.describe());
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
        cx.spawn(async move |this, cx| {
            let reply = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                match reply {
                    Ok(v) if sign_in => match v["authUrl"].as_str() {
                        // The default browser: your SSO session, passkeys and device checks live there.
                        Some(url) if std::env::var("YH_MCP_BROWSER").as_deref() == Ok("log") => eprintln!("MCP sign-in page: {url}"),
                        Some(url) => cx.open_url(url),
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

    /// Adds what's typed (a URL, a command line, or JSON) where chosen, with Claude Code's own
    /// `claude mcp add-json`, then gets the running `claude` to see it.
    pub fn add(&mut self, cx: &mut Context<Self>) {
        let source = self.add_source.read(cx).value().trim().to_string();
        if source.is_empty() {
            return;
        }
        let Some(thread) = self.thread.clone() else { return };
        let mut entries: Vec<(String, Value)> = vec![];
        let typed_name = self.add_name.read(cx).value().trim().to_string();
        if source.starts_with('{') {
            let Ok(v) = serde_json::from_str::<Value>(&source) else {
                self.error = Some("That JSON doesn't parse.".into());
                cx.notify();
                return;
            };
            // One config, {"name": {…}}, or a whole mcpServers block.
            let block = v.get("mcpServers").cloned().unwrap_or(v.clone());
            if block.get("command").is_some() || block.get("url").is_some() {
                entries.push((if typed_name.is_empty() { "server".into() } else { typed_name.clone() }, block));
            } else if let Some(o) = block.as_object() {
                entries.extend(o.iter().map(|(k, c)| (k.clone(), c.clone())));
            }
        } else if source.starts_with("http://") || source.starts_with("https://") {
            let host = source.split('/').nth(2).unwrap_or("server").split(':').next().unwrap_or("server");
            let parts: Vec<&str> = host.split('.').collect();
            let guess = if parts.len() >= 2 { parts[parts.len() - 2] } else { host };
            let guess = if guess == "mcp" && parts.len() >= 3 { parts[parts.len() - 2] } else { guess };
            entries.push((if typed_name.is_empty() { guess.to_string() } else { typed_name.clone() }, json!({ "type": "http", "url": source })));
        } else {
            let words = yh_core::forge::shell_words(&source);
            let Some((command, args)) = words.split_first() else { return };
            let base = args.iter().rev().find(|a| !a.starts_with('-')).unwrap_or(command).rsplit('/').next().unwrap_or("server").trim_end_matches(".py").trim_end_matches(".js").to_string();
            entries.push((if typed_name.is_empty() { base } else { typed_name.clone() }, json!({ "type": "stdio", "command": command, "args": args })));
        }
        let folder = self.workspace.read(cx).folder_of(thread.read(cx));
        let scope = self.add_scope;
        self.busy = Some("Adding…".into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let names: Vec<String> = entries.iter().map(|(n, _)| n.clone()).collect();
            let result = cx
                .background_executor()
                .spawn(async move {
                    for (name, config) in entries {
                        run_claude(&folder, &["mcp", "add-json", &name, &config.to_string(), "--scope", scope])?;
                    }
                    Ok::<(), String>(())
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
                        this.add_source.update(cx, |_, _| {});
                        this.after_change(scope == "local", names, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn remove(&mut self, server: &Server, cx: &mut Context<Self>) {
        let Some(thread) = self.thread.clone() else { return };
        let folder = self.workspace.read(cx).folder_of(thread.read(cx));
        let (name, scope) = (server.name.clone(), server.scope.clone());
        self.busy = Some(format!("Removing {name}…"));
        cx.spawn(async move |this, cx| {
            let n = name.clone();
            let result = cx.background_executor().spawn(async move { run_claude(&folder, &["mcp", "remove", &n, "--scope", &scope]) }).await;
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

/// `claude <args>` in a folder, for `claude mcp add-json` and `remove`.
fn run_claude(folder: &str, args: &[&str]) -> Result<(), String> {
    let claude = yh_core::claude::find_claude().ok_or("Claude Code isn't installed.")?;
    let mut command = std::process::Command::new(claude);
    command.args(args).current_dir(folder);
    if let Ok(p) = std::env::var("YH_PATH") {
        command.env("PATH", p);
    }
    let out = command.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(if out.stderr.is_empty() { &out.stdout } else { &out.stderr }).trim().to_string())
    }
}

fn dot(status: &str, theme: &Theme) -> gpui_kit::Div {
    let color: gpui_kit::Hsla = match status {
        "connected" => gpui_kit::rgb(0x34c759).into(),
        "failed" => theme.error,
        "needs-auth" => theme.warning,
        "disabled" => theme.secondary,
        _ => theme.accent,
    };
    div().flex_none().w(px(8.)).h(px(8.)).rounded_full().bg(color)
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

impl McpSheet {
    fn list(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        if !self.loaded {
            return div().p(px(20.)).text_color(theme.secondary).child(self.error.clone().unwrap_or_else(|| "Asking Claude Code…".into())).into_any_element();
        }
        let mut column = div().flex().flex_col().gap(px(10.));
        if self.servers.is_empty() {
            column = column.child(div().py(px(16.)).text_color(theme.secondary).child("No MCP servers here. Add one with a URL, a command or JSON."));
        }
        let mut scopes: Vec<String> = vec![];
        for s in &self.servers {
            if !scopes.contains(&s.scope) {
                scopes.push(s.scope.clone());
            }
        }
        for scope in scopes {
            column = column.child(div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(theme.secondary).child(scope_label(&scope).to_uppercase()));
            for s in self.servers.iter().filter(|s| s.scope == scope) {
                let (open, name) = (s.name.clone(), s.name.clone());
                let action: Option<(&str, &str)> = match s.status.as_str() {
                    "needs-auth" => Some(("Sign In", "mcp_authenticate")),
                    "failed" => Some(("Reconnect", "mcp_reconnect")),
                    _ => None,
                };
                let on = s.status != "disabled";
                let (n1, n2) = (name.clone(), name.clone());
                column = column.child(
                    div()
                        .id(SharedString::from(format!("mcp-{name}")))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .p(px(8.))
                        .rounded(px(8.))
                        .bg(theme.card)
                        .hover(|d| d.bg(theme.selected))
                        .child(dot(&s.status, theme))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(div().flex().gap(px(6.)).child(div().font_weight(FontWeight::MEDIUM).child(s.name.clone())).child(div().text_size(px(11.)).text_color(theme.secondary).child(format!("{} · {} · {} tools", s.transport, status_text(&s.status), s.tools.len()))))
                                .when_some(s.error.clone(), |d, e| d.child(div().text_size(px(11.)).text_color(theme.error).line_clamp(2).child(e))),
                        )
                        .when_some(action, |d, (label, subtype)| {
                            let subtype = subtype.to_string();
                            d.child(Button::new(SharedString::from(format!("mcp-act-{name}"))).xsmall().primary().label(label).on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.act(&subtype, &n1, json!({}), cx)
                            })))
                        })
                        // The app's own servers (browser, memory, threads) aren't Claude Code's to turn off.
                        .when(s.scope != "claudeai" && s.scope != "dynamic", |d| {
                            d.child(Button::new(SharedString::from(format!("mcp-toggle-{name}"))).xsmall().ghost().label(if on { "Turn Off" } else { "Turn On" }).on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.act("mcp_toggle", &n2, json!({ "enabled": !on }), cx)
                            })))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.page = Some(open.clone());
                            cx.notify();
                        })),
                );
            }
        }
        column.into_any_element()
    }

    fn server_page(&self, s: &Server, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let mut tools = div().flex().flex_col().gap(px(2.));
        for (name, read_only, destructive) in &s.tools {
            tools = tools.child(
                div()
                    .flex()
                    .gap(px(6.))
                    .child(div().font_family(MONO).text_size(px(12.)).child(name.clone()))
                    .when(*read_only, |d| d.child(div().text_size(px(10.)).text_color(theme.secondary).child("read only")))
                    .when(*destructive, |d| d.child(div().text_size(px(10.)).text_color(theme.error).child("destructive"))),
            );
        }
        let server = s.clone();
        let (n1, n2) = (s.name.clone(), s.name.clone());
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(Button::new("mcp-back").xsmall().ghost().label("‹ All Servers").on_click(cx.listener(|this, _, _, cx| {
                this.page = None;
                cx.notify();
            })))
            .child(div().flex().items_center().gap(px(8.)).child(dot(&s.status, theme)).child(div().text_size(px(17.)).font_weight(FontWeight::SEMIBOLD).child(s.name.clone())).child(div().text_color(theme.secondary).child(status_text(&s.status).to_string())))
            .child(div().text_size(px(12.)).text_color(theme.secondary).child(format!("{} · {}", scope_label(&s.scope), s.transport)))
            .child(div().font_family(MONO).text_size(px(11.)).text_color(theme.secondary).child(s.summary.clone()))
            .when_some(s.error.clone(), |d, e| d.child(div().text_color(theme.error).child(e)))
            .child(
                div()
                    .flex()
                    .gap(px(6.))
                    .when(s.status == "needs-auth", |d| d.child(Button::new("mcp-sign-in").small().primary().label("Sign In").on_click(cx.listener(move |this, _, _, cx| this.act("mcp_authenticate", &n1, json!({}), cx)))))
                    .when(s.status == "connected" && s.transport != "stdio", |d| d.child(Button::new("mcp-sign-out").small().label("Sign Out").on_click(cx.listener(move |this, _, _, cx| this.act("mcp_clear_auth", &n2, json!({}), cx)))))
                    .when(["local", "project", "user"].contains(&s.scope.as_str()), |d| d.child(Button::new("mcp-remove").small().ghost().label("Remove").on_click(cx.listener(move |this, _, _, cx| this.remove(&server, cx))))),
            )
            .child(div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(theme.secondary).child(format!("TOOLS · {}", s.tools.len())))
            .child(tools)
            .into_any_element()
    }

    fn add_page(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let mut scopes = div().flex().gap(px(2.));
        for (scope, label) in SCOPES {
            let b = Button::new(SharedString::from(format!("scope-{scope}"))).xsmall().label(label);
            scopes = scopes.child((if self.add_scope == scope { b.primary() } else { b.ghost() }).on_click(cx.listener(move |this, _, _, cx| {
                this.add_scope = scope;
                cx.notify();
            })));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(div().text_size(px(17.)).font_weight(FontWeight::SEMIBOLD).child("Add Server"))
            .child(div().text_color(theme.secondary).child("A server's URL (you'll sign in in your browser), the command that runs it, or its JSON."))
            .child(Input::new(&self.add_source))
            .child(Input::new(&self.add_name).small())
            .child(div().text_size(px(11.)).text_color(theme.secondary).child("WHERE IT APPLIES"))
            .child(scopes)
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(6.))
                    .child(Button::new("add-cancel").small().label("Cancel").on_click(cx.listener(|this, _, _, cx| {
                        this.adding = false;
                        cx.notify();
                    })))
                    .child(Button::new("add-submit").small().primary().label("Add").on_click(cx.listener(|this, _, _, cx| this.add(cx)))),
            )
            .into_any_element()
    }

    /// For test runs: what's typed in Add Server.
    pub fn fill_add(&mut self, source: &str, scope: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        self.adding = true;
        self.add_scope = scope;
        self.add_source.update(cx, |i, cx| i.set_value(source.to_string(), window, cx));
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
                    .gap(px(12.))
                    .p(px(18.))
                    .rounded(px(14.))
                    .bg(theme.canvas)
                    .border_1()
                    .border_color(theme.separator)
                    .shadow_lg()
                    .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().flex_1().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child("MCP Servers"))
                            .when_some(self.busy.clone(), |d, b| d.child(div().text_size(px(11.)).text_color(theme.secondary).child(b)))
                            .when(!self.adding, |d| {
                                d.child(Button::new("mcp-add").small().label("Add Server…").on_click(cx.listener(|this, _, _, cx| {
                                    this.adding = true;
                                    this.page = None;
                                    cx.notify();
                                })))
                            })
                            .child(Button::new("mcp-close").small().ghost().label("Done").on_click(cx.listener(|this, _, _, cx| this.close(cx)))),
                    )
                    .when_some(self.error.clone().filter(|_| self.loaded), |d, e| d.child(div().text_color(theme.error).child(e)))
                    .child(body),
            )
    }
}

/// Whether an `/mcp` typed in the composer is ours to answer.
pub fn is_mcp_command(text: &str) -> bool {
    text.trim() == "/mcp"
}
