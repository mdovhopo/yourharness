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
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Disableable, Sizable};
use gpui_kit::{div, prelude::*, px, App, Context, Entity, FontWeight, SharedString, Subscription, Window};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet};
use yh_core::attachments::FileAttachment;
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
    /// The menu under the caret: slash commands (at the start, or later to chain skills), files
    /// after an `@`.
    suggest: Option<Suggest>,
    /// What waits to go with each thread's next message: images (names in `attachments/`), files
    /// and folders. Kept by thread until sent, not saved.
    attached: HashMap<String, Vec<Attached>>,
    /// Queued messages you asked to remove, until the CLI lets go of them.
    withdrawing: HashSet<String>,
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
    /// ⌘↩ (`allow`) and ⌘⌫ on a question or plan card: Answer or approve in the recommended mode;
    /// Skip or Keep Planning. False when the card is a plain permission, which takes Allow/Deny.
    pub fn card_shortcut(&mut self, allow: bool, cx: &mut Context<Self>) -> bool {
        match self.card.as_ref().map(|(_, c)| c) {
            Some(Card::Question(q)) => {
                let q = q.clone();
                q.update(cx, |q, cx| if allow { q.submit(cx) } else { q.skip(cx) });
                true
            }
            Some(Card::Plan(p)) => {
                let p = p.clone();
                p.update(cx, |p, cx| if allow { p.approve(cx) } else { p.keep_planning(cx) });
                true
            }
            _ => false,
        }
    }

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
                this.take_restored(window, cx);
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
            attached: HashMap::new(),
            withdrawing: HashSet::new(),
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
        let Some(thread) = self.thread(cx) else { return };
        let key = thread.read(cx).id().to_string();
        if text.trim().is_empty() && self.attached.get(&key).is_none_or(Vec::is_empty) {
            return;
        }
        let attached = self.attached.remove(&key).unwrap_or_default();
        let (mut images, mut files) = (vec![], vec![]);
        for a in attached {
            match a {
                Attached::Image(name) => images.push(name),
                Attached::File(f) => files.push(f),
            }
        }
        self.suggest = None;
        self.input.update(cx, |i, cx| i.set_value("", window, cx));
        // The app's own commands (`/btw`, `/mcp`, `/model`…) are answered in `send_message`.
        self.workspace.update(cx, |ws, cx| ws.send_message(&thread, &text, images, files, now, cx));
        cx.notify();
    }

    /// The thread the composer is for (what's attached is kept by it).
    fn key(&self, cx: &App) -> Option<String> {
        self.current.clone().or_else(|| self.workspace.read(cx).selected.clone())
    }

    fn attach_ready(&mut self, key: String, ready: Vec<Attached>, cx: &mut Context<Self>) {
        let list = self.attached.entry(key).or_default();
        for a in ready {
            // One already there isn't added twice; at most `ATTACH_LIMIT` in all.
            let dup = matches!(&a, Attached::File(f) if list.iter().any(|b| matches!(b, Attached::File(g) if g.path == f.path)));
            if !dup && list.len() < ATTACH_LIMIT {
                list.push(a);
            }
        }
        cx.notify();
    }

    /// Images (any common format) prepared and kept for the next message, off the main thread.
    pub fn add_images(&mut self, sources: Vec<Vec<u8>>, cx: &mut Context<Self>) {
        let Some(thread) = self.key(cx) else { return };
        let dir = self.workspace.read(cx).data_dir.clone();
        cx.spawn(async move |this, cx| {
            let key = thread.clone();
            let ready: Vec<Attached> = cx
                .background_executor()
                .spawn(async move { sources.iter().filter_map(|b| yh_core::attachments::save(&dir, &thread, b)).map(Attached::Image).collect() })
                .await;
            let _ = this.update(cx, |this, cx| this.attach_ready(key, ready, cx));
        })
        .detach();
    }

    /// Files, folders and images for the next message, read off the main thread: images Claude
    /// sees inline, anything else (a PDF, a log, a folder) by its path. An image that can't be
    /// read goes as a file.
    pub fn add_files(&mut self, paths: &[std::path::PathBuf], cx: &mut Context<Self>) {
        let Some(thread) = self.key(cx) else { return };
        let dir = self.workspace.read(cx).data_dir.clone();
        let paths: Vec<std::path::PathBuf> = paths.iter().take(ATTACH_LIMIT).cloned().collect();
        cx.spawn(async move |this, cx| {
            let key = thread.clone();
            let ready: Vec<Attached> = cx
                .background_executor()
                .spawn(async move {
                    paths
                        .iter()
                        .filter_map(|p| {
                            let image = (yh_core::attachments::is_image_path(p) && p.is_file())
                                .then(|| std::fs::read(p).ok().and_then(|b| yh_core::attachments::save(&dir, &thread, &b)))
                                .flatten();
                            image.map(Attached::Image).or_else(|| FileAttachment::from_path(p).map(Attached::File))
                        })
                        .collect()
                })
                .await;
            let _ = this.update(cx, |this, cx| this.attach_ready(key, ready, cx));
        })
        .detach();
    }

    /// ⌘V with files (copied in Finder) or an image (a screenshot) on the clipboard attaches
    /// them; text goes to the field.
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
        if sources.is_empty() && paths.is_empty() {
            return false;
        }
        if !paths.is_empty() {
            self.add_files(&paths, cx);
        }
        if !sources.is_empty() {
            self.add_images(sources, cx);
        }
        true
    }

    /// The paperclip: pick files or folders.
    fn attach(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions { files: true, directories: true, multiple: true, prompt: Some("Attach".into()) });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                let _ = this.update(cx, |this, cx| this.add_files(&paths, cx));
            }
        })
        .detach();
    }

    /// Queued messages taken back into the composer: their text after what's there, their
    /// images and files attached again.
    fn take_restored(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = self.thread(cx) else { return };
        if thread.read(cx).composer.restored.is_empty() {
            return;
        }
        let restored = thread.update(cx, |t, _| std::mem::take(&mut t.composer.restored));
        let key = thread.read(cx).id().to_string();
        let dir = self.workspace.read(cx).data_dir.clone();
        let mut draft = self.input.read(cx).value().to_string();
        for (id, text, attachments) in restored {
            self.withdrawing.remove(&id);
            if !text.is_empty() {
                draft = if draft.is_empty() { text } else { format!("{draft}\n{text}") };
            }
            let ready: Vec<Attached> = attachments
                .into_iter()
                .filter_map(|name| {
                    if yh_core::attachments::is_image_name(&name) {
                        return Some(Attached::Image(name));
                    }
                    let path = if name.starts_with('/') { std::path::PathBuf::from(name.trim_end_matches('/')) } else { yh_core::attachments::path(&dir, &name) };
                    FileAttachment::from_path(&path).map(Attached::File)
                })
                .collect();
            self.attach_ready(key.clone(), ready, cx);
        }
        let len = draft.len();
        self.input.update(cx, |i, cx| {
            i.set_value(draft, window, cx);
            i.set_selected_range(len..len, cx);
        });
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

/// At most this many attachments go with one message.
const ATTACH_LIMIT: usize = 20;

/// Something waiting to go with the next message.
#[derive(Clone, Debug)]
enum Attached {
    /// An image Claude sees inline: its name in `attachments/`.
    Image(String),
    /// Any other file, or a folder: Claude is told where it is.
    File(FileAttachment),
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
        if let Some(ask) = thread.read(cx).sign_in.clone() {
            // Claude asks you to sign in to an MCP server: in your default browser, from here.
            let (t1, t2, t3) = (thread.clone(), thread.clone(), thread.clone());
            let answer = |t: &Entity<ThreadState>, yes: bool, cx: &mut App| {
                t.update(cx, |t, cx| {
                    if let Some(a) = t.sign_in.as_mut() {
                        if let Some(d) = a.decision.take() {
                            let _ = d.send(yes);
                        }
                    }
                    cx.notify();
                })
            };
            let asking = ask.decision.is_some();
            let cancelled = ask.cancelled.clone();
            inner = inner.child(
                card_frame(&theme)
                    .child(div().flex().gap(px(4.)).child("Claude needs you to sign in to").child(div().font_weight(FontWeight::BOLD).child(ask.server.clone())))
                    .when_some(ask.reason.clone(), |d, r| d.child(div().text_color(theme.secondary).child(r)))
                    .child(if asking {
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(6.))
                            .child(Button::new("sign-in-no").small().label("Not Now").on_click(move |_, _, cx| answer(&t1, false, cx)))
                            .child(Button::new("sign-in-yes").small().primary().label("Sign In").on_click(move |_, _, cx| answer(&t2, true, cx)))
                    } else {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(div().flex_1().text_color(theme.secondary).child("Waiting for you to finish in your browser…"))
                            .child(Button::new("sign-in-cancel").small().label("Cancel").on_click(move |_, _, cx| {
                                cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
                                t3.update(cx, |_, cx| cx.notify());
                            }))
                    }),
            );
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
        if let Some(queued) = self.queued_strip(&thread, &theme, cx) {
            inner = inner.child(queued);
        }
        let attached = self.attached_strip(&thread, &theme, cx);
        let (context, mcp_chip) = {
            let ws = self.workspace.read(cx);
            let t = thread.read(cx);
            let context = t.composer.context_tokens.map(|tokens| (tokens, ws.context_window_of(t)));
            (context, crate::state::workspace::mcp_needing_you(&t.composer.mcp_servers))
        };
        let stop_thread = thread.clone();
        let composer = div()
            .capture_key_down(cx.listener(Self::on_key))
            .on_drop(cx.listener(|this, paths: &gpui_kit::ExternalPaths, _, cx| this.add_files(paths.paths(), cx)))
            .children(attached)
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
                        Button::new("attach")
                            .ghost()
                            .xsmall()
                            .label("+")
                            .tooltip("Attach Files… (files, folders or images; or paste and drop them)")
                            .on_click(cx.listener(|this, _, _, cx| this.attach(cx))),
                    )
                    .child(self.pickers(&thread, &theme, cx))
                    .children((!mcp_chip.is_empty()).then(|| mcp_attention_chip(&mcp_chip, self.workspace.downgrade(), &theme)))
                    .child(div().flex_1())
                    .children(context.map(|(tokens, window)| context_ring(tokens, window, &theme)))
                    .when(running, |d| {
                        let w = w.clone();
                        d.child(Button::new("stop").small().label("Stop ⌘.").on_click(cx.listener(move |this, _, _, cx| {
                            // Anything queued goes back into the composer first, so stopping never sends it.
                            let queued = stop_thread.read(cx).queued_messages();
                            for (id, ..) in queued {
                                if !this.withdrawing.contains(&id) {
                                    let _ = w.update(cx, |ws, cx| ws.withdraw(&stop_thread, &id, true, cx));
                                }
                            }
                            let _ = w.update(cx, |ws, cx| ws.interrupt(&stop_thread, cx));
                        })))
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

impl Composer {
    /// Messages sent while Claude works, waiting for its current step. Each can still go now
    /// (Claude is interrupted), come back to the composer, or be dropped.
    fn queued_strip(&self, thread: &Entity<ThreadState>, theme: &Theme, cx: &mut Context<Self>) -> Option<gpui_kit::Div> {
        let queued = thread.read(cx).queued_messages();
        if queued.is_empty() {
            return None;
        }
        let mut list = div().flex().flex_col().gap(px(6.)).mb(px(8.));
        for (i, (id, text, attachments)) in queued.into_iter().enumerate() {
            let (sending, taking) = {
                let c = &thread.read(cx).composer;
                (c.sending_now.contains(&id), c.restore_on_withdraw.contains(&id) || self.withdrawing.contains(&id))
            };
            let status = if sending {
                "Interrupting Claude to send this…"
            } else if taking {
                "Taking it back…"
            } else {
                "Queued · Claude reads it after its current step"
            };
            let label = match (text.is_empty(), attachments) {
                (false, _) => text,
                (true, 1) => "1 attachment".to_string(),
                (true, n) => format!("{n} attachments"),
            };
            let mut row = div()
                .flex()
                .items_start()
                .gap(px(8.))
                .px(px(10.))
                .py(px(7.))
                .rounded(px(10.))
                .border_1()
                .border_color(theme.separator)
                .bg(theme.card)
                .text_size(px(12.))
                .child(div().w(px(14.)).flex_none().text_color(theme.secondary).child(if sending { "↯" } else { "◷" }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(div().max_h(px(34.)).overflow_hidden().child(label))
                        .child(div().text_size(px(11.)).text_color(theme.secondary).child(status)),
                );
            if !sending && !taking {
                let w = self.workspace.downgrade();
                let (t1, t2, t3) = (thread.clone(), thread.clone(), thread.clone());
                let (i1, i2, i3) = (id.clone(), id.clone(), id.clone());
                let w2 = w.clone();
                row = row.child(
                    div()
                        .flex()
                        .flex_none()
                        .gap(px(2.))
                        .child(Button::new(SharedString::from(format!("queued-now-{i}"))).ghost().xsmall().label("Send Now").tooltip("Interrupt Claude and send this now").on_click(
                            move |_, _, cx| {
                                let _ = w.update(cx, |ws, cx| ws.send_now(&t1, &i1, cx));
                            },
                        ))
                        .child(Button::new(SharedString::from(format!("queued-edit-{i}"))).ghost().xsmall().label("Edit").tooltip("Take it back into the composer").on_click(
                            move |_, _, cx| {
                                let _ = w2.update(cx, |ws, cx| ws.withdraw(&t2, &i2, true, cx));
                            },
                        ))
                        .child(Button::new(SharedString::from(format!("queued-remove-{i}"))).ghost().xsmall().label("Remove").tooltip("Don't send it").on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.withdrawing.insert(i3.clone());
                                this.workspace.update(cx, |ws, cx| ws.withdraw(&t3, &i3, false, cx));
                            },
                        ))),
                );
            }
            list = list.child(row);
        }
        Some(list)
    }

    /// What waits to go with the next message: images as thumbnails, files and folders as chips.
    /// A click opens one; its menu opens it, shows it in Finder or removes it.
    fn attached_strip(&self, thread: &Entity<ThreadState>, theme: &Theme, cx: &mut Context<Self>) -> Option<gpui_kit::Div> {
        let key = thread.read(cx).id().to_string();
        let list = self.attached.get(&key).filter(|l| !l.is_empty())?;
        let dir = self.workspace.read(cx).data_dir.join("attachments");
        let mut strip = div().flex().flex_wrap().gap(px(6.));
        for (i, a) in list.iter().enumerate() {
            let path = match a {
                Attached::Image(name) => dir.join(name),
                Attached::File(f) => f.path.clone(),
            };
            let body = match a {
                Attached::Image(_) => div().child(gpui_kit::img(path.clone()).h(px(56.)).max_w(px(120.)).rounded(px(6.))),
                Attached::File(f) => file_chip(f, theme),
            };
            let (open, reveal) = (path.clone(), path.clone());
            let this = cx.entity().downgrade();
            let remove_key = key.clone();
            let chip = div()
                .id(("attached", i))
                .relative()
                .cursor_pointer()
                .child(body)
                .on_click(move |_, _, cx| cx.open_with_system(&open))
                .context_menu(move |menu, _, _| {
                    let (open, reveal, this, key) = (path.clone(), reveal.clone(), this.clone(), remove_key.clone());
                    menu.item(PopupMenuItem::new("Open").on_click(move |_, _, cx| cx.open_with_system(&open)))
                        .item(PopupMenuItem::new("Show in Finder").on_click(move |_, _, cx| cx.reveal_path(&reveal)))
                        .separator()
                        .item(PopupMenuItem::new("Remove").on_click(move |_, _, cx| {
                            let _ = this.update(cx, |this, cx| this.remove_attached(&key, i, cx));
                        }))
                })
                .child(
                    div()
                        .id(("remove-attached", i))
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
                        .on_click(cx.listener({
                            let key = key.clone();
                            move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.remove_attached(&key, i, cx);
                            }
                        })),
                );
            strip = strip.child(chip);
        }
        Some(strip)
    }

    fn remove_attached(&mut self, key: &str, index: usize, cx: &mut Context<Self>) {
        if let Some(list) = self.attached.get_mut(key) {
            if index < list.len() {
                list.remove(index);
            }
        }
        cx.notify();
    }
}

