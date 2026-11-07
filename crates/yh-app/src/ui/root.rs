//! The window: sidebar, the open thread's transcript, composer. It follows the workspace: a
//! thread opened makes or brings back its transcript (six stay made for going back and forth,
//! the rest let go of their rows) and moves the composer to it.

use crate::state::workspace::{Workspace, WorkspaceEvent};
use crate::theme::{radius, space, text, Theme, UI};
use crate::ui::composer::Composer;
use crate::ui::find::FindBar;
use crate::ui::inspector::Inspector;
use crate::ui::memory::MemoryPage;
use crate::ui::spotlight::Spotlight;
use crate::ui::sidebar::Sidebar;
use crate::ui::transcript::TranscriptView;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::Sizable;
use gpui_kit::{
    actions, div, prelude::*, px, AnyView, App, Context, Entity, FocusHandle, Focusable, FontWeight, StyleRefinement,
    Subscription, Window,
};
use yh_core::claude::Decision;

actions!(
    harness,
    [
        Stop, Allow, Deny, Quit, NewThread, Find, FindNext, FindPrevious, OpenSettings, OpenSpotlight, ToggleInspector, OpenMemory, ToggleUsage,
        PreviousThread, NextThread, PreviousMessage, NextMessage, TopOfThread, EndOfThread, AddProject, ResumeSession, ExportState, ImportState,
        CheckForUpdates, UndoThreadAction, ToggleSidebar
    ]
);

/// Transcripts kept made for going back and forth.
const KEPT_TRANSCRIPTS: usize = 6;

/// The sidebar's width, as dragged (`sidebarWidth`), and its limits.
const SIDEBAR_DEFAULT: f32 = 260.;
const SIDEBAR_MIN: f32 = 200.;
const SIDEBAR_MAX: f32 = 440.;
/// The middle column (a thread or a page) asks for this much: below it the sidebar narrows
/// to its minimum, then the right sidebar narrows to `INSPECTOR_MIN`.
const MAIN_MIN: f32 = 360.;
/// Below this even then, the left sidebar folds away (not saved: it comes back when there's
/// room, or when Show Sidebar closes the right sidebar to make some).
const MAIN_FOLD: f32 = 300.;
const INSPECTOR_MIN: f32 = 320.;

/// The two sidebars' widths in a window this wide: (left, right), 0 when not shown.
fn column_widths(window: f32, left: f32, right: f32) -> (f32, f32) {
    let (mut left, mut right) = (left, right);
    if left > 0. && window - left - right < MAIN_MIN {
        left = (window - right - MAIN_MIN).max(SIDEBAR_MIN).min(left);
        if window - left - right < MAIN_FOLD {
            left = 0.;
        }
    }
    if right > 0. && window - left - right < MAIN_MIN {
        right = (window - left - MAIN_MIN).max(INSPECTOR_MIN).min(right);
    }
    (left, right)
}

pub struct RootView {
    /// The left sidebar: hidden with ⌃⌘S (`sidebarHidden`), and its width while resizing.
    sidebar_hidden: bool,
    sidebar_width: f32,
    /// Folded away for want of room at the last render (`column_widths`), though not hidden.
    sidebar_folded: bool,
    resizing_sidebar: bool,
    save_frame: Option<gpui_kit::Task<()>>,
    pub workspace: Entity<Workspace>,
    pub sidebar: Entity<Sidebar>,
    pub composer: Entity<Composer>,
    pub find: Entity<FindBar>,
    pub spotlight: Entity<Spotlight>,
    pub resume: Entity<crate::ui::resume_sheet::ResumeSheet>,
    pub inspector: Entity<Inspector>,
    pub mcp: Entity<crate::ui::mcp::McpSheet>,
    /// What Claude left running, above the composer.
    pub jobs: Entity<crate::ui::jobs::JobsStrip>,
    deck: Vec<(String, Entity<TranscriptView>)>,
    /// The Memory page when one is open (or was last: it's kept for going back to).
    memory: Option<Entity<MemoryPage>>,
    /// The open project's page (PRs and repos), kept while it's the same project.
    project: Option<Entity<crate::ui::project::ProjectPage>>,
    /// Usage, made the first time it's opened.
    usage: Option<Entity<crate::ui::usage::UsagePage>>,
    /// The open thread's tasks from memory, for its header's chip and its popover.
    tasks: Vec<yh_core::memory::Memory>,
    tasks_for: Option<(String, u64)>,
    focus: FocusHandle,
    /// "Working 0:12" over the transcript's bottom: outside it, so its tick doesn't redraw it.
    working: Entity<crate::ui::working::WorkingTimer>,
    _subscriptions: Vec<Subscription>,
}

