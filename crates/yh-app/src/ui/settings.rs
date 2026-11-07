//! Settings (⌘,), the Swift app's four tabs: Agents (where `claude` is, its account, `gh` and
//! `glab`, the agents still to come), General (appearance, notifications, sleep, new thread
//! defaults, settling, titles, memory, plans, browser, diagrams, Home and data), MCP and Releases.
//! Each change is saved at once, in the Swift app's settings keys, and takes effect at once or
//! with each thread's next `claude`.

use crate::state::workspace::Workspace;
use crate::theme::Theme;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable, Sizable, WindowExt};
use gpui_kit::{div, prelude::*, px, AnyElement, App, Context, Entity, FontWeight, SharedString, Window};
use serde_json::{json, Value};
use yh_core::agents::{Account, Status};

fn picker(id: impl Into<SharedString>, current: String, options: Vec<(String, String)>, on_pick: impl Fn(String, &mut Window, &mut App) + 'static) -> impl IntoElement {
    let label = options.iter().find(|(v, _)| *v == current).map(|(_, l)| l.clone()).unwrap_or(current.clone());
    let on_pick = std::rc::Rc::new(on_pick);
    Button::new(gpui_kit::ElementId::Name(id.into())).small().outline().label(label).dropdown_caret(true).dropdown_menu(move |mut menu, _, _| {
        for (value, label) in &options {
            let (on_pick, value) = (on_pick.clone(), value.clone());
            menu = menu.item(PopupMenuItem::new(label.clone()).checked(value == current).on_click(move |_, window, cx| on_pick(value.clone(), window, cx)));
        }
        menu
    })
}

/// What Settings opens besides itself: the MCP sheet and the updater (set by the window).
pub struct SettingsLinks {
    pub mcp: Entity<crate::ui::mcp::McpSheet>,
    pub updater: Entity<crate::state::updater::Updater>,
}

impl gpui_kit::Global for SettingsLinks {}

#[derive(Clone, Copy, PartialEq)]
pub enum Tab {
    Agents,
    General,
    Mcp,
    Releases,
}

impl Tab {
    pub fn from_name(name: &str) -> Option<Tab> {
        Some(match name {
            "agents" => Tab::Agents,
            "general" => Tab::General,
            "mcp" => Tab::Mcp,
            "releases" => Tab::Releases,
            _ => return None,
        })
    }
}

/// What the Agents tab found: each CLI's status, and `claude`'s account.
#[derive(Default)]
struct Found {
    statuses: Vec<(&'static str, Status)>,
    account: Option<Account>,
    refreshing: bool,
}

pub struct SettingsView {
    workspace: Entity<Workspace>,
    tab: Tab,
    found: Found,
    title_prompt: Entity<TextareaState>,
}

impl SettingsView {
    fn new(workspace: Entity<Workspace>, tab: Tab, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let rules = workspace.read(cx).title_prompt().unwrap_or_else(|| yh_core::titles::DEFAULT_RULES.to_string());
        let title_prompt = cx.new(|cx| TextareaState::new(window, cx).default_value(rules));
        crate::platform::refresh_notification_status();
        let mut view = SettingsView { workspace, tab, found: Found::default(), title_prompt };
        view.detect(cx);
        view
    }

