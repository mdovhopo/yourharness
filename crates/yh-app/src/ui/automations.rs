//! The project page's Automations: the prompts this project runs by itself (yh-core's
//! `automations.rs`), each a card with its schedule, where its runs go, its next and last run,
//! a switch to pause it, Run Now, and a menu to edit or delete it; New Automation and Edit open
//! a sheet whose schedule is read as you type and said back. What agents make with their
//! `automations` tools shows here too: both go through the same setting and
//! `Workspace::automations_revision`.

use crate::state::workspace::Workspace;
use crate::theme::{space, text, Theme};
use crate::ui::page_icons as icons;
use crate::ui::page_kit::{self, Segment};
use crate::ui::thread_toolbar::icon_button;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable, Icon, IconName, Sizable, WindowExt};
use gpui_kit::{div, prelude::*, px, Context, Entity, FontWeight, SharedString, Subscription, WeakEntity, Window};
use yh_core::automations::{self, Automation};

fn logs() -> bool {
    std::env::var("YH_AUTOMATION_LOG").is_ok_and(|v| v == "1")
}

pub struct AutomationsSection {
    workspace: Entity<Workspace>,
    project: String,
    /// This project's, read when `automations_revision` moves.
    list: Vec<Automation>,
    revision: Option<u64>,
    /// `YH_AUTOMATION_FORM=new|edit`: the sheet opened at the first draw, for snapshots.
    probe: Option<String>,
    _observe: Subscription,
}

