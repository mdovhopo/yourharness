//! Settings (⌘,), five tabs: Agents (where `claude` is, its account, `gh` and
//! `glab`, the agents still to come), General (appearance, notifications, sleep, new thread
//! defaults, settling, titles, memory, plans, browser, diagrams, Home and data), MCP, Phone
//! (phones on this network, pairing by QR code) and Releases.
//! Each change is saved at once, in the settings keys 0.0.x used, and takes effect at once or
//! with each thread's next `claude`.

use crate::state::workspace::Workspace;
use crate::theme::{radius, space, text, Theme, MONO};
use crate::ui::icons;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable, Icon, IconName, Sizable};
use gpui_kit::{div, prelude::*, px, AnyElement, App, Context, Entity, FontWeight, IntoElement, RenderOnce, SharedString, Window};
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

/// What Settings shows from the main window: the updater and its release notes.
pub struct SettingsLinks {
    pub updater: Entity<crate::state::updater::Updater>,
    /// The release notes view the popovers show.
    pub notes: Entity<crate::ui::updates::ReleaseNotesView>,
}

impl gpui_kit::Global for SettingsLinks {}

#[derive(Clone, Copy, PartialEq)]
pub enum Tab {
    Agents,
    General,
    Mcp,
    Phone,
    Releases,
}

