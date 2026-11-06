//! The window: sidebar, the open thread's transcript, composer. The twin of web/ui/src/App.tsx
//! and store.ts. Views that change rarely (sidebar, composer) are cached: a frame redraws
//! them only when they were told something changed, so streaming redraws the transcript only.

use crate::claude::{ClaudeProcess, Event, LaunchOptions};
use crate::input::{Submitted, TextInput};
use crate::library::{Library, ProjectMeta, ThreadMeta};
use crate::model::ThreadState;
use crate::probe;
use crate::rows;
use crate::theme::{Theme, MONO, UI};
use gpui::{
    actions, div, list, prelude::*, px, AnyView, App, Context, Entity, FocusHandle, Focusable, FontWeight,
    ScrollHandle, StyleRefinement, Subscription, WeakEntity, Window,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

actions!(harness, [Stop, Allow, Deny, Quit]);

/// Transcripts kept made for going back and forth; the rest let go of their rows.
const KEPT_TRANSCRIPTS: usize = 6;
const PAGE: usize = 160;
const SHOWN_PER_PROJECT: usize = 12;

pub const MODELS: &[(&str, &str)] = &[("", "Default model"), ("haiku", "Haiku"), ("sonnet", "Sonnet"), ("opus", "Opus")];
pub const MODES: &[(&str, &str)] =
    &[("default", "Ask"), ("acceptEdits", "Accept edits"), ("auto", "Auto"), ("plan", "Plan"), ("bypassPermissions", "Bypass")];

pub struct HarnessApp {
    pub library: Arc<Library>,
    pub projects: Vec<ProjectMeta>,
    pub threads: Vec<Entity<ThreadState>>,
    by_id: HashMap<String, Entity<ThreadState>>,
    pub selected: Option<String>,
    deck: Vec<(String, Entity<TranscriptView>)>,
    pub sidebar: Entity<Sidebar>,
    pub composer: Entity<Composer>,
    live: HashMap<String, ClaudeProcess>,
    pub model: String,
    pub mode: String,
    new_count: usize,
    focus: FocusHandle,
}

impl HarnessApp {
    pub fn new(library: Arc<Library>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = cx.entity().downgrade();
        let threads: Vec<Entity<ThreadState>> = library
            .threads
            .iter()
            .map(|meta| Self::make_thread(meta.clone(), app.clone(), cx))
            .collect();
        let by_id = threads.iter().map(|t| (t.read(cx).id().to_string(), t.clone())).collect();
        let sidebar = cx.new(|_| Sidebar { app: app.clone(), expanded: HashSet::new(), scroll: ScrollHandle::new() });
        let composer = cx.new(|cx| Composer::new(app.clone(), window, cx));
        HarnessApp {
            projects: library.projects.clone(),
            library,
            threads,
            by_id,
            selected: None,
            deck: vec![],
            sidebar,
            composer,
            live: HashMap::new(),
            model: std::env::var("YHG_MODEL").unwrap_or_default(),
            mode: std::env::var("YHG_MODE").unwrap_or_else(|_| "default".into()),
            new_count: 0,
            focus: cx.focus_handle(),
        }
    }

    fn make_thread(meta: ThreadMeta, app: WeakEntity<HarnessApp>, cx: &mut Context<Self>) -> Entity<ThreadState> {
        let thread = cx.new(|_| ThreadState::new(meta));
        // Near the top of what's held: read the page before it.
        let weak = thread.downgrade();
        thread.read(cx).list.set_scroll_handler(move |event, _, cx| {
            if event.visible_range.start > 4 {
                return;
            }
            let (app, weak) = (app.clone(), weak.clone());
            cx.defer(move |cx| {
                if let (Some(app), Some(thread)) = (app.upgrade(), weak.upgrade()) {
                    app.update(cx, |app, cx| app.load_earlier(&thread, cx));
                }
            });
        });
        thread
    }

    pub fn thread(&self, id: &str) -> Option<Entity<ThreadState>> {
        self.by_id.get(id).cloned()
    }

    pub fn selected_thread(&self) -> Option<Entity<ThreadState>> {
        self.selected.as_deref().and_then(|id| self.thread(id))
    }

    fn refresh_chrome(&self, cx: &mut Context<Self>) {
        self.sidebar.update(cx, |_, cx| cx.notify());
        self.composer.update(cx, |_, cx| cx.notify());
        cx.notify();
    }

    /// The click is answered in the next frame (selection, title, a held transcript); reading
    /// a history that isn't held follows, off the main thread.
    pub fn select(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = self.thread(id) else { return };
        if self.selected.as_deref() == Some(id) {
            return;
        }
        self.selected = Some(id.to_string());
        if let Some(i) = self.deck.iter().position(|(t, _)| t == id) {
            let slot = self.deck.remove(i);
            self.deck.push(slot);
        } else {
            let view = cx.new(|cx| TranscriptView::new(thread.clone(), cx));
            thread.update(cx, |t, _| t.in_deck = true);
            self.deck.push((id.to_string(), view));
            while self.deck.len() > KEPT_TRANSCRIPTS {
                let (old, _) = self.deck.remove(0);
                if let Some(t) = self.thread(&old) {
                    t.update(cx, |t, cx| {
                        t.in_deck = false;
                        if t.running {
                            t.trim(200);
                        } else {
                            t.unload();
                        }
                        cx.notify();
                    });
                }
            }
        }
        self.ensure_loaded(&thread, cx);
        self.composer.update(cx, |c, cx| c.switch_to(id, window, cx));
        self.refresh_chrome(cx);
    }

    pub fn ensure_loaded(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        let (id, needed) = {
            let t = thread.read(cx);
            (t.id().to_string(), !t.loaded && !t.loading)
        };
        if !needed {
            return;
        }
        if id.starts_with("new:") {
            thread.update(cx, |t, _| t.loaded = true);
            return;
        }
        thread.update(cx, |t, _| t.loading = true);
        let library = self.library.clone();
        let weak = thread.downgrade();
        cx.spawn(async move |_, cx| {
            let page = cx.background_executor().spawn(async move { library.page(&id, None, PAGE) }).await;
            let _ = weak.update(cx, |t, cx| {
                if !t.loading {
                    return; // let go of meanwhile
                }
                t.prepend(page.entries, page.from);
                t.loaded = true;
                t.loading = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn load_earlier(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        let (id, before) = {
            let t = thread.read(cx);
            if !t.loaded || t.loading_earlier || t.before == 0 {
                return;
            }
            (t.id().to_string(), t.before)
        };
        thread.update(cx, |t, _| t.loading_earlier = true);
        let library = self.library.clone();
        let weak = thread.downgrade();
        cx.spawn(async move |_, cx| {
            let page = cx.background_executor().spawn(async move { library.page(&id, Some(before), PAGE) }).await;
            let _ = weak.update(cx, |t, cx| {
                t.loading_earlier = false;
                if t.loaded && t.before == before {
                    t.prepend(page.entries, page.from);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn new_thread(&mut self, project: &ProjectMeta, window: &mut Window, cx: &mut Context<Self>) {
        self.new_count += 1;
        let meta = ThreadMeta {
            id: format!("new:{}", self.new_count),
            project_id: project.id.clone(),
            title: "New thread".into(),
            updated_at: crate::library::now_ms(),
            session_id: None,
        };
        let thread = Self::make_thread(meta, cx.entity().downgrade(), cx);
        let id = thread.read(cx).id().to_string();
        let at = self.threads.iter().position(|t| t.read(cx).meta.project_id == project.id).unwrap_or(0);
        self.threads.insert(at, thread.clone());
        self.by_id.insert(id.clone(), thread);
        self.select(&id, window, cx);
    }

    pub fn send(&mut self, thread: &Entity<ThreadState>, text: &str, cx: &mut Context<Self>) {
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        let (id, project_id, session, title) = {
            let t = thread.read(cx);
            (t.id().to_string(), t.meta.project_id.clone(), t.session_id.clone(), t.title.clone())
        };
        thread.update(cx, |t, cx| {
            if title == "New thread" {
                t.title = text.lines().next().unwrap_or("").chars().take(60).collect();
            }
            t.add_user(&text);
            t.running = true;
            cx.notify();
        });
        if !self.live.contains_key(&id) {
            let cwd = self
                .library
                .project_path(&project_id)
                .or_else(|| self.projects.iter().find(|p| p.id == project_id).map(|p| p.path.clone()))
                .filter(|p| std::path::Path::new(p).is_dir())
                .unwrap_or_else(|| std::env::var("HOME").unwrap_or("/tmp".into()));
            let (tx, rx) = flume::unbounded();
            let options = LaunchOptions { cwd, session_id: session, model: Some(self.model.clone()), mode: self.mode.clone() };
            match ClaudeProcess::spawn(options, tx) {
                Ok(process) => {
                    self.live.insert(id.clone(), process);
                    self.pump(thread.clone(), id.clone(), rx, cx);
                }
                Err(error) => {
                    thread.update(cx, |t, cx| {
                        t.notice(&error);
                        t.running = false;
                        cx.notify();
                    });
                    self.refresh_chrome(cx);
                    return;
                }
            }
        }
        if let Some(process) = self.live.get(&id) {
            process.send(&text);
        }
        self.refresh_chrome(cx);
    }

    /// Claude's events into the thread, what's arrived at most once a frame, deltas joined.
    fn pump(&mut self, thread: Entity<ThreadState>, id: String, rx: flume::Receiver<Event>, cx: &mut Context<Self>) {
        let log = std::env::var("YHG_EVENT_LOG").as_deref() == Ok("1");
        cx.spawn(async move |this, cx| {
            while let Ok(first) = rx.recv_async().await {
                let started = std::time::Instant::now();
                let mut batch: Vec<Event> = vec![first];
                while let Ok(next) = rx.try_recv() {
                    if !batch.last_mut().unwrap().merge(&next) {
                        batch.push(next);
                    }
                }
                let exited = batch.iter().any(|e| matches!(e, Event::Exit { .. }));
                let count = batch.len();
                if log {
                    for e in &batch {
                        eprintln!("event {id}: {e:?}");
                    }
                }
                let Ok(chrome) = thread.update(cx, |t, cx| {
                    let mut chrome = false;
                    for event in batch {
                        chrome |= t.apply(event, cx);
                    }
                    cx.notify();
                    chrome
                }) else {
                    break;
                };
                probe::note_batch(started.elapsed().as_secs_f64() * 1000.0, count);
                if chrome || exited {
                    let id = id.clone();
                    let _ = this.update(cx, |app, cx| {
                        if exited {
                            app.live.remove(&id);
                        }
                        app.refresh_chrome(cx);
                    });
                }
                if exited {
                    break;
                }
                cx.background_executor().timer(Duration::from_millis(8)).await;
            }
        })
        .detach();
    }

    pub fn interrupt(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        let id = thread.read(cx).id().to_string();
        if let Some(process) = self.live.get_mut(&id) {
            process.interrupt();
        }
    }

    pub fn respond(&mut self, thread: &Entity<ThreadState>, allow: bool, cx: &mut Context<Self>) {
        let (id, permission) = {
            let t = thread.read(cx);
            (t.id().to_string(), t.permission.clone())
        };
        let Some(permission) = permission else { return };
        if let Some(process) = self.live.get(&id) {
            process.respond(&permission.id, &permission.input, allow);
        }
        thread.update(cx, |t, cx| {
            t.permission = None;
            cx.notify();
        });
        self.refresh_chrome(cx);
    }

    pub fn set_model(&mut self, model: &str, cx: &mut Context<Self>) {
        self.model = model.to_string();
        if let Some(id) = self.selected.clone() {
            if let Some(p) = self.live.get_mut(&id) {
                p.set_model(model);
            }
        }
        self.composer.update(cx, |_, cx| cx.notify());
    }

    pub fn set_mode(&mut self, mode: &str, cx: &mut Context<Self>) {
        self.mode = mode.to_string();
        if let Some(id) = self.selected.clone() {
            if let Some(p) = self.live.get_mut(&id) {
                p.set_mode(mode);
            }
        }
        self.composer.update(cx, |_, cx| cx.notify());
    }

    pub fn running_count(&self, cx: &App) -> usize {
        self.threads.iter().filter(|t| t.read(cx).running).count()
    }

    pub fn stop_all(&mut self) {
        for process in self.live.values() {
            process.terminate();
        }
    }

    pub fn transcript_view(&self) -> Option<Entity<TranscriptView>> {
        let id = self.selected.as_ref()?;
        self.deck.iter().find(|(t, _)| t == id).map(|(_, v)| v.clone())
    }

    fn on_stop(&mut self, _: &Stop, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = self.selected_thread() {
            if t.read(cx).running {
                self.interrupt(&t, cx);
            }
        }
    }

    fn on_allow(&mut self, _: &Allow, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = self.selected_thread() {
            self.respond(&t, true, cx);
        }
    }

    fn on_deny(&mut self, _: &Deny, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = self.selected_thread() {
            self.respond(&t, false, cx);
        }
    }
}

impl Focusable for HarnessApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for HarnessApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let selected = self.selected_thread();
        let title = selected.as_ref().map(|t| t.read(cx).title.clone()).unwrap_or_else(|| "YourHarness".into());
        let transcript = self.transcript_view();
        div()
            .id("root")
            .key_context("Harness")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_stop))
            .on_action(cx.listener(Self::on_allow))
            .on_action(cx.listener(Self::on_deny))
            .size_full()
            .flex()
            .font_family(UI)
            .text_size(px(13.))
            .line_height(px(19.5))
            .text_color(theme.text)
            .bg(theme.canvas)
            .child(AnyView::from(self.sidebar.clone()).cached(StyleRefinement::default().h_full().w(px(260.)).flex_none()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(px(44.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .px(px(20.))
                            .border_b_1()
                            .border_color(theme.separator)
                            .child(div().font_weight(FontWeight::SEMIBOLD).overflow_hidden().whitespace_nowrap().child(title)),
                    )
                    .child(match transcript {
                        Some(view) => div().flex_1().min_h_0().relative().child(view).into_any_element(),
                        None => div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(theme.secondary)
                            .child("Pick a thread, or start one with + next to a project.")
                            .into_any_element(),
                    })
                    .when(selected.is_some(), |d| {
                        // Not cached: a cached view needs a fixed size, and the composer's
                        // grows with the permission card. Its render is a handful of elements.
                        d.child(div().flex_none().w_full().child(self.composer.clone()))
                    }),
            )
    }
}

pub struct TranscriptView {
    pub thread: Entity<ThreadState>,
    _observe: Subscription,
}

impl TranscriptView {
    fn new(thread: Entity<ThreadState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&thread, |_, _, cx| cx.notify());
        TranscriptView { thread, _observe: observe }
    }
}

impl Render for TranscriptView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let t = self.thread.read(cx);
        if !t.loaded {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.secondary)
                .child("Loading…")
                .into_any_element();
        }
        let thread = self.thread.clone();
        let mut root = div().size_full().relative();
        if probe::enabled() {
            root = root.child(probe::tracker("transcript".into()));
        }
        root.child(list(t.list.clone(), move |ix, window, cx| rows::render_item(&thread, ix, window, cx)).size_full().pb(px(16.)))
            .into_any_element()
    }
}

/// The projects and their threads as one flat column, so a row can be scrolled to by index.
pub struct Sidebar {
    app: WeakEntity<HarnessApp>,
    expanded: HashSet<String>,
    pub scroll: ScrollHandle,
}

enum SidebarRow {
    Project(ProjectMeta),
    Thread(Entity<ThreadState>),
    More(String, usize, bool),
}

impl Sidebar {
    fn rows(&self, cx: &App) -> Vec<SidebarRow> {
        let Some(app) = self.app.upgrade() else { return vec![] };
        let app = app.read(cx);
        let mut by_project: HashMap<&str, Vec<&Entity<ThreadState>>> = HashMap::new();
        for t in &app.threads {
            by_project.entry(t.read(cx).meta.project_id.as_str()).or_default().push(t);
        }
        let mut rows = vec![];
        for project in &app.projects {
            rows.push(SidebarRow::Project(project.clone()));
            let threads = by_project.get(project.id.as_str()).cloned().unwrap_or_default();
            let all = self.expanded.contains(&project.id);
            let shown = if all { threads.len() } else { threads.len().min(SHOWN_PER_PROJECT) };
            for t in &threads[..shown] {
                rows.push(SidebarRow::Thread((*t).clone()));
            }
            if threads.len() > SHOWN_PER_PROJECT {
                rows.push(SidebarRow::More(project.id.clone(), threads.len() - SHOWN_PER_PROJECT, all));
            }
        }
        rows
    }

    /// Scrolls the thread's row into view (expanding its project if it's behind "Show more"):
    /// what the probe does before clicking, as a person would.
    pub fn row_index(&self, id: &str, cx: &App) -> Option<usize> {
        self.rows(cx).iter().position(|r| matches!(r, SidebarRow::Thread(t) if t.read(cx).id() == id))
    }

    pub fn more_index(&self, project_id: &str, cx: &App) -> Option<usize> {
        self.rows(cx).iter().position(|r| matches!(r, SidebarRow::More(p, _, false) if p == project_id))
    }
}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let Some(app) = self.app.upgrade() else { return div() };
        let selected = app.read(cx).selected.clone();
        let tracking = probe::enabled();
        let mut column = div().id("sidebar-scroll").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.scroll).px(px(8.)).pb(px(16.));
        for row in self.rows(cx) {
            let element = match row {
                SidebarRow::Project(project) => {
                    let app = app.downgrade();
                    let p = project.clone();
                    div()
                        .flex()
                        .items_center()
                        .px(px(6.))
                        .pt(px(10.))
                        .pb(px(2.))
                        .text_color(theme.secondary)
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(project.name.clone()))
                        .child(
                            div()
                                .id(gpui::SharedString::from(format!("new-{}", project.id)))
                                .w(px(20.))
                                .h(px(20.))
                                .rounded(px(5.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(15.))
                                .hover(|s| s.bg(theme.selected))
                                .child("+")
                                .on_click(move |_, window, cx| {
                                    let p = p.clone();
                                    let _ = app.update(cx, |app, cx| app.new_thread(&p, window, cx));
                                }),
                        )
                        .into_any_element()
                }
                SidebarRow::Thread(thread) => {
                    let t = thread.read(cx);
                    let id = t.id().to_string();
                    let is_selected = selected.as_deref() == Some(id.as_str());
                    let app = app.downgrade();
                    let click_id = id.clone();
                    let waiting = t.permission.is_some();
                    let running = t.running && !waiting;
                    div()
                        .id(gpui::SharedString::from(format!("thread-{id}")))
                        .relative()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .w_full()
                        .h(px(28.))
                        .px(px(8.))
                        .rounded(px(6.))
                        .when(is_selected, |d| d.bg(theme.accent).text_color(gpui::white()))
                        .when(!is_selected, |d| d.hover(|s| s.bg(theme.selected)))
                        .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(t.title.clone()))
                        .when(waiting, |d| {
                            d.child(
                                div()
                                    .text_size(px(10.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .px(px(6.))
                                    .rounded(px(8.))
                                    .bg(theme.warning)
                                    .text_color(gpui::white())
                                    .child("waiting"),
                            )
                        })
                        .when(running, |d| {
                            d.child(div().w(px(7.)).h(px(7.)).rounded_full().bg(if is_selected { gpui::white() } else { theme.accent }))
                        })
                        .when(tracking, |d| d.child(probe::tracker(format!("thread:{id}"))))
                        .on_click(move |_, window, cx| {
                            let id = click_id.clone();
                            let _ = app.update(cx, |app, cx| app.select(&id, window, cx));
                        })
                        .into_any_element()
                }
                SidebarRow::More(project, count, all) => {
                    let key = project.clone();
                    div()
                        .id(gpui::SharedString::from(format!("more-{project}")))
                        .relative()
                        .px(px(8.))
                        .py(px(2.))
                        .text_size(px(11.))
                        .text_color(theme.secondary)
                        .child(if all { "Show less".to_string() } else { format!("Show {count} more") })
                        .when(tracking, |d| d.child(probe::tracker(format!("more:{project}"))))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.expanded.remove(&key) {
                                this.expanded.insert(key.clone());
                            }
                            cx.notify();
                        }))
                        .into_any_element()
                }
            };
            column = column.child(element);
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.separator)
            .child(div().h(px(44.)).flex_none())
            .child(column)
    }
}

pub struct Composer {
    app: WeakEntity<HarnessApp>,
    pub input: Entity<TextInput>,
    drafts: HashMap<String, String>,
    current: Option<String>,
    _submit: Subscription,
}

impl Composer {
    fn new(app: WeakEntity<HarnessApp>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new("Ask Claude…", cx));
        let submit = cx.subscribe(&input, |this, _, _: &Submitted, cx| this.submit(cx));
        Composer { app, input, drafts: HashMap::new(), current: None, _submit: submit }
    }

    /// Moving between threads keeps each one's draft.
    fn switch_to(&mut self, id: &str, _window: &mut Window, cx: &mut Context<Self>) {
        if self.current.as_deref() == Some(id) {
            return;
        }
        let text = self.input.read(cx).text().to_string();
        if let Some(old) = self.current.take() {
            if text.is_empty() {
                self.drafts.remove(&old);
            } else {
                self.drafts.insert(old, text);
            }
        }
        let draft = self.drafts.get(id).cloned().unwrap_or_default();
        self.input.update(cx, |i, cx| i.set_text(&draft, cx));
        self.current = Some(id.to_string());
    }

    pub fn submit(&mut self, cx: &mut Context<Self>) {
        let text = self.input.read(cx).text().to_string();
        if text.trim().is_empty() {
            return;
        }
        self.input.update(cx, |i, cx| i.set_text("", cx));
        if let Some(id) = self.current.take() {
            self.drafts.remove(&id);
            self.current = Some(id);
        }
        let _ = self.app.update(cx, |app, cx| {
            if let Some(t) = app.selected_thread() {
                app.send(&t, &text, cx);
            }
        });
    }
}

