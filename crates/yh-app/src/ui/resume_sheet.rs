//! Resume a Claude Code Session (⇧⌘O), as Views/ResumeSessionSheet.swift: paste a session ID
//! (or a `claude --resume` command), see what it is, pick the project it goes in, and open it
//! as a thread that continues it. A copied ID fills itself in; one already open shows its
//! thread instead.

use crate::state::workspace::{session_id_in, Workspace};
use crate::theme::{radius, space, text, Theme, MONO};
use crate::ui::icons;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::{Disableable, Icon, IconName, Sizable};
use gpui_kit::{div, prelude::*, px, Context, Entity, FontWeight, Subscription, Task, Window};
use yh_core::history::Summary;

#[derive(Clone, PartialEq)]
enum Lookup {
    Idle,
    Loading,
    Found(String, Summary),
    Failed(String),
}

/// Which project the thread goes in.
#[derive(Clone, PartialEq, Debug)]
enum Destination {
    Project(String),
    /// A new project for the session's folder.
    NewProject,
}

pub struct ResumeSheet {
    workspace: Entity<Workspace>,
    pub input: Entity<InputState>,
    pub open: bool,
    lookup: Lookup,
    destination: Destination,
    looked_up: Option<String>,
    _lookup: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

fn standardized(path: &str) -> String {
    let p = std::path::Path::new(path);
    std::fs::canonicalize(p).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| path.trim_end_matches('/').to_string())
}

fn tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path.to_string(),
    }
}