impl Tab {
    pub fn from_name(name: &str) -> Option<Tab> {
        Some(match name {
            "agents" => Tab::Agents,
            "general" => Tab::General,
            "mcp" => Tab::Mcp,
            "phone" => Tab::Phone,
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
    /// The body's scroll, which the title prompt hands the wheel on to at its ends.
    body_scroll: gpui_kit::ScrollHandle,
    /// Settings → MCP: the servers everywhere, the `/mcp` sheet's list drawn in place.
    /// Made when the tab shows and let go when it doesn't, so it only asks while it's seen.
    mcp: Option<Entity<crate::ui::mcp::McpSheet>>,
    /// "Copied" for a moment after Copy in the sign-in hint.
    copied: bool,
    _watch: Vec<gpui_kit::Subscription>,
}

impl SettingsView {
    fn new(workspace: Entity<Workspace>, tab: Tab, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let rules = workspace.read(cx).title_prompt().unwrap_or_else(|| yh_core::titles::DEFAULT_RULES.to_string());
        // No bounce at its ends: there the wheel goes on to the page (`page_kit::wheel_passes_to_page`).
        let title_prompt = cx.new(|cx| TextareaState::new(window, cx).default_value(rules).scroll_bounce(false));
        crate::platform::refresh_notification_status();
        let mut watch = vec![cx.observe(&workspace, |_, _, cx| cx.notify())];
        if let Some(u) = cx.try_global::<SettingsLinks>().map(|l| l.updater.clone()) {
            watch.push(cx.observe(&u, |_, _, cx| cx.notify()));
        }
        let mut view = SettingsView { workspace, tab, found: Found::default(), title_prompt, body_scroll: gpui_kit::ScrollHandle::new(), mcp: None, copied: false, _watch: watch };
        view.detect(cx);
        view.tab_shown(window, cx);
        view
    }

    /// What a tab needs when it shows: MCP's servers asked for, Releases' notes read.
    fn tab_shown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.tab {
            Tab::Mcp => {
                if self.mcp.is_none() {
                    let ws = self.workspace.clone();
                    self.mcp = Some(cx.new(|cx| crate::ui::mcp::McpSheet::new_embedded(ws, window, cx)));
                }
            }
            Tab::Releases => {
                if let Some(u) = cx.try_global::<SettingsLinks>().map(|l| l.updater.clone()) {
                    u.update(cx, |u, cx| u.load_notes(cx));
                }
            }
            // Drawn again now and then while it shows: a pairing code goes out of date, phones
            // are seen.
            Tab::Phone => {
                cx.spawn(async move |this, cx| loop {
                    cx.background_executor().timer(std::time::Duration::from_secs(30)).await;
                    let Ok(on) = this.update(cx, |v, cx| {
                        cx.notify();
                        v.tab == Tab::Phone
                    }) else {
                        break;
                    };
                    if !on {
                        break;
                    }
                })
                .detach();
            }
            _ => {}
        }
        if self.tab != Tab::Mcp {
            self.mcp = None;
        }
    }

    pub fn set_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab != tab {
            self.tab = tab;
            self.tab_shown(window, cx);
            cx.notify();
        }
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
            statuses.push(("git", yh_core::agents::probe("git", None)));
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
                    .child(div().text_color(theme.secondary).font_family(MONO).text_size(px(text::FOOTNOTE)).truncate().child(claude.as_ref().and_then(|s| s.path.clone()).map_or("Not found".into(), |p| p.display().to_string())))
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
                    .gap(px(space::S))
                    .px(px(space::M))
                    .py(px(space::S))
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
            claude_section = claude_section.footer("Install Claude Code from docs.claude.com, then Detect Again.");
        }
        let account: Group = match &self.found.account {
            Some(a) if a.logged_in => section("Account")
                .when_some(a.email.clone(), |s, e| s.child(row("Signed in as", div().text_color(theme.secondary).child(e))))
                .when_some(a.plan_name(), |s, p| s.child(row("Plan", div().px(px(8.)).rounded_full().bg(theme.accent.opacity(0.15)).font_weight(FontWeight::MEDIUM).child(p))))
                .when_some(a.org_name.clone(), |s, o| s.child(row("Organization", div().text_color(theme.secondary).child(o))))
                .when_some(a.sign_in_method(), |s, m| s.child(row("Signed in with", div().text_color(theme.secondary).child(m))))
                .child(self.usage_bars(theme, cx)),
            Some(_) => section("Account").child(self.sign_in_hint(theme, cx)),
            None if claude.as_ref().is_some_and(Status::is_ready) && self.found.refreshing => section("Account").child(row("Account", div().text_color(theme.secondary).child("Checking…"))),
            None if claude.as_ref().is_some_and(Status::is_ready) => section("Account").child(row("Account", div().text_color(theme.secondary).child("Couldn't read sign-in status"))),
            None => Group::untitled(),
        };
        let account = (!account.is_empty()).then_some(account);
        // Who `gh` and `glab` are signed in as, from the last look at a project's PRs.
        let accounts: std::collections::BTreeMap<yh_core::git::Forge, String> = {
            let ws = self.workspace.read(cx);
            ws.scans.values().flat_map(|w| w.pulls.accounts.iter().map(|(f, a)| (*f, a.clone()))).collect()
        };
        let tool = |id: &'static str, name: &'static str, url: &'static str, account: Option<String>| {
            let status = self.status(id);
            let missing = status.is_some_and(|s| s.path.is_none() && !s.is_ready());
            row(
                name,
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .min_w_0()
                    .when_some(account.clone(), |d, a| {
                        d.child(div().id(SharedString::from(format!("{id}-account"))).px(px(6.)).rounded(px(radius::SMALL)).bg(theme.accent.opacity(0.15)).text_color(theme.accent).text_size(px(text::CAPTION)).child(a))
                    })
                    .child(div().text_color(theme.secondary).child(status.map_or_else(checking, Status::describe)))
                    .when(missing, |d| d.child(Button::new(SharedString::from(format!("{id}-get"))).small().ghost().label("Get").on_click(move |_, _, cx| cx.open_url(url)))),
            )
        };
        let git = section("Git & Code Hosting")
            .child(tool("git", "Git", "https://git-scm.com/download/mac", None))
            .child(tool("gh", "GitHub CLI (gh)", "https://cli.github.com", accounts.get(&yh_core::git::Forge::GitHub).cloned()))
            .child(tool("glab", "GitLab CLI (glab)", "https://gitlab.com/gitlab-org/cli", accounts.get(&yh_core::git::Forge::GitLab).cloned()))
            .footer(match yh_core::shell_env::outcome() {
                // How the tools are found: the shell's PATH, or the usual folders when it didn't answer.
                Some(how) => format!("Pull requests come from `gh` and `glab`, signed in as you. YourHarness never sees their tokens. Environment: {how}"),
                None => "Pull requests come from `gh` and `glab`, signed in as you. YourHarness never sees their tokens.".to_string(),
            });
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
        others = others.footer("Support for these is coming. YourHarness detects them now, so they're ready when it lands.");
        let refreshing = self.found.refreshing;
        div()
            .flex()
            .flex_col()
            .gap(px(space::L + 4.))
            .child(claude_section)
            .when_some(account, |d, a| d.child(a))
            .child(git)
            .child(others)
            .child(Group::untitled().child(row(
                "Installed or signed in just now?",
                Button::new("detect-again").small().outline().label(if refreshing { "Detecting…" } else { "Detect Again" }).disabled(refreshing).on_click(cx.listener(|this, _, _, cx| {
                    this.detect(cx);
                    cx.notify();
                })),
            )))
            .into_any_element()
    }

    /// Not signed in: how to, in Terminal (`claude auth login`), with Copy and Open Terminal.
    fn sign_in_hint(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        const COMMAND: &str = "claude auth login";
        div()
            .flex()
            .flex_col()
            .gap(px(space::S))
            .px(px(space::M))
            .py(px(space::S + 2.))
            .child(div().flex().items_center().gap(px(6.)).text_color(theme.warning).child(Icon::new(IconName::CircleUser).size(px(14.)).text_color(theme.warning)).child("Not signed in"))
            .child(div().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child("Sign in once in Terminal and YourHarness uses that login. It never sees your credentials."))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S))
                    .child(div().px(px(8.)).py(px(3.)).rounded(px(radius::SMALL + 1.)).bg(theme.selected).font_family(MONO).text_size(px(text::CODE)).child(COMMAND))
                    .child(Button::new("signin-copy").small().outline().label(if self.copied { "Copied" } else { "Copy" }).on_click(cx.listener(|this, _, _, cx| {
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(COMMAND.to_string()));
                        this.copied = true;
                        cx.notify();
                    })))
                    .child(Button::new("signin-terminal").small().outline().label("Open Terminal").on_click(|_, _, _| {
                        let _ = std::process::Command::new("open").args(["-a", "Terminal"]).spawn();
                    })),
            )
            .into_any_element()
    }

    /// The account's usage limits, as the sidebar's meters read them.
    fn usage_bars(&self, theme: &Theme, cx: &App) -> AnyElement {
        let ws = self.workspace.read(cx);
        let meters: Vec<(String, yh_core::limits::Window)> = yh_core::limits::meters(ws.limits.as_ref()).into_iter().filter_map(|(n, w)| w.map(|w| (n, w))).collect();
        if meters.is_empty() {
            return row("Usage", div().text_color(theme.secondary).child("Appears after your first message"));
        }
        let now = yh_core::library::now_ms() / 1000.;
        let mut list = div().flex().flex_col().gap(px(space::S + 2.)).px(px(space::M)).py(px(space::S + 2.));
        for (name, w) in meters {
            let used = yh_core::limits::utilization(&w, now);
            let tint = if used < 0.7 { theme.accent } else if used < 0.9 { theme.warning } else { theme.error };
            let reset = w.resets_at.filter(|at| *at > now).map(|at| format!("Resets {}", yh_core::clock::relative(at, now))).unwrap_or_default();
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(div().flex().items_center().child(div().flex_1().child(yh_core::limits::title(&name))).child(div().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(format!("{}% used", (used * 100.).round() as i64))))
                    .child(div().h(px(6.)).rounded_full().bg(theme.selected).child(div().h_full().rounded_full().min_w(px(6.)).w(gpui_kit::relative(used as f32)).bg(tint)))
                    .when(!reset.is_empty(), |d| d.child(div().text_size(px(text::CAPTION)).text_color(theme.tertiary).child(reset))),
            );
        }
        list.into_any_element()
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
        let default_home = ws.default_home_folder();
        let home_moved = home != default_home;
        let title_error = ws.title_error.clone();
        let mut title_models: Vec<(String, String)> = vec![("".into(), "Default · Haiku".into())];
        title_models.extend(ws.models.iter().filter(|m| m.value != "default" && m.value != "haiku").map(|m| (m.value.clone(), m.display_name.clone())));
        if title_models.len() == 1 {
            title_models.extend([("haiku".to_string(), "Haiku".to_string()), ("sonnet".to_string(), "Sonnet".to_string())]);
        }
        let mut agent_models: Vec<(String, String)> = vec![("".into(), "Same as new threads".into())];
        agent_models.extend(ws.models.iter().map(|m| (m.value.clone(), m.display_name.clone())));
        let agent_model = lib.setting("memoryAgentModel").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        let agent_effort = lib.setting("memoryAgentEffort").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        let memory_on = lib.setting("memoryEnabled").and_then(|v| v.as_bool()).unwrap_or(true);
        let config = yh_core::history::config_directory();
        let off_everywhere = yh_core::memory_archive::claude_code::setting("/nonexistent-folder", &config).off_everywhere;
        let data_dir = ws.data_dir.clone();
        let (wsa, wsb, wsc, wsd, wse) = (self.workspace.clone(), self.workspace.clone(), self.workspace.clone(), self.workspace.clone(), self.workspace.clone());
        let (k2, k3, k4) = (wsa.clone(), wsa.clone(), wsa);
        let denied = crate::platform::notifications_denied();
        let (l1, l2, l3) = (lib.clone(), lib.clone(), lib.clone());
        let title_prompt = self.title_prompt.clone();
        let body_scroll = self.body_scroll.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(space::L + 4.))
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
                    .footer("System follows macOS, including its automatic switch between light and dark."),
            )
            .child(
                section("Notifications")
                    .child(self.toggle("notifications", "Notify when Claude finishes or needs you", cx))
                    .when(denied, |s| {
                        s.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(space::S))
                                .px(px(space::M))
                                .py(px(space::S))
                                .text_color(theme.warning)
                                .text_size(px(text::FOOTNOTE))
                                .child(Icon::new(IconName::TriangleAlert).size(px(14.)).text_color(theme.warning))
                                .child(div().flex_1().child("Notifications are off for YourHarness in System Settings."))
                                .child(Button::new("notifications-settings").small().outline().label("Open System Settings").on_click(|_, _, _| crate::platform::open_notification_settings())),
                        )
                    })
                    .footer("Only for threads you aren't looking at: a turn that finishes while YourHarness is in the background, and Claude asking for approval, an answer or a plan review. The Dock icon counts threads waiting for you."),
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
                    .footer("The cup in the sidebar keeps it awake for a while on its own."),
            )
            .child(
                section("New Thread Defaults")
                    .child(row("Model", picker("settings-model", s("model"), models, set_default("model"))))
                    .child(row("Reasoning effort", picker("settings-effort", s("effort"), efforts.clone(), set_default("effort"))))
                    .child(row("Permissions for new projects", picker("settings-mode", if s("permissionMode").is_empty() { "auto".into() } else { s("permissionMode") }, modes, set_default("permissionMode"))))
                    .footer("Each thread can override these from the composer. Threads that haven't overridden them follow these defaults."),
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
                    .footer("Settled threads move to a shelf at the end of their project and stop their Claude process; the conversation stays. Threads that are working or waiting on you never settle.")
                    .child(row(
                        "Home",
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .min_w_0()
                            .child(div().text_color(theme.secondary).font_family(MONO).text_size(px(text::FOOTNOTE)).truncate().child(home))
                            .when(home_moved, |d| {
                                let ws = self.workspace.clone();
                                let to = default_home.clone();
                                d.child(Button::new("home-reset").small().ghost().label("Reset").tooltip("Back to your home folder").on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.set_home_folder(&to, cx))))
                            })
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
                    .footer("New threads that aren't in a project run here."),
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
                                title_models,
                                move |v, _, cx| {
                                    let lib = ws.read(cx).library.clone();
                                    let mut t = lib.setting("titleGeneration").filter(Value::is_object).unwrap_or(json!({"provider": "claude"}));
                                    t["model"] = if v.is_empty() { Value::Null } else { Value::from(v) };
                                    lib.set_setting("titleGeneration", &t);
                                },
                            ),
                        ))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(space::S))
                            .px(px(space::M))
                            .pt(px(space::S + 2.))
                            .child("Title prompt")
                            .child(
                                div()
                                    .p(px(space::S))
                                    .rounded(px(radius::MEDIUM))
                                    .bg(theme.canvas)
                                    .border_1()
                                    .border_color(theme.separator)
                                    .font_family(MONO)
                                    .text_size(px(text::CODE))
                                    .child(crate::ui::page_kit::wheel_passes_to_page(&title_prompt, &body_scroll, Textarea::new(&title_prompt).appearance(false).h(px(110.)))),
                            ),
                    )
                    .inline({
                        let (a, b) = (title_prompt.clone(), title_prompt.clone());
                        let (wa, wb) = (wse.clone(), wse.clone());
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(space::S))
                            .px(px(space::M))
                            .pt(px(space::S))
                            .pb(px(space::S + 2.))
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
                    .child(self.toggle("agentUpdatesTitles", "Update titles when the subject changes", cx))
                    .when_some(title_error.filter(|_| titles_on), |s, e| {
                        s.inline(div().flex().items_center().gap(px(6.)).px(px(space::M)).pb(px(space::S + 2.)).text_size(px(text::FOOTNOTE)).text_color(theme.warning).child(Icon::new(IconName::TriangleAlert).size(px(13.)).text_color(theme.warning)).child(format!("Last title failed: {e}")))
                    }),
            )
            .child(
                section("Memory")
                    .child(self.toggle("memoryEnabled", "Give agents the workspace's memory", cx))
                    .child(self.toggle("claudeCodeMemoryOff", "Turn off Claude Code's own memory files", cx))
                    .child(row(
                        "Claude Code's memory outside the app",
                        if off_everywhere {
                            div().text_color(theme.secondary).child("Off for every folder").into_any_element()
                        } else {
                            let settings_file = config.join("settings.json");
                            let shown = match std::env::var("HOME") {
                                Ok(h) if !h.is_empty() => settings_file.to_string_lossy().replacen(&h, "~", 1),
                                _ => settings_file.to_string_lossy().into_owned(),
                            };
                            let ws = self.workspace.clone();
                            Button::new("memory-off-everywhere").small().outline().label("Turn Off Everywhere…").disabled(!memory_on).on_click(move |_, window, cx| {
                                let detail = format!("Sets autoMemoryEnabled to false in your Claude Code settings ({shown}), so claude keeps no memory files of its own in any folder, in YourHarness or in Terminal. YourHarness keeps the memory. Its existing files are left as they are; import them from a project's Memory page.");
                                let title = "Turn off Claude Code's memory everywhere?";
                                let (file, ws) = (settings_file.clone(), ws.clone());
                                let turn_off = move |cx: &mut App| {
                                    match yh_core::memory_archive::claude_code::turn_off(&file) {
                                        Ok(_) => eprintln!("MEMORY Claude Code's memory off everywhere"),
                                        Err(e) => eprintln!("MEMORY couldn't turn off Claude Code's memory: {e}"),
                                    }
                                    ws.update(cx, |ws, cx| ws.memory_changed(cx));
                                };
                                if let Ok(answer) = std::env::var("YH_MEMORY_OFF_ANSWER") {
                                    eprintln!("MEMORY asks: {title} / {detail} -> {answer}");
                                    if answer == "ok" {
                                        turn_off(cx);
                                    }
                                    return;
                                }
                                let answer = window.prompt(gpui_kit::PromptLevel::Warning, title, Some(&detail), &["Turn Off", "Cancel"], cx);
                                cx.spawn(async move |cx| {
                                    if answer.await == Ok(0) {
                                        let _ = cx.update(turn_off);
                                    }
                                })
                                .detach();
                            })
                            .into_any_element()
                        },
                    ))
                    .child(row(
                        "Importer and Reconciler",
                        div()
                            .flex()
                            .gap(px(space::XS + 2.))
                            .child(picker("memory-agent-model", agent_model, agent_models, {
                                let lib = lib.clone();
                                move |v, _, _| lib.set_setting("memoryAgentModel", &if v.is_empty() { Value::Null } else { Value::from(v) })
                            }))
                            .child(picker("memory-agent-effort", agent_effort, efforts.clone(), {
                                let lib = lib.clone();
                                move |v, _, _| lib.set_setting("memoryAgentEffort", &if v.is_empty() { Value::Null } else { Value::from(v) })
                            })),
                    ))
                    .footer("Each project has its own memory, which every thread there starts with: an index of what's remembered and tools to search, read and save. With the second switch on, Claude Code's memory files are off in YourHarness's threads, so there's one memory, not two."),
            )
            .child(
                section("Plans")
                    .child(self.toggle("planChecklist", "Let Claude keep a plan checklist", cx))
                    .footer("For work with several steps, Claude lists them and ticks them off: the checklist shows in the thread and above the composer while it works."),
            )
            .child(
                section("Browser")
                    .child(self.toggle("browserTools", "Let Claude use the browser", cx))
                    .child(self.toggle("browserAutoShow", "Show the browser when Claude opens a page", cx))
                    .footer("Claude opens, reads, clicks and types in a browser built into the app, to test what it builds; you watch in the sidebar and can take control. Each project keeps its own cookies."),
            )
            .child(
                section("Diagrams")
                    .child(self.toggle("diagramInstructions", "Let agents draw diagrams", cx))
                    .footer("Mermaid flowcharts and sequence diagrams are always drawn. With this on, agents are also told they can use them (about 150 tokens per session)."),
            )
            .child(
                section("Data")
                    .child(row(
                        "Everything the app keeps",
                        div()
                            .flex()
                            .gap(px(space::XS + 2.))
                            .child(Button::new("data-reveal").small().ghost().label("Show in Finder").on_click(move |_, _, cx| cx.reveal_path(&data_dir)))
                            .child(Button::new("state-export").small().outline().label("Export State…").on_click({
                                let ws = self.workspace.clone();
                                move |_, window, cx| crate::ui::state_transfer::export(ws.clone(), window, cx)
                            }))
                            .child(Button::new("state-import").small().outline().label("Import State…").on_click({
                                let ws = self.workspace.clone();
                                move |_, window, cx| crate::ui::state_transfer::import(ws.clone(), window, cx)
                            })),
                    ))
                    .footer("Export State writes every project, thread, memory and setting into one file: for another Mac, or for reproducing a bug exactly. Import State replaces everything you have with a file's, after asking."),
            )
            .into_any_element()
    }

    fn mcp(&self, theme: &Theme, _cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(space::S))
            .children(self.mcp.clone())
            .child(div().px(px(space::XS)).text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child("Your servers for every project, and claude.ai connectors: sign in, add and remove. A project's own servers are in its threads' /mcp."))
            .into_any_element()
    }

    /// Phones on this network: on or off, where they connect, a QR code to pair one, the phones
    /// paired.
    fn phone(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let on = ws.phones_on();
        let running = ws.phones.running();
        let name = crate::state::workspace::mac_name();
        let link = ws.phones.pairing_link(&name);
        let port = ws.phones.port;
        let address = match (&ws.phones.error, ws.phones.addresses.first()) {
            (Some(e), _) if on => e.clone(),
            _ if !running => "Off".into(),
            (_, Some(ip)) => format!("{ip}:{port}"),
            (_, None) => "Not on a network".into(),
        };
        let mut devices = ws.phones.pairing.devices();
        devices.sort_by(|a, b| b.paired_at.total_cmp(&a.paired_at));
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.);
        let switch_ws = self.workspace.clone();
        let mut page = div().flex().flex_col().gap(px(space::L)).child(
            section("Phones on This Network")
                .child(row(
                    "Let phones connect",
                    Switch::new("phones-enabled").checked(on).on_change(move |on, _, cx| {
                        let on = *on;
                        switch_ws.update(cx, |ws, cx| ws.set_phones(on, cx));
                    }),
                ))
                .child(row("Address", div().text_color(if ws.phones.error.is_some() && on { theme.warning } else { theme.secondary }).child(address)))
                .footer("Your iPhone or iPad on the same Wi‑Fi follows and runs your threads, with YourHarness on the phone. What they send isn't encrypted on the network yet: use it on networks you trust."),
        );
        if let Some(link) = link {
            let new_ws = self.workspace.clone();
            page = page.child(
                section("Pair a Phone")
                    .inline(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(space::S))
                            .py(px(space::M))
                            .child(qr_code(&link, 196.))
                            .child(div().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(format!("Pairs with {name}"))),
                    )
                    .child(row(
                        "Code",
                        Button::new("phones-new-code").small().outline().label("New Code").on_click(move |_, _, cx| {
                            new_ws.update(cx, |ws, cx| ws.new_pairing_code(cx));
                        }),
                    ))
                    .footer("Scan it with the phone's camera, or in YourHarness on the phone: Macs, then Scan. The code works once, for 10 minutes; each phone gets a key of its own."),
            );
        }
        let mut paired = section("Paired Phones");
        if devices.is_empty() {
            paired = paired.inline(div().px(px(space::M)).py(px(space::S + 2.)).text_color(theme.secondary).child("No phones paired yet."));
        }
        for d in devices {
            let forget_ws = self.workspace.clone();
            let id = d.id.clone();
            let seen = d.last_seen.map(|t| format!(" · seen {}", yh_core::clock::relative(t, now))).unwrap_or_default();
            paired = paired.child(row_owned(
                d.name.clone(),
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S))
                    .child(div().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(format!("paired {}{seen}", yh_core::clock::relative(d.paired_at, now))))
                    .child(Button::new(gpui_kit::ElementId::Name(format!("forget-{}", d.id).into())).small().ghost().label("Forget").on_click(move |_, _, cx| {
                        let id = id.clone();
                        forget_ws.update(cx, |ws, cx| ws.forget_phone(&id, cx));
                    })),
            ));
        }
        page.child(paired.footer("A phone forgotten here can't connect again until it's paired again.")).into_any_element()
    }

    fn releases(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let Some(links) = cx.try_global::<SettingsLinks>() else { return div().into_any_element() };
        let (updater, notes) = (links.updater.clone(), links.notes.clone());
        let u = updater.read(cx);
        let (label, build) = (u.version_label(), u.build.clone());
        div()
            .flex()
            .flex_col()
            .gap(px(space::L))
            .child(
                Group::untitled()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::M))
                            .px(px(space::M))
                            .py(px(space::M))
                            .child(div().size(px(44.)).rounded(px(radius::LARGE - 2.)).bg(theme.accent_soft).flex().items_center().justify_center().child(icons::bot().size(px(24.)).text_color(theme.accent)))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(div().text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).child(format!("YourHarness {label}")))
                                    .when_some(build, |d, b| d.child(div().text_size(px(text::CAPTION)).text_color(theme.secondary).child(format!("Build {b}")))),
                            ),
                    )
                    .child(div().px(px(space::M)).py(px(space::S)).child(crate::ui::updates::status(&updater, &self.workspace, &notes, None, theme, cx)))
                    .footer("Looks for a new release on GitHub at launch and every few hours. Updates download only when you ask, and apply when you restart."),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .pt(px(space::XS))
                    .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child("Release Notes"))
                    .child(div().id("settings-all-releases").text_color(theme.accent).text_size(px(text::FOOTNOTE)).child("All Releases on GitHub").on_click(|_, _, cx| cx.open_url(yh_core::updates::RELEASES_PAGE))),
            )
            .child(crate::ui::updates::all_notes(&updater, theme, cx))
            .into_any_element()
    }
}