/// A file or folder sent with a message: a badge for its kind, its name and size. No preview:
/// Claude reads it, and a click opens it in its own app.
pub fn file_chip(file: &FileAttachment, theme: &Theme) -> gpui_kit::Div {
    let ext = file.path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let (badge, color) = file_kind(&ext, file.is_dir, theme);
    let detail = if file.is_dir {
        "Folder".to_string()
    } else {
        let kind = if ext.is_empty() { "File".to_string() } else { ext.to_uppercase() };
        file.size.map(|s| format!("{kind} · {}", byte_count(s))).unwrap_or(kind)
    };
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(56.))
        .max_w(px(240.))
        .pl(px(8.))
        .pr(px(12.))
        .rounded(px(8.))
        .border_1()
        .border_color(theme.separator)
        .bg(theme.code)
        .child(
            div()
                .flex_none()
                .w(px(28.))
                .h(px(28.))
                .rounded(px(6.))
                .bg(color)
                .text_color(gpui_kit::white())
                .text_size(px(9.))
                .font_weight(FontWeight::BOLD)
                .flex()
                .items_center()
                .justify_center()
                .child(badge),
        )
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).overflow_hidden().whitespace_nowrap().text_ellipsis().child(file.name()))
                .child(div().text_size(px(11.)).text_color(theme.secondary).overflow_hidden().whitespace_nowrap().text_ellipsis().child(detail)),
        )
}