impl AutomationsSection {
    pub fn new(workspace: Entity<Workspace>, project: String, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&workspace, |s, _, cx| s.read(cx));
        let mut section = AutomationsSection { workspace, project, list: vec![], revision: None, probe: std::env::var("YH_AUTOMATION_FORM").ok(), _observe: observe };
        section.read(cx);
        section
    }

    fn read(&mut self, cx: &mut Context<Self>) {
        let ws = self.workspace.read(cx);
        if self.revision == Some(ws.automations_revision) {
            return;
        }
        self.revision = Some(ws.automations_revision);
        self.list = ws.automations().into_iter().filter(|a| a.project_id == self.project).collect();
        if logs() {
            for a in &self.list {
                eprintln!("AUTOMATION listed “{}” · {} · {} · next {:?} · runs {}", a.name, a.schedule.describe(automations::now()), if a.enabled { "on" } else { "off" }, a.next_run.map(|t| automations::when(t, automations::now())), a.runs.len());
            }
        }
        cx.notify();
    }

    pub fn open_form(&mut self, editing: Option<Automation>, window: &mut Window, cx: &mut Context<Self>) {
        let section = cx.entity().downgrade();
        let title = if editing.is_some() { "Edit Automation" } else { "New Automation" };
        let form = cx.new(|cx| AutomationForm::new(section, self.workspace.clone(), self.project.clone(), editing, window, cx));
        window.open_dialog(cx, move |d, _, _| d.title(title).w(px(560.)).child(form.clone()));
    }

    fn set_enabled(&mut self, id: String, on: bool, cx: &mut Context<Self>) {
        let now = automations::now();
        self.workspace.update(cx, |ws, cx| {
            ws.edit_automations(cx, |list| {
                if let Some(a) = list.iter_mut().find(|a| a.id == id) {
                    a.set_enabled(on, now);
                }
            })
        });
    }

    fn run_now(&mut self, id: String, cx: &mut Context<Self>) {
        self.workspace.update(cx, |ws, cx| {
            let _ = ws.run_automation(&id, true, cx);
        });
    }

    fn delete(&mut self, auto: Automation, window: &mut Window, cx: &mut Context<Self>) {
        let ws = self.workspace.clone();
        let id = auto.id.clone();
        window.open_alert_dialog(cx, move |a, _, _| {
            let (ws, id) = (ws.clone(), id.clone());
            a.title(format!("Delete “{}”?", auto.name))
                .description("It won't run again. The threads it started stay.")
                .ok_text("Delete")
                .ok_variant(ButtonVariant::Danger)
                .show_cancel(true)
                .on_ok(move |_, _, cx| {
                    let id = id.clone();
                    ws.update(cx, |ws, cx| ws.edit_automations(cx, |list| list.retain(|a| a.id != id)));
                    true
                })
        });
    }

    fn card(&self, a: &Automation, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let now = automations::now();
        let done = !a.enabled && a.next_run.is_none() && !a.schedule.repeats() && !a.runs.is_empty();
        let target = if a.same_thread { "Same thread each run" } else { "New thread each run" };
        let ws = self.workspace.read(cx);
        let mut status: Vec<gpui_kit::AnyElement> = vec![];
        if let Some(next) = a.next_run.filter(|_| a.enabled) {
            status.push(div().child(format!("Next {}", automations::when(next, now))).into_any_element());
        } else if done {
            status.push(div().child("Done").into_any_element());
        } else {
            status.push(div().child("Paused").into_any_element());
        }
        if let Some(run) = a.last_run() {
            let mut last = div().flex().items_center().gap(px(4.)).min_w_0().child(format!("Last {}", automations::when(run.at, now)));
            if let Some(e) = &run.error {
                last = last.child(div().text_color(theme.error).truncate().child(format!("· couldn't run: {e}")));
            } else if let Some(thread) = run.thread_id.as_ref().and_then(|t| ws.thread(t)) {
                let title = thread.read(cx).title.clone();
                let id = thread.read(cx).id().to_string();
                let w = self.workspace.clone();
                last = last.child("·").child(
                    div()
                        .id(SharedString::from(format!("auto-last-{}", a.id)))
                        .truncate()
                        .text_color(theme.accent)
                        .cursor_pointer()
                        .child(title)
                        .on_click(move |_, _, cx| w.update(cx, |ws, cx| ws.select(&id, cx))),
                );
            }
            status.push(last.into_any_element());
        }
        let (id1, id2) = (a.id.clone(), a.id.clone());
        let (edit, delete) = (a.clone(), a.clone());
        let section = cx.entity().downgrade();
        let menu = Button::new(SharedString::from(format!("auto-menu-{}", a.id))).small().ghost().icon(Icon::new(IconName::Ellipsis).size(px(14.)).text_color(theme.secondary)).dropdown_menu(move |menu, _, _| {
            let (s1, s2) = (section.clone(), section.clone());
            let (edit, delete) = (edit.clone(), delete.clone());
            menu.item(PopupMenuItem::new("Edit…").on_click(move |_, window, cx| {
                let edit = edit.clone();
                let _ = s1.update(cx, |s, cx| s.open_form(Some(edit), window, cx));
            }))
            .separator()
            .item(PopupMenuItem::new("Delete…").on_click(move |_, window, cx| {
                let delete = delete.clone();
                let _ = s2.update(cx, |s, cx| s.delete(delete, window, cx));
            }))
        });
        page_kit::card(theme)
            .px(px(space::M))
            .py(px(10.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S))
                    .child(div().flex_none().text_color(if a.enabled { theme.accent } else { theme.tertiary }).child(icons::clock().size(px(14.))))
                    .child(div().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).child(a.name.clone()))
                    .child(page_kit::quiet_chip(a.schedule.describe(now), theme))
                    .child(div().flex_1())
                    .child(
                        icon_button(SharedString::from(format!("auto-run-{}", a.id)), Icon::new(IconName::Play), "Run Now", theme)
                            .on_click(cx.listener(move |s, _, _, cx| s.run_now(id1.clone(), cx))),
                    )
                    .child(menu)
                    .child(
                        Switch::new(SharedString::from(format!("auto-on-{}", a.id)))
                            .checked(a.enabled)
                            .disabled(done)
                            .tooltip(if a.enabled { "Pause" } else { "Resume" })
                            .on_change(cx.listener(move |s, on: &bool, _, cx| s.set_enabled(id2.clone(), *on, cx))),
                    ),
            )
            .child(div().pl(px(22.)).text_size(px(text::FOOTNOTE)).text_color(theme.secondary).line_clamp(2).child(a.prompt.clone()))
            .child(
                div()
                    .pl(px(22.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(text::CAPTION))
                    .text_color(theme.tertiary)
                    .children(status.into_iter().flat_map(|s| [div().child("·").into_any_element(), s]).skip(1))
                    .child("·")
                    .child(target)
                    .when_some(a.model.clone(), |d, m| d.child("·").child(m)),
            )
            .into_any_element()
    }
}