fn update_keep(ws: &Entity<Workspace>, cx: &mut App, change: impl FnOnce(&mut crate::keep_awake::Settings)) {
    let mut k = crate::keep_awake::Settings::from(ws.read(cx).library.setting("keepAwake"));
    change(&mut k);
    ws.update(cx, |ws, cx| ws.configure_keep_awake(k, cx));
}

/// A group of settings as System Settings draws one: its title above, its rows in a rounded
/// box with hairlines between them, what it means in a footnote under them.
#[derive(IntoElement)]
struct Group {
    title: Option<&'static str>,
    /// Each item, and whether a hairline goes above it (rows) or not (parts of the row above).
    items: Vec<(AnyElement, bool)>,
    footers: Vec<SharedString>,
}

impl Group {
    fn untitled() -> Group {
        Group { title: None, items: vec![], footers: vec![] }
    }

    fn is_empty(&self) -> bool {
        self.items.is_empty() && self.footers.is_empty()
    }

    /// A row, under a hairline.
    fn child(mut self, item: impl IntoElement) -> Group {
        self.items.push((item.into_any_element(), true));
        self
    }

    /// More of the row above, no hairline between.
    fn inline(mut self, item: impl IntoElement) -> Group {
        self.items.push((item.into_any_element(), false));
        self
    }

