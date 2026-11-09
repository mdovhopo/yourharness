//! The composer and the cards above it. ↩ sends; while Claude works it steers (joins the turn
//! after the current step), ⌘↩ stops the turn and sends now. A draft stays with its thread.
//! Model, effort and mode are pickers from the CLI's catalog. Cards: a tool waiting for
//! permission, a question Claude asks (`AskUserQuestion`), a plan to approve (`ExitPlanMode`).

use crate::state::thread::{Permission, ThreadState};
use crate::state::workspace::Workspace;
use crate::theme::{radius, space, text, Theme, MONO};
use crate::ui::icons;
use crate::ui::rows;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{DeleteToBeginningOfLine, Enter, Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Disableable, Icon, IconName, Sizable};
use gpui_kit::{div, prelude::*, px, App, Context, Entity, Focusable, FontWeight, SharedString, Subscription, Window};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet};
use yh_core::attachments::FileAttachment;
use std::rc::Rc;
use yh_core::claude::Decision;
use yh_core::markdown;

/// Built-in models until the CLI's catalog arrives (it does on the first start).
const FALLBACK_MODELS: &[(&str, &str)] = &[("haiku", "Haiku"), ("sonnet", "Sonnet"), ("opus", "Opus")];
/// The composer grows to this many lines (its 320 pt cap), then scrolls.
const COMPOSER_MAX_ROWS: usize = 15;
pub const MODES: &[(&str, &str)] =
    &[("auto", "Auto"), ("ask", "Ask"), ("acceptEdits", "Accept Edits"), ("plan", "Plan"), ("bypass", "Bypass")];

pub struct Composer {
    workspace: Entity<Workspace>,
    /// Its height when last laid out, for the cached view that places it (see `measure`).
    pub height: Rc<std::cell::Cell<f32>>,
    /// Told after a frame where the height changed: whoever places the composer.
    pub on_height: Option<gpui_kit::AnyWeakEntity>,
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
    pub withdrawing: HashSet<String>,
    /// The placeholder says how to steer (Claude works in the open thread).
    steering: bool,
    /// The "/" menu's scroll, kept on the item picked with the arrows.
    suggest_scroll: gpui_kit::ScrollHandle,
    _mentions: Option<gpui_kit::Task<()>>,
    /// An MCP server's question, as a card.
    elicit: Option<(String, Entity<ElicitCard>)>,
    /// Nothing typed (Send greyed out), kept so typing redraws the composer only when it flips.
    empty: bool,
    _subscriptions: Vec<Subscription>,
}

/// What an MCP server asks mid-call: a form, its fields filled with their defaults (yes/no as
/// a switch, a choice as a menu, the rest typed), or a page to visit. Accept sends the fields.
pub struct ElicitCard {
    asked: crate::state::thread::Elicitation,
    fields: Vec<ElicitField>,
    workspace: gpui_kit::WeakEntity<Workspace>,
}

struct ElicitField {
    key: String,
    label: String,
    required: bool,
    input: FieldInput,
}