impl Render for AutomationsSection {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(probe) = self.probe.take() {
            let editing = (probe == "edit").then(|| self.list.first().cloned()).flatten();
            cx.defer_in(window, move |s, window, cx| s.open_form(editing, window, cx));
        }
        let theme = Theme::get(cx);
        let mut column = div().flex().flex_col().gap(px(6.)).pb(px(space::XL)).child(
            div()
                .flex()
                .items_center()
                .gap(px(space::M))
                .child(div().flex_1().child(page_kit::section_title("Automations", Some(self.list.len()), &theme)))
                .child(Button::new("automation-new").small().ghost().label("New Automation").icon(IconName::Plus).on_click(cx.listener(|s, _, window, cx| s.open_form(None, window, cx)))),
        );
        if self.list.is_empty() {
            column = column.child(
                div()
                    .text_size(px(text::FOOTNOTE))
                    .text_color(theme.secondary)
                    .child("Prompts this project runs by itself, once or on a schedule, each in a thread of its own. Make one here, or ask in any thread: “every weekday at 9, look over my open PRs”."),
            );
        }
        let cards: Vec<gpui_kit::AnyElement> = self.list.clone().iter().map(|a| self.card(a, &theme, cx)).collect();
        column.children(cards)
    }
}

// MARK: New / Edit

struct AutomationForm {
    section: WeakEntity<AutomationsSection>,
    workspace: Entity<Workspace>,
    project: String,
    editing: Option<Automation>,
    name: Entity<InputState>,
    prompt: Entity<TextareaState>,
    schedule: Entity<InputState>,
    same_thread: bool,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

impl AutomationForm {
    fn new(section: WeakEntity<AutomationsSection>, workspace: Entity<Workspace>, project: String, editing: Option<Automation>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Morning PR review"));
        let prompt = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx).placeholder("What each run does, written to stand alone: it starts in a fresh thread.");
            s.set_auto_grow(4, 14, cx);
            s
        });
        let schedule = cx.new(|cx| InputState::new(window, cx).placeholder("weekdays 9:00"));
        if let Some(a) = &editing {
            name.update(cx, |s, cx| s.set_value(a.name.clone(), window, cx));
            prompt.update(cx, |s, cx| s.set_value(a.prompt.clone(), window, cx));
            schedule.update(cx, |s, cx| s.set_value(schedule_text(&a.schedule), window, cx));
        }
        let subs = vec![cx.subscribe(&schedule, |f, _, _: &InputEvent, cx| {
            f.error = None;
            cx.notify();
        })];
        let same_thread = editing.as_ref().is_some_and(|a| a.same_thread);
        AutomationForm { section, workspace, project, editing, name, prompt, schedule, same_thread, error: None, _subs: subs }
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = automations::now();
        let name = self.name.read(cx).value().trim().to_string();
        let prompt = self.prompt.read(cx).value().trim().to_string();
        let when = self.schedule.read(cx).value().trim().to_string();
        if name.is_empty() || prompt.is_empty() || when.is_empty() {
            self.error = Some("Give it a name, a prompt and a schedule.".into());
            cx.notify();
            return;
        }
        let schedule = match automations::parse(&when, now) {
            Ok(s) => s,
            Err(e) => {
                self.error = Some(e);
                cx.notify();
                return;
            }
        };
        let same_thread = self.same_thread;
        let result = match self.editing.clone() {
            Some(old) => self.workspace.update(cx, |ws, cx| {
                ws.edit_automations(cx, |list| {
                    let Some(a) = list.iter_mut().find(|a| a.id == old.id) else { return Err("It was deleted meanwhile.".to_string()) };
                    // An unchanged schedule keeps its next run (and an "every" its rhythm).
                    if schedule_text(&a.schedule) != when {
                        a.set_schedule(schedule, now)?;
                    }
                    a.name = name;
                    a.prompt = prompt;
                    if a.same_thread != same_thread {
                        a.same_thread = same_thread;
                        a.thread_id = None;
                    }
                    Ok(())
                })
            }),
            None => Automation::new(&self.project, &name, &prompt, schedule, now).map(|mut a| {
                a.same_thread = same_thread;
                self.workspace.update(cx, |ws, cx| ws.edit_automations(cx, |list| list.push(a)));
            }),
        };
        match result {
            Ok(()) => {
                window.close_dialog(cx);
                let _ = self.section.update(cx, |_, cx| cx.notify());
            }
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }
}

