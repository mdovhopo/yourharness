//! ⌘K: one search over everything, as the Swift app's (Model/Spotlight.swift): recent threads
//! before you type; then threads by title, tasks and memories in every project, projects,
//! what was said in conversations, and what the app can do. Titles, projects and actions
//! match as you type; transcripts and memories come from the full-text indexes a moment later,
//! off the main thread. A Claude Code session ID resumes that session. ↑↓ choose, ↩ opens,
//! ⎋ closes.

use crate::state::workspace::{session_id_in, Workspace};
use crate::theme::{radius, space, text, Theme};
use crate::ui::icons;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, IconName, Sizable};
use gpui_kit::{div, prelude::*, px, Context, Entity, EventEmitter, FontWeight, Hsla, ScrollHandle, Subscription, Task, Window};
use std::collections::HashSet;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Section {
    Recent,
    Threads,
    Tasks,
    Memories,
    Projects,
    Conversations,
    Actions,
}

impl Section {
    fn title(self) -> &'static str {
        match self {
            Section::Recent => "Recent Threads",
            Section::Threads => "Threads",
            Section::Tasks => "Tasks",
            Section::Memories => "Memories",
            Section::Projects => "Projects",
            Section::Conversations => "In Conversations",
            Section::Actions => "Actions",
        }
    }
}

/// What a result does: the window carries it out (`SpotlightEvent`).
#[derive(Clone, PartialEq, Debug)]
pub enum Action {
    Thread(String),
    /// A thread, and Find on the word that matched.
    ThreadAt(String, Option<String>),
    Project(String),
    /// A memory or task in a project's Memory page.
    Memory { project: String, memory: String, task: bool },
    NewThread(String),
    OpenMemory(String),
    OpenTasks(String),
    Importer(String),
    Reconciler(String),
    AddProject,
    ResumeSession,
    Settings,
}

pub struct SpotlightEvent(pub Action);

#[derive(Clone, Copy, PartialEq)]
enum Glyph {
    Thread,
    Settled,
    Home,
    Folder,
    Memory(&'static str),
    Reply,
    New,
    Brain,
    Tasks,
    Import,
    Reconcile,
    AddProject,
    Resume,
    Settings,
}

#[derive(Clone)]
struct Hit {
    id: String,
    section: Section,
    glyph: Glyph,
    title: String,
    subtitle: String,
    snippet: Option<String>,
    shortcut: Option<&'static str>,
    action: Action,
}

pub struct Spotlight {
    workspace: Entity<Workspace>,
    pub input: Entity<InputState>,
    pub open: bool,
    results: Vec<Hit>,
    index: usize,
    error: Option<String>,
    scroll: ScrollHandle,
    _search: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SpotlightEvent> for Spotlight {}

fn words(query: &str) -> Vec<String> {
    query.to_lowercase().split_whitespace().map(str::to_string).collect()
}

fn matches(text: &str, words: &[String]) -> bool {
    let text = text.to_lowercase();
    words.iter().all(|w| text.contains(w.as_str()))
}

fn ordered(mut hits: Vec<Hit>) -> Vec<Hit> {
    // Stable: within a section, the order they were found in.
    hits.sort_by_key(|h| h.section);
    hits
}

fn tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path.to_string(),
    }
}