impl ResumeSheet {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("e.g. 3f2b9c1e-…"));
        let subscriptions = vec![cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| match event {
            InputEvent::Change => this.look_up(cx),
            InputEvent::PressEnter { .. } => this.resume(cx),
            _ => {}
        })];
        ResumeSheet { workspace, input, open: false, lookup: Lookup::Idle, destination: Destination::NewProject, looked_up: None, _lookup: None, _subscriptions: subscriptions }
    }

    /// Opens it, filled in with `YH_RESUME_SESSION`, else a session ID on the clipboard.
    pub fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        let preset = std::env::var("YH_RESUME_SESSION").ok().filter(|_| self.looked_up.is_none());
        let pasted = preset.or_else(|| {
            cx.read_from_clipboard().and_then(|c| c.text()).filter(|t| t.len() < 300 && session_id_in(t).is_some()).map(|t| t.trim().to_string())
        });
        self.input.update(cx, |i, cx| {
            if let Some(text) = pasted {
                i.set_value(text, window, cx);
            }
            i.focus(window, cx);
            i.select_all(window, cx);
        });
        self.look_up(cx);
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        cx.notify();
    }

    fn session(&self, cx: &Context<Self>) -> Option<String> {
        session_id_in(&self.input.read(cx).value())
    }

    fn existing(&self, cx: &Context<Self>) -> Option<(String, String)> {
        let session = self.session(cx)?;
        let t = self.workspace.read(cx).thread_for_session(&session, cx)?;
        let t = t.read(cx);
        Some((t.id().to_string(), t.title.clone()))
    }

    /// The session's file, read off the main thread.
    fn look_up(&mut self, cx: &mut Context<Self>) {
        let session = self.session(cx);
        if session == self.looked_up && !matches!(self.lookup, Lookup::Idle) {
            return;
        }
        self.looked_up = session.clone();
        let Some(session) = session else {
            self.lookup = Lookup::Idle;
            self._lookup = None;
            cx.notify();
            return;
        };
        self.lookup = Lookup::Loading;
        cx.notify();
        self._lookup = Some(cx.spawn(async move |this, cx| {
            let id = session.clone();
            let found = cx
                .background_executor()
                .spawn(async move {
                    let path = yh_core::history::locate(&yh_core::history::config_directory(), &id).ok_or_else(|| "No Claude Code session with this ID.".to_string())?;
                    Ok::<_, String>(yh_core::history::summary(&path))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                match found {
                    Ok(summary) => {
                        if let Some(cwd) = summary.cwd.as_deref().filter(|c| !std::path::Path::new(c).is_dir()) {
                            this.lookup = Lookup::Failed(format!("The session's folder isn't there any more: {}", tilde(cwd)));
                        } else {
                            this.destination = this.suggested(summary.cwd.as_deref(), cx).map_or(Destination::NewProject, Destination::Project);
                            this.lookup = Lookup::Found(session.clone(), summary);
                            if std::env::var("YH_DATA_DIR").is_ok() {
                                eprintln!("RESUME found {session}: into {:?}", this.destination);
                            }
                            // Development: `YH_RESUME_CONFIRM=1` resumes into the suggested project.
                            if std::env::var("YH_RESUME_CONFIRM").as_deref() == Ok("1") {
                                this.resume(cx);
                            }
                        }
                    }
                    Err(e) => this.lookup = Lookup::Failed(e),
                }
                cx.notify();
            });
        }));
    }

    /// Where a resumed session goes unless you pick: the project for its folder, else the
    /// innermost project its folder is inside, else a new project for the folder. Home holds
    /// every folder under it, so it only counts as itself.
    fn suggested(&self, cwd: Option<&str>, cx: &Context<Self>) -> Option<String> {
        let ws = self.workspace.read(cx);
        let home = ws.home_project();
        let Some(cwd) = cwd else { return home };
        let path = standardized(cwd);
        if let Some(p) = ws.projects.iter().find(|p| standardized(&p.path) == path) {
            return Some(p.id.clone());
        }
        ws.projects
            .iter()
            .filter(|p| Some(&p.id) != home.as_ref() && path.starts_with(&(standardized(&p.path) + "/")))
            .max_by_key(|p| standardized(&p.path).len())
            .map(|p| p.id.clone())
    }

    fn has_own_project(&self, cwd: Option<&str>, cx: &Context<Self>) -> bool {
        let Some(cwd) = cwd else { return true };
        let path = standardized(cwd);
        self.workspace.read(cx).projects.iter().any(|p| standardized(&p.path) == path)
    }

    fn resume(&mut self, cx: &mut Context<Self>) {
        if let Some((id, _)) = self.existing(cx) {
            self.workspace.update(cx, |ws, cx| ws.select(&id, cx));
            self.close(cx);
            return;
        }
        let Lookup::Found(session, summary) = self.lookup.clone() else { return };
        let destination = self.destination.clone();
        let result = self.workspace.update(cx, |ws, cx| {
            let project = match destination {
                Destination::Project(id) => id,
                Destination::NewProject => match summary.cwd.clone() {
                    Some(cwd) => ws.add_project(&cwd, cx),
                    None => ws.home_project().ok_or("No project to put it in.")?,
                },
            };
            let (id, messages) = ws.import_session(&session, &project, cx)?;
            ws.select(&id, cx);
            Ok::<_, String>((id, messages))
        });
        match result {
            Ok((id, messages)) => {
                if std::env::var("YH_DATA_DIR").is_ok() {
                    eprintln!("RESUME resumed {session} as {id} ({messages} messages)");
                }
                self.close(cx);
            }
            Err(e) => {
                self.lookup = Lookup::Failed(e);
                cx.notify();
            }
        }
    }

    fn preview(&self, theme: &Theme, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let row = |icon: Icon, line: String, color| div().flex().items_center().gap(px(space::S - 2.)).text_size(px(text::FOOTNOTE)).text_color(color).child(icon.size(px(14.)).text_color(color)).child(line);
        match &self.lookup {
            Lookup::Idle if !self.input.read(cx).value().trim().is_empty() => {
                row(Icon::new(IconName::Info), "That doesn't look like a session ID.".into(), theme.secondary).into_any_element()
            }
            Lookup::Idle => div().into_any_element(),
            Lookup::Loading => row(Icon::new(IconName::LoaderCircle), "Looking in Claude Code's history…".into(), theme.secondary).into_any_element(),
            Lookup::Failed(e) => row(Icon::new(IconName::TriangleAlert), e.clone(), theme.warning).into_any_element(),
            Lookup::Found(_, s) => {
                let ws = self.workspace.read(cx);
                let folder = s.cwd.as_deref().map(tilde).unwrap_or_else(|| ws.home_project().and_then(|h| ws.project(&h).map(|p| p.name.clone())).unwrap_or_default());
                let mut details = vec![if s.messages == 1 { "1 message".to_string() } else { format!("{} messages", s.messages) }];
                if let Some(at) = s.updated_at {
                    details.push(format!("active {}", yh_core::clock::relative(at, yh_core::library::now_ms() / 1000.0)));
                }
                if let Some(m) = &s.model {
                    details.push(m.clone());
                }
                let title = s.title.clone().or_else(|| s.first_message.clone()).unwrap_or_else(|| "Untitled session".into());
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::XS + 2.))
                    .p(px(space::M))
                    .rounded(px(radius::MEDIUM + 1.))
                    .bg(theme.card)
                    .border_1()
                    .border_color(theme.separator)
                    .child(div().font_weight(FontWeight::SEMIBOLD).line_clamp(2).child(title))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S - 2.))
                            .font_family(MONO)
                            .text_size(px(text::CODE))
                            .text_color(theme.secondary)
                            .child(Icon::new(IconName::Folder).size(px(14.)).text_color(theme.tertiary))
                            .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(folder)),
                    )
                    .child(div().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(details.join(" · ")))
                    .when_some(self.existing(cx), |d, (_, title)| d.child(row(Icon::new(IconName::CircleCheck), format!("Already open as “{title}”."), theme.secondary)))
                    .into_any_element()
            }
        }
    }

    fn destination_picker(&self, s: &Summary, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let ws = self.workspace.read(cx);
        let home = ws.home_project();
        let folder_name = s.cwd.as_deref().and_then(|c| std::path::Path::new(c).file_name().map(|n| n.to_string_lossy().into_owned()));
        let offer_new = !self.has_own_project(s.cwd.as_deref(), cx) && folder_name.is_some();
        let label = match &self.destination {
            Destination::NewProject => format!("New Project “{}”", folder_name.clone().unwrap_or_default()),
            Destination::Project(id) => ws.project(id).map(|p| p.name.clone()).unwrap_or_default(),
        };
        let projects: Vec<(String, String, bool)> = ws.projects.iter().map(|p| (p.id.clone(), p.name.clone(), Some(&p.id) == home.as_ref())).collect();
        let this = cx.entity().downgrade();
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(space::M))
            .child(div().text_color(theme.secondary).child("Add to"))
            .child(
                Button::new("resume-destination").small().outline().label(label).dropdown_caret(true).dropdown_menu(move |mut menu, _, _| {
                    if offer_new {
                        let this = this.clone();
                        menu = menu
                            .item(PopupMenuItem::new(format!("New Project “{}”", folder_name.clone().unwrap_or_default())).icon(Icon::new(IconName::FolderOpen)).on_click(move |_, _, cx| {
                                let _ = this.update(cx, |s, cx| {
                                    s.destination = Destination::NewProject;
                                    cx.notify();
                                });
                            }))
                            .separator();
                    }
                    for (id, name, is_home) in &projects {
                        let (this, id) = (this.clone(), id.clone());
                        menu = menu.item(PopupMenuItem::new(name.clone()).icon(if *is_home { icons::house() } else { Icon::new(IconName::Folder) }).on_click(move |_, _, cx| {
                            let _ = this.update(cx, |s, cx| {
                                s.destination = Destination::Project(id.clone());
                                cx.notify();
                            });
                        }));
                    }
                    menu
                }),
            )
            .into_any_element()
    }
}

