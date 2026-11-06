//! The composer and the cards above it. ↩ sends; while Claude works it steers (joins the turn
//! after the current step), ⌘↩ stops the turn and sends now. A draft stays with its thread.
//! Model, effort and mode are pickers from the CLI's catalog. Cards: a tool waiting for
//! permission, a question Claude asks (`AskUserQuestion`), a plan to approve (`ExitPlanMode`).

use crate::state::thread::{Permission, ThreadState};
use crate::state::workspace::Workspace;
use crate::theme::{Theme, MONO};
use crate::ui::rows;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::Sizable;
use gpui_kit::{div, prelude::*, px, App, Context, Entity, FontWeight, SharedString, Subscription, Window};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::rc::Rc;
use yh_core::claude::Decision;
use yh_core::markdown;

/// Built-in models until the CLI's catalog arrives (it does on the first start).
const FALLBACK_MODELS: &[(&str, &str)] = &[("haiku", "Haiku"), ("sonnet", "Sonnet"), ("opus", "Opus")];
pub const MODES: &[(&str, &str)] =
    &[("auto", "Auto"), ("ask", "Ask"), ("acceptEdits", "Accept Edits"), ("plan", "Plan"), ("bypass", "Bypass")];

pub struct Composer {
    workspace: Entity<Workspace>,
    pub input: Entity<TextareaState>,
    current: Option<String>,
    card: Option<(String, Card)>,
    plan_open: bool,
    /// The menu under the caret: slash commands at the start, files after an `@`.
    suggest: Option<Suggest>,
    /// Images waiting to go with the next message: names in `attachments/`.
    images: Vec<String>,
    _mentions: Option<gpui_kit::Task<()>>,
    /// An MCP server's question, as a card.
    elicit: Option<(String, Entity<ElicitCard>)>,
    _subscriptions: Vec<Subscription>,
}

/// What an MCP server asks mid-call: a form, its fields filled with their defaults, or a page
/// to visit. Accept sends what's in the fields.
pub struct ElicitCard {
    asked: crate::state::thread::Elicitation,
    fields: Vec<(String, String, Entity<InputState>)>,
    workspace: gpui_kit::WeakEntity<Workspace>,
}

/// A form's defaults as its answer: each property's `default`, else empty.
pub fn elicit_defaults(schema: &Value) -> Value {
    let mut out = serde_json::Map::new();
    if let Some(props) = schema["properties"].as_object() {
        for (k, p) in props {
            if !p["default"].is_null() {
                out.insert(k.clone(), p["default"].clone());
            }
        }
    }
    Value::Object(out)
}

impl ElicitCard {
    fn new(asked: crate::state::thread::Elicitation, workspace: gpui_kit::WeakEntity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut fields = vec![];
        if let Some(props) = asked.schema["properties"].as_object() {
            for (k, p) in props {
                let label = p["title"].as_str().or(p["description"].as_str()).unwrap_or(k).to_string();
                let default = match &p["default"] {
                    Value::String(s) => s.clone(),
                    Value::Null => String::new(),
                    other => other.to_string(),
                };
                fields.push((k.clone(), label, cx.new(|cx| InputState::new(window, cx).default_value(default))));
            }
        }
        ElicitCard { asked, fields, workspace }
    }

    /// The fields as the schema's types: numbers and booleans parsed, the rest text.
    fn content(&self, cx: &App) -> Value {
        let mut out = serde_json::Map::new();
        for (k, _, input) in &self.fields {
            let text = input.read(cx).value().to_string();
            let kind = self.asked.schema["properties"][k]["type"].as_str().unwrap_or("string");
            let v = match kind {
                "number" | "integer" => text.trim().parse::<f64>().map(Value::from).unwrap_or(Value::Null),
                "boolean" => Value::Bool(matches!(text.trim(), "true" | "yes" | "1")),
                _ => Value::String(text),
            };
            if !v.is_null() {
                out.insert(k.clone(), v);
            }
        }
        Value::Object(out)
    }

    fn answer(&self, action: &str, cx: &mut Context<Self>) {
        let content = (action == "accept" && self.asked.mode == "form").then(|| self.content(cx));
        let action = action.to_string();
        if let Some(ws) = self.workspace.upgrade() {
            if let Some(t) = ws.read(cx).selected_thread() {
                ws.update(cx, |ws, cx| ws.answer_elicitation(&t, &action, content, cx));
            }
        }
    }
}