/// The index marks matches with `[` `]`: plain text, one line.
fn plain(snippet: &str) -> String {
    snippet.replace(['[', ']'], "").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn first_match(snippet: &str) -> Option<String> {
    let open = snippet.find('[')?;
    let close = snippet[open..].find(']')? + open;
    let word = &snippet[open + 1..close];
    (!word.is_empty()).then(|| word.to_string())
}

impl Spotlight {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search threads, memories, tasks… or paste a Claude Code session ID"));
        let subscriptions = vec![cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::Change => this.query(window, cx),
            InputEvent::PressEnter { .. } => this.choose(this.index, cx),
            _ => {}
        })];
        Spotlight { workspace, input, open: false, results: vec![], index: 0, error: None, scroll: ScrollHandle::new(), _search: None, _subscriptions: subscriptions }
    }

    pub fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.error = None;
        self.input.update(cx, |i, cx| {
            i.focus(window, cx);
            i.select_all(window, cx);
        });
        self.query(window, cx);
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        self._search = None;
        cx.notify();
    }

    /// The project actions are about: the open page's, else the open thread's, else Home.
    fn current_project(&self, cx: &Context<Self>) -> Option<(String, String)> {
        let ws = self.workspace.read(cx);
        let id = match &ws.page {
            Some(crate::state::workspace::Page::Project(p) | crate::state::workspace::Page::Memory(p)) => Some(p.clone()),
            _ => None,
        }
        .or_else(|| ws.selected_thread().map(|t| t.read(cx).meta.project_id.clone()))
        .or_else(|| ws.home_project())
        .or_else(|| ws.projects.first().map(|p| p.id.clone()))?;
        let name = ws.project(&id).map(|p| p.name.clone()).unwrap_or_default();
        Some((id, name))
    }

    fn thread_hit(&self, id: &str, section: Section, cx: &Context<Self>) -> Option<Hit> {
        let ws = self.workspace.read(cx);
        let t = ws.thread(id)?;
        let t = t.read(cx);
        let project = ws.project(&t.meta.project_id).map(|p| p.name.clone()).unwrap_or_default();
        let now = yh_core::library::now_ms() / 1000.0;
        let settled = t.meta.settled_at.is_some();
        let when = yh_core::clock::relative(t.meta.updated_at / 1000.0, now);
        Some(Hit {
            id: format!("thread-{id}"),
            section,
            glyph: if settled { Glyph::Settled } else { Glyph::Thread },
            title: t.title.clone(),
            subtitle: format!("{project} · {when}{}", if settled { " · Settled" } else { "" }),
            snippet: None,
            shortcut: None,
            action: Action::Thread(id.to_string()),
        })
    }

    fn actions(&self, cx: &Context<Self>) -> Vec<Hit> {
        let Some((project, name)) = self.current_project(cx) else { return vec![] };
        let hit = |id: &str, glyph, title: &str, subtitle: String, shortcut, action| Hit {
            id: format!("action-{id}"),
            section: Section::Actions,
            glyph,
            title: title.into(),
            subtitle,
            snippet: None,
            shortcut,
            action,
        };
        vec![
            hit("new-thread", Glyph::New, "New Thread", format!("In {name}"), Some("⌘N"), Action::NewThread(project.clone())),
            hit("memory", Glyph::Brain, "Open Memory", name.clone(), Some("⇧⌘M"), Action::OpenMemory(project.clone())),
            hit("tasks", Glyph::Tasks, "Open Tasks", name.clone(), None, Action::OpenTasks(project.clone())),
            hit("import", Glyph::Import, "Import Memory…", format!("The Importer, for {name}"), None, Action::Importer(project.clone())),
            hit("reconcile", Glyph::Reconcile, "Reconcile Memory…", format!("The Reconciler, for {name}"), None, Action::Reconciler(project.clone())),
            hit("add-project", Glyph::AddProject, "Add Project…", "Pick a folder to work in".into(), Some("⌘O"), Action::AddProject),
            hit("resume", Glyph::Resume, "Resume Claude Code Session…", "Continue a session started elsewhere".into(), Some("⇧⌘O"), Action::ResumeSession),
            hit("settings", Glyph::Settings, "Settings…", "Agents, memory, browser, notifications".into(), Some("⌘,"), Action::Settings),
        ]
    }

    /// What matches as typed, at once.
    fn immediate(&self, query: &str, cx: &Context<Self>) -> Vec<Hit> {
        let words = words(query);
        let ws = self.workspace.read(cx);
        let mut threads: Vec<(f64, String, String)> = ws.threads.iter().map(|t| (t.read(cx).meta.updated_at, t.read(cx).id().to_string(), t.read(cx).title.clone())).collect();
        threads.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut hits = vec![];
        if words.is_empty() {
            hits.extend(threads.iter().take(6).filter_map(|(_, id, _)| self.thread_hit(id, Section::Recent, cx)));
        } else {
            hits.extend(threads.iter().filter(|(_, _, title)| matches(title, &words)).take(8).filter_map(|(_, id, _)| self.thread_hit(id, Section::Threads, cx)));
            let home = ws.home_project();
            for p in ws.projects.iter().filter(|p| matches(&p.name, &words)) {
                hits.push(Hit {
                    id: format!("project-{}", p.id),
                    section: Section::Projects,
                    glyph: if home.as_ref() == Some(&p.id) { Glyph::Home } else { Glyph::Folder },
                    title: p.name.clone(),
                    subtitle: tilde(&p.path),
                    snippet: None,
                    shortcut: None,
                    action: Action::Project(p.id.clone()),
                });
            }
        }
        hits.extend(self.actions(cx).into_iter().filter(|h| words.is_empty() || matches(&h.title, &words)));
        ordered(hits)
    }

    pub fn query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let q = self.input.read(cx).value().trim().to_string();
        self.error = None;
        let immediate = self.immediate(&q, cx);
        self.results = immediate.clone();
        self.index = 0;
        self.scroll.scroll_to_item(0);
        cx.notify();
        self._search = None;
        if q.chars().count() < 2 || session_id_in(&q).is_some() {
            return;
        }
        let ws = self.workspace.read(cx);
        let db = ws.library.db.clone();
        let store = ws.memory_store();
        let projects: Vec<(String, String)> = ws.projects.iter().map(|p| (yh_core::memory::scope_of(&p.id), p.id.clone())).collect();
        let names: std::collections::HashMap<String, String> = ws.projects.iter().map(|p| (p.id.clone(), p.name.clone())).collect();
        let shown: HashSet<String> = immediate.iter().filter_map(|h| h.id.strip_prefix("thread-").map(str::to_string)).collect();
        self._search = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(90)).await;
            let query = q.clone();
            let (hits, memories) = cx
                .background_executor()
                .spawn(async move {
                    let hits = db.as_ref().map(|db| db.search(&query, 80)).unwrap_or_default();
                    let memories = store.map(|s| s.find(&query, None, false, 16)).unwrap_or_default();
                    (hits, memories)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.input.read(cx).value().trim() != q {
                    return;
                }
                let mut found = vec![];
                for (m, snippet) in memories {
                    if m.status != "active" {
                        continue;
                    }
                    let Some((_, project)) = projects.iter().find(|(scope, _)| *scope == m.scope) else { continue };
                    let name = names.get(project).cloned().unwrap_or_default();
                    let task = m.is_task();
                    let subtitle = if task {
                        let mut s = format!("{name} · {}", yh_core::memory::state_label(m.task_state.as_deref().unwrap_or("active")));
                        if !m.steps.is_empty() {
                            s.push_str(&format!(" · {} of {} steps", m.steps_done(), m.steps.len()));
                        }
                        s
                    } else {
                        format!("{name} · {}", memory_kind_label(&m.kind))
                    };
                    found.push(Hit {
                        id: format!("memory-{}", m.id),
                        section: if task { Section::Tasks } else { Section::Memories },
                        glyph: Glyph::Memory(memory_kind(&m.kind)),
                        title: m.title.clone(),
                        subtitle,
                        snippet: Some(if m.description.is_empty() { plain(&snippet) } else { m.description.clone() }).filter(|s| !s.is_empty()),
                        shortcut: None,
                        action: Action::Memory { project: project.clone(), memory: m.id.clone(), task },
                    });
                }
                // One hit a thread, none for a thread already found by its title.
                let mut seen = shown.clone();
                let mut conversations = 0;
                for hit in hits {
                    if conversations >= 8 || !seen.insert(hit.thread.clone()) {
                        continue;
                    }
                    let Some(mut h) = this.thread_hit(&hit.thread, Section::Conversations, cx) else { continue };
                    h.id = format!("hit-{}", hit.thread);
                    h.glyph = Glyph::Reply;
                    h.snippet = Some(plain(&hit.snippet));
                    h.action = Action::ThreadAt(hit.thread.clone(), first_match(&hit.snippet));
                    found.push(h);
                    conversations += 1;
                }
                let selected = this.results.get(this.index).map(|h| h.id.clone());
                let mut all = this.results.clone();
                all.extend(found);
                this.results = ordered(all);
                this.index = selected.and_then(|id| this.results.iter().position(|h| h.id == id)).unwrap_or(0);
                cx.notify();
            });
        }));
    }

    pub fn choose(&mut self, index: usize, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        if session_id_in(&text).is_some() {
            match self.workspace.update(cx, |ws, cx| ws.resume_session(&text, cx)) {
                Ok(()) => self.close(cx),
                Err(e) => {
                    self.error = Some(e);
                    cx.notify();
                }
            }
            return;
        }
        let Some(hit) = self.results.get(index).cloned() else { return };
        self.close(cx);
        if std::env::var("YH_DATA_DIR").is_ok() {
            eprintln!("SPOTLIGHT chose {:?}", hit.action);
        }
        match hit.action {
            Action::Thread(id) => self.workspace.update(cx, |ws, cx| ws.select(&id, cx)),
            Action::Project(id) => self.workspace.update(cx, |ws, cx| ws.open_project(&id, cx)),
            other => cx.emit(SpotlightEvent(other)),
        }
    }

    fn on_key(&mut self, e: &gpui_kit::KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let n = self.results.len().max(1);
        match e.keystroke.key.as_str() {
            "up" => self.index = self.index.saturating_sub(1),
            "down" => self.index = (self.index + 1).min(n - 1),
            "escape" => self.close(cx),
            _ => return,
        }
        // The row and, for the first of a section, its heading.
        self.scroll.scroll_to_item(self.index);
        cx.stop_propagation();
        cx.notify();
    }

    /// The results as listed, for test runs: `section · title · subtitle`.
    pub fn describe(&self) -> Vec<String> {
        self.results.iter().map(|h| format!("{} · {} · {}", h.section.title(), h.title, h.subtitle)).collect()
    }
}

