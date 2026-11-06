//! A project's Memory page: what its agents remember (search, filter by kind, read, edit, pin,
//! archive, delete, write a new one, see how each changed), the tasks they track, and what
//! they did with memory. Shown in place of the thread when you click a project's name.
//! Memory is read off the main thread, again whenever it changes (`memory_revision`).

use crate::state::workspace::Workspace;
use crate::theme::{Theme, MONO};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenuItem};
use gpui_kit::component::Sizable;
use gpui_kit::{div, prelude::*, px, App, Context, Entity, FontWeight, Hsla, SharedString, Subscription, Window};
use yh_core::memory::{Actor, Memory, Store};

const KINDS: [(&str, &str); 5] =
    [("user", "About You"), ("feedback", "Feedback"), ("project", "Project"), ("reference", "Reference"), ("task", "Task")];

fn kind_label(kind: &str) -> &'static str {
    KINDS.iter().find(|(k, _)| *k == kind).map_or("Project", |(_, l)| l)
}

fn kind_color(kind: &str, theme: &Theme) -> Hsla {
    match kind {
        "user" => gpui_kit::rgb(0xaf52de).into(),
        "feedback" => theme.warning,
        "reference" => gpui_kit::rgb(0x30b0c7).into(),
        "task" => gpui_kit::rgb(0x34c759).into(),
        _ => theme.accent,
    }
}

fn state_color(state: &str, theme: &Theme) -> Hsla {
    match state {
        "blocked" => theme.warning,
        "done" => gpui_kit::rgb(0x34c759).into(),
        "todo" => theme.secondary,
        _ => theme.accent,
    }
}

fn user() -> Actor {
    Actor { kind: "user".into(), thread: None }
}

/// "5 minutes ago", from seconds since 1970.
fn ago(t: f64) -> String {
    crate::state::thread_tools::ago(t * 1000.0)
}

#[derive(Clone, Copy, PartialEq)]
pub enum Tab {
    Memories,
    Tasks,
    Activity,
}

pub struct MemoryPage {
    workspace: Entity<Workspace>,
    pub project: String,
    store: Option<Store>,
    pub tab: Tab,
    /// Every memory of the project, archived ones too; read again when memory changes.
    pub memories: Vec<Memory>,
    events: Vec<(String, Option<String>, Option<String>, f64, Option<String>, Option<i64>)>,
    loaded_revision: Option<u64>,
    pub loaded: bool,
    search: Entity<InputState>,
    kind_filter: Option<&'static str>,
    archived: bool,
    pub selection: Option<String>,
    /// The selected memory's fields as you edit them, and its kind.
    title: Entity<InputState>,
    description: Entity<InputState>,
    body: Entity<TextareaState>,
    draft_kind: String,
    /// The editors hold this memory as it was when they were filled.
    editing: Option<Memory>,
    versions: Vec<(i64, String, String, String, f64, Option<String>)>,
    shown_version: Option<i64>,
    /// A memory to put in the editors at the next render (setting a value needs the window).
    pending_fill: Option<Memory>,
    _subscriptions: Vec<Subscription>,
}