impl Render for ElicitCard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let mut card = card_frame(&theme)
            .child(div().flex().gap(px(4.)).child(div().font_weight(FontWeight::BOLD).child(self.asked.server.clone())).child("asks"))
            .child(div().whitespace_normal().child(self.asked.message.clone()));
        for (_, label, input) in &self.fields {
            card = card.child(div().flex().flex_col().gap(px(2.)).child(div().text_size(px(11.)).text_color(theme.secondary).child(label.clone())).child(Input::new(input).small()));
        }
        let url = self.asked.url.clone();
        card.child(
            div()
                .flex()
                .justify_end()
                .gap(px(6.))
                .when_some(url, |d, url| {
                    d.child(Button::new("elicit-open").small().label("Open Page").on_click(move |_, _, cx| {
                        if std::env::var("YH_MCP_BROWSER").as_deref() == Ok("log") {
                            eprintln!("MCP page: {url}");
                        } else {
                            cx.open_url(&url);
                        }
                    }))
                })
                .child(Button::new("elicit-decline").small().label("Decline").on_click(cx.listener(|this, _, _, cx| this.answer("decline", cx))))
                .child(Button::new("elicit-accept").small().primary().label(if self.asked.mode == "url" { "Done" } else { "Send" }).on_click(cx.listener(|this, _, _, cx| this.answer("accept", cx)))),
        )
    }
}

enum Card {
    Question(Entity<QuestionCard>),
    Plan(Entity<PlanCard>),
    Tool,
}