fn memory_kind(kind: &str) -> &'static str {
    match kind {
        "user" => "user",
        "feedback" => "feedback",
        "reference" => "reference",
        "task" => "task",
        _ => "project",
    }
}

fn memory_kind_label(kind: &str) -> &'static str {
    match kind {
        "user" => "About You",
        "feedback" => "Feedback",
        "reference" => "Reference",
        "task" => "Task",
        _ => "Project",
    }
}

fn glyph(g: Glyph, theme: &Theme) -> (Icon, Option<Hsla>) {
    match g {
        Glyph::Thread => (Icon::new(IconName::FileText), None),
        Glyph::Settled => (Icon::new(IconName::Inbox), None),
        Glyph::Home => (icons::house(), None),
        Glyph::Folder => (Icon::new(IconName::Folder), None),
        Glyph::Memory(kind) => (
            if kind == "task" { Icon::new(IconName::CircleCheck) } else { Icon::new(IconName::BookOpen) },
            Some(match kind {
                "user" => gpui_kit::rgb(0xaf52de).into(),
                "feedback" => theme.warning,
                "reference" => gpui_kit::rgb(0x30b0c7).into(),
                "task" => gpui_kit::rgb(0x34c759).into(),
                _ => theme.accent,
            }),
        ),
        Glyph::Reply => (Icon::new(IconName::Search), None),
        Glyph::New => (icons::square_pen(), None),
        Glyph::Brain => (icons::brain(), None),
        Glyph::Tasks => (Icon::new(IconName::CircleCheck), None),
        Glyph::Import => (Icon::new(IconName::ArrowDown), None),
        Glyph::Reconcile => (Icon::new(IconName::Replace), None),
        Glyph::AddProject => (Icon::new(IconName::FolderOpen), None),
        Glyph::Resume => (Icon::new(IconName::Redo2), None),
        Glyph::Settings => (Icon::new(IconName::Settings), None),
    }
}