enum FieldInput {
    Text(Entity<InputState>),
    Flag(bool),
    /// The value picked, and the choices as (value, label).
    Choice(Option<String>, Vec<(String, String)>),
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

/// A property's choices, if it's a choice: `enum` (with `enumNames`), or `oneOf` of `const`s.
fn choices(p: &Value) -> Option<Vec<(String, String)>> {
    let text = |v: &Value| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
    if let Some(values) = p["enum"].as_array() {
        let names = p["enumNames"].as_array();
        return Some(values.iter().enumerate().map(|(i, v)| (text(v), names.and_then(|n| n.get(i)).map(text).unwrap_or_else(|| text(v)))).collect());
    }
    let one_of = p["oneOf"].as_array().or(p["anyOf"].as_array())?;
    let list: Vec<(String, String)> = one_of.iter().filter(|o| !o["const"].is_null()).map(|o| (text(&o["const"]), o["title"].as_str().map(str::to_string).unwrap_or_else(|| text(&o["const"])))).collect();
    (!list.is_empty()).then_some(list)
}

impl ElicitCard {
    fn new(asked: crate::state::thread::Elicitation, workspace: gpui_kit::WeakEntity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut fields = vec![];
        let required: Vec<&str> = asked.schema["required"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        if let Some(props) = asked.schema["properties"].as_object() {
            for (k, p) in props {
                let label = p["title"].as_str().or(p["description"].as_str()).unwrap_or(k).to_string();
                let input = if p["type"].as_str() == Some("boolean") {
                    FieldInput::Flag(p["default"].as_bool().unwrap_or(false))
                } else if let Some(list) = choices(p) {
                    let picked = p["default"].as_str().map(str::to_string).filter(|d| list.iter().any(|(v, _)| v == d));
                    FieldInput::Choice(picked, list)
                } else {
                    let default = match &p["default"] {
                        Value::String(s) => s.clone(),
                        Value::Null => String::new(),
                        other => other.to_string(),
                    };
                    let hint = p["description"].as_str().unwrap_or("").to_string();
                    FieldInput::Text(cx.new(|cx| InputState::new(window, cx).placeholder(hint).default_value(default)))
                };
                fields.push(ElicitField { key: k.clone(), label, required: required.contains(&k.as_str()), input });
            }
        }
        ElicitCard { asked, fields, workspace }
    }

    /// The fields as the schema's types: numbers parsed, yes/no and choices as they are, the
    /// rest text; None while a required one is empty or a number isn't one.
    fn content(&self, cx: &App) -> Option<Value> {
        let mut out = serde_json::Map::new();
        for f in &self.fields {
            let v = match &f.input {
                FieldInput::Flag(on) => Value::Bool(*on),
                FieldInput::Choice(picked, _) => picked.clone().map(Value::from).unwrap_or(Value::Null),
                FieldInput::Text(input) => {
                    let text = input.read(cx).value().to_string();
                    match self.asked.schema["properties"][&f.key]["type"].as_str().unwrap_or("string") {
                        "number" | "integer" if text.trim().is_empty() => Value::Null,
                        "number" | "integer" => Value::from(text.trim().parse::<f64>().ok()?),
                        _ if text.is_empty() => Value::Null,
                        _ => Value::String(text),
                    }
                }
            };
            if v.is_null() {
                if f.required {
                    return None;
                }
                continue;
            }
            out.insert(f.key.clone(), v);
        }
        Some(Value::Object(out))
    }

    fn answer(&self, action: &str, cx: &mut Context<Self>) {
        let content = if action == "accept" && self.asked.mode == "form" {
            match self.content(cx) {
                Some(c) => Some(c),
                None => return,
            }
        } else {
            None
        };
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
            .child(card_title(icons::plug(), format!("{} asks", self.asked.server), &theme))
            .child(div().text_size(px(text::BODY)).whitespace_normal().child(self.asked.message.clone()));
        for (i, f) in self.fields.iter().enumerate() {
            let label = format!("{}{}", f.label, if f.required { "" } else { " (optional)" });
            let field: gpui_kit::AnyElement = match &f.input {
                FieldInput::Text(input) => Input::new(input).small().into_any_element(),
                FieldInput::Flag(on) => gpui_kit::component::switch::Switch::new(SharedString::from(format!("elicit-flag-{i}")))
                    .checked(*on)
                    .on_click(cx.listener(move |this, on: &bool, _, cx| {
                        if let Some(FieldInput::Flag(v)) = this.fields.get_mut(i).map(|f| &mut f.input) {
                            *v = *on;
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
                FieldInput::Choice(picked, list) => {
                    let shown = picked.as_ref().and_then(|p| list.iter().find(|(v, _)| v == p)).map(|(_, l)| l.clone()).unwrap_or_else(|| "—".into());
                    let (list, entity, optional) = (list.clone(), cx.entity().downgrade(), !f.required);
                    Button::new(SharedString::from(format!("elicit-choice-{i}")))
                        .small()
                        .outline()
                        .label(shown)
                        .dropdown_caret(true)
                        .dropdown_menu(move |mut menu, _, _| {
                            let pick = |value: Option<String>| {
                                let entity = entity.clone();
                                move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
                                    let value = value.clone();
                                    let _ = entity.update(cx, |this, cx| {
                                        if let Some(FieldInput::Choice(p, _)) = this.fields.get_mut(i).map(|f| &mut f.input) {
                                            *p = value;
                                        }
                                        cx.notify();
                                    });
                                }
                            };
                            if optional {
                                menu = menu.item(PopupMenuItem::new("—").on_click(pick(None)));
                            }
                            for (value, label) in &list {
                                menu = menu.item(PopupMenuItem::new(label.clone()).on_click(pick(Some(value.clone()))));
                            }
                            menu
                        })
                        .into_any_element()
                }
            };
            card = card.child(if matches!(f.input, FieldInput::Flag(_)) {
                div().flex().items_center().gap(px(space::S)).child(field).child(div().text_size(px(text::FOOTNOTE)).child(f.label.clone())).into_any_element()
            } else {
                div().flex().flex_col().gap(px(space::XS)).child(div().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(label)).child(div().flex().child(field)).into_any_element()
            });
        }
        let url = self.asked.url.clone();
        let ready = self.asked.mode != "form" || self.content(cx).is_some();
        card.child(
            card_buttons()
                .when_some(url, |d, url| {
                    d.child(Button::new("elicit-open").small().ghost().icon(Icon::new(IconName::ExternalLink).size(px(13.))).label("Open Page").on_click(move |_, _, cx| {
                        if std::env::var("YH_MCP_BROWSER").as_deref() == Ok("log") {
                            eprintln!("MCP page: {url}");
                        } else {
                            cx.open_url(&url);
                        }
                    }))
                })
                .child(Button::new("elicit-decline").small().outline().label("Decline").on_click(cx.listener(|this, _, _, cx| this.answer("decline", cx))))
                .child(Button::new("elicit-accept").small().primary().label(if self.asked.mode == "url" { "Done" } else { "Send" }).disabled(!ready).on_click(cx.listener(|this, _, _, cx| this.answer("accept", cx)))),
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

    /// `YH_KEYS=<step>[;<step>…]`, for tests of the cards' keys: once a question or plan card
    /// shows (and `YH_KEYS_DELAY` s more, 2), each step a second apart through the window as
    /// typed: a keystroke (`cmd-enter`), `text:<words>` typed, or `focus:composer`,
    /// `focus:other` (the first question's own answer; `focus:other2` the second's…),
    /// `focus:feedback` (the plan's).
    fn key_probe(keys: String, window: &mut Window, cx: &mut Context<Self>) {
        let delay: f64 = std::env::var("YH_KEYS_DELAY").ok().and_then(|v| v.parse().ok()).unwrap_or(2.0);
        cx.spawn_in(window, async move |this, cx| {
            use std::time::Duration;
            loop {
                cx.background_executor().timer(Duration::from_millis(250)).await;
                match this.read_with(cx, |c, _| matches!(c.card, Some((_, Card::Question(_) | Card::Plan(_))))) {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(_) => return,
                }
            }
            eprintln!("KEYS card shown");
            cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
            for step in keys.split(';').map(str::trim).filter(|s| !s.is_empty()) {
                if let Some(target) = step.strip_prefix("focus:") {
                    let _ = this.update_in(cx, |c, window, cx| {
                        let handle = match (target, c.card.as_ref().map(|(_, k)| k)) {
                            (t, Some(Card::Question(q))) if t.starts_with("other") => {
                                let n: usize = t["other".len()..].parse().unwrap_or(1);
                                q.read(cx).questions.get(n.saturating_sub(1)).map(|q| q.other.focus_handle(cx))
                            }
                            ("feedback", Some(Card::Plan(p))) => Some(p.read(cx).feedback.focus_handle(cx)),
                            ("composer", _) => Some(c.input.focus_handle(cx)),
                            _ => None,
                        };
                        match handle {
                            Some(h) => window.focus(&h, cx),
                            None => eprintln!("KEYS no field {target}"),
                        }
                    });
                } else {
                    let strokes: Vec<String> = match step.strip_prefix("text:") {
                        Some(text) => text.chars().map(|c| if c == ' ' { "space".into() } else { c.to_string() }).collect(),
                        None => vec![step.to_string()],
                    };
                    for k in strokes {
                        let parsed = gpui_kit::Keystroke::parse_with_os(&k, gpui_kit::OperatingSystem::Mac).map_err(|_| ());
                        let Ok(ks) = parsed else {
                            eprintln!("KEYS can't read {k}");
                            continue;
                        };
                        // A window update, not the composer's: the key's handlers update it.
                        let _ = cx.update(|window, cx| window.dispatch_keystroke(ks, cx));
                    }
                }
                let card = this.read_with(cx, |c, cx| match c.card.as_ref().map(|(_, k)| k) {
                    Some(Card::Question(q)) => format!("question, Answer {}", if q.read(cx).all_answered(cx) { "on" } else { "off" }),
                    Some(Card::Plan(_)) => "plan".into(),
                    Some(Card::Tool) => "permission".into(),
                    None => "none".into(),
                });
                eprintln!("KEYS {step} -> card {}", card.unwrap_or_default());
                cx.background_executor().timer(Duration::from_secs(1)).await;
            }
        })
        .detach();
    }

    /// ⌘↩ inside the composer (its field or a card's): the input propagates `Enter`, and this
    /// takes it so the window's Allow binding never answers a second request too. With a draft
    /// in the composer it sends now (the field's own `PressEnter`); otherwise it answers the
    /// card: a question (once every one is answered), a plan in the recommended mode, a tool.
    fn on_enter(&mut self, action: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        if !action.secondary || action.shift {
            return cx.propagate();
        }
        let draft = self.input.focus_handle(cx).is_focused(window) && !self.input.read(cx).value().trim().is_empty();
        if draft || self.card_shortcut(true, cx) {
            return;
        }
        if let Some(thread) = self.thread(cx).filter(|t| t.read(cx).permission().is_some()) {
            self.workspace.update(cx, |ws, cx| ws.respond(&thread, Decision::allow(), cx));
        }
    }

    /// ⌘⌫ with a card waiting: Skip a question, Keep Planning, Deny a tool, unless a field with
    /// something in it has the focus, which deletes to the start of the line as usual.
    fn on_delete_line(&mut self, _: &DeleteToBeginningOfLine, window: &mut Window, cx: &mut Context<Self>) {
        let typing = match self.card.as_ref().map(|(_, c)| c) {
            Some(Card::Question(q)) => q.read(cx).typing(window, cx),
            Some(Card::Plan(p)) => p.read(cx).typing(window, cx),
            _ => false,
        } || (self.input.focus_handle(cx).is_focused(window) && !self.input.read(cx).value().is_empty());
        let Some(thread) = self.thread(cx).filter(|t| t.read(cx).permission().is_some()) else { return cx.propagate() };
        if typing {
            return cx.propagate();
        }
        cx.stop_propagation();
        if !self.card_shortcut(false, cx) {
            self.workspace.update(cx, |ws, cx| ws.respond(&thread, Decision::deny(), cx));
        }
    }

    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Grows with what's in it, typed or put there (a pick from the browser, a queued message
        // taken back), up to the field's cap, then scrolls.
        let input = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx).placeholder("Ask Claude…").submit_on_enter(true);
            state.set_auto_grow(1, COMPOSER_MAX_ROWS, cx);
            state
        });
        let subscriptions = vec![
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { shift: false, secondary } => this.enter(*secondary, window, cx),
                InputEvent::Blur => {
                    this.save_draft(cx);
                    this.suggest = None;
                }
                InputEvent::Change => {
                    this.update_suggestions(window, cx);
                    // Send greys out with nothing to send: a redraw only when that flips.
                    let empty = this.input.read(cx).value().trim().is_empty();
                    if empty != this.empty {
                        this.empty = empty;
                        cx.notify();
                    }
                }
                _ => {}
            }),
            cx.observe_in(&workspace, window, |this, _, window, cx| {
                this.sync_card(window, cx);
                this.take_restored(window, cx);
                cx.notify();
            }),
        ];
        if let Ok(keys) = std::env::var("YH_KEYS") {
            Self::key_probe(keys, window, cx);
        }
        Composer {
            workspace,
            height: Rc::new(std::cell::Cell::new(0.)),
            on_height: None,
            input,
            current: None,
            card: None,
            plan_open: false,
            suggest: None,
            attached: HashMap::new(),
            withdrawing: HashSet::new(),
            _mentions: None,
            elicit: None,
            empty: true,
            steering: false,
            suggest_scroll: gpui_kit::ScrollHandle::new(),
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

    /// Kept with the thread: on leaving the field, switching threads and quitting.
    pub fn save_draft(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.current.clone() else { return };
        let text = self.input.read(cx).value().to_string();
        self.workspace.update(cx, |ws, cx| ws.save_draft(&id, &text, cx));
    }

    /// ↩ sends (steering while Claude works); ⌘↩ answers a waiting tool, or stops the turn and
    /// sends now.
    fn enter(&mut self, secondary: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = self.thread(cx) else { return };
        // ⌘↩ with nothing typed and a card waiting was answered by `on_enter`.
        if secondary && thread.read(cx).waiting_for_you() && self.input.read(cx).value().trim().is_empty() {
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
        if thread.read(cx).run.composer.restored.is_empty() {
            return;
        }
        let restored = thread.update(cx, |t, _| std::mem::take(&mut t.run.composer.restored));
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
        let asked = self.thread(cx).and_then(|t| t.read(cx).elicitation().cloned());
        match (asked, &self.elicit) {
            (None, Some(_)) => self.elicit = None,
            (Some(a), current) if current.as_ref().is_none_or(|(id, _)| *id != a.id) => {
                let ws = self.workspace.downgrade();
                let id = a.id.clone();
                self.elicit = Some((id, cx.new(|cx| ElicitCard::new(a, ws, window, cx))));
            }
            _ => {}
        }
        let permission = self.thread(cx).and_then(|t| t.read(cx).permission().cloned());
        match (&permission, &self.card) {
            (None, None) => {}
            (Some(p), Some((id, _))) if *id == p.id => {}
            (None, Some(_)) => self.card = None,
            (Some(p), _) => {
                let ws = self.workspace.downgrade();
                let card = match p.tool.as_str() {
                    "AskUserQuestion" => match QuestionCard::new(p, ws, window, cx) {
                        Some(card) => Card::Question(card),
                        None => Card::Tool,
                    },
                    "ExitPlanMode" => Card::Plan(cx.new(|cx| PlanCard::new(p, ws, window, cx))),
                    _ => Card::Tool,
                };
                self.card = Some((p.id.clone(), card));
            }
        }
    }

    /// Model, effort and permissions, as chips: each menu says what its choices
    /// mean, with a footnote on when they apply.
    fn pickers(&self, thread: &Entity<ThreadState>, compact: bool, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = self.workspace.read(cx);
        let t = thread.read(cx);
        let id = t.id().to_string();
        let project = t.meta.project_id.clone();
        let project_name = ws.project(&project).map(|p| p.name.clone()).unwrap_or_default();
        let mode = ws.project(&project).map_or("auto".to_string(), |p| p.mode.clone());
        let model = ws.model_of(t);
        let effort = ws.effort_of(t);
        let model_name = ws.model_name(t);
        let auto_ok = ws.supports_auto_mode(t);
        let models: Vec<PickerModel> = if ws.models.is_empty() {
            FALLBACK_MODELS.iter().map(|(v, n)| PickerModel { value: v.to_string(), name: n.to_string(), description: None, efforts: vec![], alias: true }).collect()
        } else {
            ws.models
                .iter()
                .map(|m| PickerModel { value: m.value.clone(), name: m.display_name.clone(), description: m.description.clone(), efforts: m.efforts.clone(), alias: m.is_alias() })
                .collect()
        };
        let current_model = model.clone().unwrap_or_else(|| "default".into());
        let efforts = models.iter().find(|m| m.value == current_model).map(|m| m.efforts.clone()).unwrap_or_default();
        let mode_label = MODES.iter().find(|(v, _)| *v == mode).map_or("Auto", |(_, l)| l);
        let models = Rc::new(models);
        let w = self.workspace.downgrade();
        let (secondary, error) = (theme.secondary, theme.error);
        let model_menu = {
            let (w, id, models, current) = (w.clone(), id.clone(), models.clone(), current_model.clone());
            Button::new("model").ghost().small().flex_none().icon(icons::sparkles().size(px(13.)).text_color(secondary)).child(picker_label(model_name.clone(), secondary)).accessibility_label(format!("Model: {model_name}")).tooltip("Model").dropdown_caret(true).dropdown_menu(move |mut menu, window, cx| {
                menu = menu.min_w(px(260.)).label("Claude Code");
                let pick = |value: Option<String>| {
                    let (w, id) = (w.clone(), id.clone());
                    move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
                        let _ = w.update(cx, |ws, cx| ws.set_model(&id, value.clone(), cx));
                    }
                };
                // The catalog lists "default" itself; without it, Default comes first.
                if !models.iter().any(|m| m.value == "default") {
                    menu = menu.item(picker_row("Default".into(), None, secondary, None).checked(current == "default").on_click(pick(None)));
                }
                for m in models.iter().filter(|m| m.alias) {
                    // The catalog names the default itself ("Default (recommended)"), its model in the description.
                    let title = m.name.clone();
                    let target = (m.value != "default").then(|| m.value.clone());
                    menu = menu.item(picker_row(title, m.description.clone(), secondary, None).checked(current == m.value).on_click(pick(target)));
                }
                let pinned: Vec<PickerModel> = models.iter().filter(|m| !m.alias).cloned().collect();
                if !pinned.is_empty() {
                    let (w, id, current) = (w.clone(), id.clone(), current.clone());
                    menu = menu.separator().submenu("Pinned Versions", window, cx, move |mut sub, _, _| {
                        for m in &pinned {
                            let (w, id, value) = (w.clone(), id.clone(), m.value.clone());
                            sub = sub.item(picker_row(m.name.clone(), m.description.clone(), secondary, None).checked(current == m.value).on_click(move |_, _, cx| {
                                let _ = w.update(cx, |ws, cx| ws.set_model(&id, Some(value.clone()), cx));
                            }));
                        }
                        sub
                    });
                }
                menu.separator().item(crate::ui::page_kit::menu_note("Switches instantly. Set the default in Settings."))
            })
        };
        let effort_menu = (!efforts.is_empty()).then(|| {
            let (w, id, current) = (w.clone(), id.clone(), effort.clone());
            let efforts = efforts.clone();
            let label = effort.as_deref().map(effort_label).unwrap_or("Auto").to_string();
            Button::new("effort").ghost().small().flex_none().icon(icons::gauge().size(px(13.)).text_color(secondary)).accessibility_label(format!("Reasoning effort: {label}")).when(!compact, |b| b.child(picker_label(label, secondary))).tooltip("Reasoning effort").dropdown_caret(!compact).dropdown_menu(move |mut menu, _, _| {
                menu = menu.min_w(px(240.)).label("Reasoning Effort");
                {
                    let (w, id) = (w.clone(), id.clone());
                    menu = menu.item(picker_row("Auto".into(), Some("Let Claude Code decide".into()), secondary, None).checked(current.is_none()).on_click(move |_, _, cx| {
                        let _ = w.update(cx, |ws, cx| ws.set_effort(&id, None, cx));
                    }));
                }
                for e in &efforts {
                    let (w, id, e) = (w.clone(), id.clone(), e.clone());
                    let checked = current.as_deref() == Some(e.as_str());
                    menu = menu.item(picker_row(effort_label(&e).to_string(), effort_detail(&e).map(str::to_string), secondary, None).checked(checked).on_click(move |_, _, cx| {
                        let _ = w.update(cx, |ws, cx| ws.set_effort(&id, Some(e.clone()), cx));
                    }));
                }
                menu.separator().item(crate::ui::page_kit::menu_note("Applies from your next message."))
            })
        });
        let mode_menu = {
            let (w, project, current) = (w.clone(), project.clone(), mode.clone());
            let help = if mode == "auto" && !auto_ok { format!("{model_name} doesn't support Auto, so Claude asks before changes") } else { format!("Permissions for {project_name}") };
            let model_name = model_name.clone();
            let mode_icon = if mode == "bypass" { icons::shield().text_color(error) } else if mode == "auto" { icons::shield_check().text_color(secondary) } else { icons::shield().text_color(secondary) };
            Button::new("mode").ghost().small().flex_none().icon(mode_icon.size(px(13.))).accessibility_label(format!("Permissions: {mode_label}")).when(!compact, |b| b.child(picker_label(mode_label.to_string(), secondary))).tooltip(help).dropdown_caret(!compact).dropdown_menu(move |mut menu, _, _| {
                menu = menu.min_w(px(280.)).label(format!("Permissions · {project_name}"));
                for (value, _) in MODES {
                    let (w, project) = (w.clone(), project.clone());
                    let (title, detail) = mode_detail(value);
                    let detail = if *value == "auto" && !auto_ok { format!("Not available with {model_name}. Claude asks instead.") } else { detail.to_string() };
                    let tint = (*value == "bypass").then_some(error);
                    menu = menu.item(picker_row(title.to_string(), Some(detail), secondary, tint).checked(current == *value).on_click(move |_, _, cx| {
                        let _ = w.update(cx, |ws, cx| ws.set_mode(&project, value, cx));
                    }));
                }
                menu.separator().item(crate::ui::page_kit::menu_note("Applies to every thread in this project, right away."))
            })
        };
        div()
            .flex()
            .items_center()
            .gap(px(2.))
            .text_size(px(text::FOOTNOTE))
            .text_color(theme.secondary)
            .child(model_menu)
            .children(effort_menu)
            .child(mode_menu)
    }
}

/// A picker's label: footnote size, secondary, so the row stays quiet next to Send.
fn picker_label(label: String, color: gpui_kit::Hsla) -> gpui_kit::Div {
    div().text_size(px(text::FOOTNOTE)).text_color(color).child(label)
}

/// A model as the picker lists it.
#[derive(Clone)]
struct PickerModel {
    value: String,
    name: String,
    description: Option<String>,
    efforts: Vec<String>,
    /// Follows the latest model (`opus`), as against a pinned version.
    alias: bool,
}

/// A picker's row: its name, and what it means under it.
fn picker_row(title: String, subtitle: Option<String>, secondary: gpui_kit::Hsla, tint: Option<gpui_kit::Hsla>) -> PopupMenuItem {
    PopupMenuItem::element(move |_, _| {
        div()
            .flex()
            .flex_col()
            .py(px(3.))
            .child(div().when_some(tint, |d, c| d.text_color(c)).child(title.clone()))
            .when_some(subtitle.clone(), |d, s| d.child(div().text_size(px(text::CAPTION)).line_height(px(14.)).text_color(secondary).whitespace_normal().max_w(px(260.)).child(s)))
    })
}

/// Effort levels' names, as the menu shows them.
fn effort_label(e: &str) -> &str {
    match e {
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        "xhigh" => "Extra High",
        "max" => "Max",
        other => other,
    }
}

fn effort_detail(e: &str) -> Option<&'static str> {
    Some(match e {
        "low" => "Quickest replies, light reasoning",
        "medium" => "Balanced speed and depth",
        "high" => "Thinks things through",
        "xhigh" => "Deep reasoning for hard problems",
        "max" => "Everything it's got. Slowest.",
        _ => return None,
    })
}

/// A permission mode's long name and what it does.
fn mode_detail(mode: &str) -> (&'static str, &'static str) {
    match mode {
        "ask" => ("Ask Before Changes", "Claude asks before editing files or running commands."),
        "acceptEdits" => ("Auto-Accept Edits", "File edits go through. Commands still ask."),
        "plan" => ("Plan Only", "Claude explores and plans, but changes nothing."),
        "bypass" => ("Bypass Permissions", "Everything runs without asking. Use with care."),
        _ => ("Auto", "Claude runs what looks safe and asks when something looks risky."),
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

/// A card above the composer: something waiting for you, so an orange hairline (the colour for
/// "needs you"), on the composer's own card colour and radius.
fn card_frame(theme: &Theme) -> gpui_kit::Div {
    div()
        .border_1()
        .border_color(theme.warning.opacity(0.55))
        .rounded(px(radius::LARGE))
        .bg(theme.card)
        .px(px(space::M + 2.))
        .py(px(space::M))
        .mb(px(space::S))
        .flex()
        .flex_col()
        .gap(px(space::S + 2.))
}

/// A card's title row: its icon, then what it asks, in the headline style.
fn card_title(icon: Icon, title: impl IntoElement, theme: &Theme) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .gap(px(space::S))
        .child(icon.size(px(16.)).text_color(theme.warning))
        .child(div().min_w_0().text_size(px(text::HEADLINE)).font_weight(FontWeight::SEMIBOLD).child(title))
}

/// A card's buttons, right-aligned.
fn card_buttons() -> gpui_kit::Div {
    div().flex().flex_wrap().justify_end().items_center().gap(px(space::S))
}

/// A tool waiting for your permission: the tool's icon,
/// "Allow Run command?", the argument that matters and Claude's description. The buttons sit
/// beside it, or under it when the card is narrow (the side chat). `more`: requests waiting
/// after this one (parallel sub-agents), each shown in turn.
pub fn permission_card(p: &Permission, thread: &Entity<ThreadState>, workspace: gpui_kit::WeakEntity<Workspace>, more: usize, theme: &Theme) -> gpui_kit::Div {
    let input: Value = serde_json::from_str(&p.input).unwrap_or(Value::Null);
    let shown = crate::ui::steps::present(&p.tool, &input, None);
    // A command shows whole (up to four lines), as it will run.
    let detail = match input["command"].as_str() {
        Some(c) if p.tool == "Bash" => c.trim().to_string(),
        _ => shown.detail.clone(),
    };
    let description = Some(p.description.trim().to_string()).filter(|d| !d.is_empty() && *d != detail);
    let (allow, deny) = (workspace.clone(), workspace);
    let (t1, t2) = (thread.clone(), thread.clone());
    let (id1, id2) = (p.id.clone(), p.id.clone());
    let summary = div()
        .flex_1()
        .min_w(px(260.))
        .flex()
        .items_start()
        .gap(px(10.))
        .child(div().flex_none().w(px(28.)).h(px(28.)).rounded(px(radius::MEDIUM)).bg(theme.warning.opacity(0.14)).flex().items_center().justify_center().child(rows::icon(shown.icon, 15., theme.warning)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(div().pt(px(1.)).text_size(px(text::HEADLINE)).font_weight(FontWeight::SEMIBOLD).child(format!("Allow {}?", shown.verb)))
                .when(!detail.is_empty(), |d| d.child(div().font_family(MONO).text_size(px(text::CODE)).line_height(px(text::CODE_LINE)).line_clamp(4).whitespace_normal().child(detail)))
                .when_some(description, |d, t| d.child(div().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).whitespace_normal().child(t)))
                .when(more > 0, |d| {
                    d.child(div().text_size(px(text::CAPTION)).text_color(theme.secondary).child(if more == 1 { "1 more request waiting".to_string() } else { format!("{more} more requests waiting") }))
                }),
        );
    let buttons = div()
        .flex_none()
        .ml_auto()
        .flex()
        .items_center()
        .gap(px(space::S))
        .child(Button::new(SharedString::from(format!("deny-{}", p.id))).small().outline().label("Deny").tooltip("Deny (⌘⌫)").on_click(move |_, _, cx| {
            let _ = deny.update(cx, |ws, cx| ws.respond_to(&t2, &id2, Decision::deny(), cx));
        }))
        .child(Button::new(SharedString::from(format!("allow-{}", p.id))).small().primary().label("Allow").tooltip("Allow (⌘↩)").on_click(move |_, _, cx| {
            let _ = allow.update(cx, |ws, cx| ws.respond_to(&t1, &id1, Decision::allow(), cx));
        }));
    card_frame(theme).child(div().flex().flex_wrap().items_center().gap(px(space::M)).child(summary).child(buttons))
}

impl Render for Composer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let Some(thread) = self.thread(cx) else { return div() };
        let (running, permission, waiting) = {
            let t = thread.read(cx);
            (t.run.running, t.permission().cloned(), t.run.permissions.len())
        };
        // While Claude works, what typing does: it steers, or with ⌘↩ goes now.
        if running != self.steering {
            self.steering = running;
            let input = self.input.clone();
            cx.defer_in(window, move |_, window, cx| {
                let text = if running { "Steer Claude… ↩ after its current step, ⌘↩ right now" } else { "Ask Claude…" };
                input.update(cx, |i, cx| i.set_placeholder(text, window, cx));
            });
        }
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
                    .child(card_title(icons::key_round(), format!("Sign in to {}", ask.server), &theme))
                    .child(div().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).whitespace_normal().child(ask.reason.clone().unwrap_or_else(|| "Claude needs you to sign in to this MCP server to go on. It opens in your browser.".into())))
                    .child(if asking {
                        card_buttons()
                            .child(Button::new("sign-in-no").small().outline().label("Not Now").on_click(move |_, _, cx| answer(&t1, false, cx)))
                            .child(Button::new("sign-in-yes").small().primary().icon(icons::log_in().size(px(13.))).label("Sign In…").on_click(move |_, _, cx| answer(&t2, true, cx)))
                    } else {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S))
                            .child(Icon::new(IconName::Globe).size(px(14.)).text_color(theme.secondary))
                            .child(div().flex_1().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child("Waiting for you to finish in your browser…"))
                            .child({
                                // The page is known once the browser's been sent to it: looked up on the click.
                                let server = ask.server.clone();
                                Button::new("sign-in-again").small().outline().label("Open Again").on_click(move |_, _, cx| {
                                    if let Some(page) = crate::ui::mcp::sign_in_page(&server) {
                                        crate::ui::mcp::open_sign_in_page(&page, cx);
                                    }
                                })
                            })
                            .child(Button::new("sign-in-cancel").small().outline().label("Cancel").on_click(move |_, _, cx| {
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
                Card::Tool => permission_card(p, &thread, w.clone(), waiting.saturating_sub(1), &theme).into_any_element(),
            });
        }
        // Only while Claude works on it: a finished plan, or one left behind, is in the transcript.
        let plan = thread.read(cx).plan.clone();
        if thread.read(cx).run.running && !plan.items.is_empty() && !plan.is_finished() {
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
            let context = t.run.composer.context_tokens.map(|tokens| (tokens, ws.context_window_of(t)));
            (context, crate::state::workspace::mcp_needing_you(&t.run.composer.mcp_servers))
        };
        let stop_thread = thread.clone();
        // A narrow composer (a small window, the right sidebar open) shows effort and mode as
        // icons alone, their tooltips saying what they are, so Send never gets pushed out.
        let compact = {
            let inspector = if self.workspace.read(cx).shows_inspector { crate::ui::inspector::WIDTH } else { 0. };
            f32::from(window.viewport_size().width) - inspector - 260. - 40. < 540.
        };
        let has_text = !self.input.read(cx).value().trim().is_empty() || attached.is_some();
        self.empty = self.input.read(cx).value().trim().is_empty();
        let composer = div()
            .capture_key_down(cx.listener(Self::on_key))
            .children(attached.map(|a| a.px(px(space::XS)).pt(px(space::XS))))
            .relative()
            .border_1()
            .border_color(theme.separator)
            .rounded(px(radius::LARGE))
            .bg(theme.card)
            .px(px(space::S))
            .pt(px(space::S))
            .pb(px(space::S))
            .flex()
            .flex_col()
            .gap(px(space::XS))
            .child(div().px(px(space::XS)).child(Textarea::new(&self.input).appearance(false).max_h(px(320.))))
            .when(crate::probe::enabled(), |d| d.child(crate::probe::tracker("composer".into())))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .child(
                        Button::new("attach")
                            .ghost()
                            .small()
                            .icon(icons::paperclip().size(px(14.)).text_color(theme.secondary))
                            .tooltip("Attach Files… (files, folders or images; or paste and drop them)")
                            .on_click(cx.listener(|this, _, _, cx| this.attach(cx))),
                    )
                    // The pickers give way first when the window is narrow: Send always shows.
                    .child(div().flex_1().min_w_0().overflow_hidden().flex().items_center().gap(px(2.)).child(self.pickers(&thread, compact, &theme, cx)).children(
                        (!mcp_chip.is_empty()).then(|| mcp_attention_chip(&mcp_chip, self.workspace.downgrade(), &theme)),
                    ))
                    .children(context.map(|(tokens, window)| context_ring(tokens, window, &theme)))
                    .when(running, |d| {
                        let w = w.clone();
                        d.child(Button::new("stop").small().ghost().icon(icons::circle_stop().size(px(16.)).text_color(theme.text)).tooltip("Stop (⌘.)").on_click(cx.listener(move |this, _, _, cx| {
                            // Anything queued goes back into the composer first, so stopping never sends it.
                            let removing = this.withdrawing.clone();
                            let _ = w.update(cx, |ws, cx| ws.stop(&stop_thread, &removing, cx));
                        })))
                    })
                    // Something to send while Claude works: stop it and send this now (⌘↩).
                    .when(running && has_text, |d| {
                        d.child(
                            Button::new("interrupt-send")
                                .small()
                                .ghost()
                                .icon(icons::zap().size(px(15.)).text_color(theme.text))
                                .tooltip("Interrupt and Send: stop Claude and send this now (⌘↩)")
                                .on_click(cx.listener(|this, _, window, cx| this.submit(true, window, cx))),
                        )
                    })
                    .child(if running {
                        // While Claude works, ↩ steers: the message joins the turn after the current step.
                        Button::new("send")
                            .small()
                            .primary()
                            .disabled(!has_text)
                            .icon(icons::corner_down_right().size(px(13.)))
                            .label("Steer")
                            .tooltip("Steer (↩): Claude reads it after its current step. ⌘↩ stops and sends now.")
                            .on_click(cx.listener(|this, _, window, cx| this.submit(false, window, cx)))
                    } else {
                        Button::new("send")
                            .small()
                            .primary()
                            .disabled(!has_text)
                            .rounded(px(12.))
                            .icon(icons::arrow_up().size(px(15.)))
                            .tooltip("Send (↩)")
                            .on_click(cx.listener(|this, _, window, cx| this.submit(false, window, cx)))
                    }),
            );
        div()
            .w_full()
            .relative()
            .flex()
            .justify_center()
            .px(px(space::XL - 4.))
            .pb(px(space::L))
            .on_action(cx.listener(Self::on_enter))
            .capture_action(cx.listener(Self::on_delete_line))
            .child(inner.child(composer))
            .child(self.measure())
    }
}