impl Composer {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextareaState::new(window, cx).placeholder("Ask Claude…").submit_on_enter(true));
        let subscriptions = vec![
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { shift: false, secondary } => this.enter(*secondary, window, cx),
                InputEvent::Blur => {
                    this.save_draft(cx);
                    this.suggest = None;
                }
                InputEvent::Change => this.update_suggestions(window, cx),
                _ => {}
            }),
            cx.observe_in(&workspace, window, |this, _, window, cx| {
                this.sync_card(window, cx);
                cx.notify();
            }),
        ];
        Composer {
            workspace,
            input,
            current: None,
            card: None,
            plan_open: false,
            suggest: None,
            images: vec![],
            _mentions: None,
            elicit: None,
            _subscriptions: subscriptions,
        }
    }

    fn thread(&self, cx: &App) -> Option<Entity<ThreadState>> {
        self.workspace.read(cx).selected_thread()
    }

    /// Moving between threads keeps each one's draft (saved with the thread).
    pub fn switch_to(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.current.as_deref() == Some(id) {
            return;
        }
        self.save_draft(cx);
        let draft = self.workspace.read(cx).thread(id).and_then(|t| t.read(cx).meta.draft.clone()).unwrap_or_default();
        self.input.update(cx, |i, cx| i.set_value(draft, window, cx));
        self.current = Some(id.to_string());
        self.sync_card(window, cx);
    }

    fn save_draft(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.current.clone() else { return };
        let text = self.input.read(cx).value().to_string();
        self.workspace.update(cx, |ws, cx| ws.save_draft(&id, &text, cx));
    }

    /// ↩ sends (steering while Claude works); ⌘↩ answers a waiting tool, or stops the turn and
    /// sends now.
    fn enter(&mut self, secondary: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = self.thread(cx) else { return };
        let waiting = thread.read(cx).permission.is_some();
        if secondary && waiting && self.input.read(cx).value().trim().is_empty() {
            self.workspace.update(cx, |ws, cx| ws.respond(&thread, Decision::allow(), cx));
            return;
        }
        self.submit(secondary, window, cx);
    }

    pub fn submit(&mut self, now: bool, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        if text.trim().is_empty() && self.images.is_empty() {
            return;
        }
        let Some(thread) = self.thread(cx) else { return };
        // `/mcp` is the app's: the sheet, not Claude Code's headless one-line answer.
        if crate::ui::mcp::is_mcp_command(&text) {
            self.input.update(cx, |i, cx| i.set_value("", window, cx));
            self.workspace.update(cx, |ws, cx| ws.open_mcp(cx));
            return;
        }
        let images = std::mem::take(&mut self.images);
        self.input.update(cx, |i, cx| i.set_value("", window, cx));
        self.workspace.update(cx, |ws, cx| ws.send_with_images(&thread, &text, images, now, cx));
        cx.notify();
    }

    /// Images (any common format) prepared and kept for the next message, off the main thread.
    pub fn add_images(&mut self, sources: Vec<Vec<u8>>, cx: &mut Context<Self>) {
        let Some(thread) = self.current.clone().or_else(|| self.workspace.read(cx).selected.clone()) else { return };
        let dir = self.workspace.read(cx).data_dir.clone();
        cx.spawn(async move |this, cx| {
            let names: Vec<String> = cx
                .background_executor()
                .spawn(async move { sources.iter().filter_map(|b| yh_core::attachments::save(&dir, &thread, b)).collect() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.images.extend(names);
                cx.notify();
            });
        })
        .detach();
    }

    pub fn add_files(&mut self, paths: &[std::path::PathBuf], cx: &mut Context<Self>) {
        let sources: Vec<Vec<u8>> =
            paths.iter().filter(|p| yh_core::attachments::is_image_path(p)).filter_map(|p| std::fs::read(p).ok()).collect();
        if !sources.is_empty() {
            self.add_images(sources, cx);
        }
    }

    /// ⌘V with an image (or image files) on the clipboard attaches it; text goes to the field.
    fn paste_image(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(item) = cx.read_from_clipboard() else { return false };
        let mut sources = vec![];
        let mut paths = vec![];
        for entry in item.entries() {
            match entry {
                gpui_kit::ClipboardEntry::Image(image) => sources.push(image.bytes.clone()),
                gpui_kit::ClipboardEntry::ExternalPaths(p) => paths.extend(p.paths().iter().cloned()),
                _ => {}
            }
        }
        paths.retain(|p| yh_core::attachments::is_image_path(p));
        if sources.is_empty() && paths.is_empty() {
            return false;
        }
        sources.extend(paths.iter().filter_map(|p| std::fs::read(p).ok()));
        self.add_images(sources, cx);
        true
    }

    fn attach(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("Attach".into()) });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                let _ = this.update(cx, |this, cx| this.add_files(&paths, cx));
            }
        })
        .detach();
    }

    /// The card follows the open thread's pending request.
    fn sync_card(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let asked = self.thread(cx).and_then(|t| t.read(cx).elicitation.clone());
        match (asked, &self.elicit) {
            (None, Some(_)) => self.elicit = None,
            (Some(a), current) if current.as_ref().is_none_or(|(id, _)| *id != a.id) => {
                let ws = self.workspace.downgrade();
                let id = a.id.clone();
                self.elicit = Some((id, cx.new(|cx| ElicitCard::new(a, ws, window, cx))));
            }
            _ => {}
        }
        let permission = self.thread(cx).and_then(|t| t.read(cx).permission.clone());
        match (&permission, &self.card) {
            (None, None) => {}
            (Some(p), Some((id, _))) if *id == p.id => {}
            (None, Some(_)) => self.card = None,
            (Some(p), _) => {
                let ws = self.workspace.downgrade();
                let card = match p.tool.as_str() {
                    "AskUserQuestion" => match QuestionCard::new(p, ws, window, cx) {
                        Some(card) => Card::Question(cx.new(|_| card)),
                        None => Card::Tool,
                    },
                    "ExitPlanMode" => Card::Plan(cx.new(|cx| PlanCard::new(p, ws, window, cx))),
                    _ => Card::Tool,
                };
                self.card = Some((p.id.clone(), card));
            }
        }
    }

    fn pickers(&self, thread: &Entity<ThreadState>, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = self.workspace.read(cx);
        let t = thread.read(cx);
        let id = t.id().to_string();
        let project = t.meta.project_id.clone();
        let mode = ws.project(&project).map_or("auto".to_string(), |p| p.mode.clone());
        let model = ws.model_of(t);
        let effort = ws.effort_of(t);
        let models: Vec<(String, String, Vec<String>)> = if ws.models.is_empty() {
            FALLBACK_MODELS.iter().map(|(v, n)| (v.to_string(), n.to_string(), vec![])).collect()
        } else {
            ws.models.iter().map(|m| (m.value.clone(), m.display_name.clone(), m.efforts.clone())).collect()
        };
        let model_label = model
            .as_ref()
            .and_then(|m| models.iter().find(|(v, ..)| v == m).map(|(_, n, _)| n.clone()).or(Some(m.clone())))
            .unwrap_or_else(|| "Default model".into());
        let efforts = models.iter().find(|(v, ..)| Some(v) == model.as_ref()).map(|(.., e)| e.clone()).unwrap_or_default();
        let mode_label = MODES.iter().find(|(v, _)| *v == mode).map_or("Auto", |(_, l)| l);
        let models = Rc::new(models);
        let w = self.workspace.downgrade();
        let model_menu = {
            let (w, id, models, current) = (w.clone(), id.clone(), models.clone(), model.clone());
            Button::new("model").ghost().xsmall().label(model_label).dropdown_caret(true).dropdown_menu(move |mut menu, _, _| {
                {
                    let (w, id) = (w.clone(), id.clone());
                    menu = menu.item(PopupMenuItem::new("Default").checked(current.is_none()).on_click(move |_, _, cx| {
                        let _ = w.update(cx, |ws, cx| ws.set_model(&id, None, cx));
                    }));
                }
                for (value, name, _) in models.iter() {
                    let (w, id, value) = (w.clone(), id.clone(), value.clone());
                    let checked = current.as_deref() == Some(value.as_str());
                    menu = menu.item(PopupMenuItem::new(name.clone()).checked(checked).on_click(move |_, _, cx| {
                        let _ = w.update(cx, |ws, cx| ws.set_model(&id, Some(value.clone()), cx));
                    }));
                }
                menu
            })
        };
        let effort_menu = (!efforts.is_empty()).then(|| {
            let (w, id, current) = (w.clone(), id.clone(), effort.clone());
            let efforts = efforts.clone();
            let label = effort.clone().map(|e| capitalize(&e)).unwrap_or_else(|| "Effort".into());
            Button::new("effort").ghost().xsmall().label(label).dropdown_caret(true).dropdown_menu(move |mut menu, _, _| {
                for e in &efforts {
                    let (w, id, e) = (w.clone(), id.clone(), e.clone());
                    let checked = current.as_deref() == Some(e.as_str());
                    menu = menu.item(PopupMenuItem::new(capitalize(&e)).checked(checked).on_click(move |_, _, cx| {
                        let _ = w.update(cx, |ws, cx| ws.set_effort(&id, Some(e.clone()), cx));
                    }));
                }
                menu
            })
        });
        let mode_menu = {
            let (w, project, current) = (w.clone(), project.clone(), mode.clone());
            Button::new("mode").ghost().xsmall().label(mode_label).dropdown_caret(true).dropdown_menu(move |mut menu, _, _| {
                for (value, label) in MODES {
                    let (w, project) = (w.clone(), project.clone());
                    menu = menu.item(PopupMenuItem::new(*label).checked(current == *value).on_click(move |_, _, cx| {
                        let _ = w.update(cx, |ws, cx| ws.set_mode(&project, value, cx));
                    }));
                }
                menu
            })
        };
        div()
            .flex()
            .items_center()
            .gap(px(2.))
            .text_size(px(12.))
            .text_color(theme.secondary)
            .child(model_menu)
            .children(effort_menu)
            .child(mode_menu)
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn card_frame(theme: &Theme) -> gpui_kit::Div {
    div()
        .border_1()
        .border_color(theme.warning)
        .rounded(px(12.))
        .bg(theme.card)
        .px(px(12.))
        .py(px(10.))
        .mb(px(8.))
        .flex()
        .flex_col()
        .gap(px(8.))
}

fn mono(text: String, theme: &Theme) -> impl IntoElement {
    div()
        .font_family(MONO)
        .text_size(px(11.))
        .bg(theme.code)
        .rounded(px(6.))
        .px(px(8.))
        .py(px(6.))
        .max_h(px(160.))
        .overflow_hidden()
        .child(text)
}

fn pretty_input(input: &str) -> String {
    match serde_json::from_str::<Value>(input) {
        Ok(value) => value["command"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| serde_json::to_string_pretty(&value).unwrap_or_else(|_| input.to_string())),
        Err(_) => input.to_string(),
    }
}

impl Render for Composer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let Some(thread) = self.thread(cx) else { return div() };
        let (running, permission) = {
            let t = thread.read(cx);
            (t.running, t.permission.clone())
        };
        let w = self.workspace.downgrade();
        let mut inner = div().w_full().max_w(px(828.)).flex().flex_col();
        if let Some((_, card)) = &self.elicit {
            inner = inner.child(card.clone());
        }
        if let (Some(p), Some((_, card))) = (&permission, &self.card) {
            inner = inner.child(match card {
                Card::Question(q) => q.clone().into_any_element(),
                Card::Plan(plan) => plan.clone().into_any_element(),
                Card::Tool => {
                    let (allow, deny) = (w.clone(), w.clone());
                    let (t1, t2) = (thread.clone(), thread.clone());
                    card_frame(&theme)
                        .child(
                            div()
                                .flex()
                                .gap(px(4.))
                                .child("Claude wants to use")
                                .child(div().font_weight(FontWeight::BOLD).child(p.tool.clone()))
                                .when(!p.description.is_empty(), |d| d.child(div().ml(px(8.)).text_color(theme.secondary).child(p.description.clone()))),
                        )
                        .child(mono(pretty_input(&p.input), &theme))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(px(8.))
                                .child(Button::new("deny").small().label("Deny ⌘⌫").on_click(move |_, _, cx| {
                                    let _ = deny.update(cx, |ws, cx| ws.respond(&t2, Decision::deny(), cx));
                                }))
                                .child(Button::new("allow").small().primary().label("Allow ⌘↩").on_click(move |_, _, cx| {
                                    let _ = allow.update(cx, |ws, cx| ws.respond(&t1, Decision::allow(), cx));
                                })),
                        )
                        .into_any_element()
                }
            });
        }
        let plan = thread.read(cx).plan.clone();
        if !plan.items.is_empty() {
            inner = inner.child(self.plan_strip(&plan, &theme, cx));
        }
        if let Some(menu) = self.suggestions(&theme, cx) {
            inner = inner.child(menu);
        }
        let stop_thread = thread.clone();
        let thumbs = (!self.images.is_empty()).then(|| {
            let dir = self.workspace.read(cx).data_dir.join("attachments");
            div().flex().flex_wrap().gap(px(6.)).children(self.images.iter().enumerate().map(|(i, name)| {
                div()
                    .relative()
                    .child(gpui_kit::img(dir.join(name)).h(px(56.)).max_w(px(120.)).rounded(px(6.)))
                    .child(
                        div()
                            .id(("remove-image", i))
                            .absolute()
                            .top(px(-4.))
                            .right(px(-4.))
                            .w(px(16.))
                            .h(px(16.))
                            .rounded_full()
                            .bg(theme.secondary)
                            .text_color(gpui_kit::white())
                            .text_size(px(11.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child("×")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if i < this.images.len() {
                                    this.images.remove(i);
                                }
                                cx.notify();
                            })),
                    )
            }))
        });
        let composer = div()
            .capture_key_down(cx.listener(Self::on_key))
            .on_drop(cx.listener(|this, paths: &gpui_kit::ExternalPaths, _, cx| this.add_files(paths.paths(), cx)))
            .children(thumbs)
            .relative()
            .border_1()
            .border_color(theme.separator)
            .rounded(px(14.))
            .bg(theme.card)
            .px(px(12.))
            .pt(px(10.))
            .pb(px(8.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(Textarea::new(&self.input).appearance(false).max_h(px(320.)))
            .when(crate::probe::enabled(), |d| d.child(crate::probe::tracker("composer".into())))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        Button::new("attach").ghost().xsmall().label("+").tooltip("Attach images").on_click(cx.listener(|this, _, _, cx| this.attach(cx))),
                    )
                    .child(self.pickers(&thread, &theme, cx))
                    .child(div().flex_1())
                    .when(running, |d| {
                        let w = w.clone();
                        d.child(Button::new("stop").small().label("Stop ⌘.").on_click(move |_, _, cx| {
                            let _ = w.update(cx, |ws, cx| ws.interrupt(&stop_thread, cx));
                        }))
                    })
                    .child(
                        Button::new("send")
                            .small()
                            .primary()
                            .label(if running { "Steer ↩" } else { "Send" })
                            .on_click(cx.listener(|this, _, window, cx| this.submit(false, window, cx))),
                    ),
            );
        div().w_full().flex().justify_center().px(px(20.)).pb(px(16.)).child(inner.child(composer))
    }
}