    fn footer(mut self, text: impl Into<SharedString>) -> Group {
        self.footers.push(text.into());
        self
    }
}

impl RenderOnce for Group {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::get(cx);
        let has_rows = !self.items.is_empty();
        let mut card = div().flex().flex_col().rounded(px(radius::LARGE - 2.)).bg(theme.card).border_1().border_color(theme.separator).overflow_hidden();
        for (i, (item, ruled)) in self.items.into_iter().enumerate() {
            card = card.child(div().when(i > 0 && ruled, |d| d.border_t_1().border_color(theme.separator)).child(item));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(space::S - 2.))
            .when_some(self.title, |d, t| d.child(div().px(px(space::XS)).text_size(px(text::FOOTNOTE)).font_weight(FontWeight::SEMIBOLD).text_color(theme.secondary).child(t)))
            .when(has_rows, |d| d.child(card))
            .children(self.footers.into_iter().map(move |f| div().px(px(space::XS)).text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(f)))
    }
}

fn section(title: &'static str) -> Group {
    Group { title: Some(title), items: vec![], footers: vec![] }
}

fn row(label: &'static str, control: impl IntoElement) -> AnyElement {
    row_owned(label.to_string(), control)
}

fn row_owned(label: String, control: impl IntoElement) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(space::L))
        .min_h(px(40.))
        .px(px(space::M))
        .py(px(space::XS + 2.))
        .child(div().flex_shrink_0().child(label))
        .child(div().min_w_0().flex().justify_end().child(control))
        .into_any_element()
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        // A segmented control, each tab with its icon.
        let current = self.tab;
        // The chosen segment stands up from the track: white in light, a lighter grey in dark.
        let raised = if theme.canvas.l < 0.5 { theme.bubble } else { theme.canvas };
        let tab_button = |id: &'static str, icon: Icon, label: &'static str, tab: Tab, cx: &mut Context<Self>| {
            let on = current == tab;
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(space::XS + 2.))
                .h(px(26.))
                .px(px(space::M))
                .rounded(px(radius::MEDIUM - 1.))
                .text_color(if on { theme.text } else { theme.secondary })
                .when(on, |d| d.bg(raised).border_1().border_color(theme.separator).font_weight(FontWeight::MEDIUM))
                .when(!on, |d| d.hover(|s| s.bg(theme.hover)))
                .child(icon.size(px(14.)).text_color(if on { theme.accent } else { theme.secondary }))
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| this.set_tab(tab, window, cx)))
        };
        let body = match self.tab {
            Tab::Agents => self.agents(&theme, cx),
            Tab::General => self.general(&theme, cx),
            Tab::Mcp => self.mcp(&theme, cx),
            Tab::Phone => self.phone(&theme, cx),
            Tab::Releases => self.releases(&theme, cx),
        };
        let _ = window;
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(space::L))
            .pt(px(space::M))
            .bg(theme.canvas)
            .text_color(theme.text)
            .font_family(crate::theme::UI)
            .text_size(px(text::BODY))
            .child(
                div().flex().justify_center().child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .p(px(2.))
                        .rounded(px(radius::MEDIUM + 1.))
                        .bg(theme.selected)
                        .child(tab_button("settings-tab-agents", icons::bot(), "Agents", Tab::Agents, cx))
                        .child(tab_button("settings-tab-general", Icon::new(IconName::Settings), "General", Tab::General, cx))
                        .child(tab_button("settings-tab-mcp", icons::plug(), "MCP", Tab::Mcp, cx))
                        .child(tab_button("settings-tab-phone", Icon::new(IconName::Inbox), "Phone", Tab::Phone, cx))
                        .child(tab_button("settings-tab-releases", icons::download(), "Releases", Tab::Releases, cx)),
                ),
            )
            // The body scrolls in what the window leaves it.
            .child(div().id("settings-body").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.body_scroll).px(px(space::XL)).pb(px(space::XL)).child(body))
    }
}