/// A file's badge and its color, by kind.
fn file_kind(ext: &str, is_dir: bool, theme: &Theme) -> (String, gpui_kit::Hsla) {
    if is_dir {
        return ("DIR".into(), theme.accent);
    }
    let color = match ext {
        "pdf" => theme.error,
        "zip" | "gz" | "tgz" | "tar" | "dmg" | "7z" | "xz" => theme.warning,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "tiff" | "svg" | "psd" | "mov" | "mp4" => theme.string,
        "rs" | "swift" | "js" | "ts" | "tsx" | "jsx" | "py" | "go" | "c" | "h" | "cpp" | "java" | "kt" | "rb" | "sh" | "json" | "toml" | "yml" | "yaml" | "html" | "css" => theme.keyword,
        _ => theme.secondary,
    };
    let badge = if ext.is_empty() || ext.len() > 4 { "DOC".to_string() } else { ext.to_uppercase() };
    (badge, color)
}

fn byte_count(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} KB", b / 1e3)
    } else {
        format!("{bytes} bytes")
    }
}

/// Only when an MCP server needs you: one to sign in to, or one that won't start. Opens /mcp.
fn mcp_attention_chip(needing: &[(String, bool)], workspace: gpui_kit::WeakEntity<Workspace>, theme: &Theme) -> impl IntoElement {
    let label = match needing {
        [(name, true)] => format!("{name} won't start"),
        [(name, false)] => format!("Sign in to {name}"),
        _ => format!("{} MCP servers need you", needing.len()),
    };
    let help = format!(
        "MCP servers: {}",
        needing.iter().map(|(n, failed)| format!("{n} {}", if *failed { "won't start" } else { "needs sign-in" })).collect::<Vec<_>>().join(", ")
    );
    div()
        .id("mcp-attention")
        .flex_none()
        .px(px(9.))
        .py(px(3.))
        .rounded_full()
        .bg(theme.warning.opacity(0.14))
        .hover(|d| d.bg(theme.warning.opacity(0.22)))
        .text_color(theme.warning)
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .child(label)
        .tooltip(move |window, cx| Tooltip::new(help.clone()).build(window, cx))
        .on_click(move |_, _, cx| {
            let _ = workspace.update(cx, |ws, cx| ws.open_mcp(cx));
        })
}