impl Composer {
    /// Claude's checklist: progress and what it's on, opening into the whole list.
    fn plan_strip(&self, plan: &yh_core::plan::Plan, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        use yh_core::plan::Status;
        let current = plan.current().map(|i| i.active_form.clone().unwrap_or_else(|| i.subject.clone()));
        let head = format!(
            "{} Plan {}/{}{}",
            if self.plan_open { "⌄" } else { "›" },
            plan.completed(),
            plan.items.len(),
            current.map(|c| format!(" · {c}")).unwrap_or_else(|| if plan.is_finished() { " · done".into() } else { String::new() })
        );
        let mut strip = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .mb(px(6.))
            .px(px(12.))
            .py(px(6.))
            .rounded(px(10.))
            .bg(theme.card)
            .text_size(px(12.))
            .child(
                div()
                    .id("plan-head")
                    .text_color(theme.secondary)
                    .child(head)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.plan_open = !this.plan_open;
                        cx.notify();
                    })),
            );
        if self.plan_open {
            for item in &plan.items {
                let (mark, color) = match item.status {
                    Status::Completed => ("✓", theme.secondary),
                    Status::InProgress => ("◐", theme.accent),
                    Status::Pending => ("○", theme.secondary),
                };
                strip = strip.child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .child(div().w(px(14.)).text_color(color).child(mark))
                        .child(div().when(item.status == Status::Completed, |d| d.text_color(theme.secondary)).child(item.subject.clone())),
                );
            }
        }
        strip
    }
}