    /// Looks for the CLIs and asks `claude` who's signed in, aside.
    fn detect(&mut self, cx: &mut Context<Self>) {
        if self.found.refreshing {
            return;
        }
        self.found.refreshing = true;
        let chosen = self.workspace.read(cx).library.setting("cliPathOverrides").and_then(|o| o["claude"].as_str().map(std::path::PathBuf::from));
        let task = cx.background_executor().spawn(async move {
            let mut statuses: Vec<(&'static str, Status)> = vec![];
            for cli in &yh_core::agents::ALL {
                let chosen = (cli.id == "claude").then(|| chosen.clone()).flatten();
                statuses.push((cli.id, yh_core::agents::probe(cli.executable, chosen.as_deref())));
            }
            statuses.push(("gh", yh_core::agents::probe("gh", None)));
            statuses.push(("glab", yh_core::agents::probe("glab", None)));
            let account = statuses.iter().find(|(id, s)| *id == "claude" && s.is_ready()).and_then(|(_, s)| yh_core::agents::account(s.path.as_ref()?));
            (statuses, account)
        });
        cx.spawn(async move |this, cx| {
            let (statuses, account) = task.await;
            let _ = this.update(cx, |this, cx| {
                this.found = Found { statuses, account, refreshing: false };
                if std::env::var("YH_DATA_DIR").is_ok() {
                    for (id, s) in &this.found.statuses {
                        eprintln!("Settings agent {id}: {}", s.describe());
                    }
                    if let Some(a) = &this.found.account {
                        eprintln!("Settings account: signed in {} via {:?}, plan {:?}", a.logged_in, a.sign_in_method(), a.plan_name());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn status(&self, id: &str) -> Option<&Status> {
        self.found.statuses.iter().find(|(i, _)| *i == id).map(|(_, s)| s)
    }

    fn setting(&self, key: &str, cx: &App) -> Option<Value> {
        self.workspace.read(cx).library.setting(key)
    }

    fn flag(&self, key: &str, cx: &App) -> bool {
        self.setting(key, cx).and_then(|v| v.as_bool()).unwrap_or(true)
    }

    /// A switch saved under `key`, on unless turned off.
    fn toggle(&self, key: &'static str, label: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let on = self.flag(key, cx);
        let ws = self.workspace.clone();
        row(
            label,
            Switch::new(key).checked(on).on_change(move |on, _, cx| {
                ws.read(cx).library.set_setting(key, &Value::Bool(*on));
                ws.update(cx, |_, cx| cx.notify());
            }),
        )
    }

    fn agents(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let claude = self.status("claude").cloned();
        let chosen = self.setting("cliPathOverrides", cx).and_then(|o| o["claude"].as_str().map(str::to_string));
        let checking = || "Checking…".to_string();
        let mut claude_section = section("Claude Code")
            .child(row("Status", div().text_color(theme.secondary).child(claude.as_ref().map_or_else(checking, Status::describe))))
            .child(row(
                "Location",
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .min_w_0()
                    .child(div().text_color(theme.secondary).font_family("Menlo").text_size(px(11.)).truncate().child(claude.as_ref().and_then(|s| s.path.clone()).map_or("Not found".into(), |p| p.display().to_string())))
                    .when_some(claude.as_ref().and_then(|s| s.path.clone()), |d, path| {
                        d.child(Button::new("claude-reveal").small().ghost().label("Show in Finder").on_click(move |_, _, cx| cx.reveal_path(&path)))
                    }),
            ))
            .child({
                let (ws, ws2) = (self.workspace.clone(), self.workspace.clone());
                let view = cx.entity().downgrade();
                let view2 = view.clone();
                div()
                    .flex()
                    .justify_end()
                    .gap(px(6.))
                    .when(chosen.is_some(), |d| {
                        d.child(Button::new("claude-detected").small().outline().label("Use Detected").on_click(move |_, _, cx| {
                            ws2.update(cx, |ws, cx| ws.choose_claude(None, cx));
                            let _ = view2.update(cx, |v, cx| v.detect(cx));
                        }))
                    })
                    .child(Button::new("claude-choose").small().outline().label("Choose…").on_click(move |_, _, cx| {
                        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Use This Claude".into()) });
                        let (ws, view) = (ws.clone(), view.clone());
                        cx.spawn(async move |cx| {
                            if let Ok(Ok(Some(paths))) = paths.await {
                                if let Some(path) = paths.first() {
                                    let path = path.to_string_lossy().into_owned();
                                    let _ = cx.update(|cx| {
                                        ws.update(cx, |ws, cx| ws.choose_claude(Some(path), cx));
                                        let _ = view.update(cx, |v, cx| v.detect(cx));
                                    });
                                }
                            }
                        })
                        .detach();
                    }))
            });
        if claude.as_ref().is_some_and(|s| !s.is_ready() && s.path.is_none()) {
            claude_section = claude_section.child(footer("Install Claude Code from docs.claude.com, then Detect Again.", theme));
        }
        let account = match &self.found.account {
            Some(a) if a.logged_in => section("Account")
                .when_some(a.email.clone(), |s, e| s.child(row("Signed in as", div().text_color(theme.secondary).child(e))))
                .when_some(a.plan_name(), |s, p| s.child(row("Plan", div().font_weight(FontWeight::MEDIUM).child(p))))
                .when_some(a.sign_in_method(), |s, m| s.child(row("Signed in with", div().text_color(theme.secondary).child(m))))
                .when_some(a.org_name.clone(), |s, o| s.child(row("Organization", div().text_color(theme.secondary).child(o)))),
            Some(_) => section("Account").child(footer("Not signed in. Run `claude` in Terminal and sign in, then Detect Again.", theme)),
            None => div(),
        };
        let tool = |id: &str, name: &'static str| row(name, div().text_color(theme.secondary).child(self.status(id).map_or_else(checking, Status::describe)));
        let git = section("Git & Code Hosting")
            .child(tool("gh", "GitHub CLI (gh)"))
            .child(tool("glab", "GitLab CLI (glab)"))
            .child(footer("Pull requests come from `gh` and `glab`, signed in as you. YourHarness never sees their tokens.", theme));
        let mut others = section("Other Agents");
        for cli in yh_core::agents::ALL.iter().filter(|c| !c.supported) {
            let url = cli.install_url;
            let found = self.status(cli.id).is_some_and(Status::is_ready);
            others = others.child(row(
                cli.name,
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().text_color(theme.secondary).child(self.status(cli.id).map_or_else(checking, |s| if s.is_ready() { "Installed".into() } else { "Not installed".into() })))
                    .when(!found, |d| d.child(Button::new(cli.id).small().ghost().label("Get…").on_click(move |_, _, cx| cx.open_url(url)))),
            ));
        }
        others = others.child(footer("Support for these is coming. YourHarness detects them now, so they're ready when it lands.", theme));
        let refreshing = self.found.refreshing;
        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(claude_section)
            .child(account)
            .child(git)
            .child(others)
            .child(row(
                "Installed or signed in just now?",
                Button::new("detect-again").small().outline().label(if refreshing { "Detecting…" } else { "Detect Again" }).disabled(refreshing).on_click(cx.listener(|this, _, _, cx| {
                    this.detect(cx);
                    cx.notify();
                })),
            ))
            .into_any_element()
    }

    fn general(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let lib = ws.library.clone();
        let defaults = lib.setting("defaults").filter(Value::is_object).unwrap_or(json!({}));
        let s = |k: &str| defaults[k].as_str().unwrap_or("").to_string();
        let mut models: Vec<(String, String)> = vec![("".into(), "Claude Code's default".into())];
        models.extend(ws.models.iter().map(|m| (m.value.clone(), m.display_name.clone())));
        let efforts: Vec<(String, String)> = std::iter::once(("".into(), "Auto".into()))
            .chain(["low", "medium", "high", "xhigh", "max"].iter().map(|e| (e.to_string(), e[..1].to_uppercase() + &e[1..])))
            .collect();
        let modes: Vec<(String, String)> = [("auto", "Auto"), ("ask", "Ask"), ("acceptEdits", "Accept Edits"), ("plan", "Plan"), ("bypass", "Bypass Permissions")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        let set_default = |key: &'static str| {
            let lib = lib.clone();
            move |value: String, _: &mut Window, _: &mut App| {
                let mut d = lib.setting("defaults").filter(Value::is_object).unwrap_or(json!({}));
                if value.is_empty() {
                    d.as_object_mut().unwrap().remove(key);
                } else {
                    d[key] = Value::from(value);
                }
                lib.set_setting("defaults", &d);
            }
        };
        let appearance = lib.setting("appearance").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| "system".into());
        let keep = crate::keep_awake::Settings::from(lib.setting("keepAwake"));
        let settle = lib.setting("autoSettleDays").and_then(|v| v.as_i64()).unwrap_or(3);
        let titles = lib.setting("titleGeneration");
        let titles_on = titles.as_ref().is_none_or(|t| t.get("provider").is_some_and(|p| !p.is_null()));
        let title_model = titles.as_ref().and_then(|t| t["model"].as_str().map(str::to_string)).unwrap_or_default();
        let home = ws.home_project().and_then(|id| ws.project(&id).map(|p| p.path.clone())).unwrap_or_default();
        let data_dir = ws.data_dir.clone();
        let (wsa, wsb, wsc, wsd, wse) = (self.workspace.clone(), self.workspace.clone(), self.workspace.clone(), self.workspace.clone(), self.workspace.clone());
        let (k2, k3, k4) = (wsa.clone(), wsa.clone(), wsa);
        let denied = crate::platform::notifications_denied();
        let (l1, l2, l3) = (lib.clone(), lib.clone(), lib.clone());
        let title_prompt = self.title_prompt.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(
                section("Appearance")
                    .child(row(
                        "Appearance",
                        picker("settings-appearance", appearance, vec![("system".into(), "System".into()), ("light".into(), "Light".into()), ("dark".into(), "Dark".into())], move |v, window, cx| {
                            l1.set_setting("appearance", &Value::from(v.clone()));
                            crate::theme::set_chosen_appearance(Some(v));
                            crate::theme::apply_appearance(window.appearance(), window, cx);
                            cx.refresh_windows();
                        }),
                    ))
                    .child(footer("System follows macOS, including its automatic switch between light and dark.", theme)),
            )
            .child(
                section("Notifications")
                    .child(self.toggle("notifications", "Notify when Claude finishes or needs you", cx))
                    .when(denied, |s| s.child(div().text_color(theme.warning).text_size(px(12.)).child("⚠︎ Notifications are off for YourHarness in System Settings.")))
                    .child(footer("Only for threads you aren't looking at: a turn that finishes while YourHarness is in the background, and Claude asking for approval, an answer or a plan review. The Dock icon counts threads waiting for you.", theme)),
            )
            .child(
                section("Sleep")
                    .child(row(
                        "Keep the Mac awake",
                        picker(
                            "keep-mode",
                            if !keep.enabled { "off".into() } else { keep.mode.clone() },
                            vec![("whileWorking".into(), "While Claude works".into()), ("always".into(), "While YourHarness is open".into()), ("off".into(), "Never".into())],
                            move |v, _, cx| {
                                update_keep(&k2, cx, |k| {
                                    k.enabled = v != "off";
                                    if v != "off" {
                                        k.mode = v.clone();
                                    }
                                })
                            },
                        ),
                    ))
                    .child(row(
                        "Afterwards",
                        picker(
                            "keep-after",
                            keep.after_work_minutes.to_string(),
                            [(0, "Sleep at once"), (5, "5 more minutes"), (15, "15 more minutes"), (30, "30 more minutes"), (60, "1 more hour")].iter().map(|(m, l)| (m.to_string(), l.to_string())).collect(),
                            move |v, _, cx| update_keep(&k3, cx, |k| k.after_work_minutes = v.parse().unwrap_or(0)),
                        ),
                    ))
                    .child(row(
                        "Keep the display on too",
                        Switch::new("keep-display").checked(keep.keeps_display_on).on_change(move |on, _, cx| update_keep(&k4, cx, |k| k.keeps_display_on = *on)),
                    ))
                    .child(footer("The cup in the sidebar keeps it awake for a while on its own.", theme)),
            )
            .child(
                section("New Thread Defaults")
                    .child(row("Model", picker("settings-model", s("model"), models, set_default("model"))))
                    .child(row("Reasoning effort", picker("settings-effort", s("effort"), efforts, set_default("effort"))))
                    .child(row("Permissions for new projects", picker("settings-mode", if s("permissionMode").is_empty() { "auto".into() } else { s("permissionMode") }, modes, set_default("permissionMode"))))
                    .child(footer("Each thread can override these from the composer. Threads that haven't overridden them follow these defaults.", theme)),
            )
            .child(
                section("Threads")
                    .child(row(
                        "Settle idle threads",
                        picker(
                            "settle-days",
                            settle.to_string(),
                            [(0, "Never"), (1, "After 1 day"), (3, "After 3 days"), (7, "After 1 week"), (14, "After 2 weeks")].iter().map(|(d, l)| (d.to_string(), l.to_string())).collect(),
                            move |v, _, cx| {
                                l2.set_setting("autoSettleDays", &Value::from(v.parse::<i64>().unwrap_or(3)));
                                wsb.update(cx, |ws, cx| ws.settle_idle_threads(cx));
                            },
                        ),
                    ))
                    .child(footer("Settled threads move to a shelf at the end of their project and stop their Claude process; the conversation stays. Threads that are working or waiting on you never settle.", theme))
                    .child(row(
                        "Home",
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .min_w_0()
                            .child(div().text_color(theme.secondary).font_family("Menlo").text_size(px(11.)).truncate().child(home))
                            .child(Button::new("home-change").small().outline().label("Change…").on_click(move |_, _, cx| {
                                let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Use as Home".into()) });
                                let ws = wsc.clone();
                                cx.spawn(async move |cx| {
                                    if let Ok(Ok(Some(paths))) = paths.await {
                                        if let Some(path) = paths.first() {
                                            let path = path.to_string_lossy().into_owned();
                                            let _ = cx.update(|cx| ws.update(cx, |ws, cx| ws.set_home_folder(&path, cx)));
                                        }
                                    }
                                })
                                .detach();
                            })),
                    ))
                    .child(footer("New threads that aren't in a project run here.", theme)),
            )
            .child(
                section("Thread Titles")
                    .child(row(
                        "Generate with",
                        picker("titles-provider", if titles_on { "claude".into() } else { "off".into() }, vec![("claude".into(), "Claude Code".into()), ("off".into(), "Off: use the first message".into())], move |v, _, _| {
                            let value = if v == "claude" { json!({"provider": "claude"}) } else { json!({"provider": null}) };
                            l3.set_setting("titleGeneration", &value);
                        }),
                    ))
                    .when(titles_on, |s| {
                        let ws = wsd.clone();
                        s.child(row(
                            "Model",
                            picker(
                                "titles-model",
                                title_model,
                                vec![("".into(), "Default · Haiku".into()), ("haiku".into(), "Haiku".into()), ("sonnet".into(), "Sonnet".into())],
                                move |v, _, cx| {
                                    let lib = ws.read(cx).library.clone();
                                    let mut t = lib.setting("titleGeneration").filter(Value::is_object).unwrap_or(json!({"provider": "claude"}));
                                    t["model"] = if v.is_empty() { Value::Null } else { Value::from(v) };
                                    lib.set_setting("titleGeneration", &t);
                                },
                            ),
                        ))
                    })
                    .child(div().pt(px(4.)).text_color(theme.secondary).text_size(px(12.)).child("Title prompt"))
                    .child(div().p(px(6.)).rounded(px(6.)).bg(theme.card).child(Textarea::new(&title_prompt).appearance(false).h(px(110.))))
                    .child({
                        let (a, b) = (title_prompt.clone(), title_prompt.clone());
                        let (wa, wb) = (wse.clone(), wse.clone());
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(6.))
                            .child(Button::new("title-reset").small().ghost().label("Use Built-in").on_click(move |_, window, cx: &mut App| {
                                wa.read(cx).library.set_setting("titlePrompt", &Value::Null);
                                a.update(cx, |s, cx| s.set_value(yh_core::titles::DEFAULT_RULES, window, cx));
                            }))
                            .child(Button::new("title-save").small().outline().label("Save Prompt").on_click(move |_, _, cx| {
                                let text = b.read(cx).value().to_string();
                                let keep = text.trim().is_empty() || text.trim() == yh_core::titles::DEFAULT_RULES.trim();
                                wb.read(cx).library.set_setting("titlePrompt", &if keep { Value::Null } else { Value::from(text) });
                            }))
                    })
                    .child(self.toggle("agentUpdatesTitles", "Update titles when the subject changes", cx)),
            )
            .child(
                section("Memory")
                    .child(self.toggle("memoryEnabled", "Give agents the workspace's memory", cx))
                    .child(self.toggle("claudeCodeMemoryOff", "Turn off Claude Code's own memory files", cx))
                    .child(footer("Each project has its own memory, which every thread there starts with: an index of what's remembered and tools to search, read and save. With the second switch on, Claude Code's memory files are off in YourHarness's threads, so there's one memory, not two.", theme)),
            )
            .child(
                section("Plans")
                    .child(self.toggle("planChecklist", "Let Claude keep a plan checklist", cx))
                    .child(footer("For work with several steps, Claude lists them and ticks them off: the checklist shows in the thread and above the composer while it works.", theme)),
            )
            .child(
                section("Browser")
                    .child(self.toggle("browserTools", "Let Claude use the browser", cx))
                    .child(self.toggle("browserAutoShow", "Show the browser when Claude opens a page", cx))
                    .child(footer("Claude opens, reads, clicks and types in a browser built into the app, to test what it builds; you watch in the sidebar and can take control. Each project keeps its own cookies.", theme)),
            )
            .child(
                section("Diagrams")
                    .child(self.toggle("diagramInstructions", "Let agents draw diagrams", cx))
                    .child(footer("Mermaid flowcharts and sequence diagrams are always drawn. With this on, agents are also told they can use them (about 150 tokens per session).", theme)),
            )
            .child(
                section("Data")
                    .child(row(
                        "Everything the app keeps",
                        div()
                            .flex()
                            .gap(px(6.))
                            .child(Button::new("data-reveal").small().ghost().label("Show in Finder").on_click(move |_, _, cx| cx.reveal_path(&data_dir)))
                            .child(Button::new("state-export").small().outline().label("Export State…").on_click({
                                let ws = self.workspace.clone();
                                move |_, _, cx| crate::ui::state_transfer::export(ws.clone(), cx)
                            }))
                            .child(Button::new("state-import").small().outline().label("Import State…").on_click({
                                let ws = self.workspace.clone();
                                move |_, window, cx| crate::ui::state_transfer::import(ws.clone(), window, cx)
                            })),
                    ))
                    .child(footer("Export State writes every project, thread, memory and setting into one file: for another Mac, or for reproducing a bug exactly. Import State replaces everything you have with a file's, after asking.", theme)),
            )
            .into_any_element()
    }

    fn mcp(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let mcp = cx.try_global::<SettingsLinks>().map(|l| l.mcp.clone());
        section("MCP Servers")
            .child(row(
                "Servers that apply in every project",
                Button::new("settings-mcp").small().outline().label("MCP Servers…").on_click(move |_, window, cx| {
                    window.close_dialog(cx);
                    if let Some(m) = mcp.clone() {
                        m.update(cx, |m, cx| m.show_everywhere(cx));
                    }
                }),
            ))
            .child(footer("Your servers for every project, and claude.ai connectors: sign in, add and remove. A project's own servers are in its threads' /mcp.", theme))
            .into_any_element()
    }

    fn releases(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let updater = cx.try_global::<SettingsLinks>().map(|l| l.updater.clone());
        let version_line: String = match updater.as_ref().map(|u| u.read(cx).phase.clone()) {
            Some(crate::state::updater::Phase::UpToDate) => format!("Version {} · up to date", env!("CARGO_PKG_VERSION")),
            Some(crate::state::updater::Phase::Available(_)) => "A new version is out (see the sidebar)".into(),
            Some(crate::state::updater::Phase::Checking) => "Checking…".into(),
            _ => format!("This is version {}", env!("CARGO_PKG_VERSION")),
        };
        let mut notes = section("What's New");
        for (version, body) in changelog(3) {
            notes = notes
                .child(div().pt(px(6.)).font_weight(FontWeight::SEMIBOLD).child(version))
                .children(body.into_iter().map(|line| div().text_size(px(12.)).text_color(if line.starts_with("###") { theme.text } else { theme.secondary }).child(line.trim_start_matches("### ").trim_start_matches("- ").to_string())));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(section("YourHarness").child(row_owned(
                version_line,
                Button::new("settings-updates").small().outline().label("Check for Updates").on_click(move |_, _, cx| {
                    if let Some(u) = updater.clone() {
                        u.update(cx, |u, cx| u.check(cx));
                    }
                }),
            )))
            .child(notes)
            .into_any_element()
    }
}

/// The newest `n` released versions in CHANGELOG.md, as (heading, lines).
fn changelog(n: usize) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = vec![];
    for line in include_str!("../../../../CHANGELOG.md").lines() {
        if let Some(rest) = line.strip_prefix("## [") {
            if rest.starts_with("Unreleased") {
                out.push((String::new(), vec![]));
                continue;
            }
            out.push((rest.replacen("] - ", " · ", 1).trim_end_matches(']').to_string(), vec![]));
        } else if let Some((_, lines)) = out.last_mut() {
            if !line.trim().is_empty() {
                lines.push(line.to_string());
            }
        }
    }
    out.into_iter().filter(|(v, _)| !v.is_empty()).take(n).collect()
}

fn update_keep(ws: &Entity<Workspace>, cx: &mut App, change: impl FnOnce(&mut crate::keep_awake::Settings)) {
    let mut k = crate::keep_awake::Settings::from(ws.read(cx).library.setting("keepAwake"));
    change(&mut k);
    ws.update(cx, |ws, cx| ws.configure_keep_awake(k, cx));
}

fn section(title: &'static str) -> gpui_kit::Div {
    div().flex().flex_col().gap(px(4.)).child(div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).child(title))
}

fn row(label: &'static str, control: impl IntoElement) -> AnyElement {
    row_owned(label.to_string(), control)
}

fn row_owned(label: String, control: impl IntoElement) -> AnyElement {
    div().flex().items_center().justify_between().gap(px(16.)).py(px(3.)).child(div().flex_shrink_0().child(label)).child(control).into_any_element()
}

fn footer(text: &'static str, theme: &Theme) -> AnyElement {
    div().text_size(px(11.)).text_color(theme.secondary).child(text).into_any_element()
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let tab_button = |id: &'static str, label: &'static str, tab: Tab, cx: &mut Context<Self>| {
            let b = Button::new(id).small().label(label);
            let b = if self.tab == tab { b.primary() } else { b.ghost() };
            b.on_click(cx.listener(move |this, _, _, cx| {
                this.tab = tab;
                cx.notify();
            }))
        };
        let body = match self.tab {
            Tab::Agents => self.agents(&theme, cx),
            Tab::General => self.general(&theme, cx),
            Tab::Mcp => self.mcp(&theme, cx),
            Tab::Releases => self.releases(&theme, cx),
        };
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .text_size(px(13.))
            .child(
                div()
                    .flex()
                    .justify_center()
                    .gap(px(4.))
                    .child(tab_button("settings-tab-agents", "Agents", Tab::Agents, cx))
                    .child(tab_button("settings-tab-general", "General", Tab::General, cx))
                    .child(tab_button("settings-tab-mcp", "MCP", Tab::Mcp, cx))
                    .child(tab_button("settings-tab-releases", "Releases", Tab::Releases, cx)),
            )
            .child(div().id("settings-body").max_h(px(560.)).overflow_y_scroll().pr(px(6.)).child(body))
    }
}

/// Opens Settings on General.
pub fn open(workspace: Entity<Workspace>, window: &mut Window, cx: &mut App) {
    open_on(workspace, Tab::General, window, cx);
}

pub fn open_on(workspace: Entity<Workspace>, tab: Tab, window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| SettingsView::new(workspace, tab, window, cx));
    window.open_dialog(cx, move |dialog, _, _| dialog.title("Settings").w(px(620.)).child(view.clone()));
}