impl Composer {
    /// The composer is a cached view (the window redraws on every frame of scrolling, and laid
    /// out each time it cost about a tenth of the main thread), and a cached view needs a fixed
    /// height. Laid over it, this reads the height it took; when that changes it tells whoever
    /// placed it, after this frame, to place it at the new height.
    fn measure(&self) -> impl IntoElement {
        let (cell, parent) = (self.height.clone(), self.on_height.clone());
        gpui_kit::canvas(
            move |bounds, _, cx| {
                let h = f32::from(bounds.size.height);
                if (cell.replace(h) - h).abs() > 0.5 {
                    if let Some(parent) = parent.clone() {
                        cx.defer(move |cx| {
                            if let Some(e) = parent.upgrade() {
                                cx.notify(e.entity_id());
                            }
                        });
                    }
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }
}

impl Composer {
    /// Claude's checklist: progress and what it's on, opening into the whole list.
    fn plan_strip(&self, plan: &yh_core::plan::Plan, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        use yh_core::plan::Status;
        let current = plan.current().map(|i| i.active_form.clone().unwrap_or_else(|| i.subject.clone()));
        let status = current.unwrap_or_else(|| if plan.is_finished() { "Done".into() } else { String::new() });
        let open = self.plan_open;
        let mut strip = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .mb(px(space::S))
            .px(px(space::M))
            .py(px(space::XS + 2.))
            .rounded(px(radius::LARGE))
            .border_1()
            .border_color(theme.separator)
            .bg(theme.card)
            .text_size(px(text::FOOTNOTE))
            .child(
                div()
                    .id("plan-head")
                    .flex()
                    .items_center()
                    .gap(px(space::S))
                    .cursor_pointer()
                    .child(icons::list_todo().size(px(14.)).text_color(if plan.is_finished() { theme.success } else { theme.accent }))
                    .child(div().flex_none().font_weight(FontWeight::MEDIUM).child("Plan"))
                    .child(div().flex_none().text_color(theme.secondary).child(format!("{}/{}", plan.completed(), plan.items.len())))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(theme.secondary).child(status))
                    .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(13.)).text_color(theme.tertiary))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.plan_open = !this.plan_open;
                        cx.notify();
                    })),
            );
        if open {
            let mut list = div().flex().flex_col().gap(px(3.)).pt(px(space::XS)).pb(px(2.));
            for item in &plan.items {
                let mark = match item.status {
                    Status::Completed => Icon::new(IconName::CircleCheck).text_color(theme.success),
                    Status::InProgress => Icon::new(IconName::LoaderCircle).text_color(theme.accent),
                    Status::Pending => icons::circle_dashed().text_color(theme.tertiary),
                };
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S))
                        .pl(px(1.))
                        .child(mark.size(px(13.)))
                        .child(
                            div()
                                .min_w_0()
                                .when(item.status == Status::Completed, |d| d.text_color(theme.secondary).line_through())
                                .when(item.status == Status::InProgress, |d| d.font_weight(FontWeight::MEDIUM))
                                .child(item.subject.clone()),
                        ),
                );
            }
            strip = strip.child(list);
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
        let mut list = div().flex().flex_col().gap(px(space::XS + 2.)).mb(px(space::S));
        for (i, (id, text, attachments)) in queued.into_iter().enumerate() {
            let (sending, taking) = {
                let c = &thread.read(cx).run.composer;
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
                .gap(px(space::S + 2.))
                .pl(px(space::M))
                .pr(px(space::XS + 2.))
                .py(px(space::S))
                .rounded(px(radius::LARGE))
                .border_1()
                .border_color(theme.separator)
                .bg(theme.card)
                .text_size(px(text::FOOTNOTE))
                .child(div().pt(px(1.)).flex_none().child(if sending { icons::zap() } else { icons::clock() }.size(px(14.)).text_color(if sending { theme.warning } else { theme.secondary })))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(div().max_h(px(34.)).overflow_hidden().child(label))
                        .child(div().text_size(px(text::CAPTION)).text_color(theme.secondary).child(status)),
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
                        .child(Button::new(SharedString::from(format!("queued-now-{i}"))).ghost().small().icon(icons::zap().size(px(13.)).text_color(theme.secondary)).label("Send Now").tooltip("Interrupt Claude and send this now").on_click(
                            move |_, _, cx| {
                                let _ = w.update(cx, |ws, cx| ws.send_now(&t1, &i1, cx));
                            },
                        ))
                        .child(Button::new(SharedString::from(format!("queued-edit-{i}"))).ghost().small().icon(Icon::new(IconName::Undo2).size(px(13.)).text_color(theme.secondary)).tooltip("Edit: take it back into the composer").on_click(
                            move |_, _, cx| {
                                let _ = w2.update(cx, |ws, cx| ws.withdraw(&t2, &i2, true, cx));
                            },
                        ))
                        .child(Button::new(SharedString::from(format!("queued-remove-{i}"))).ghost().small().icon(icons::x().size(px(13.)).text_color(theme.secondary)).tooltip("Remove: don't send it").on_click(cx.listener(
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
        let mut strip = div().flex().flex_wrap().gap(px(space::S));
        // A click opens Quick Look on what's waiting, at the one clicked; the arrow keys go
        // through the rest, as in Finder.
        let all: Vec<String> = list
            .iter()
            .map(|a| match a {
                Attached::Image(name) => dir.join(name),
                Attached::File(f) => f.path.clone(),
            })
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        for (i, a) in list.iter().enumerate() {
            let path = match a {
                Attached::Image(name) => dir.join(name),
                Attached::File(f) => f.path.clone(),
            };
            let body = match a {
                Attached::Image(_) => div()
                    .rounded(px(radius::MEDIUM))
                    .border_1()
                    .border_color(theme.separator)
                    .overflow_hidden()
                    .child(gpui_kit::img(path.clone()).h(px(56.)).max_w(px(120.)).rounded(px(radius::MEDIUM - 1.))),
                Attached::File(f) => file_chip(f, theme),
            };
            let (reveal, look, look_menu) = (path.clone(), all.clone(), all.clone());
            let this = cx.entity().downgrade();
            let remove_key = key.clone();
            let chip = div()
                .id(("attached", i))
                .relative()
                .cursor_pointer()
                .child(body)
                .on_click(move |_, _, _| crate::platform::quick_look(look.clone(), i))
                .context_menu(move |menu, _, _| {
                    let (open, reveal, this, key, look) = (path.clone(), reveal.clone(), this.clone(), remove_key.clone(), look_menu.clone());
                    menu.item(PopupMenuItem::new("Quick Look").on_click(move |_, _, _| crate::platform::quick_look(look.clone(), i)))
                        .item(PopupMenuItem::new("Open").on_click(move |_, _, cx| cx.open_with_system(&open)))
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
                        .w(px(18.))
                        .h(px(18.))
                        .rounded_full()
                        .bg(theme.text)
                        .border_2()
                        .border_color(theme.card)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icons::x().size(px(9.)).text_color(theme.card))
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

    /// What waits in the open thread's composer, as paths (`YH_PREVIEW_MEDIA=draft`).
    pub fn attached_paths(&self, cx: &App) -> Vec<String> {
        let Some(key) = self.key(cx) else { return vec![] };
        let dir = self.workspace.read(cx).data_dir.join("attachments");
        self.attached
            .get(&key)
            .map(|l| {
                l.iter()
                    .map(|a| match a {
                        Attached::Image(name) => dir.join(name),
                        Attached::File(f) => f.path.clone(),
                    })
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
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
        .pl(px(space::S + 2.))
        .pr(px(space::M))
        .rounded(px(radius::MEDIUM))
        .border_1()
        .border_color(theme.separator)
        .bg(theme.canvas)
        .child(
            div()
                .flex_none()
                .w(px(30.))
                .h(px(30.))
                .rounded(px(radius::SMALL + 1.))
                .bg(color.opacity(0.14))
                .text_color(color)
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
                .child(div().text_size(px(text::FOOTNOTE)).font_weight(FontWeight::MEDIUM).overflow_hidden().whitespace_nowrap().text_ellipsis().child(file.name()))
                .child(div().text_size(px(text::CAPTION)).text_color(theme.secondary).overflow_hidden().whitespace_nowrap().text_ellipsis().child(detail)),
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
        .flex()
        .items_center()
        .gap(px(space::XS + 1.))
        .ml(px(space::XS))
        .h(px(22.))
        .px(px(space::S))
        .rounded_full()
        .bg(theme.warning.opacity(0.14))
        .hover(|d| d.bg(theme.warning.opacity(0.22)))
        .text_color(theme.warning)
        .text_size(px(text::FOOTNOTE))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .child(icons::plug().size(px(12.)).text_color(theme.warning))
        .child(div().whitespace_nowrap().child(label))
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
    /// Where a command comes from: YourHarness, MCP, project, user, a plugin.
    chip: Option<String>,
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
            // The project's own commands, read with a throwaway `claude` once per launch, so
            // the menu is right before any session there has started.
            self.workspace.update(cx, |ws, cx| ws.load_commands(&project, cx));
            let commands = {
                let ws = self.workspace.read(cx);
                if chained {
                    ws.chainable_commands(&project)
                } else {
                    ws.menu_commands(&project)
                }
            };
            // An MCP prompt is found by what it shows ("github:review") too.
            let rank = |c: &yh_core::claude::SlashCommand| {
                let name = c.name.to_lowercase();
                let shown = c.display_name().to_lowercase();
                if q.is_empty() || name == q || shown == q {
                    Some(0)
                } else if name.starts_with(&q) || shown.starts_with(&q) {
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
            let own = crate::state::workspace::app_commands();
            // Every match: the menu scrolls.
            let items: Vec<Suggestion> = ranked
                .into_iter()
                .map(|(_, c)| {
                    let (description, chip) = c.source(own.iter().any(|o| o.name == c.name));
                    (c, description, chip)
                })
                .map(|(c, description, chip)| Suggestion {
                    label: format!("/{}", c.display_name()),
                    detail: if c.hint.is_empty() { description } else { format!("{} · {}", c.hint, description) },
                    chip,
                    insert: format!("/{} ", c.display_name()),
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
                        chip: None,
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
            "up" => {
                s.index = (s.index + n - 1) % n;
                self.suggest_scroll.scroll_to_item(s.index);
            }
            "down" => {
                s.index = (s.index + 1) % n;
                self.suggest_scroll.scroll_to_item(s.index);
            }
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
        // Every match, scrolling past 300 pt.
        let mut list = div()
            .id("suggestions")
            .track_scroll(&self.suggest_scroll)
            .max_h(px(300.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .mb(px(space::S))
            .p(px(space::XS))
            .rounded(px(radius::LARGE))
            .border_1()
            .border_color(theme.separator)
            .bg(theme.card);
        for (i, item) in s.items.iter().enumerate() {
            let on = i == s.index;
            list = list.child(
                div()
                    .id(("suggestion", i))
                    .flex()
                    .items_center()
                    .gap(px(space::S + 2.))
                    .h(px(28.))
                    .px(px(space::S))
                    .rounded(px(radius::MEDIUM))
                    .cursor_pointer()
                    .when(on, |d| d.bg(theme.accent_soft))
                    .when(!on, |d| d.hover(|d| d.bg(theme.hover)))
                    .child(div().flex_none().font_weight(FontWeight::MEDIUM).when(on, |d| d.text_color(theme.accent)).child(item.label.clone()))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(item.detail.clone()))
                    .when_some(item.chip.clone(), |d, chip| {
                        let own = chip == "YourHarness";
                        d.child(
                            div()
                                .flex_none()
                                .px(px(6.))
                                .rounded(px(radius::SMALL))
                                .text_size(px(text::CAPTION))
                                .line_height(px(16.))
                                .bg(if own { theme.accent_soft } else { theme.selected })
                                .text_color(if own { theme.accent } else { theme.secondary })
                                .child(chip),
                        )
                    })
                    .flex_none()
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

/// What Claude hears when you skip its question.
const SKIP_MESSAGE: &str = "The user chose not to answer. Continue with your best judgment, or ask in chat.";

/// Claude asks (`AskUserQuestion`): pick options or write your own; Skip declines.
pub struct QuestionCard {
    workspace: Option<gpui_kit::WeakEntity<Workspace>>,
    input: String,
    questions: Vec<Question>,
    _subscriptions: Vec<Subscription>,
}

impl QuestionCard {
    /// The card for `p`, or None when its input has no questions (it shows as a plain
    /// permission then). Typing an answer redraws the card (Answer waits for every question),
    /// and replaces a single choice.
    fn new(p: &Permission, workspace: gpui_kit::WeakEntity<Workspace>, window: &mut Window, cx: &mut Context<Composer>) -> Option<Entity<QuestionCard>> {
        let input: Value = serde_json::from_str(&p.input).ok()?;
        let parsed: Vec<(String, Option<String>, bool, Vec<(String, Option<String>)>)> = input["questions"]
            .as_array()?
            .iter()
            .filter_map(|q| {
                Some((
                    q["question"].as_str()?.to_string(),
                    q["header"].as_str().map(str::to_string),
                    q["multiSelect"].as_bool().unwrap_or(false),
                    q["options"]
                        .as_array()
                        // A description that only says the label again (or nothing) isn't shown.
                        .map(|o| {
                            o.iter()
                                .filter_map(|o| {
                                    let label = o["label"].as_str()?.to_string();
                                    let description = o["description"].as_str().map(|d| d.trim().to_string()).filter(|d| !d.is_empty() && !d.eq_ignore_ascii_case(label.trim()));
                                    Some((label, description))
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                ))
            })
            .collect();
        if parsed.is_empty() {
            return None;
        }
        let raw = p.input.clone();
        Some(cx.new(|cx| {
            let mut subscriptions = vec![];
            let questions = parsed
                .into_iter()
                .enumerate()
                .map(|(qi, (question, header, multi, options))| {
                    let placeholder = if options.is_empty() { "Your answer" } else { "Or write your own answer" };
                    let other = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
                    subscriptions.push(cx.subscribe(&other, move |this: &mut QuestionCard, other, event: &InputEvent, cx| {
                        if let InputEvent::Change = event {
                            let typed = !other.read(cx).value().trim().is_empty();
                            let q = &mut this.questions[qi];
                            if typed && !q.multi {
                                q.chosen.clear();
                            }
                            cx.notify();
                        }
                    }));
                    Question { question, header, multi, options, chosen: BTreeSet::new(), other }
                })
                .collect();
            QuestionCard { workspace: Some(workspace), input: raw, questions, _subscriptions: subscriptions }
        }))
    }

    /// A field of the card's has the focus and something in it: ⌘⌫ edits it then.
    fn typing(&self, window: &Window, cx: &App) -> bool {
        self.questions.iter().any(|q| q.other.focus_handle(cx).is_focused(window) && !q.other.read(cx).value().is_empty())
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
        let title = card_title(icons::message_question(), if self.questions.len() == 1 { "Claude asks".to_string() } else { format!("Claude asks {} questions", self.questions.len()) }, &theme);
        // The questions scroll past 420 pt; the title and buttons stay.
        let mut questions = div().id("question-list").max_h(px(420.)).overflow_y_scroll().flex().flex_col().gap(px(space::S + 2.));
        for (qi, q) in self.questions.iter().enumerate() {
            // With descriptions, each option on its own line with what it means beside it.
            let described = q.options.iter().any(|o| o.1.is_some());
            let mut options = if described { div().flex().flex_col().gap(px(space::XS + 2.)) } else { div().flex().flex_wrap().gap(px(space::XS + 2.)) };
            for (oi, (label, description)) in q.options.iter().enumerate() {
                let on = q.chosen.contains(&oi);
                let button = Button::new(SharedString::from(format!("q{qi}-{oi}")))
                        .small()
                        .when(on, |b| b.primary())
                        .when(!on, |b| b.outline())
                        .when(on, |b| b.icon(Icon::new(IconName::Check).size(px(12.))))
                        .label(label.clone())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let q = &mut this.questions[qi];
                            if !q.chosen.remove(&oi) {
                                if !q.multi {
                                    q.chosen.clear();
                                    // A single choice replaces what was written.
                                    let other = q.other.clone();
                                    other.update(cx, |i, cx| i.set_value("", window, cx));
                                }
                                this.questions[qi].chosen.insert(oi);
                            }
                            cx.notify();
                        }));
                options = options.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(div().flex_none().child(button))
                        .when_some(description.clone().filter(|_| described), |d, t| d.child(div().min_w_0().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(t))),
                );
            }
            questions = questions.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S))
                    .when(qi > 0, |d| d.pt(px(space::S)).border_t_1().border_color(theme.separator))
                    .when_some(q.header.clone(), |d, h| {
                        d.child(div().flex().child(div().px(px(space::XS + 2.)).rounded(px(radius::SMALL)).bg(theme.selected).text_size(px(text::CAPTION)).font_weight(FontWeight::MEDIUM).text_color(theme.secondary).child(h)))
                    })
                    .child(div().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).whitespace_normal().child(q.question.clone()))
                    .child(options)
                    .child(Input::new(&q.other).small()),
            );
        }
        let ready = self.all_answered(cx);
        card_frame(&theme).child(title).child(questions).child(
            card_buttons()
                .child(Button::new("skip").small().outline().label("Skip").tooltip("Skip (⌘⌫)").on_click(cx.listener(|this, _, _, cx| this.skip(cx))))
                .child(
                    Button::new("answer")
                        .small()
                        .primary()
                        .label("Answer")
                        .tooltip(if ready { "Answer (⌘↩)" } else { "Answer every question first" })
                        .disabled(!ready)
                        .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                ),
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
    /// Test runs print what it offers once.
    logged: bool,
}

impl PlanCard {
    fn new(p: &Permission, workspace: gpui_kit::WeakEntity<Workspace>, window: &mut Window, cx: &mut Context<PlanCard>) -> PlanCard {
        let text = serde_json::from_str::<Value>(&p.input).ok().and_then(|v| v["plan"].as_str().map(str::to_string)).unwrap_or_default();
        PlanCard {
            workspace,
            plan: std::rc::Rc::new(markdown::parse(&text)),
            feedback: cx.new(|cx| InputState::new(window, cx).placeholder("Feedback for Claude (optional)")),
            logged: false,
        }
    }
}

impl PlanCard {
    /// Auto when the thread's model can use it, else Accept Edits (edits go through, commands
    /// still ask). Also whether Auto is offered at all.
    fn recommended(&self, cx: &App) -> (&'static str, bool) {
        let auto = self.workspace.upgrade().and_then(|ws| ws.read(cx).selected_thread().map(|t| ws.read(cx).supports_auto_mode(t.read(cx)))).unwrap_or(true);
        (if auto { "auto" } else { "acceptEdits" }, auto)
    }

    /// Approves into `mode`; what's written as feedback follows as your next message.
    fn approve_in(&self, mode: &'static str, cx: &mut App) {
        let feedback = self.feedback.read(cx).value().trim().to_string();
        let Some(ws) = self.workspace.upgrade() else { return };
        if let Some(t) = ws.read(cx).selected_thread() {
            ws.update(cx, |ws, cx| {
                ws.approve_plan(&t, mode, cx);
                if !feedback.is_empty() {
                    ws.send_message(&t, &feedback, vec![], vec![], false, cx);
                }
            });
        }
    }

    /// The feedback field has the focus and something in it: ⌘⌫ edits it then.
    fn typing(&self, window: &Window, cx: &App) -> bool {
        self.feedback.focus_handle(cx).is_focused(window) && !self.feedback.read(cx).value().is_empty()
    }

    /// ⌘↩: approve in the recommended mode.
    fn approve(&self, cx: &mut App) {
        let (mode, _) = self.recommended(cx);
        self.approve_in(mode, cx);
    }

    /// ⌘⌫: keep planning, with what's written as feedback.
    fn keep_planning(&self, cx: &mut App) {
        let feedback = self.feedback.read(cx).value().trim().to_string();
        let message = if feedback.is_empty() {
            "The user wants to keep planning. Ask what to change.".to_string()
        } else {
            format!("The user wants changes to the plan before you start: {feedback}")
        };
        let Some(ws) = self.workspace.upgrade() else { return };
        if let Some(t) = ws.read(cx).selected_thread() {
            ws.update(cx, |ws, cx| ws.respond(&t, Decision::Deny { message }, cx));
        }
    }
}

impl Render for PlanCard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::component::button::DropdownButton;
        let theme = Theme::get(cx);
        let (recommended, auto) = self.recommended(cx);
        if !self.logged && std::env::var("YH_DATA_DIR").is_ok() {
            self.logged = true;
            eprintln!("PLAN card: Approve · {recommended}, Auto {}", if auto { "offered" } else { "disabled" });
        }
        let label = MODES.iter().find(|(v, _)| *v == recommended).map_or("Auto", |(_, l)| l);
        let this = cx.entity().downgrade();
        let approve = DropdownButton::new("approve-plan")
            .small()
            .primary()
            .button(
                Button::new("approve-plan-main")
                    .label(format!("Approve · {label}"))
                    .tooltip(format!("Approve and continue in {label} (⌘↩). The menu has other modes."))
                    .on_click(cx.listener(|this, _, _, cx| this.approve(cx))),
            )
            .dropdown_menu(move |mut menu, _, _| {
                for (mode, label) in [("auto", "Approve, Auto"), ("acceptEdits", "Approve, Accept Edits"), ("ask", "Approve, Ask")] {
                    let this = this.clone();
                    menu = menu.item(PopupMenuItem::new(label).disabled(mode == "auto" && !auto).on_click(move |_, _, cx| {
                        let _ = this.update(cx, |this, cx| this.approve_in(mode, cx));
                    }));
                }
                menu
            });
        card_frame(&theme)
            .child(card_title(Icon::new(IconName::Map), "Plan ready for review", &theme))
            .child(
                div()
                    .id("plan-body")
                    .max_h(px(320.))
                    .overflow_y_scroll()
                    .px(px(space::M))
                    .py(px(space::S))
                    .rounded(px(radius::MEDIUM))
                    .bg(theme.canvas)
                    .border_1()
                    .border_color(theme.separator)
                    .child(rows::markdown_view(&self.plan, &theme)),
            )
            .child(Input::new(&self.feedback).small())
            .child(
                card_buttons()
                    .child(Button::new("keep-planning").small().outline().label("Keep Planning").tooltip("Send your feedback and keep planning (⌘⌫)").on_click(cx.listener(|this, _, _, cx| this.keep_planning(cx))))
                    .child(approve),
            )
    }
}