struct Suggestion {
    label: String,
    detail: String,
    insert: String,
}

struct Suggest {
    items: Vec<Suggestion>,
    index: usize,
    /// Where what's being completed starts (the `/` or `@`).
    start: usize,
}

impl Composer {
    /// `YH_DRAFT`: text put in the composer as if typed, the caret at its end.
    pub fn type_draft(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let len = text.len();
        self.input.update(cx, |i, cx| {
            i.set_value(text.to_string(), window, cx);
            i.set_selected_range(len..len, cx);
        });
        self.update_suggestions(window, cx);
    }

    fn update_suggestions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (text, cursor) = {
            let i = self.input.read(cx);
            (i.value().to_string(), i.cursor())
        };
        let before = text.get(..cursor).unwrap_or(&text);
        let Some(thread) = self.thread(cx) else { return };
        // `/name` alone at the start: commands and skills.
        if let Some(q) = before.strip_prefix('/').filter(|q| !q.contains(char::is_whitespace)) {
            let q = q.to_lowercase();
            let project = thread.read(cx).meta.project_id.clone();
            let mut items: Vec<&yh_core::claude::SlashCommand> =
                self.workspace.read(cx).commands(&project).iter().filter(|c| c.name.to_lowercase().contains(&q)).collect();
            items.sort_by_key(|c| !c.name.to_lowercase().starts_with(&q));
            let items: Vec<Suggestion> = items
                .into_iter()
                .take(12)
                .map(|c| Suggestion {
                    label: format!("/{}", c.name),
                    detail: if c.hint.is_empty() { c.description.clone() } else { format!("{} · {}", c.hint, c.description) },
                    insert: format!("/{} ", c.name),
                })
                .collect();
            self.suggest = (!items.is_empty()).then_some(Suggest { items, index: 0, start: 0 });
            cx.notify();
            return;
        }
        // `@word` after a space or at the start: files and folders of the thread's folder.
        let at = before.rfind('@').filter(|&i| (i == 0 || before[..i].ends_with(char::is_whitespace)) && !before[i + 1..].contains(char::is_whitespace));
        let Some(at) = at else {
            if self.suggest.take().is_some() {
                cx.notify();
            }
            return;
        };
        let query = before[at + 1..].to_string();
        let task = self.workspace.update(cx, |ws, cx| ws.mention_files(&thread, query, cx));
        self._mentions = Some(cx.spawn_in(window, async move |this, cx| {
            let files = task.await;
            let _ = this.update(cx, |this, cx| {
                let items: Vec<Suggestion> = files
                    .into_iter()
                    .map(|f| Suggestion {
                        label: f.trim_end_matches('/').rsplit('/').next().unwrap_or(&f).to_string(),
                        insert: if f.contains(' ') { format!("@\"{f}\" ") } else { format!("@{f} ") },
                        detail: f,
                    })
                    .collect();
                this.suggest = (!items.is_empty()).then_some(Suggest { items, index: 0, start: at });
                cx.notify();
            });
        }));
    }

    fn accept(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.suggest.take() else { return };
        let Some(choice) = s.items.into_iter().nth(index) else { return };
        self.input.update(cx, |i, cx| {
            let cursor = i.cursor();
            i.set_selected_range(s.start..cursor, cx);
            i.replace(choice.insert, window, cx);
        });
        cx.notify();
    }

    /// ↑↓ move through the menu, ↩ or ⇥ take an item, ⎋ closes it; before the field sees them.
    fn on_key(&mut self, event: &gpui_kit::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        if k.key == "v" && k.modifiers.platform && !k.modifiers.shift && self.paste_image(cx) {
            cx.stop_propagation();
            return;
        }
        let Some(s) = &mut self.suggest else { return };
        let n = s.items.len();
        match event.keystroke.key.as_str() {
            "up" => s.index = (s.index + n - 1) % n,
            "down" => s.index = (s.index + 1) % n,
            "enter" | "tab" if !event.keystroke.modifiers.shift => {
                let index = s.index;
                self.accept(index, window, cx);
            }
            "escape" => self.suggest = None,
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn suggestions(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let s = self.suggest.as_ref()?;
        let mut list = div()
            .flex()
            .flex_col()
            .mb(px(6.))
            .py(px(4.))
            .rounded(px(10.))
            .border_1()
            .border_color(theme.separator)
            .bg(theme.card);
        for (i, item) in s.items.iter().enumerate() {
            list = list.child(
                div()
                    .id(("suggestion", i))
                    .flex()
                    .gap(px(10.))
                    .px(px(10.))
                    .py(px(3.))
                    .when(i == s.index, |d| d.bg(theme.selected))
                    .child(div().flex_none().font_weight(FontWeight::MEDIUM).child(item.label.clone()))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_color(theme.secondary).child(item.detail.clone()))
                    .on_mouse_down(gpui_kit::MouseButton::Left, cx.listener(move |this, _, window, cx| this.accept(i, window, cx))),
            );
        }
        Some(list)
    }
}