impl MemoryPage {
    pub fn new(workspace: Entity<Workspace>, project: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = workspace.read(cx).memory_store();
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search memories"));
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("Title"));
        let description = cx.new(|cx| InputState::new(window, cx).placeholder("One line: when it matters"));
        let body = cx.new(|cx| TextareaState::new(window, cx).placeholder("What to remember"));
        let subscriptions = vec![
            cx.observe(&workspace, |this, ws, cx| {
                if this.loaded_revision != Some(ws.read(cx).memory_revision) {
                    this.reload(cx);
                }
            }),
            cx.subscribe(&search, |_, _, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe(&title, |_, _, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe(&description, |_, _, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe(&body, |_, _, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        let mut page = MemoryPage {
            workspace,
            project,
            store,
            tab: Tab::Memories,
            memories: vec![],
            events: vec![],
            loaded_revision: None,
            loaded: false,
            search,
            kind_filter: None,
            archived: false,
            selection: None,
            title,
            description,
            body,
            draft_kind: "project".into(),
            editing: None,
            versions: vec![],
            shown_version: None,
            pending_fill: None,
            _subscriptions: subscriptions,
        };
        page.reload(cx);
        page
    }

    fn scope(&self) -> String {
        yh_core::memory::scope_of(&self.project)
    }

    /// Reads the project's memory and activity again, off the main thread, after any queued
    /// writes have landed.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else {
            self.loaded = true;
            return;
        };
        let revision = self.workspace.read(cx).memory_revision;
        self.loaded_revision = Some(revision);
        let scope = self.scope();
        cx.spawn(async move |this, cx| {
            let (memories, events) = cx
                .background_executor()
                .spawn(async move {
                    store.db.flush();
                    let mut all = store.memories(Some(&scope), None);
                    all.retain(|m| m.status != "proposed" || m.merged_into.is_none());
                    all.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(b.updated_at.total_cmp(&a.updated_at)));
                    (all, store.events(&scope, 300))
                })
                .await;
            let _ = this.update(cx, |page, cx| {
                page.memories = memories;
                page.events = events;
                page.loaded = true;
                page.after_reload(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Keeps the selection on a memory that's still listed, and shows changes made elsewhere
    /// in the editors unless you've edited them.
    fn after_reload(&mut self, cx: &mut Context<Self>) {
        let visible = self.visible(cx);
        if self.selection.as_ref().is_none_or(|id| !self.memories.iter().any(|m| &m.id == id)) {
            self.selection = visible.first().map(|m| m.id.clone());
        }
        let current = self.selected().cloned();
        let edited = self.is_dirty(cx);
        if let Some(m) = current {
            if self.editing.as_ref() != Some(&m) && !edited {
                self.pending_fill = Some(m);
            }
        }
    }

    pub fn selected(&self) -> Option<&Memory> {
        let id = self.selection.as_ref()?;
        self.memories.iter().find(|m| &m.id == id)
    }

    fn visible(&self, cx: &App) -> Vec<&Memory> {
        let words: Vec<String> = self.search.read(cx).value().to_lowercase().split_whitespace().map(str::to_string).collect();
        self.memories
            .iter()
            .filter(|m| (m.status == "archived") == self.archived)
            .filter(|m| self.kind_filter.is_none_or(|k| m.kind == k))
            .filter(|m| {
                let text = format!("{} {} {}", m.title, m.description, m.body).to_lowercase();
                words.iter().all(|w| text.contains(w))
            })
            .collect()
    }

    fn select(&mut self, id: &str, cx: &mut Context<Self>) {
        self.selection = Some(id.to_string());
        self.pending_fill = self.selected().cloned();
        self.shown_version = None;
        cx.notify();
    }

    fn is_dirty(&self, cx: &App) -> bool {
        let Some(m) = &self.editing else { return false };
        self.title.read(cx).value().trim() != m.title
            || self.description.read(cx).value().trim() != m.description
            || self.body.read(cx).value().trim() != m.body
            || self.draft_kind != m.kind
    }

    /// Puts a memory in the editors (needs the window, so it's done at the next render).
    fn fill(&mut self, m: Memory, window: &mut Window, cx: &mut Context<Self>) {
        self.title.update(cx, |i, cx| i.set_value(m.title.clone(), window, cx));
        self.description.update(cx, |i, cx| i.set_value(m.description.clone(), window, cx));
        self.body.update(cx, |i, cx| i.set_value(m.body.clone(), window, cx));
        self.draft_kind = m.kind.clone();
        self.versions = self.store.as_ref().map(|s| s.versions(&m.id)).unwrap_or_default();
        self.editing = Some(m);
    }

    fn written(&mut self, cx: &mut Context<Self>) {
        self.workspace.update(cx, |ws, cx| ws.memory_changed(cx));
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let (Some(store), Some(mut m)) = (self.store.clone(), self.editing.clone()) else { return };
        let title = self.title.read(cx).value().trim().to_string();
        if title.is_empty() {
            return;
        }
        m.title = title;
        m.description = self.description.read(cx).value().trim().to_string();
        m.body = self.body.read(cx).value().trim().to_string();
        if m.kind != self.draft_kind {
            m.kind = self.draft_kind.clone();
            if m.kind == "task" && m.task_state.is_none() {
                m.task_state = Some("active".into());
            }
        }
        m.updated_at = yh_core::library::now_ms() / 1000.0;
        let created = !self.memories.iter().any(|x| x.id == m.id);
        store.save(m.clone(), &user(), Some(if created { "created" } else { "updated" }), None);
        self.editing = Some(m);
        self.written(cx);
    }

    fn revert(&mut self, cx: &mut Context<Self>) {
        let restored = self.selected().cloned().or(self.editing.clone());
        if let Some(m) = restored {
            self.pending_fill = Some(m);
        }
        cx.notify();
    }

    /// Asks for files or folders, then hands them to the Importer.
    fn import_with_agent(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui_kit::PathPromptOptions { files: true, directories: true, multiple: true, prompt: Some("Import".into()) });
        let (ws, project) = (self.workspace.clone(), self.project.clone());
        cx.spawn(async move |_, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else { return };
            let sources: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
            let _ = cx.update(|cx| {
                ws.update(cx, |ws, cx| {
                    let message = yh_core::memory_agents::import_request(&sources, None);
                    ws.open_memory_agent("importer", &project, sources, Some(message), cx)
                })
            });
        })
        .detach();
    }

    pub fn new_memory(&mut self, cx: &mut Context<Self>) {
        let kind = self.kind_filter.unwrap_or("project");
        let m = Memory::new(&self.scope(), kind, "", "", "", "user");
        self.tab = Tab::Memories;
        self.archived = false;
        self.selection = Some(m.id.clone());
        // Listed once saved; until then it's only in the editors.
        self.pending_fill = Some(m);
        cx.notify();
    }

    fn set_pinned(&mut self, id: &str, on: bool, cx: &mut Context<Self>) {
        if let Some(store) = &self.store {
            store.set_pinned(id, on, &user());
            self.written(cx);
        }
    }

    fn set_status(&mut self, id: &str, status: &str, cx: &mut Context<Self>) {
        if let Some(store) = &self.store {
            store.set_status(id, status, &user(), None);
            self.written(cx);
        }
    }

    fn delete(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(store) = &self.store {
            store.delete(id, &user());
            self.selection = None;
            self.editing = None;
            self.written(cx);
        }
    }

    fn set_task_state(&mut self, id: &str, state: &str, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else { return };
        let Some(mut m) = self.memories.iter().find(|m| m.id == id).cloned() else { return };
        m.task_state = Some(state.into());
        m.updated_at = yh_core::library::now_ms() / 1000.0;
        store.save(m, &user(), Some("updated"), None);
        self.written(cx);
    }

    fn toggle_step(&mut self, id: &str, step: usize, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else { return };
        let Some(mut m) = self.memories.iter().find(|m| m.id == id).cloned() else { return };
        let Some(s) = m.steps.get_mut(step) else { return };
        s.done = !s.done;
        if m.steps.iter().all(|s| s.done) {
            m.task_state = Some("done".into());
        } else if m.task_state.as_deref() == Some("done") {
            m.task_state = Some("active".into());
        }
        m.updated_at = yh_core::library::now_ms() / 1000.0;
        store.save(m, &user(), Some("updated"), None);
        self.written(cx);
    }

    fn summary(&self) -> String {
        let active: Vec<&Memory> = self.memories.iter().filter(|m| m.status == "active").collect();
        let archived = self.memories.iter().filter(|m| m.status == "archived").count();
        if active.is_empty() && archived == 0 {
            return "Nothing remembered yet".into();
        }
        let bytes: usize = active.iter().map(|m| m.size()).sum();
        let mut parts = vec![
            if active.len() == 1 { "1 memory".to_string() } else { format!("{} memories", active.len()) },
            if bytes < 1024 { format!("{bytes} bytes") } else { format!("{:.1} KB", bytes as f64 / 1024.0) },
        ];
        let reads: i64 = active.iter().map(|m| m.recall_count).sum();
        if reads > 0 {
            parts.push(format!("{reads} reads"));
        }
        if archived > 0 {
            parts.push(format!("{archived} archived"));
        }
        parts.join(" · ")
    }
}

// MARK: Drawing

fn chip(label: impl Into<SharedString>, color: Hsla) -> gpui_kit::Div {
    div()
        .flex_none()
        .px(px(7.))
        .py(px(1.))
        .rounded(px(9.))
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .bg(color.opacity(0.14))
        .child(label.into())
}

fn section(title: &str, theme: &Theme) -> gpui_kit::Div {
    div().pt(px(6.)).text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(theme.secondary).child(title.to_uppercase())
}

impl MemoryPage {
    fn header(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self.workspace.read(cx).project(&self.project).map(|p| {
            if self.workspace.read(cx).home_project().as_deref() == Some(p.id.as_str()) { "Home".to_string() } else { p.name.clone() }
        });
        let memory_on = self.workspace.read(cx).library.setting("memoryEnabled").and_then(|v| v.as_bool()).unwrap_or(true);
        let tab_button = |id: &'static str, label: &'static str, tab: Tab, cx: &mut Context<Self>| {
            let mut b = Button::new(id).small().label(label);
            b = if self.tab == tab { b.primary() } else { b.ghost() };
            b.on_click(cx.listener(move |this, _, _, cx| {
                this.tab = tab;
                cx.notify();
            }))
        };
        let tasks = self.memories.iter().filter(|m| m.is_task() && m.status == "active").count();
        div()
            .flex()
            .items_center()
            .gap(px(14.))
            .px(px(28.))
            .py(px(16.))
            .border_b_1()
            .border_color(theme.separator)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).child(format!("{} Memory", name.unwrap_or_default())))
                    .child(div().text_color(theme.secondary).child(if self.loaded { self.summary() } else { "Loading…".into() })),
            )
            .when(!memory_on, |d| d.child(chip("Off for agents", theme.warning)))
            .child(
                div()
                    .flex()
                    .gap(px(2.))
                    .child(tab_button("tab-memories", "Memories", Tab::Memories, cx))
                    .child(tab_button("tab-tasks", if tasks > 0 { "Tasks ·" } else { "Tasks" }, Tab::Tasks, cx).when(tasks > 0, |b| b.label(format!("Tasks {tasks}"))))
                    .child(tab_button("tab-activity", "Activity", Tab::Activity, cx)),
            )
            .child(Button::new("memory-import").small().ghost().label("Import…").tooltip("Bring memory in from files or folders: the Importer reads them and saves what's worth keeping").on_click(cx.listener(|this, _, _, cx| this.import_with_agent(cx))))
            .child(Button::new("memory-reconcile").small().ghost().label("Reconcile").tooltip("The Reconciler finds duplicates, stale and vague memories, and fixes them with you").on_click(cx.listener(|this, _, _, cx| {
                let project = this.project.clone();
                this.workspace.update(cx, |ws, cx| ws.open_memory_agent("reconciler", &project, vec![], Some(yh_core::memory_agents::RECONCILE_REQUEST.to_string()), cx));
            })))
            .child(Button::new("new-memory").small().label("New Memory").on_click(cx.listener(|this, _, _, cx| this.new_memory(cx))))
    }

    fn list(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let visible = self.visible(cx);
        let count = visible.len();
        let mut chips = div().flex().flex_wrap().gap(px(4.));
        let filter_chip = |id: String, label: &'static str, on: bool, color: Hsla, value: Option<&'static str>, cx: &mut Context<Self>| {
            div()
                .id(SharedString::from(id))
                .px(px(8.))
                .py(px(2.))
                .rounded(px(9.))
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if on { color } else { theme.secondary })
                .bg(if on { color.opacity(0.16) } else { theme.selected })
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.kind_filter = if this.kind_filter == value { None } else { value };
                    cx.notify();
                }))
        };
        chips = chips.child(filter_chip("kind-all".into(), "All", self.kind_filter.is_none(), theme.accent, None, cx));
        for (kind, label) in KINDS {
            chips = chips.child(filter_chip(format!("kind-{kind}"), label, self.kind_filter == Some(kind), kind_color(kind, theme), Some(kind), cx));
        }
        let status = |id: &'static str, label: &'static str, archived: bool, cx: &mut Context<Self>| {
            let b = Button::new(id).xsmall().label(label);
            (if self.archived == archived { b.primary() } else { b.ghost() }).on_click(cx.listener(move |this, _, _, cx| {
                this.archived = archived;
                cx.notify();
            }))
        };
        let mut rows = div().id("memory-list").flex_1().min_h_0().overflow_y_scroll().p(px(8.)).flex().flex_col().gap(px(2.));
        if count == 0 {
            rows = rows.child(div().p(px(16.)).text_color(theme.secondary).child(if self.memories.is_empty() { "Nothing remembered yet." } else { "No memories match." }));
        }
        for m in visible {
            let id = m.id.clone();
            let selected = self.selection.as_deref() == Some(m.id.as_str());
            let preview = if m.description.is_empty() { m.body.clone() } else { m.description.clone() };
            let (pin_id, status_id, del_id, pinned, archived) = (id.clone(), id.clone(), id.clone(), m.pinned, m.status == "archived");
            let page = cx.entity().downgrade();
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("memory-{id}")))
                    .px(px(10.))
                    .py(px(7.))
                    .rounded(px(7.))
                    .when(selected, |d| d.bg(theme.accent.opacity(0.16)))
                    .when(!selected, |d| d.hover(|s| s.bg(theme.selected)))
                    .flex()
                    .gap(px(9.))
                    .child(div().mt(px(6.)).w(px(7.)).h(px(7.)).flex_none().rounded_full().bg(kind_color(&m.kind, theme)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .flex()
                                    .gap(px(4.))
                                    .child(div().font_weight(FontWeight::MEDIUM).overflow_hidden().whitespace_nowrap().text_ellipsis().child(if m.title.is_empty() { "Untitled".to_string() } else { m.title.clone() }))
                                    .when(m.pinned, |d| d.child(div().text_size(px(10.)).text_color(theme.secondary).child("pinned"))),
                            )
                            .child(div().text_size(px(12.)).text_color(theme.secondary).line_clamp(2).child(preview))
                            .when(m.is_task(), |d| {
                                let state = m.task_state.clone().unwrap_or_else(|| "active".into());
                                d.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .child(chip(yh_core::memory::state_label(&state), state_color(&state, theme)))
                                        .when(!m.steps.is_empty(), |d| {
                                            d.child(div().text_size(px(11.)).text_color(theme.secondary).child(format!("{} of {} steps", m.steps_done(), m.steps.len())))
                                        }),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .gap(px(8.))
                                    .text_size(px(11.))
                                    .text_color(theme.secondary.opacity(0.8))
                                    .child(format!("read {}×", m.recall_count))
                                    .when(!m.links.is_empty(), |d| d.child(format!("{} links", m.links.len())))
                                    .child(div().flex_1())
                                    .child(ago(m.updated_at)),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.select(&id, cx)))
                    .context_menu(move |menu, _, _| {
                        let (a, b, c) = (page.clone(), page.clone(), page.clone());
                        let (i1, i2, i3) = (pin_id.clone(), status_id.clone(), del_id.clone());
                        menu.item(PopupMenuItem::new(if pinned { "Unpin" } else { "Pin (Always Load in Full)" }).on_click(move |_, _, cx| {
                            let _ = a.update(cx, |p, cx| p.set_pinned(&i1, !pinned, cx));
                        }))
                        .item(PopupMenuItem::new(if archived { "Restore" } else { "Archive" }).on_click(move |_, _, cx| {
                            let _ = b.update(cx, |p, cx| p.set_status(&i2, if archived { "active" } else { "archived" }, cx));
                        }))
                        .separator()
                        .item(PopupMenuItem::new("Delete").on_click(move |_, _, cx| {
                            let _ = c.update(cx, |p, cx| p.delete(&i3, cx));
                        }))
                    }),
            );
        }
        div()
            .w(px(320.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(theme.separator)
            .child(
                div()
                    .p(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .border_b_1()
                    .border_color(theme.separator)
                    .child(Input::new(&self.search).small().cleanable(true))
                    .child(chips)
                    .child(div().flex().gap(px(2.)).child(status("in-use", "In Use", false, cx)).child(status("archived", "Archived", true, cx))),
            )
            .child(rows)
            .child(div().py(px(5.)).text_center().text_size(px(11.)).text_color(theme.secondary).child(if count == 1 { "1 memory".to_string() } else { format!("{count} memories") }))
    }

    fn detail(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(m) = self.editing.clone().filter(|e| Some(&e.id) == self.selection.as_ref()) else {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.secondary)
                .child("Choose a memory to see what it says, how agents use it and how it changed.")
                .into_any_element();
        };
        let saved = self.memories.iter().find(|x| x.id == m.id).cloned();
        let listed = saved.is_some();
        let current = saved.unwrap_or_else(|| m.clone());
        let dirty = self.is_dirty(cx) || !listed;
        let id = m.id.clone();
        let page = cx.entity().downgrade();
        let kind_menu = {
            let page = page.clone();
            let current_kind = self.draft_kind.clone();
            Button::new("kind").xsmall().ghost().label(kind_label(&self.draft_kind)).dropdown_caret(true).dropdown_menu(move |mut menu, _, _| {
                for (kind, label) in KINDS {
                    let page = page.clone();
                    menu = menu.item(PopupMenuItem::new(label).checked(current_kind == kind).on_click(move |_, _, cx| {
                        let _ = page.update(cx, |p, cx| {
                            p.draft_kind = kind.to_string();
                            cx.notify();
                        });
                    }));
                }
                menu
            })
        };
        let (pin_id, status_id, del_id) = (id.clone(), id.clone(), id.clone());
        let archived = current.status == "archived";
        let pinned = current.pinned;
        let mut body = div()
            .id("memory-detail")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(24.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().w(px(9.)).h(px(9.)).rounded_full().bg(kind_color(&self.draft_kind, theme)))
                    .child(kind_menu)
                    .when(pinned, |d| d.child(chip("Pinned", theme.secondary)))
                    .when(current.status != "active", |d| d.child(chip(if archived { "Archived" } else { "Proposed" }, theme.warning)))
                    .when(listed, |d| d.child(div().font_family(MONO).text_size(px(11.)).text_color(theme.secondary).child(current.short_id())))
                    .child(div().flex_1())
                    .when(listed, |d| {
                        d.child(Button::new("pin").xsmall().ghost().label(if pinned { "Unpin" } else { "Pin" }).tooltip("Pinned: every session gets it in full, not only its index line").on_click(
                            cx.listener(move |this, _, _, cx| this.set_pinned(&pin_id, !pinned, cx)),
                        ))
                        .child(Button::new("archive").xsmall().ghost().label(if archived { "Restore" } else { "Archive" }).on_click(cx.listener(move |this, _, _, cx| {
                            this.set_status(&status_id, if archived { "active" } else { "archived" }, cx)
                        })))
                        .child(Button::new("delete").xsmall().ghost().label("Delete").on_click(cx.listener(move |this, _, _, cx| this.delete(&del_id, cx))))
                    }),
            )
            .child(Input::new(&self.title).appearance(false).text_size(px(20.)).font_weight(FontWeight::SEMIBOLD))
            .child(Input::new(&self.description).appearance(false).text_color(theme.secondary))
            .child(div().p(px(10.)).rounded(px(8.)).bg(theme.card).child(Textarea::new(&self.body).appearance(false).max_h(px(420.))));
        if current.is_task() {
            let state = current.task_state.clone().unwrap_or_else(|| "active".into());
            let mut states = div().flex().gap(px(2.));
            for s in ["todo", "active", "blocked", "done"] {
                let tid = id.clone();
                let b = Button::new(SharedString::from(format!("state-{s}"))).xsmall().label(yh_core::memory::state_label(s));
                states = states.child((if state == s { b.primary() } else { b.ghost() }).on_click(cx.listener(move |this, _, _, cx| this.set_task_state(&tid, s, cx))));
            }
            let mut steps = div().flex().flex_col().gap(px(3.));
            for (i, step) in current.steps.iter().enumerate() {
                let tid = id.clone();
                steps = steps.child(
                    div()
                        .id(SharedString::from(format!("step-{i}")))
                        .flex()
                        .gap(px(8.))
                        .rounded(px(5.))
                        .px(px(4.))
                        .hover(|s| s.bg(theme.selected))
                        .child(div().w(px(16.)).text_color(if step.done { state_color("done", theme) } else { theme.secondary }).child(if step.done { "✓" } else { "○" }))
                        .child(div().flex_1().when(step.done, |d| d.text_color(theme.secondary).line_through()).child(step.text.clone()))
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_step(&tid, i, cx))),
                );
            }
            body = body.child(section("Task", theme)).child(states).child(steps);
        }
        if listed {
            body = body.child(section("Use", theme)).child(
                div().text_color(theme.secondary).child(format!(
                    "Read by agents {}×, in the index of {} sessions{}. Saved {}, changed {}.",
                    current.recall_count,
                    current.surfaced_count,
                    current.last_recalled_at.map(|t| format!(", last {}", ago(t))).unwrap_or_default(),
                    ago(current.created_at),
                    ago(current.updated_at),
                )),
            );
        }
        if !current.links.is_empty() {
            let mut links = div().flex().flex_col().gap(px(2.));
            for (to, relation) in &current.links {
                let title = self.memories.iter().find(|x| &x.id == to).map(|x| x.title.clone()).unwrap_or_else(|| to.chars().take(8).collect());
                let target = to.clone();
                links = links.child(
                    div()
                        .id(SharedString::from(format!("link-{to}")))
                        .flex()
                        .gap(px(6.))
                        .text_color(theme.accent)
                        .child(title)
                        .when_some(relation.clone(), |d, r| d.child(div().text_color(theme.secondary).child(r)))
                        .on_click(cx.listener(move |this, _, _, cx| this.select(&target, cx))),
                );
            }
            body = body.child(section("Linked", theme)).child(links);
        }
        if !current.sources.is_empty() {
            let mut sources = div().flex().flex_col().gap(px(2.));
            let ws = self.workspace.clone();
            for (thread, _) in current.sources.iter().take(10) {
                let title = ws.read(cx).thread(thread).map(|t| t.read(cx).title.clone());
                let (ws, thread_id) = (ws.clone(), thread.clone());
                sources = sources.child(
                    div()
                        .id(SharedString::from(format!("source-{thread}")))
                        .text_color(if title.is_some() { theme.accent } else { theme.secondary })
                        .child(title.clone().unwrap_or_else(|| "A thread that's gone".into()))
                        .when(title.is_some(), |d| d.on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.select(&thread_id, cx)))),
                );
            }
            body = body.child(section("Learned in", theme)).child(sources);
        }
        if self.versions.len() > 1 {
            let mut history = div().flex().flex_col().gap(px(2.));
            for (version, title, text, status, at, actor) in self.versions.iter().rev() {
                let open = self.shown_version == Some(*version);
                let v = *version;
                history = history.child(
                    div()
                        .id(SharedString::from(format!("version-{version}")))
                        .flex()
                        .gap(px(8.))
                        .rounded(px(5.))
                        .px(px(4.))
                        .hover(|s| s.bg(theme.selected))
                        .child(div().w(px(28.)).text_color(theme.secondary).child(format!("v{version}")))
                        .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_ellipsis().child(title.clone()))
                        .when(status != "active", |d| d.child(div().text_color(theme.secondary).child(status.clone())))
                        .child(div().text_color(theme.secondary).child(format!("{} · {}", actor.clone().unwrap_or_else(|| "agent".into()), ago(*at))))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.shown_version = if this.shown_version == Some(v) { None } else { Some(v) };
                            cx.notify();
                        })),
                );
                if open {
                    history = history.child(div().ml(px(36.)).p(px(10.)).rounded(px(8.)).bg(theme.card).text_size(px(12.)).whitespace_normal().child(text.clone()));
                }
            }
            body = body.child(section("History", theme)).child(history);
        }
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(body)
            .when(dirty, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(16.))
                        .py(px(10.))
                        .border_t_1()
                        .border_color(theme.separator)
                        .child(div().flex_1().text_color(theme.secondary).child(if listed { "Edited" } else { "New memory" }))
                        .child(Button::new("revert").small().label(if listed { "Revert" } else { "Discard" }).on_click(cx.listener(move |this, _, _, cx| {
                            if listed {
                                this.revert(cx);
                            } else {
                                this.selection = None;
                                this.editing = None;
                                this.after_reload(cx);
                                cx.notify();
                            }
                        })))
                        .child(Button::new("save").small().primary().label("Save ⌘S").on_click(cx.listener(|this, _, _, cx| this.save(cx)))),
                )
            })
            .into_any_element()
    }

    fn tasks(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let mut column = div().id("memory-tasks").size_full().overflow_y_scroll().p(px(24.)).flex().flex_col().gap(px(14.));
        let tasks: Vec<&Memory> = self.memories.iter().filter(|m| m.is_task() && m.status == "active").collect();
        if tasks.is_empty() {
            return column.child(div().text_color(theme.secondary).child("No tasks. When you ask an agent for work with more than one step, it tracks it here."));
        }
        for (state, heading) in [("active", "In Progress"), ("blocked", "Blocked"), ("todo", "To Do"), ("done", "Done")] {
            let group: Vec<&&Memory> = tasks.iter().filter(|m| m.task_state.as_deref().unwrap_or("active") == state).collect();
            if group.is_empty() {
                continue;
            }
            column = column.child(section(&format!("{heading} · {}", group.len()), theme));
            for m in group {
                let id = m.id.clone();
                column = column.child(
                    div()
                        .id(SharedString::from(format!("task-{id}")))
                        .p(px(12.))
                        .rounded(px(8.))
                        .bg(theme.card)
                        .hover(|s| s.bg(theme.selected))
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(chip(yh_core::memory::state_label(state), state_color(state, theme)))
                                .child(div().flex_1().font_weight(FontWeight::MEDIUM).child(m.title.clone()))
                                .when(!m.steps.is_empty(), |d| d.child(div().text_color(theme.secondary).child(format!("{} of {}", m.steps_done(), m.steps.len()))))
                                .child(div().text_size(px(11.)).text_color(theme.secondary).child(ago(m.updated_at))),
                        )
                        .when_some(m.next_step().filter(|_| state != "done").map(str::to_string), |d, next| {
                            d.child(div().text_size(px(12.)).text_color(theme.secondary).child(format!("Next: {next}")))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.tab = Tab::Memories;
                            this.archived = false;
                            this.select(&id, cx);
                        })),
                );
            }
        }
        column
    }

    fn activity(&self, theme: &Theme) -> impl IntoElement {
        let mut column = div().id("memory-activity").size_full().overflow_y_scroll().p(px(24.)).flex().flex_col().gap(px(4.));
        if self.events.is_empty() {
            return column.child(div().text_color(theme.secondary).child("Nothing yet: agents' saves, reads and searches show here."));
        }
        for (kind, title, actor, at, query, count) in &self.events {
            let what = match kind.as_str() {
                "created" => format!("saved “{}”", title.clone().unwrap_or_default()),
                "updated" => format!("updated “{}”", title.clone().unwrap_or_default()),
                "archived" => format!("archived “{}”", title.clone().unwrap_or_default()),
                "restored" => format!("restored “{}”", title.clone().unwrap_or_default()),
                "deleted" => format!("deleted “{}”", title.clone().unwrap_or_default()),
                "recalled" => format!("read “{}”", title.clone().unwrap_or_default()),
                "searched" => format!("searched “{}”, {} found", query.clone().unwrap_or_default(), count.unwrap_or(0)),
                "injected" => format!("started a session with {} memories in its index", count.unwrap_or(0)),
                other => format!("{other} {}", title.clone().unwrap_or_default()),
            };
            let who = match actor.as_deref() {
                Some("user") => "You",
                Some("importer") => "The importer",
                Some("reconciler") => "The reconciler",
                Some("app") => "The app",
                _ => "An agent",
            };
            column = column.child(
                div()
                    .flex()
                    .gap(px(10.))
                    .child(div().w(px(110.)).flex_none().text_color(theme.secondary).child(ago(*at)))
                    .child(div().flex_1().child(format!("{who} {what}"))),
            );
        }
        column
    }
}

impl Render for MemoryPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(m) = self.pending_fill.take() {
            self.fill(m, window, cx);
        }
        let theme = Theme::get(cx);
        let content = match self.tab {
            Tab::Memories => div().flex_1().min_h_0().flex().child(self.list(&theme, cx)).child(self.detail(&theme, cx)).into_any_element(),
            Tab::Tasks => div().flex_1().min_h_0().child(self.tasks(&theme, cx)).into_any_element(),
            Tab::Activity => div().flex_1().min_h_0().child(self.activity(&theme)).into_any_element(),
        };
        div()
            .key_context("MemoryPage")
            .on_action(cx.listener(|this, _: &SaveMemory, _, cx| this.save(cx)))
            .size_full()
            .flex()
            .flex_col()
            .child(self.header(&theme, cx))
            .child(content)
    }
}

gpui_kit::actions!(memory, [SaveMemory]);