impl RootView {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let updater = cx.new(crate::state::updater::Updater::new);
        let sidebar = cx.new(|cx| Sidebar::new(workspace.clone(), updater.clone(), window, cx));
        let composer = cx.new(|cx| Composer::new(workspace.clone(), window, cx));
        let root = cx.entity().downgrade().into();
        composer.update(cx, |c, _| c.on_height = Some(root));
        let find = cx.new(|cx| FindBar::new(workspace.clone(), window, cx));
        let spotlight = cx.new(|cx| Spotlight::new(workspace.clone(), window, cx));
        let resume = cx.new(|cx| crate::ui::resume_sheet::ResumeSheet::new(workspace.clone(), window, cx));
        let inspector = cx.new(|cx| Inspector::new(workspace.clone(), window, cx));
        let mcp = cx.new(|cx| crate::ui::mcp::McpSheet::new(workspace.clone(), window, cx));
        let jobs = cx.new(|_| crate::ui::jobs::JobsStrip::new(workspace.clone()));
        cx.set_global(crate::ui::settings::SettingsLinks { mcp: mcp.clone(), updater: updater.clone() });
        let links = inspector.downgrade();
        cx.set_global(crate::ui::inspector::FileLinks(std::rc::Rc::new(move |path, lines, cx| {
            let _ = links.update(cx, |i, cx| i.open_file(path, lines, cx));
        })));
        let subscriptions = vec![
            // The window's place and size, saved a moment after it stops changing, for next launch.
            cx.observe_window_bounds(window, |this, window, cx| {
                if std::env::var("YH_PERF").is_ok() || window.is_fullscreen() {
                    return;
                }
                let b = window.bounds();
                let frame = serde_json::json!([f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height)]);
                this.save_frame = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(std::time::Duration::from_millis(400)).await;
                    let _ = this.update(cx, |this, cx| this.workspace.read(cx).library.set_setting("windowFrame", &frame));
                }));
            }),
            cx.subscribe_in(&workspace, window, |this, _, event, window, cx| match event {
                WorkspaceEvent::Selected(id) => this.show(id, window, cx),
                WorkspaceEvent::Removed(id) => {
                    this.deck.retain(|(t, _)| t != id);
                    cx.notify();
                }
                WorkspaceEvent::PageOpened => this.open_page(window, cx),
                WorkspaceEvent::OpenMcp => this.mcp.update(cx, |m, cx| m.show(cx)),
                WorkspaceEvent::AppendToComposer(text) => this.composer.update(cx, |c, cx| c.append_draft(text, window, cx)),
            }),
            cx.observe(&workspace, |this, _, cx| {
                this.refresh_tasks(cx);
                cx.notify();
            }),
            cx.subscribe_in(&spotlight, window, |this, _, event: &crate::ui::spotlight::SpotlightEvent, window, cx| this.spotlight_action(event.0.clone(), window, cx)),
        ];
        let (sidebar_hidden, sidebar_width) = {
            let lib = &workspace.read(cx).library;
            (
                lib.setting("sidebarHidden").and_then(|v| v.as_bool()).unwrap_or(false),
                lib.setting("sidebarWidth").and_then(|v| v.as_f64()).map_or(SIDEBAR_DEFAULT, |w| (w as f32).clamp(SIDEBAR_MIN, SIDEBAR_MAX)),
            )
        };
        let working = cx.new(|cx| crate::ui::working::WorkingTimer::new(workspace.clone(), cx));
        RootView { sidebar_hidden, sidebar_width, sidebar_folded: false, resizing_sidebar: false, save_frame: None, workspace, sidebar, composer, find, spotlight, resume, inspector, mcp, jobs, deck: vec![], memory: None, project: None, usage: None, tasks: vec![], tasks_for: None, focus: cx.focus_handle(), working, _subscriptions: subscriptions }
    }

    fn show(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = self.workspace.read(cx).thread(id) else { return };
        if let Some(i) = self.deck.iter().position(|(t, _)| t == id) {
            let slot = self.deck.remove(i);
            self.deck.push(slot);
        } else {
            thread.update(cx, |t, _| t.in_deck = true);
            let ws = self.workspace.clone();
            let view = cx.new(|cx| TranscriptView::new(thread.clone(), ws, cx));
            self.deck.push((id.to_string(), view));
            while self.deck.len() > KEPT_TRANSCRIPTS {
                let (old, _) = self.deck.remove(0);
                let ws = self.workspace.clone();
                if let Some(t) = ws.read(cx).thread(&old) {
                    ws.update(cx, |ws, cx| ws.release(&t, cx));
                }
            }
        }
        self.composer.update(cx, |c, cx| c.switch_to(id, window, cx));
        self.jobs.update(cx, |j, cx| j.switch_to(id, cx));
        cx.notify();
    }

    fn open_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(crate::state::workspace::Page::Project(project)) = self.workspace.read(cx).page.clone() {
            if self.project.as_ref().is_none_or(|p| p.read(cx).project != project) {
                let ws = self.workspace.clone();
                self.project = Some(cx.new(|cx| crate::ui::project::ProjectPage::new(ws, project, window, cx)));
            } else if let Some(p) = &self.project {
                p.update(cx, |p, cx| p.refresh(false, cx));
            }
            cx.notify();
            return;
        }
        let Some(crate::state::workspace::Page::Memory(project)) = self.workspace.read(cx).page.clone() else {
            if self.workspace.read(cx).page == Some(crate::state::workspace::Page::Usage) && self.usage.is_none() {
                let ws = self.workspace.clone();
                self.usage = Some(cx.new(|cx| crate::ui::usage::UsagePage::new(ws, window, cx)));
            }
            cx.notify();
            return;
        };
        if self.memory.as_ref().is_none_or(|m| m.read(cx).project != project) {
            let ws = self.workspace.clone();
            self.memory = Some(cx.new(|cx| MemoryPage::new(ws, project, window, cx)));
        }
        cx.notify();
    }

    /// The open thread's tasks, read off the main thread when the thread or memory changes.
    fn refresh_tasks(&mut self, cx: &mut Context<Self>) {
        let ws = self.workspace.read(cx);
        let key = ws.selected.clone().map(|id| (id, ws.memory_revision));
        if key == self.tasks_for {
            return;
        }
        self.tasks_for = key.clone();
        let (Some((id, _)), Some(store)) = (key, ws.memory_store()) else {
            self.tasks.clear();
            return;
        };
        cx.spawn(async move |this, cx| {
            let tasks = cx
                .background_executor()
                .spawn(async move {
                    store
                        .tasks_of_thread(&id)
                        .into_iter()
                        .filter(|m| m.status == "active")
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.tasks != tasks {
                    this.tasks = tasks;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn memory_page(&self, cx: &App) -> Option<Entity<MemoryPage>> {
        let Some(crate::state::workspace::Page::Memory(project)) = self.workspace.read(cx).page.clone() else { return None };
        self.memory.clone().filter(|m| m.read(cx).project == project)
    }

    pub fn transcript_view(&self, cx: &App) -> Option<Entity<TranscriptView>> {
        let id = self.workspace.read(cx).selected.clone()?;
        self.deck.iter().find(|(t, _)| *t == id).map(|(_, v)| v.clone())
    }

    fn on_stop(&mut self, _: &Stop, _: &mut Window, cx: &mut Context<Self>) {
        let ws = self.workspace.clone();
        if let Some(t) = ws.read(cx).selected_thread() {
            if t.read(cx).running {
                ws.update(cx, |ws, cx| ws.interrupt(&t, cx));
            }
        }
    }

    fn respond(&mut self, decision: Decision, cx: &mut Context<Self>) {
        let ws = self.workspace.clone();
        if let Some(t) = ws.read(cx).selected_thread() {
            ws.update(cx, |ws, cx| ws.respond(&t, decision, cx));
        }
    }

    fn end_sidebar_resize(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.resizing_sidebar) || self.sidebar_width == SIDEBAR_DEFAULT {
            self.workspace.read(cx).library.set_setting("sidebarWidth", &serde_json::Value::from(self.sidebar_width as f64));
            cx.notify();
        }
    }

    fn on_toggle_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.dispatch_action(Box::new(ToggleSidebar), cx);
    }

    /// The thread above or below the open one in the sidebar.
    fn adjacent_thread(&mut self, step: isize, cx: &mut Context<Self>) {
        let order = self.sidebar.read(cx).thread_order(cx);
        let current = self.workspace.read(cx).selected.clone();
        let i = current.and_then(|c| order.iter().position(|t| *t == c));
        let next = match i {
            Some(i) => (i as isize + step).clamp(0, order.len() as isize - 1) as usize,
            None => 0,
        };
        if let Some(id) = order.get(next).cloned() {
            self.workspace.update(cx, |ws, cx| ws.select(&id, cx));
        }
    }

    /// Jumps between your messages in the open thread: -1 the one above, 1 the one below,
    /// i64::MIN the top, i64::MAX the end.
    fn jump(&mut self, to: i64, cx: &mut Context<Self>) {
        let Some(thread) = self.workspace.read(cx).selected_thread() else { return };
        if to == i64::MIN && thread.read(cx).before > 0 {
            // Earlier pages first: the top is the thread's start.
            let ws = self.workspace.clone();
            ws.update(cx, |ws, cx| ws.load_earlier(&thread, cx));
        }
        thread.update(cx, |t, cx| {
            let count = t.items.len();
            if count == 0 {
                return;
            }
            let top = t.list.logical_scroll_top().item_ix;
            let users: Vec<usize> = t.items.iter().enumerate().filter(|(_, i)| i.ty == crate::state::thread::ItemType::User).map(|(n, _)| n).collect();
            let target = match to {
                i64::MIN => 0,
                i64::MAX => count - 1,
                -1 => users.iter().rev().find(|&&n| n < top).copied().unwrap_or(0),
                _ => users.iter().find(|&&n| n > top).copied().unwrap_or(count - 1),
            };
            if to == i64::MAX {
                t.list.scroll_to_end();
            } else {
                t.list.scroll_to(gpui_kit::ListOffset { item_ix: target, offset_in_item: gpui_kit::px(0.) });
            }
            cx.notify();
        });
    }

    /// "Project · worktree · model", under the title.
    fn subtitle(&self, thread: &Entity<crate::state::thread::ThreadState>, cx: &App) -> String {
        let ws = self.workspace.read(cx);
        let t = thread.read(cx);
        let mut parts = vec![ws.project(&t.meta.project_id).map(|p| p.name.clone()).unwrap_or_default()];
        // Running in a worktree: which one.
        if let Some(folder) = &t.meta.working_directory {
            parts.push(std::path::Path::new(folder).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| folder.clone()));
        }
        if let Some(model) = ws.model_of(t) {
            parts.push(ws.models.iter().find(|m| m.value == model).map(|m| m.display_name.clone()).unwrap_or(model));
        }
        parts.retain(|p| !p.is_empty());
        parts.join(" · ")
    }

    /// The tasks this thread works on, from memory: the first's state and steps, "+N" for the
    /// rest; it opens a popover with each one's steps and the way to them in Memory.
    fn tasks_chip(&self, tasks: Vec<yh_core::memory::Memory>, theme: &Theme, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        use gpui_kit::component::popover::Popover;
        use gpui_kit::component::{Icon, IconName};
        let first = tasks.iter().find(|m| m.task_state.as_deref() != Some("done")).or(tasks.first())?.clone();
        let state = first.task_state.clone().unwrap_or_else(|| "active".into());
        let theme = *theme;
        let color = move |state: &str| -> gpui_kit::Hsla {
            match state {
                "blocked" => theme.warning,
                "done" => gpui_kit::rgb(0x34c759).into(),
                "todo" => theme.secondary,
                _ => theme.accent,
            }
        };
        let label = if first.steps.is_empty() { yh_core::memory::state_label(&state).to_string() } else { format!("{}/{}", first.steps_done(), first.steps.len()) };
        let tip = format!("Tasks this thread is working on: {}", tasks.iter().map(|t| t.title.as_str()).collect::<Vec<_>>().join(", "));
        let more = tasks.len().saturating_sub(1);
        let ws = self.workspace.clone();
        let project = self.workspace.read(cx).selected_thread().map(|t| t.read(cx).meta.project_id.clone());
        let this = cx.entity().downgrade();
        Some(
            Popover::new("thread-tasks")
                // `YH_OPEN_TASKS=1` opens it, for checking it without a click.
                .default_open(std::env::var("YH_OPEN_TASKS").is_ok())
                .trigger(
                    gpui_kit::component::button::Button::new("thread-tasks-button")
                        .small()
                        .ghost()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(5.))
                                .child(div().w(px(7.)).h(px(7.)).rounded_full().bg(color(&state)))
                                .child(label)
                                .when(more > 0, |d| d.child(div().text_color(theme.secondary).child(format!("+{more}")))),
                        )
                        .tooltip(tip),
                )
                .content(move |_, _, _| {
                    let mut list = div().w(px(340.)).p(px(space::L)).flex().flex_col().gap(px(space::M + 2.));
                    for task in &tasks {
                        let state = task.task_state.clone().unwrap_or_else(|| "active".into());
                        let mut card = div().flex().flex_col().gap(px(6.)).child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(div().flex_1().min_w_0().font_weight(FontWeight::SEMIBOLD).child(task.title.clone()))
                                .child(
                                    div()
                                        .flex_none()
                                        .px(px(6.))
                                        .rounded(px(radius::SMALL))
                                        .bg(color(&state).opacity(0.15))
                                        .text_color(color(&state))
                                        .text_size(px(text::CAPTION))
                                        .child(yh_core::memory::state_label(&state)),
                                ),
                        );
                        if !task.steps.is_empty() {
                            let done = task.steps_done() as f32 / task.steps.len() as f32;
                            card = card.child(div().h(px(4.)).rounded(px(2.)).bg(theme.selected).child(div().h_full().rounded(px(2.)).w(gpui_kit::relative(done)).bg(color(&state))));
                            for step in &task.steps {
                                card = card.child(
                                    div()
                                        .flex()
                                        .items_start()
                                        .gap(px(6.))
                                        .text_size(px(text::FOOTNOTE))
                                        .when(step.done, |d| d.text_color(theme.secondary))
                                        .child(Icon::new(if step.done { IconName::CircleCheck } else { IconName::Square }).size(px(12.)).text_color(if step.done { theme.secondary } else { theme.text }))
                                        .child(step.text.clone()),
                                );
                            }
                        }
                        list = list.child(card);
                    }
                    let (ws, project, this) = (ws.clone(), project.clone(), this.clone());
                    list.child(
                        div()
                            .id("tasks-show-in-memory")
                            .text_color(theme.accent)
                            .text_size(px(text::FOOTNOTE))
                            .child("Show in Memory")
                            .on_click(move |_, window, cx| {
                                let Some(project) = project.clone() else { return };
                                ws.update(cx, |ws, cx| ws.open_memory(&project, cx));
                                let _ = this.update(cx, |root, cx| root.after_memory_opens(None, true, window, cx));
                            }),
                    )
                })
                .into_any_element(),
        )
    }

    /// Once the Memory page is up: on Tasks or Memories, at a memory.
    fn after_memory_opens(&mut self, memory: Option<String>, tasks: bool, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(50)).await;
            let _ = this.update(cx, |root, cx| {
                let Some(page) = root.memory_page(cx) else { return };
                page.update(cx, |p, cx| {
                    p.tab = if tasks { crate::ui::memory::Tab::Tasks } else { crate::ui::memory::Tab::Memories };
                    if let Some(id) = &memory {
                        p.select(id, cx);
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// What a Spotlight result asks of the window.
    fn spotlight_action(&mut self, action: crate::ui::spotlight::Action, window: &mut Window, cx: &mut Context<Self>) {
        use crate::ui::spotlight::Action;
        let ws = self.workspace.clone();
        match action {
            Action::Thread(id) => ws.update(cx, |ws, cx| ws.select(&id, cx)),
            Action::ThreadAt(id, word) => {
                ws.update(cx, |ws, cx| ws.select(&id, cx));
                // Find, on the word that matched: it highlights it and scrolls there, once the
                // transcript has its rows.
                if let Some(word) = word {
                    let find = self.find.clone();
                    cx.spawn_in(window, async move |_, cx| {
                        cx.background_executor().timer(std::time::Duration::from_millis(400)).await;
                        let _ = find.update_in(cx, |f, window, cx| {
                            f.show(window, cx);
                            f.input.update(cx, |i, cx| i.set_value(word.clone(), window, cx));
                            f.search(cx);
                        });
                    })
                    .detach();
                }
            }
            Action::Project(id) => ws.update(cx, |ws, cx| ws.open_project(&id, cx)),
            Action::Memory { project, memory, task } => {
                ws.update(cx, |ws, cx| ws.open_memory(&project, cx));
                self.after_memory_opens(Some(memory), task, window, cx);
            }
            Action::NewThread(project) => self.sidebar.update(cx, |s, cx| s.new_thread(&project, cx)),
            Action::OpenMemory(project) => {
                ws.update(cx, |ws, cx| ws.open_memory(&project, cx));
                self.after_memory_opens(None, false, window, cx);
            }
            Action::OpenTasks(project) => {
                ws.update(cx, |ws, cx| ws.open_memory(&project, cx));
                self.after_memory_opens(None, true, window, cx);
            }
            Action::Importer(project) => ws.update(cx, |ws, cx| ws.open_memory_agent("importer", &project, vec![], None, cx)),
            Action::Reconciler(project) => ws.update(cx, |ws, cx| ws.open_memory_agent("reconciler", &project, vec![], None, cx)),
            Action::AddProject => self.sidebar.update(cx, |s, cx| s.add_project(cx)),
            Action::ResumeSession => self.resume.update(cx, |s, cx| s.show(window, cx)),
            Action::Settings => crate::ui::settings::open(ws, window, cx),
        }
    }

    /// No thread open: the first project to add, or a new thread to start.
    fn empty_state(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::component::button::Button;
        use gpui_kit::component::{Icon, IconName};
        // Home is always there; a first run has nothing else, and no threads.
        let ws = self.workspace.read(cx);
        let home = ws.home_project();
        let first_run = ws.threads.is_empty() && ws.projects.iter().all(|p| Some(&p.id) == home.as_ref());
        let (icon, title, body): (Icon, &str, &str) = if first_run {
            (crate::ui::icons::folder_plus(), "Add Your First Project", "Pick a folder to work in. Claude runs there, using your own Claude Code login.")
        } else {
            (crate::ui::icons::message_square(), "No Thread Selected", "Choose a thread in the sidebar, or start a new one.")
        };
        div()
            .flex_1()
            .relative()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .pb(px(40.))
            .when(self.sidebar_hidden || self.sidebar_folded, |d| {
                d.child(
                    div().absolute().top(px(8.)).left(px(84.)).child(
                        crate::ui::thread_toolbar::icon_button("show-sidebar-empty", Icon::new(IconName::PanelLeft), "Show Sidebar (⌃⌘S)", theme)
                            .on_click(cx.listener(|this, _, window, cx| this.on_toggle_sidebar(window, cx))),
                    ),
                )
            })
            .child(icon.size(px(36.)).text_color(theme.tertiary))
            .child(div().mt(px(space::L)).text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).child(title))
            .child(div().mt(px(space::XS)).max_w(px(340.)).text_color(theme.secondary).text_center().child(body))
            .child(div().mt(px(space::L)).child(if first_run {
                Button::new("empty-add-project")
                    .primary()
                    .label("Add Project…")
                    .on_click(cx.listener(|this, _, _, cx| this.sidebar.update(cx, |s, cx| s.add_project(cx))))
                    .into_any_element()
            } else {
                Button::new("empty-new-thread")
                    .outline()
                    .icon(crate::ui::icons::square_pen().size(px(14.)))
                    .label("New Thread")
                    .tooltip("New Thread (⌘N)")
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(NewThread), cx))
                    .into_any_element()
            }))
    }

    fn on_new_thread(&mut self, _: &NewThread, _: &mut Window, cx: &mut Context<Self>) {
        let ws = self.workspace.clone();
        let project = ws
            .read(cx)
            .selected_thread()
            .map(|t| t.read(cx).meta.project_id.clone())
            .or_else(|| ws.read(cx).projects.first().map(|p| p.id.clone()));
        if let Some(project) = project {
            self.sidebar.update(cx, |s, cx| s.new_thread(&project, cx));
        }
    }
}

impl Focusable for RootView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl RootView {
    /// The composer, cached at the height it last measured (`Composer::measure`): this view
    /// draws again on every frame of scrolling, and the composer needn't be laid out each time.
    /// Not cached until it has measured once.
    fn placed_composer(&self, cx: &App) -> gpui_kit::AnyElement {
        let h = self.composer.read(cx).height.get();
        if h <= 0. {
            return self.composer.clone().into_any_element();
        }
        AnyView::from(self.composer.clone()).cached(StyleRefinement::default().w_full().h(px(h))).into_any_element()
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let project_page = match &self.workspace.read(cx).page {
            Some(crate::state::workspace::Page::Project(p)) => self.project.clone().filter(|e| e.read(cx).project == *p),
            _ => None,
        };
        let page: Option<AnyView> = project_page.map(AnyView::from).or_else(|| self.memory_page(cx).map(AnyView::from)).or_else(|| {
            (self.workspace.read(cx).page == Some(crate::state::workspace::Page::Usage)).then(|| self.usage.clone().map(AnyView::from)).flatten()
        });
        let selected = if page.is_some() { None } else { self.workspace.read(cx).selected_thread() };
        let title = selected.as_ref().map(|t| t.read(cx).title.clone()).unwrap_or_else(|| "YourHarness".into());
        let subtitle = selected.as_ref().map(|t| self.subtitle(t, cx)).unwrap_or_default();
        let transcript = self.transcript_view(cx);
        // The browser's page shows only over its panel; anything else hides it.
        let ws = self.workspace.read(cx);
        if !(selected.is_some() && ws.shows_inspector && ws.panel == crate::state::workspace::Panel::Browser) {
            crate::browser::Browsers::show(None);
        }
        let tasks = if selected.is_some() { self.tasks.clone() } else { vec![] };
        // What each column gets in this window: a narrow one squeezes the sidebars before the
        // thread or page, and folds the left one away last.
        let inspector_open = selected.is_some() && self.workspace.read(cx).shows_inspector;
        let (left_w, right_w) = column_widths(
            f32::from(window.viewport_size().width),
            if self.sidebar_hidden { 0. } else { self.sidebar_width },
            if inspector_open { crate::ui::inspector::WIDTH } else { 0. },
        );
        self.sidebar_folded = !self.sidebar_hidden && left_w == 0.;
        let sidebar_shown = left_w > 0.;
        let start = selected.as_ref().and_then(|t| start_prompt(&self.workspace.read(cx), &t.read(cx)));
        div()
            .id("root")
            .key_context("Harness")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_stop))
            .on_action(cx.listener(|this, _: &Allow, _, cx| {
                if !this.composer.update(cx, |c, cx| c.card_shortcut(true, cx)) {
                    this.respond(Decision::allow(), cx)
                }
            }))
            .on_action(cx.listener(|this, _: &Deny, _, cx| {
                if !this.composer.update(cx, |c, cx| c.card_shortcut(false, cx)) {
                    this.respond(Decision::deny(), cx)
                }
            }))
            .on_action(cx.listener(Self::on_new_thread))
            .on_action(cx.listener(|this, _: &OpenSpotlight, window, cx| this.spotlight.update(cx, |s, cx| s.show(window, cx))))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| crate::ui::settings::open(this.workspace.clone(), window, cx)))
            .on_action(cx.listener(|this, _: &Find, window, cx| this.find.update(cx, |f, cx| f.show(window, cx))))
            .on_action(cx.listener(|this, _: &ToggleInspector, _, cx| {
                this.workspace.update(cx, |ws, cx| {
                    let on = !ws.shows_inspector;
                    ws.set_shows_inspector(on, cx)
                })
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                // Folded for want of room: the right sidebar makes room for it.
                if this.sidebar_folded {
                    this.workspace.update(cx, |ws, cx| ws.set_shows_inspector(false, cx));
                    return cx.notify();
                }
                this.sidebar_hidden = !this.sidebar_hidden;
                this.workspace.read(cx).library.set_setting("sidebarHidden", &serde_json::Value::Bool(this.sidebar_hidden));
                cx.notify();
            }))
            .on_mouse_move(cx.listener(|this, e: &gpui_kit::MouseMoveEvent, _, cx| {
                if this.resizing_sidebar {
                    if e.dragging() {
                        this.sidebar_width = f32::from(e.position.x).clamp(SIDEBAR_MIN, SIDEBAR_MAX);
                        cx.notify();
                    } else {
                        this.end_sidebar_resize(cx);
                    }
                }
            }))
            .on_mouse_up(gpui_kit::MouseButton::Left, cx.listener(|this, _, _, cx| this.end_sidebar_resize(cx)))
            .on_action(cx.listener(|this, _: &UndoThreadAction, _, cx| this.workspace.update(cx, |ws, cx| ws.undo(cx))))
            .on_action(cx.listener(|this, _: &PreviousThread, _, cx| this.adjacent_thread(-1, cx)))
            .on_action(cx.listener(|this, _: &NextThread, _, cx| this.adjacent_thread(1, cx)))
            .on_action(cx.listener(|this, _: &PreviousMessage, _, cx| this.jump(-1, cx)))
            .on_action(cx.listener(|this, _: &NextMessage, _, cx| this.jump(1, cx)))
            .on_action(cx.listener(|this, _: &TopOfThread, _, cx| this.jump(i64::MIN, cx)))
            .on_action(cx.listener(|this, _: &EndOfThread, _, cx| this.jump(i64::MAX, cx)))
            .on_action(cx.listener(|this, _: &AddProject, _, cx| this.sidebar.update(cx, |s, cx| s.add_project(cx))))
            .on_action(cx.listener(|this, _: &ResumeSession, window, cx| this.resume.update(cx, |s, cx| s.show(window, cx))))
            .on_action(cx.listener(|this, _: &ExportState, _, cx| crate::ui::state_transfer::export(this.workspace.clone(), cx)))
            .on_action(cx.listener(|this, _: &ImportState, window, cx| crate::ui::state_transfer::import(this.workspace.clone(), window, cx)))
            .on_action(cx.listener(|_, _: &CheckForUpdates, _, cx| {
                if let Some(links) = cx.try_global::<crate::ui::settings::SettingsLinks>() {
                    let u = links.updater.clone();
                    u.update(cx, |u, cx| u.check(cx));
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleUsage, _, cx| this.workspace.update(cx, |ws, cx| ws.toggle_usage(cx))))
            .on_action(cx.listener(|this, _: &OpenMemory, _, cx| {
                let ws = this.workspace.clone();
                let project = ws
                    .read(cx)
                    .selected_thread()
                    .map(|t| t.read(cx).meta.project_id.clone())
                    .or_else(|| ws.read(cx).projects.first().map(|p| p.id.clone()));
                if let Some(project) = project {
                    ws.update(cx, |ws, cx| ws.open_memory(&project, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &FindNext, _, cx| this.find.update(cx, |f, cx| f.step(true, cx))))
            .on_action(cx.listener(|this, _: &FindPrevious, _, cx| this.find.update(cx, |f, cx| f.step(false, cx))))
            .size_full()
            .relative()
            .flex()
            .font_family(UI)
            .text_size(px(text::BODY))
            .line_height(px(19.5))
            .text_color(theme.text)
            .bg(theme.canvas)
            .when(sidebar_shown, |d| {
                let width = left_w;
                d.child(AnyView::from(self.sidebar.clone()).cached(StyleRefinement::default().h_full().w(px(width)).flex_none())).child(
                    // The sidebar's edge: drag to resize, double-click for the default width.
                    div()
                        .id("sidebar-resize")
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(px(width - 3.))
                        .w(px(6.))
                        .cursor_col_resize()
                        .on_mouse_down(gpui_kit::MouseButton::Left, cx.listener(|this, e: &gpui_kit::MouseDownEvent, _, cx| {
                            if e.click_count == 2 {
                                this.sidebar_width = SIDEBAR_DEFAULT;
                                this.end_sidebar_resize(cx);
                            } else {
                                this.resizing_sidebar = true;
                            }
                            cx.stop_propagation();
                        })),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .when_some(page, |d, page| d.child(div().flex_1().min_h_0().child(page)))
                    .when(selected.is_some(), |d| {
                        d.child(
                            div()
                                .h(px(44.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(px(space::M))
                                .px(px(space::XL - 4.))
                                .when(!sidebar_shown, |d| d.pl(px(84.)).child(
                                    crate::ui::thread_toolbar::icon_button("show-sidebar", gpui_kit::component::Icon::new(gpui_kit::component::IconName::PanelLeft), "Show Sidebar (⌃⌘S)", &theme)
                                        .on_click(cx.listener(|this, _, window, cx| this.on_toggle_sidebar(window, cx))),
                                ))
                                .border_b_1()
                                .border_color(theme.separator)
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .items_baseline()
                                        .gap(px(space::S))
                                        .child(div().flex_none().max_w(gpui_kit::relative(0.7)).text_size(px(text::HEADLINE)).font_weight(FontWeight::SEMIBOLD).overflow_hidden().whitespace_nowrap().text_ellipsis().child(title))
                                        .child(div().min_w_0().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).overflow_hidden().whitespace_nowrap().text_ellipsis().child(subtitle)),
                                )
                                .child(
                                    // The toolbar: icons right-aligned, close together.
                                    div()
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .gap(px(2.))
                                        .mr(px(-6.))
                                        .children(self.tasks_chip(tasks, &theme, cx))
                                        .children(crate::ui::thread_toolbar::checkouts(&self.workspace, &theme, cx))
                                        .child(crate::ui::thread_toolbar::buttons(&self.workspace, &theme, cx)),
                                ),
                        )
                        .child(self.find.clone())
                        .when_some(start.clone(), |d, (headline, folder)| {
                            // A new thread: its composer in the middle, under where the work happens.
                            d.child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .flex()
                                    .flex_col()
                                    .justify_center()
                                    .pb(px(60.))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .items_center()
                                            .gap(px(space::S))
                                            .pb(px(space::L))
                                            .px(px(space::XL))
                                            .child(div().text_size(px(text::LARGE_TITLE)).line_height(px(28.)).font_weight(FontWeight::SEMIBOLD).child(headline))
                                            .child(
                                                div()
                                                    .max_w_full()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(space::XS + 2.))
                                                    .text_size(px(text::FOOTNOTE))
                                                    .text_color(theme.secondary)
                                                    .child(gpui_kit::component::Icon::new(gpui_kit::component::IconName::Folder).size(px(13.)).text_color(theme.tertiary))
                                                    .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(folder)),
                                            ),
                                    )
                                    .child(div().w_full().child(self.placed_composer(cx))),
                            )
                        })
                        .when(start.is_none(), |d| d.child(match transcript {
                            // Cached: typing in the composer or a sidebar change doesn't redraw the
                            // transcript; its own thread changing does.
                            Some(view) => div()
                                .flex_1()
                                .min_h_0()
                                .relative()
                                .child(AnyView::from(view).cached(StyleRefinement::default().size_full()))
                                .child(div().absolute().bottom(px(6.)).right(px(28.)).child(self.working.clone()))
                                .into_any_element(),
                            None => div().flex_1().into_any_element(),
                        })
                        .child(div().flex_none().w_full().child(self.jobs.clone()))
                        .child(div().flex_none().w_full().child(self.placed_composer(cx))))
                        // Files dropped anywhere on the thread go with the next message. The
                        // overlay shows only while files are dragged over it (a style GPUI works
                        // out at paint, so nothing re-renders and the cached transcript stays).
                        .relative()
                        .on_drop(cx.listener(|this, paths: &gpui_kit::ExternalPaths, _, cx| this.composer.update(cx, |c, cx| c.add_files(paths.paths(), cx))))
                        .child(drop_overlay(&theme))
                    })
                    .when(selected.is_none() && self.workspace.read(cx).page.is_none(), |d| d.child(self.empty_state(&theme, cx))),
            )
            .when(inspector_open, |d| {
                // Cached: it redraws when its tab's data changes, not with the transcript.
                d.child(AnyView::from(self.inspector.clone()).cached(StyleRefinement::default().h_full().w(px(right_w)).flex_none()))
            })
            .when_some(self.workspace.read(cx).undo_notice().map(str::to_string), |d, message| {
                // What the last thread action did, with a way back.
                let ws = self.workspace.clone();
                // Centred over the content just under its header, where it covers neither the
                // sidebar's footer nor the composer.
                let (left, right) = (left_w, right_w);
                d.child(
                    div().absolute().top(px(44. + space::M)).left(px(left)).right(px(right)).flex().justify_center().child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S))
                            .h(px(40.))
                            .pl(px(space::M))
                            .pr(px(space::XS + 2.))
                            .rounded(px(radius::LARGE))
                            .bg(theme.card)
                            .border_1()
                            .border_color(theme.separator)
                            .shadow_md()
                            .child(div().max_w(px(320.)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(message))
                            .child(div().w(px(1.)).h(px(18.)).bg(theme.separator))
                            .child(
                                gpui_kit::component::button::Button::new("undo")
                                    .small()
                                    .ghost()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(space::XS + 2.))
                                            .child(div().text_color(theme.accent).font_weight(FontWeight::MEDIUM).child("Undo"))
                                            .child(div().text_size(px(text::CAPTION)).text_color(theme.tertiary).child("⌘Z")),
                                    )
                                    .on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.undo(cx))),
                            ),
                    ),
                )
            })
            .child(self.spotlight.clone())
            .child(self.resume.clone())
            .child(self.mcp.clone())
    }
}