/// A ring showing how full the context window is, like a battery for the conversation, and the
/// count beside it.
fn context_ring(tokens: u64, window: Option<u64>, theme: &Theme) -> impl IntoElement {
    use yh_core::claude::compact_tokens;
    let fraction = window.filter(|w| *w > 0).map(|w| (tokens as f32 / w as f32).min(1.)).unwrap_or(0.);
    let tint = if fraction < 0.6 {
        theme.secondary
    } else if fraction < 0.85 {
        theme.warning
    } else {
        theme.error
    };
    let track = theme.separator;
    let help = match window {
        Some(w) => format!("{} of {} tokens in context ({:.0}%)", grouped(tokens), grouped(w), fraction * 100.),
        None => format!("{} tokens in context", grouped(tokens)),
    };
    let text = match window {
        Some(w) => format!("{} / {}", compact_tokens(tokens), compact_tokens(w)),
        None => compact_tokens(tokens),
    };
    div()
        .id("context-ring")
        .flex_none()
        .flex()
        .items_center()
        .gap(px(6.))
        .mr(px(4.))
        .when(window.is_some(), |d| {
            d.child(
                gpui_kit::canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        let side = f32::from(bounds.size.width.min(bounds.size.height));
                        let (cx0, cy0) = (f32::from(bounds.origin.x) + side / 2., f32::from(bounds.origin.y) + side / 2.);
                        let r = side / 2. - 1.5;
                        let arc = |from: f32, to: f32| {
                            let mut path = gpui_kit::PathBuilder::stroke(px(2.5));
                            let steps = ((to - from) * 48.).ceil().max(1.) as usize;
                            for n in 0..=steps {
                                let a = (from + (to - from) * n as f32 / steps as f32) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                                let p = gpui_kit::point(px(cx0 + r * a.cos()), px(cy0 + r * a.sin()));
                                if n == 0 {
                                    path.move_to(p);
                                } else {
                                    path.line_to(p);
                                }
                            }
                            path.build().ok()
                        };
                        if let Some(p) = arc(0., 1.) {
                            window.paint_path(p, track);
                        }
                        if fraction > 0. {
                            if let Some(p) = arc(0., fraction.max(0.02)) {
                                window.paint_path(p, tint);
                            }
                        }
                    },
                )
                .w(px(14.))
                .h(px(14.)),
            )
        })
        .child(div().text_size(px(11.)).text_color(theme.secondary).child(text))
        .tooltip(move |window, cx| Tooltip::new(help.clone()).build(window, cx))
}