// MARK: Question

struct Question {
    question: String,
    header: Option<String>,
    multi: bool,
    options: Vec<(String, Option<String>)>,
    chosen: BTreeSet<usize>,
    other: Entity<InputState>,
}

/// Claude asks (`AskUserQuestion`): pick options or write your own; Skip declines.
pub struct QuestionCard {
    workspace: Option<gpui_kit::WeakEntity<Workspace>>,
    input: String,
    questions: Vec<Question>,
}

impl QuestionCard {
    fn new(p: &Permission, workspace: gpui_kit::WeakEntity<Workspace>, window: &mut Window, cx: &mut Context<Composer>) -> Option<QuestionCard> {
        let input: Value = serde_json::from_str(&p.input).ok()?;
        let questions: Vec<Question> = input["questions"]
            .as_array()?
            .iter()
            .filter_map(|q| {
                Some(Question {
                    question: q["question"].as_str()?.to_string(),
                    header: q["header"].as_str().map(str::to_string),
                    multi: q["multiSelect"].as_bool().unwrap_or(false),
                    options: q["options"]
                        .as_array()
                        .map(|o| o.iter().filter_map(|o| Some((o["label"].as_str()?.to_string(), o["description"].as_str().map(str::to_string)))).collect())
                        .unwrap_or_default(),
                    chosen: BTreeSet::new(),
                    other: cx.new(|cx| InputState::new(window, cx).placeholder("Or write your own answer")),
                })
            })
            .collect();
        (!questions.is_empty()).then(|| QuestionCard { workspace: Some(workspace), input: p.input.clone(), questions })
    }