thread_local! {
    /// Settings' own window, while it's open.
    static WINDOW: std::cell::RefCell<Option<(gpui_kit::AnyWindowHandle, gpui_kit::WeakEntity<SettingsView>)>> = const { std::cell::RefCell::new(None) };
}

/// Where Settings' page and its title prompt stand, `down/max`, for `YH_WHEEL`'s report.
pub fn wheel_report(cx: &App) -> Option<String> {
    let view = WINDOW.with(|w| w.borrow().as_ref().map(|(_, v)| v.clone()))?.upgrade()?;
    let v = view.read(cx);
    let (page, max) = (v.body_scroll.offset(), v.body_scroll.max_offset());
    Some(format!("settings page {:.0}/{:.0}, title prompt {:.0}", -f32::from(page.y), f32::from(max.y), -f32::from(v.title_prompt.read(cx).scroll_offset().y)))
}

/// Opens Settings on General (⌘,), or brings its window forward.
pub fn open(workspace: Entity<Workspace>, window: &mut Window, cx: &mut App) {
    let _ = window;
    open_window(workspace, None, cx);
}

pub fn open_on(workspace: Entity<Workspace>, tab: Tab, window: &mut Window, cx: &mut App) {
    let _ = window;
    open_window(workspace, Some(tab), cx);
}