fn button(id: &'static str, label: String, primary: bool, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px(px(12.))
        .py(px(4.))
        .rounded(px(8.))
        .when(primary, |d| d.bg(theme.accent).text_color(gpui::white()).font_weight(FontWeight::MEDIUM))
        .when(!primary, |d| d.bg(theme.selected))
        .child(label)
}

impl Render for Composer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let Some(app) = self.app.upgrade() else { return div() };
        let (thread, model, mode) = {
            let a = app.read(cx);
            (a.selected_thread(), a.model.clone(), a.mode.clone())
        };
        let Some(thread) = thread else { return div() };
        let (running, permission) = {
            let t = thread.read(cx);
            (t.running, t.permission.clone())
        };
        let model_label = MODELS.iter().find(|m| m.0 == model).map_or("Default model", |m| m.1);
        let mode_label = MODES.iter().find(|m| m.0 == mode).map_or("Ask", |m| m.1);
        let weak = app.downgrade();
        let mut wrap = div().w_full().flex().justify_center().px(px(20.)).pb(px(16.));
        let mut inner = div().w_full().max_w(px(828.)).flex().flex_col();
        if let Some(p) = permission {
            let (allow, deny) = (weak.clone(), weak.clone());
            let (t1, t2) = (thread.clone(), thread.clone());
            let input = pretty_input(&p.input);
            inner = inner.child(
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
                    .gap(px(6.))
                    .child(
                        div()
                            .flex()
                            .gap(px(4.))
                            .child("Claude wants to use")
                            .child(div().font_weight(FontWeight::BOLD).child(p.tool.clone()))
                            .when(!p.description.is_empty(), |d| d.child(div().ml(px(8.)).text_color(theme.secondary).child(p.description.clone()))),
                    )
                    .child(
                        div()
                            .font_family(MONO)
                            .text_size(px(11.))
                            .bg(theme.code)
                            .rounded(px(6.))
                            .px(px(8.))
                            .py(px(6.))
                            .max_h(px(160.))
                            .overflow_hidden()
                            .child(input),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(button("deny", "Deny ⌘⌫".into(), false, &theme).on_click(move |_, _, cx| {
                                let _ = deny.update(cx, |a, cx| a.respond(&t2, false, cx));
                            }))
                            .child(button("allow", "Allow ⌘↩".into(), true, &theme).on_click(move |_, _, cx| {
                                let _ = allow.update(cx, |a, cx| a.respond(&t1, true, cx));
                            })),
                    ),
            );
        }
        let (w_model, w_mode, w_stop) = (weak.clone(), weak.clone(), weak.clone());
        let stop_thread = thread.clone();
        let mut composer = div()
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
            .child(self.input.clone());
        if probe::enabled() {
            composer = composer.child(probe::tracker("composer".into()));
        }
        let model_next = model.clone();
        let mode_next = mode.clone();
        composer = composer.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(px(12.))
                .text_color(theme.secondary)
                .child(div().id("model").child(format!("{model_label} ▾")).on_click(move |_, _, cx| {
                    let i = MODELS.iter().position(|m| m.0 == model_next).unwrap_or(0);
                    let next = MODELS[(i + 1) % MODELS.len()].0;
                    let _ = w_model.update(cx, |a, cx| a.set_model(next, cx));
                }))
                .child(div().id("mode").child(format!("{mode_label} ▾")).on_click(move |_, _, cx| {
                    let i = MODES.iter().position(|m| m.0 == mode_next).unwrap_or(0);
                    let next = MODES[(i + 1) % MODES.len()].0;
                    let _ = w_mode.update(cx, |a, cx| a.set_mode(next, cx));
                }))
                .child(div().flex_1())
                .child(if running {
                    button("stop", "Stop ⌘.".into(), false, &theme)
                        .text_color(theme.text)
                        .on_click(move |_, _, cx| {
                            let _ = w_stop.update(cx, |a, cx| a.interrupt(&stop_thread, cx));
                        })
                        .into_any_element()
                } else {
                    button("send", "Send".into(), true, &theme)
                        .on_click(cx.listener(|this, _, _, cx| this.submit(cx)))
                        .into_any_element()
                }),
        );
        inner = inner.child(composer);
        wrap = wrap.child(inner);
        wrap
    }
}

fn pretty_input(input: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(input) {
        Ok(value) => value["command"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| serde_json::to_string_pretty(&value).unwrap_or_else(|_| input.to_string())),
        Err(_) => input.to_string(),
    }
}