impl Render for Spotlight {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        if !self.open {
            return div();
        }
        let resuming = session_id_in(&self.input.read(cx).value()).is_some();
        let mut list = div().id("spotlight-results").flex().flex_col().max_h(px(440.)).overflow_y_scroll().track_scroll(&self.scroll).p(px(space::S - 2.));
        if resuming {
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::M - 2.))
                    .px(px(space::M - 2.))
                    .py(px(space::S))
                    .text_color(theme.secondary)
                    .child(div().w(px(20.)).flex().justify_center().child(icons::corner_down_left().size(px(14.)).text_color(theme.secondary)))
                    .child("Resume this Claude Code session as a thread"),
            );
        }
        for (i, hit) in self.results.iter().enumerate() {
            let selected = i == self.index;
            let first = i == 0 || self.results[i - 1].section != hit.section;
            let (icon, tint) = glyph(hit.glyph, &theme);
            let fg = if selected { gpui_kit::white() } else { theme.text };
            let dim = if selected { gpui_kit::white().opacity(0.8) } else { theme.secondary };
            // The heading rides on its section's first row, so a row's index is its result's.
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .when(first, |d| {
                        d.child(
                            div()
                                .px(px(space::M - 2.))
                                .pt(px(if i == 0 { space::XS } else { space::M }))
                                .pb(px(space::XS))
                                .text_size(px(text::CAPTION))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.tertiary)
                                .child(hit.section.title()),
                        )
                    })
                    .child(
                        div()
                            .id(("spotlight", i))
                            .flex()
                            .items_center()
                            .gap(px(space::M - 2.))
                            .px(px(space::M - 2.))
                            .min_h(px(32.))
                            .py(px(space::XS + 1.))
                            .rounded(px(radius::MEDIUM))
                            .text_color(fg)
                            .when(selected, |d| d.bg(theme.accent))
                            .when(!selected, |d| d.hover(|s| s.bg(theme.hover)))
                            .child(div().flex_none().w(px(20.)).flex().justify_center().child(icon.size(px(15.)).text_color(if selected { gpui_kit::white() } else { tint.unwrap_or(theme.secondary) })))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .flex()
                                            .items_baseline()
                                            .gap(px(6.))
                                            .min_w_0()
                                            .child(div().flex_none().max_w(px(360.)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(hit.title.clone()))
                                            .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(text::FOOTNOTE)).text_color(dim).child(hit.subtitle.clone())),
                                    )
                                    .when_some(hit.snippet.clone(), |d, s| d.child(div().overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(text::FOOTNOTE)).text_color(dim).child(s))),
                            )
                            .when_some(hit.shortcut, |d, k| d.child(div().flex_none().text_size(px(text::FOOTNOTE)).text_color(if selected { gpui_kit::white().opacity(0.8) } else { theme.tertiary }).child(k)))
                            .on_click(cx.listener(move |this, _, _, cx| this.choose(i, cx))),
                    ),
            );
        }
        // Over the window, its top third, as Spotlight sits.
        div()
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(88.))
            .bg(gpui_kit::black().opacity(0.12))
            .on_mouse_down(gpui_kit::MouseButton::Left, cx.listener(|this, _, _, cx| this.close(cx)))
            .child(
                div()
                    .w(px(640.))
                    .h_auto()
                    .flex()
                    .flex_col()
                    .rounded(px(radius::LARGE + 2.))
                    .bg(theme.canvas)
                    .border_1()
                    .border_color(theme.separator)
                    .shadow_lg()
                    .overflow_hidden()
                    .capture_key_down(cx.listener(Self::on_key))
                    .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S))
                            .px(px(space::L))
                            .h(px(52.))
                            .child(Icon::new(IconName::Search).size(px(18.)).text_color(theme.tertiary))
                            .child(div().flex_1().child(Input::new(&self.input).appearance(false).large())),
                    )
                    .when_some(self.error.clone(), |d, e| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(space::S))
                                .px(px(space::L))
                                .pb(px(space::S + 2.))
                                .text_color(theme.error)
                                .child(Icon::new(IconName::TriangleAlert).size(px(14.)).text_color(theme.error))
                                .child(e),
                        )
                    })
                    .when(!self.results.is_empty() || resuming, |d| {
                        d.child(div().h(px(1.)).bg(theme.separator)).child(list).child(
                            // How to move about, as a footer that never moves.
                            div()
                                .flex()
                                .items_center()
                                .gap(px(space::L))
                                .h(px(30.))
                                .px(px(space::L))
                                .border_t_1()
                                .border_color(theme.separator)
                                .text_size(px(text::CAPTION))
                                .text_color(theme.tertiary)
                                .child(hint(icons::arrow_up_down(), "Move", &theme))
                                .child(hint(icons::corner_down_left(), "Open", &theme))
                                .child(div().flex().items_center().gap(px(space::XS + 1.)).child(div().font_weight(FontWeight::MEDIUM).child("esc")).child("Close")),
                        )
                    }),
            )
    }
}

fn hint(icon: Icon, label: &'static str, theme: &Theme) -> impl IntoElement {
    div().flex().items_center().gap(px(space::XS + 1.)).child(icon.size(px(12.)).text_color(theme.tertiary)).child(label)
}