/// Whether Settings' window is open.
pub fn is_open() -> bool {
    WINDOW.with(|w| w.borrow().is_some())
}

/// Closes Settings' window as its close button does (the perf probe).
pub fn close_window(cx: &mut App) {
    let open = WINDOW.with(|w| w.borrow().clone());
    if let Some((handle, _)) = open {
        let _ = handle.update(cx, |_, window, _| crate::platform::perform_close(window));
    }
}

/// Settings in a window of its own: one at a time, brought
/// forward (on `tab`, when given) if it's open.
pub fn open_window(workspace: Entity<Workspace>, tab: Option<Tab>, cx: &mut App) {
    use gpui_kit::{size, Bounds, TitlebarOptions, WindowBounds, WindowOptions};
    let open = WINDOW.with(|w| w.borrow().clone());
    if let Some((handle, view)) = open {
        let shown = handle
            .update(cx, |_, window, cx| {
                window.activate_window();
                if let (Some(tab), Some(v)) = (tab, view.upgrade()) {
                    v.update(cx, |v, cx| v.set_tab(tab, window, cx));
                }
            })
            .is_ok();
        if shown {
            return;
        }
    }
    // A perf run opens it too, and must never take your focus.
    let background = std::env::var("YH_BACKGROUND").as_deref() == Ok("1") || std::env::var("YH_PERF").is_ok();
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(680.), px(640.)), cx))),
        titlebar: Some(TitlebarOptions { title: Some("Settings".into()), ..Default::default() }),
        focus: !background,
        show: true,
        window_min_size: Some(size(px(560.), px(400.))),
        ..Default::default()
    };
    let tab = tab.unwrap_or(Tab::General);
    let opened = gpui_kit::open_window(options, cx, move |window, cx| {
        if background {
            crate::platform::order_back(window);
        }
        crate::theme::apply_appearance(window.appearance(), window, cx);
        cx.new(|cx| SettingsView::new(workspace, tab, window, cx))
    });
    let Ok((handle, view)) = opened else { return eprintln!("SETTINGS couldn't open its window") };
    WINDOW.with(|w| *w.borrow_mut() = Some((handle, view.downgrade())));
    let _ = handle.update(cx, |_, window, cx| {
        crate::theme::apply_appearance(window.appearance(), window, cx);
        window
            .observe_window_appearance(|window, cx| {
                crate::theme::apply_appearance(window.appearance(), window, cx);
                cx.refresh_windows();
            })
            .detach();
        window.on_window_should_close(cx, |_, _| {
            WINDOW.with(|w| *w.borrow_mut() = None);
            true
        });
    });
    if std::env::var("YH_DATA_DIR").is_ok() {
        eprintln!("SETTINGS window opened");
    }
}

/// A QR code of `text`, `size` points square: dark squares on white with a quiet border, as
/// cameras read it in light or dark.
fn qr_code(text: &str, size: f32) -> impl IntoElement {
    let modules = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M).map(|q| (q.width(), q.to_colors())).ok();
    gpui_kit::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            use gpui_kit::{point, size as sz, Bounds};
            window.paint_quad(gpui_kit::fill(bounds, gpui_kit::white()));
            let Some((width, colors)) = &modules else { return };
            let quiet = 2.;
            let cell = f32::from(bounds.size.width) / (*width as f32 + quiet * 2.);
            for (i, c) in colors.iter().enumerate() {
                if *c != qrcode::Color::Dark {
                    continue;
                }
                let (x, y) = ((i % width) as f32 + quiet, (i / width) as f32 + quiet);
                let origin = point(bounds.origin.x + px(x * cell), bounds.origin.y + px(y * cell));
                // A hair over a cell, so neighbours meet without seams.
                window.paint_quad(gpui_kit::fill(Bounds { origin, size: sz(px(cell + 0.3), px(cell + 0.3)) }, gpui_kit::black()));
            }
        },
    )
    .size(px(size))
    .rounded(px(radius::MEDIUM))
}