/// What the schedule field holds for a saved schedule: words `parse` reads back the same.
fn schedule_text(s: &automations::Schedule) -> String {
    use automations::Schedule as S;
    match s {
        S::Once { at } => {
            // SAFETY: localtime_r writes into the `tm` given and nothing else.
            let mut tm: libc::tm = unsafe { std::mem::zeroed() };
            let t = *at as libc::time_t;
            unsafe { libc::localtime_r(&t, &mut tm) };
            format!("{:04}-{:02}-{:02} {}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min)
        }
        S::Every { minutes, .. } if minutes % 60 == 0 => format!("every {}h", minutes / 60),
        S::Every { minutes, .. } => format!("every {minutes}m"),
        S::Daily { hour, minute, weekdays } => {
            let days = match weekdays.as_slice() {
                [] => "daily".to_string(),
                [1, 2, 3, 4, 5] => "weekdays".into(),
                [0, 6] => "weekends".into(),
                [one] => format!("{}s", ["sunday", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday"][*one as usize % 7]),
                many => many.iter().map(|d| ["sun", "mon", "tue", "wed", "thu", "fri", "sat"][*d as usize % 7]).collect::<Vec<_>>().join(","),
            };
            format!("{days} {hour}:{minute:02}")
        }
        S::Cron { expr } => format!("cron {expr}"),
    }
}

impl Render for AutomationForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let now = automations::now();
        let typed = self.schedule.read(cx).value().trim().to_string();
        // Said back as it's typed: what it understood and when it runs first.
        let preview = (!typed.is_empty()).then(|| match automations::parse(&typed, now) {
            Ok(s) => match s.next_after(now) {
                Some(next) => (false, format!("{} · first run {}", s.describe(now), automations::when(next, now))),
                None => (true, "That time has already passed.".to_string()),
            },
            Err(_) => (true, format!("Try {}", automations::FORMS)),
        });
        let field = |label: &str, el: gpui_kit::AnyElement| div().flex().items_start().gap(px(12.)).child(div().w(px(90.)).pt(px(5.)).flex_none().text_color(theme.secondary).child(label.to_string())).child(div().flex_1().min_w_0().child(el));
        let form = cx.entity().downgrade();
        let f2 = form.clone();
        let target = page_kit::segmented(
            vec![
                Segment::new("auto-target-new", "New thread each run", !self.same_thread, move |_, _, cx| {
                    let _ = form.update(cx, |f, cx| {
                        f.same_thread = false;
                        cx.notify();
                    });
                }),
                Segment::new("auto-target-same", "Same thread each run", self.same_thread, move |_, _, cx| {
                    let _ = f2.update(cx, |f, cx| {
                        f.same_thread = true;
                        cx.notify();
                    });
                }),
            ],
            &theme,
        );
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .text_size(px(text::BODY))
            .child(field("Name", Input::new(&self.name).into_any_element()))
            .child(field("Prompt", page_kit::card(&theme).px(px(space::S)).py(px(6.)).child(Textarea::new(&self.prompt).appearance(false)).into_any_element()))
            .child(field(
                "When",
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(Input::new(&self.schedule))
                    .when_some(preview, |d, (bad, words)| d.child(div().text_size(px(text::CAPTION)).text_color(if bad { theme.tertiary } else { theme.secondary }).child(words)))
                    .child(div().text_size(px(text::CAPTION)).text_color(theme.tertiary).child(format!("Local time ({}). Runs while YourHarness is open; one missed while it was closed runs once when it's back.", automations::zone())))
                    .into_any_element(),
            ))
            .child(field("Runs go to", div().flex().child(target).into_any_element()))
            .when_some(self.error.clone(), |d, e| d.child(div().text_color(theme.error).child(e)))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(Button::new("auto-cancel").outline().label("Cancel").on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(Button::new("auto-save").primary().label(if self.editing.is_some() { "Save" } else { "Create" }).disabled(typed.is_empty()).on_click(cx.listener(|f, _, window, cx| f.save(window, cx)))),
            )
    }
}