/// 1000000 → "1,000,000".
fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

struct Suggestion {
    label: String,
    detail: String,
    insert: String,
    /// ↩ sends this right away (a command that needs nothing more); ⇥ still completes.
    run: Option<String>,
}

struct Suggest {
    items: Vec<Suggestion>,
    index: usize,
    /// Where what's being completed starts (the `/` or `@`).
    start: usize,
}

impl Composer {
    /// `YH_DRAFT`: text put in the composer as if typed, the caret at its end.
    /// Adds text at the end of what's typed, on a line of its own.
    pub fn append_draft(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.input.read(cx).value().to_string();
        let separator = if current.is_empty() || current.ends_with('\n') { "" } else { "\n" };
        self.type_draft(&format!("{current}{separator}{text}\n"), window, cx);
    }

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
        // `/name` at the start (commands and skills), or as a word later on (skills to chain:
        // only what the agent can run itself).
        if let Some((start, q)) = yh_core::claude::trailing_command(before) {
            let q = q.to_lowercase();
            let project = thread.read(cx).meta.project_id.clone();
            let chained = start > 0;
            let commands = {
                let ws = self.workspace.read(cx);
                if chained {
                    ws.chainable_commands(&project)
                } else {
                    ws.menu_commands(&project)
                }
            };
            let rank = |c: &yh_core::claude::SlashCommand| {
                let name = c.name.to_lowercase();
                if q.is_empty() || name == q {
                    Some(0)
                } else if name.starts_with(&q) {
                    Some(1)
                } else if c.aliases.iter().any(|a| a.to_lowercase().starts_with(&q)) {
                    Some(2)
                } else if name.contains(&q) {
                    Some(3)
                } else {
                    None
                }
            };
            let mut ranked: Vec<(usize, &yh_core::claude::SlashCommand)> = commands.iter().filter_map(|c| rank(c).map(|r| (r, c))).collect();
            ranked.sort_by_key(|(r, _)| *r);
            let items: Vec<Suggestion> = ranked
                .into_iter()
                .take(12)
                .map(|(_, c)| Suggestion {
                    label: format!("/{}", c.name),
                    detail: if c.hint.is_empty() { c.description.clone() } else { format!("{} · {}", c.hint, c.description) },
                    insert: format!("/{} ", c.name),
                    // ↩ runs one that takes nothing more (`/compact`'s instructions are optional);
                    // later in a message it's only put in.
                    run: (!chained && (c.hint.is_empty() || c.hint.starts_with('[') || c.hint.to_lowercase().contains("optional"))).then(|| format!("/{}", c.name)),
                })
                .collect();
            self.suggest = (!items.is_empty()).then_some(Suggest { items, index: 0, start });
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
                        run: None,
                        detail: f,
                    })
                    .collect();
                this.suggest = (!items.is_empty()).then_some(Suggest { items, index: 0, start: at });
                cx.notify();
            });
        }));
    }

    /// Takes a menu item: `run` sends a command that needs nothing more, else it's put in.
    fn accept(&mut self, index: usize, run: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.suggest.take() else { return };
        let Some(choice) = s.items.into_iter().nth(index) else { return };
        if let Some(command) = choice.run.filter(|_| run) {
            self.input.update(cx, |i, cx| i.set_value(command, window, cx));
            self.submit(false, window, cx);
            return;
        }
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
            "enter" | "tab" if !event.keystroke.modifiers.shift && !event.keystroke.modifiers.alt => {
                let index = s.index;
                let run = event.keystroke.key == "enter";
                self.accept(index, run, window, cx);
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
                    .on_mouse_down(gpui_kit::MouseButton::Left, cx.listener(move |this, _, window, cx| this.accept(i, true, window, cx))),
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

/// What Claude hears when you skip its question (the Swift app's words).
const SKIP_MESSAGE: &str = "The user chose not to answer. Continue with your best judgment, or ask in chat.";

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

    /// Every question has an answer: an option picked, or one written.
    fn all_answered(&self, cx: &App) -> bool {
        self.questions.iter().all(|q| !q.chosen.is_empty() || !q.other.read(cx).value().trim().is_empty())
    }

    fn submit(&self, cx: &mut App) {
        if !self.all_answered(cx) {
            return;
        }
        let input = self.answered_input(cx);
        let Some(ws) = self.workspace.as_ref().and_then(|w| w.upgrade()) else { return };
        if let Some(t) = ws.read(cx).selected_thread() {
            ws.update(cx, |ws, cx| ws.respond(&t, Decision::Allow { updated_input: Some(input), mode: None }, cx));
        }
    }

    fn skip(&self, cx: &mut App) {
        let Some(ws) = self.workspace.as_ref().and_then(|w| w.upgrade()) else { return };
        if let Some(t) = ws.read(cx).selected_thread() {
            ws.update(cx, |ws, cx| ws.respond(&t, Decision::Deny { message: SKIP_MESSAGE.into() }, cx));
        }
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
            // With descriptions, each option on its own line with what it means beside it.
            let described = q.options.iter().any(|o| o.1.is_some());
            let mut options = if described { div().flex().flex_col().gap(px(4.)) } else { div().flex().flex_wrap().gap(px(6.)) };
            for (oi, (label, description)) in q.options.iter().enumerate() {
                let on = q.chosen.contains(&oi);
                let button = Button::new(SharedString::from(format!("q{qi}-{oi}")))
                        .small()
                        .when(on, |b| b.primary())
                        .label(label.clone())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let q = &mut this.questions[qi];
                            if !q.chosen.remove(&oi) {
                                if !q.multi {
                                    q.chosen.clear();
                                }
                                q.chosen.insert(oi);
                            }
                            cx.notify();
                        }));
                options = options.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(div().flex_none().child(button))
                        .when_some(description.clone().filter(|_| described), |d, text| d.child(div().min_w_0().text_size(px(12.)).text_color(theme.secondary).child(text))),
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
        let ready = self.all_answered(cx);
        div().id("question-card").max_h(px(460.)).overflow_y_scroll().child(card.child(
            div()
                .flex()
                .justify_end()
                .gap(px(8.))
                .child(Button::new("skip").small().label("Skip").tooltip("⌘⌫").on_click(cx.listener(|this, _, _, cx| this.skip(cx))))
                .child(
                    Button::new("answer")
                        .small()
                        .primary()
                        .label("Answer")
                        .tooltip(if ready { "⌘↩" } else { "Answer every question first" })
                        .disabled(!ready)
                        .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                ),
        ))
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

impl PlanCard {
    /// ⌘↩: approve in the recommended mode (Auto).
    fn approve(&self, cx: &mut App) {
        let Some(ws) = self.workspace.upgrade() else { return };
        if let Some(t) = ws.read(cx).selected_thread() {
            ws.update(cx, |ws, cx| ws.approve_plan(&t, "auto", cx));
        }
    }

    /// ⌘⌫: keep planning, with what's written as feedback.
    fn keep_planning(&self, cx: &mut App) {
        let message = self.feedback.read(cx).value().trim().to_string();
        let message = if message.is_empty() { "Keep planning.".to_string() } else { message };
        let Some(ws) = self.workspace.upgrade() else { return };
        if let Some(t) = ws.read(cx).selected_thread() {
            ws.update(cx, |ws, cx| ws.respond(&t, Decision::Deny { message }, cx));
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
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Plan ready for review"))
            .child(div().id("plan-body").max_h(px(320.)).overflow_y_scroll().child(rows::markdown_view(&self.plan, &theme)))
            .child(Input::new(&self.feedback).small())
            .child(buttons)
    }
}