    /// The tool's input with `answers` added, keyed by question; several joined with ", ".
    fn answered_input(&self, cx: &App) -> String {
        let mut input: Value = serde_json::from_str(&self.input).unwrap_or(json!({}));
        let mut answers = serde_json::Map::new();
        for q in &self.questions {
            let mut picked: Vec<String> = q.chosen.iter().filter_map(|i| q.options.get(*i).map(|o| o.0.clone())).collect();
            let other = q.other.read(cx).value().trim().to_string();
            if !other.is_empty() {
                picked.push(other);
            }
            answers.insert(q.question.clone(), picked.join(", ").into());
        }
        input["answers"] = Value::Object(answers);
        input.to_string()
    }
}

impl Render for QuestionCard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let mut card = card_frame(&theme);
        for (qi, q) in self.questions.iter().enumerate() {
            let mut options = div().flex().flex_wrap().gap(px(6.));
            for (oi, (label, description)) in q.options.iter().enumerate() {
                let on = q.chosen.contains(&oi);
                options = options.child(
                    Button::new(SharedString::from(format!("q{qi}-{oi}")))
                        .small()
                        .when(on, |b| b.primary())
                        .label(label.clone())
                        .when_some(description.clone(), |b, d| b.tooltip(d))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let q = &mut this.questions[qi];
                            if !q.chosen.remove(&oi) {
                                if !q.multi {
                                    q.chosen.clear();
                                }
                                q.chosen.insert(oi);
                            }
                            cx.notify();
                        })),
                );
            }
            card = card.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .when_some(q.header.clone(), |d, h| d.child(div().text_size(px(11.)).text_color(theme.secondary).child(h)))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(q.question.clone()))
                    .child(options)
                    .child(Input::new(&q.other).small()),
            );
        }
        let ws = self.workspace.clone();
        let ws2 = self.workspace.clone();
        card.child(
            div()
                .flex()
                .justify_end()
                .gap(px(8.))
                .child(Button::new("skip").small().label("Skip").on_click(move |_, _, cx| {
                    let Some(ws) = ws2.as_ref().and_then(|w| w.upgrade()) else { return };
                    if let Some(t) = ws.read(cx).selected_thread() {
                        ws.update(cx, |ws, cx| ws.respond(&t, Decision::Deny { message: "The user skipped this question.".into() }, cx));
                    }
                }))
                .child(Button::new("answer").small().primary().label("Answer").on_click(cx.listener(move |this, _, _, cx| {
                    let input = this.answered_input(cx);
                    let Some(ws) = ws.as_ref().and_then(|w| w.upgrade()) else { return };
                    if let Some(t) = ws.read(cx).selected_thread() {
                        ws.update(cx, |ws, cx| ws.respond(&t, Decision::Allow { updated_input: Some(input), mode: None }, cx));
                    }
                }))),
        )
    }
}