impl Render for ResumeSheet {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        if !self.open {
            return div();
        }
        let existing = self.existing(cx);
        let found = match &self.lookup {
            Lookup::Found(_, s) => Some(s.clone()),
            _ => None,
        };
        let picker = found.as_ref().filter(|_| existing.is_none()).map(|s| self.destination_picker(s, &theme, cx));
        let can = existing.is_some() || found.is_some();
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
                    .id("resume-sheet")
                    .w(px(480.))
                    .flex()
                    .flex_col()
                    .gap(px(space::M + 2.))
                    .p(px(space::L + 4.))
                    .rounded(px(radius::LARGE + 2.))
                    .bg(theme.canvas)
                    .border_1()
                    .border_color(theme.separator)
                    .shadow_lg()
                    .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .capture_key_down(cx.listener(|this, e: &gpui_kit::KeyDownEvent, _, cx| {
                        if e.keystroke.key == "escape" {
                            this.close(cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(space::XS))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(space::S))
                                    .child(icons::rotate_ccw().size(px(16.)).text_color(theme.secondary))
                                    .child(div().text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).child("Resume a Claude Code Session")),
                            )
                            .child(div().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(
                                "Paste a session ID or a `claude --resume` command. The thread shows the session's history and continues where it left off.",
                            )),
                    )
                    .child(div().font_family(MONO).child(Input::new(&self.input)))
                    .child(div().min_h(px(56.)).child(self.preview(&theme, cx)))
                    .children(picker)
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(space::S))
                            .pt(px(space::XS))
                            .child(Button::new("resume-cancel").outline().label("Cancel").on_click(cx.listener(|this, _, _, cx| this.close(cx))))
                            .child(
                                Button::new("resume-go")
                                    .primary()
                                    .label(if existing.is_some() { "Show Thread" } else { "Resume" })
                                    .disabled(!can)
                                    .on_click(cx.listener(|this, _, _, cx| this.resume(cx))),
                            ),
                    ),
            )
    }
}