/// "Drop files to attach" over the thread, hidden until files are dragged over it (the Swift
/// app's dashed outline). Hidden, it paints nothing and takes no clicks; its own hitbox (GPUI
/// gives drag styles only to an element with one) never blocks what's under it.
fn drop_overlay(theme: &Theme) -> impl IntoElement {
    let mut overlay = div()
        .absolute()
        .top(px(8.))
        .left(px(8.))
        .right(px(8.))
        .bottom(px(8.))
        .rounded(px(16.))
        .border_2()
        .border_dashed()
        .border_color(theme.accent)
        // Mostly opaque: the words read over the composer and the transcript.
        .bg(theme.canvas.opacity(0.88))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .text_size(px(17.))
        .child(crate::ui::rows::icon(gpui_kit::component::IconName::File, 18., theme.text))
        .child("Drop files to attach")
        .drag_over::<gpui_kit::ExternalPaths>(|s, _, _, _| {
            let mut s = s;
            s.visibility = Some(gpui_kit::Visibility::Visible);
            s
        });
    overlay.style().visibility = Some(gpui_kit::Visibility::Hidden);
    overlay
}

/// Over a new thread's composer: where the work happens. None once it has rows (or is loading
/// them).
fn start_prompt(ws: &crate::state::workspace::Workspace, t: &crate::state::thread::ThreadState) -> Option<(String, String)> {
    if !t.loaded || t.running || !t.items.is_empty() || t.before > 0 || t.total.unwrap_or(0) > 0 || t.id().contains(':') && !t.id().starts_with("new:") {
        return None;
    }
    let project = ws.project(&t.meta.project_id);
    let home = ws.home_project().as_deref() == Some(t.meta.project_id.as_str());
    let headline = match project {
        Some(p) if !home => format!("What are we working on in {}?", p.name),
        _ => "What are we working on?".to_string(),
    };
    let folder = ws.folder_of(t);
    let folder = match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && folder.starts_with(&h) => format!("~{}", &folder[h.len()..]),
        _ => folder,
    };
    Some((headline, folder))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_windows_squeeze_the_sidebars_first() {
        // Room for everything: nothing changes.
        assert_eq!(column_widths(1728., 260., 380.), (260., 380.));
        // The narrowest window, sidebar only: the thread keeps 380.
        assert_eq!(column_widths(640., 260., 0.), (260., 0.));
        // A bit short: the sidebar narrows, toward its minimum.
        assert_eq!(column_widths(960., 260., 380.), (220., 380.));
        // The narrowest window with the right sidebar: the left one folds, the right narrows.
        assert_eq!(column_widths(640., 260., 380.), (0., 320.));
        // A hidden sidebar stays hidden.
        assert_eq!(column_widths(900., 0., 380.), (0., 380.));
    }
}