// MARK: Plan

/// Claude's plan (`ExitPlanMode`): approve it into a mode, which becomes the project's, or keep
/// planning with feedback.
pub struct PlanCard {
    workspace: gpui_kit::WeakEntity<Workspace>,
    plan: std::rc::Rc<markdown::Markdown>,
    feedback: Entity<InputState>,
}

impl PlanCard {
    fn new(p: &Permission, workspace: gpui_kit::WeakEntity<Workspace>, window: &mut Window, cx: &mut Context<PlanCard>) -> PlanCard {
        let text = serde_json::from_str::<Value>(&p.input).ok().and_then(|v| v["plan"].as_str().map(str::to_string)).unwrap_or_default();
        PlanCard {
            workspace,
            plan: std::rc::Rc::new(markdown::parse(&text)),
            feedback: cx.new(|cx| InputState::new(window, cx).placeholder("What should change?")),
        }
    }
}

impl Render for PlanCard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let mut buttons = div().flex().justify_end().gap(px(8.));
        let keep = self.workspace.clone();
        let feedback = self.feedback.clone();
        buttons = buttons.child(Button::new("keep-planning").small().label("Keep Planning").on_click(move |_, _, cx| {
            let message = feedback.read(cx).value().trim().to_string();
            let message = if message.is_empty() { "Keep planning.".to_string() } else { message };
            let Some(ws) = keep.upgrade() else { return };
            if let Some(t) = ws.read(cx).selected_thread() {
                ws.update(cx, |ws, cx| ws.respond(&t, Decision::Deny { message }, cx));
            }
        }));
        for (mode, label) in [("ask", "Approve, Ask"), ("acceptEdits", "Approve, Accept Edits"), ("auto", "Approve, Auto")] {
            let w = self.workspace.clone();
            let b = Button::new(SharedString::from(format!("approve-{mode}"))).small().label(label);
            let b = if mode == "auto" { b.primary() } else { b };
            buttons = buttons.child(b.on_click(move |_, _, cx| {
                let Some(ws) = w.upgrade() else { return };
                if let Some(t) = ws.read(cx).selected_thread() {
                    ws.update(cx, |ws, cx| ws.approve_plan(&t, mode, cx));
                }
            }));
        }
        card_frame(&theme)
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Claude's plan"))
            .child(div().max_h(px(320.)).overflow_hidden().child(rows::markdown_view(&self.plan, &theme)))
            .child(Input::new(&self.feedback).small())
            .child(buttons)
    }
}
