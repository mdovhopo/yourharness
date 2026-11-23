//! Automations v2 UI:
//! - `AutomationsPage` (`Page::Automations`, ⇧⌘A): dedicated top-level section showing Global
//!   and per-workspace automations in a responsive master-detail view, with scope filtering,
//!   search, Pause All kill switch, starter templates, configuration summary, and full run
//!   history.
//! - `AutomationForm`: unified Create / Edit sheet supporting Global vs Workspace scope,
//!   Schedule / Mac Event / File & Repo / Manual triggers, Thread Policy (New, New & settle
//!   previous, Same), Agent / Model / Effort / Permissions, and collapsible Guardrails.
//! - `AutomationsSection`: compact per-project summary embedded on `ProjectPage`, linking into
//!   the full Automations page.

use crate::state::workspace::Workspace;
use crate::theme::{radius, space, text, Theme, MONO};
use crate::ui::page_icons as icons;
use crate::ui::page_kit::{self, Segment};
use crate::ui::thread_toolbar::icon_button;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable, Icon, IconName, Sizable, WindowExt};
use gpui_kit::{
    actions, div, prelude::*, px, App, ClipboardItem, Context, Entity, FocusHandle, Focusable, FontWeight, Hsla, ScrollHandle, SharedString,
    Subscription, Window,
};
use yh_core::automations::{
    self, ActiveHours, Automation, FileWatchTrigger, Guardrails, NotifyPolicy, OverlapPolicy, RepoEventKind, RepoTrigger, RunStatus, Schedule,
    Scope, SystemEventKind, SystemTrigger, Template, ThreadPolicy, Trigger, TriggerCategory,
};

actions!(automations, [CloseAutomations]);

fn logs() -> bool {
    std::env::var("YH_AUTOMATION_LOG").is_ok_and(|v| v == "1")
}

#[derive(Clone, PartialEq, Eq)]
enum ScopeFilter {
    All,
    Global,
    Project(String),
}

/// Dedicated top-level Automations page (`Page::Automations`, ⇧⌘A).
pub struct AutomationsPage {
    workspace: Entity<Workspace>,
    list: Vec<Automation>,
    revision: Option<u64>,
    filter: ScopeFilter,
    search: Entity<InputState>,
    selected: Option<String>,
    /// On narrow windows (`< NARROW_BELOW`), whether the list is shown instead of detail.
    narrow_list: bool,
    width: page_kit::PageWidth,
    list_scroll: ScrollHandle,
    detail_scroll: ScrollHandle,
    pub focus: FocusHandle,
    /// `YH_AUTOMATION_FORM=new|edit`: opens the form on first render, for snapshots.
    probe: Option<String>,
    _subs: Vec<Subscription>,
}

impl Focusable for AutomationsPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl AutomationsPage {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Filter automations…"));
        let subs = vec![
            cx.observe(&workspace, |this, _, cx| this.sync(cx)),
            cx.subscribe(&search, |_, _, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        let mut page = AutomationsPage {
            workspace,
            list: vec![],
            revision: None,
            filter: ScopeFilter::All,
            search,
            selected: None,
            narrow_list: true,
            width: Default::default(),
            list_scroll: ScrollHandle::new(),
            detail_scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            probe: std::env::var("YH_AUTOMATION_FORM").ok(),
            _subs: subs,
        };
        page.apply_workspace_filter(cx);
        page.sync(cx);
        page
    }

    pub fn apply_workspace_filter(&mut self, cx: &mut Context<Self>) {
        if let Some(f) = self.workspace.update(cx, |ws, _| ws.automations_filter.take()) {
            self.filter = if f == "global" { ScopeFilter::Global } else { ScopeFilter::Project(f) };
            self.selected = None;
            cx.notify();
        }
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        self.apply_workspace_filter(cx);
        let ws = self.workspace.read(cx);
        if self.revision == Some(ws.automations_revision) {
            return;
        }
        self.revision = Some(ws.automations_revision);
        self.list = ws.automations();
        if let Some(sel) = &self.selected {
            if !self.list.iter().any(|a| &a.id == sel) {
                self.selected = None;
            }
        }
        cx.notify();
    }

    fn filtered(&self, cx: &App) -> Vec<&Automation> {
        let q = self.search.read(cx).value().trim().to_lowercase();
        let ws = self.workspace.read(cx);
        self.list
            .iter()
            .filter(|a| match &self.filter {
                ScopeFilter::All => true,
                ScopeFilter::Global => a.scope().is_global(),
                ScopeFilter::Project(pid) => !a.scope().is_global() && a.project_id == *pid,
            })
            .filter(|a| {
                if q.is_empty() {
                    return true;
                }
                let proj_name = a
                    .scope()
                    .project_id()
                    .and_then(|id| ws.project(id))
                    .map(|p| p.name.as_str())
                    .unwrap_or("Global");
                a.name.to_lowercase().contains(&q)
                    || a.prompt.to_lowercase().contains(&q)
                    || proj_name.to_lowercase().contains(&q)
                    || a.effective_trigger().describe(automations::now()).to_lowercase().contains(&q)
            })
            .collect()
    }

    fn scope_name(&self, a: &Automation, cx: &App) -> String {
        match a.scope() {
            Scope::Global => "Global".into(),
            Scope::Project { project_id } => {
                let ws = self.workspace.read(cx);
                if ws.home_project().as_deref() == Some(project_id.as_str()) {
                    "Global".into()
                } else {
                    ws.project(&project_id).map(|p| p.name.clone()).unwrap_or_else(|| "Deleted project".into())
                }
            }
        }
    }

    fn default_scope_for_new(&self, cx: &App) -> Scope {
        match &self.filter {
            ScopeFilter::Global => Scope::Global,
            ScopeFilter::Project(pid) => Scope::Project { project_id: pid.clone() },
            ScopeFilter::All => {
                let ws = self.workspace.read(cx);
                ws.selected_thread()
                    .map(|t| t.read(cx).meta.project_id.clone())
                    .filter(|pid| ws.home_project().as_deref() != Some(pid.as_str()))
                    .map(|project_id| Scope::Project { project_id })
                    .unwrap_or(Scope::Global)
            }
        }
    }

    fn open_form(&mut self, editing: Option<Automation>, template: Option<Template>, window: &mut Window, cx: &mut Context<Self>) {
        let default_scope = self.default_scope_for_new(cx);
        let page = cx.entity().downgrade();
        let title = if editing.is_some() {
            "Edit Automation"
        } else if let Some(t) = &template {
            t.name
        } else {
            "New Automation"
        };
        let ws = self.workspace.clone();
        let form = cx.new(|cx| {
            AutomationForm::new(
                ws,
                default_scope,
                editing,
                template,
                Some(Box::new(move |created_id, cx| {
                    let _ = page.update(cx, |p, cx| {
                        if let Some(id) = created_id {
                            p.selected = Some(id);
                            p.narrow_list = false;
                        }
                        cx.notify();
                    });
                })),
                window,
                cx,
            )
        });
        window.open_dialog(cx, move |d, _, _| d.title(title).w(px(600.)).child(form.clone()));
    }

    fn duplicate(&mut self, auto: &Automation, cx: &mut Context<Self>) {
        let now = automations::now();
        let mut copy = auto.clone();
        copy.id = yh_core::library::new_uuid();
        copy.name = format!("{} (Copy)", copy.name);
        copy.created_at = now;
        copy.runs.clear();
        copy.thread_id = None;
        copy.last_thread_id = None;
        if let Some(s) = copy.effective_trigger().schedule() {
            copy.next_run = s.next_after(now);
        }
        let new_id = copy.id.clone();
        self.workspace.update(cx, |ws, cx| ws.edit_automations(cx, |list| list.push(copy)));
        self.selected = Some(new_id);
        cx.notify();
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

    fn status_color(&self, a: &Automation, paused_all: bool, theme: &Theme, cx: &App) -> Hsla {
        if paused_all || !a.enabled {
            return theme.tertiary;
        }
        let ws = self.workspace.read(cx);
        let running = a.last_thread_id.as_deref().and_then(|id| ws.thread(id)).is_some_and(|t| t.read(cx).run.running);
        let waiting = a.last_thread_id.as_deref().and_then(|id| ws.thread(id)).is_some_and(|t| t.read(cx).waiting_for_you());
        if waiting {
            return theme.warning;
        }
        if running {
            return theme.accent;
        }
        if a.last_run().is_some_and(|r| r.effective_status() == RunStatus::Error) {
            return theme.error;
        }
        theme.success
    }

    fn list_row(&self, a: &Automation, is_selected: bool, paused_all: bool, now: f64, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let dot_color = self.status_color(a, paused_all, theme, cx);
        let trigger_label = a.effective_trigger().describe(now);
        let sub = if paused_all {
            "Paused globally".to_string()
        } else if !a.enabled {
            if a.is_done() { "Done".into() } else { "Paused".into() }
        } else if let Some(next) = a.next_run {
            format!("Next {}", automations::when(next, now))
        } else if let Some(last) = a.last_run() {
            format!("Last {}", automations::when(last.at, now))
        } else {
            "Armed".into()
        };
        let id = a.id.clone();
        div()
            .id(SharedString::from(format!("auto-row-{}", a.id)))
            .px(px(10.))
            .py(px(8.))
            .rounded(px(radius::MEDIUM))
            .cursor_pointer()
            .when(is_selected, |d| d.bg(theme.selected))
            .when(!is_selected, |d| d.hover(|s| s.bg(theme.hover)))
            .flex()
            .flex_col()
            .gap(px(3.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .child(page_kit::dot(dot_color, 7.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(text::BODY))
                            .font_weight(if is_selected { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                            .text_color(if a.enabled && !paused_all { theme.text } else { theme.secondary })
                            .child(a.name.clone()),
                    ),
            )
            .child(
                div()
                    .pl(px(14.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(text::CAPTION))
                    .text_color(theme.tertiary)
                    .child(div().truncate().text_color(theme.secondary).child(trigger_label))
                    .child("·")
                    .child(div().flex_none().child(sub)),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected = Some(id.clone());
                this.narrow_list = false;
                cx.notify();
            }))
            .into_any_element()
    }

    fn render_list(&self, filtered: &[&Automation], selected_id: Option<&str>, paused_all: bool, now: f64, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        if filtered.is_empty() {
            return div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .p(px(space::XL))
                .gap(px(space::S))
                .text_center()
                .child(icons::zap().size(px(24.)).text_color(theme.tertiary))
                .child(div().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child("No matching automations."))
                .into_any_element();
        }

        // Group Global first, then each workspace in workspace order.
        let mut groups: Vec<(String, Vec<&Automation>)> = vec![];
        let globals: Vec<&Automation> = filtered.iter().copied().filter(|a| a.scope().is_global()).collect();
        if !globals.is_empty() {
            groups.push(("Global".into(), globals));
        }
        let ws = self.workspace.read(cx);
        let mut seen_proj: Vec<String> = vec![];
        for p in &ws.projects {
            if ws.home_project().as_deref() == Some(p.id.as_str()) {
                continue;
            }
            let items: Vec<&Automation> = filtered.iter().copied().filter(|a| !a.scope().is_global() && a.project_id == p.id).collect();
            if !items.is_empty() {
                seen_proj.push(p.id.clone());
                groups.push((p.name.clone(), items));
            }
        }
        let orphans: Vec<&Automation> = filtered
            .iter()
            .copied()
            .filter(|a| !a.scope().is_global() && !seen_proj.contains(&a.project_id))
            .collect();
        if !orphans.is_empty() {
            groups.push(("Other".into(), orphans));
        }

        let mut col = div()
            .id("automations-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.list_scroll)
            .p(px(space::S))
            .flex()
            .flex_col()
            .gap(px(2.));
        let show_headers = matches!(self.filter, ScopeFilter::All) && groups.len() > 1;
        for (gi, (label, items)) in groups.into_iter().enumerate() {
            if show_headers {
                col = col.child(
                    div()
                        .pt(px(if gi == 0 { 2. } else { 10. }))
                        .pb(px(3.))
                        .px(px(10.))
                        .text_size(px(text::CAPTION))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.tertiary)
                        .child(label.to_uppercase()),
                );
            }
            for a in items {
                col = col.child(self.list_row(a, selected_id == Some(a.id.as_str()), paused_all, now, theme, cx));
            }
        }
        col.into_any_element()
    }

    fn render_detail(&self, a: &Automation, paused_all: bool, narrow: bool, now: f64, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let scope_label = self.scope_name(a, cx);
        let trig = a.effective_trigger();
        let trigger_desc = trig.describe(now);
        let done = a.is_done();
        let policy = a.thread_policy();
        let slug = a.slug();
        let cli_cmd = format!("yh trigger {slug}");
        let ws = self.workspace.read(cx);

        let (id_run, id_sw, id_reset) = (a.id.clone(), a.id.clone(), a.id.clone());
        let (edit_a, dup_a, del_a) = (a.clone(), a.clone(), a.clone());
        let cli_copy = cli_cmd.clone();
        let has_bound_thread = a.same_thread && a.thread_id.is_some();
        let page_weak = cx.entity().downgrade();
        let ws_reset = self.workspace.clone();
        let menu = Button::new(SharedString::from(format!("auto-detail-menu-{}", a.id)))
            .small()
            .ghost()
            .icon(Icon::new(IconName::Ellipsis).size(px(14.)).text_color(theme.secondary))
            .dropdown_menu(move |mut menu, _, _| {
                let (p1, p2, p3) = (page_weak.clone(), page_weak.clone(), page_weak.clone());
                let (dup_a, del_a, cli_copy) = (dup_a.clone(), del_a.clone(), cli_copy.clone());
                menu = menu
                    .item(PopupMenuItem::new("Duplicate").on_click(move |_, _, cx| {
                        let dup = dup_a.clone();
                        let _ = p1.update(cx, |p, cx| p.duplicate(&dup, cx));
                    }))
                    .item(PopupMenuItem::new("Copy CLI Trigger Command").on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(cli_copy.clone()));
                    }));
                if has_bound_thread {
                    let (ws_r, id_r) = (ws_reset.clone(), id_reset.clone());
                    menu = menu.item(PopupMenuItem::new("Reset Bound Thread").on_click(move |_, _, cx| {
                        let id_r = id_r.clone();
                        ws_r.update(cx, |ws, cx| {
                            ws.edit_automations(cx, |list| {
                                if let Some(x) = list.iter_mut().find(|x| x.id == id_r) {
                                    x.thread_id = None;
                                }
                            })
                        });
                    }));
                }
                let _ = p2;
                menu.separator().item(PopupMenuItem::new("Delete…").on_click(move |_, window, cx| {
                    let del = del_a.clone();
                    let _ = p3.update(cx, |p, cx| p.delete(del, window, cx));
                }))
            });

        // Config rows
        let row = |label: &str, value: gpui_kit::AnyElement| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(space::M))
                .px(px(space::M))
                .py(px(8.))
                .border_b_1()
                .border_color(theme.separator)
                .child(div().flex_none().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(label.to_string()))
                .child(div().min_w_0().truncate().text_size(px(text::FOOTNOTE)).text_color(theme.text).child(value))
        };

        let trigger_detail = match &trig {
            Trigger::Schedule { .. } => {
                if let Some(next) = a.next_run.filter(|_| a.enabled && !paused_all) {
                    format!("{trigger_desc} · next {}", automations::when(next, now))
                } else {
                    trigger_desc.clone()
                }
            }
            Trigger::Manual => format!("{trigger_desc} · `{cli_cmd}`"),
            _ => format!("{trigger_desc} · active while open"),
        };

        let thread_cell: gpui_kit::AnyElement = if policy == ThreadPolicy::Same {
            if let Some(tid) = &a.thread_id {
                if let Some(th) = ws.thread(tid) {
                    let title = th.read(cx).title.clone();
                    let tid = tid.clone();
                    let ws_c = self.workspace.clone();
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(policy.label())
                        .child("·")
                        .child(
                            div()
                                .id("auto-bound-thread")
                                .text_color(theme.accent)
                                .cursor_pointer()
                                .truncate()
                                .child(title)
                                .on_click(move |_, _, cx| ws_c.update(cx, |ws, cx| ws.select(&tid, cx))),
                        )
                        .into_any_element()
                } else {
                    div().child(policy.label()).into_any_element()
                }
            } else {
                div().child(format!("{} (made on first run)", policy.label())).into_any_element()
            }
        } else {
            div().child(policy.label()).into_any_element()
        };

        let mut agent_parts: Vec<String> = vec![];
        agent_parts.push(match a.agent.as_deref() {
            Some("antigravity") => "Antigravity".into(),
            Some("claude") => "Claude".into(),
            _ => "Default agent".into(),
        });
        if let Some(m) = &a.model {
            agent_parts.push(m.clone());
        }
        if let Some(e) = &a.effort {
            agent_parts.push(format!("effort: {e}"));
        }
        if let Some(m) = &a.mode {
            agent_parts.push(format!("mode: {m}"));
        }

        let mut guard_parts: Vec<String> = vec![];
        if a.guardrails.cooldown_secs > 0 {
            guard_parts.push(format!("Cooldown {}", automations::format_duration(a.guardrails.cooldown_secs)));
        }
        if matches!(trig, Trigger::FileWatch(_)) && a.guardrails.debounce_secs > 0 {
            guard_parts.push(format!("Debounce {}s", a.guardrails.debounce_secs));
        }
        if let Some(h) = &a.guardrails.active_hours {
            guard_parts.push(h.describe());
        }
        if a.guardrails.overlap != OverlapPolicy::Skip {
            guard_parts.push(a.guardrails.overlap.label().into());
        }
        if a.guardrails.ac_power_only {
            guard_parts.push("AC power only".into());
        }
        if a.guardrails.condition_cmd.as_ref().is_some_and(|c| !c.trim().is_empty()) {
            guard_parts.push("Pre-check command".into());
        }
        if a.notify != NotifyPolicy::Always {
            guard_parts.push(format!("Notify: {}", a.notify.label()));
        }

        let mut cfg_card = page_kit::card(theme)
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(row(
                "Scope",
                div()
                    .child(match a.scope() {
                        Scope::Global => match a.working_dir.as_deref().filter(|s| !s.is_empty()) {
                            Some(wd) => format!("Global (Home) · {wd}"),
                            None => "Global (runs in Home)".into(),
                        },
                        Scope::Project { .. } => scope_label.clone(),
                    })
                    .into_any_element(),
            ))
            .child(row("Trigger", div().child(trigger_detail).into_any_element()))
            .child(row("Threads", thread_cell))
            .child(row("Agent & Mode", div().child(agent_parts.join(" · ")).into_any_element()));

        if !guard_parts.is_empty() {
            cfg_card = cfg_card.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(space::M))
                    .px(px(space::M))
                    .py(px(8.))
                    .child(div().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child("Guardrails"))
                    .child(div().text_size(px(text::FOOTNOTE)).text_color(theme.text).child(guard_parts.join(" · "))),
            );
        }

        // Run history list
        let runs_card: gpui_kit::AnyElement = if a.runs.is_empty() {
            div()
                .text_size(px(text::FOOTNOTE))
                .text_color(theme.tertiary)
                .child("No runs yet. Click Run Now to test it immediately.")
                .into_any_element()
        } else {
            let mut list = page_kit::card(theme).overflow_hidden().flex().flex_col();
            for (idx, run) in a.runs.iter().rev().enumerate() {
                let st = run.effective_status();
                let st_color = match st {
                    RunStatus::Done => theme.success,
                    RunStatus::Running => theme.accent,
                    RunStatus::Waiting => theme.warning,
                    RunStatus::Skipped => theme.tertiary,
                    RunStatus::Error => theme.error,
                };
                let reason = run.trigger_reason.clone().unwrap_or_else(|| {
                    if run.manual { "Run manually".into() } else if run.caught_up { "Scheduled (caught up)".into() } else { "Scheduled".into() }
                });
                let dur = run.duration_secs.map(|d| if d >= 60.0 { format!("{}m {}s", (d / 60.0) as u64, (d % 60.0) as u64) } else { format!("{:.0}s", d) });
                let thread_el: Option<gpui_kit::AnyElement> = run.thread_id.as_ref().and_then(|tid| {
                    let th = ws.thread(tid)?;
                    let title = th.read(cx).title.clone();
                    let tid = tid.clone();
                    let ws_c = self.workspace.clone();
                    Some(
                        div()
                            .id(SharedString::from(format!("run-thread-{idx}-{tid}")))
                            .text_size(px(text::CAPTION))
                            .text_color(theme.accent)
                            .cursor_pointer()
                            .truncate()
                            .child(format!("{title} →"))
                            .on_click(move |_, _, cx| ws_c.update(cx, |ws, cx| ws.select(&tid, cx)))
                            .into_any_element(),
                    )
                });
                let mut r = div()
                    .flex()
                    .items_center()
                    .gap(px(space::S))
                    .px(px(space::M))
                    .py(px(7.))
                    .when(idx > 0, |d| d.border_t_1().border_color(theme.separator))
                    .child(page_kit::dot(st_color, 6.))
                    .child(div().w(px(124.)).flex_none().text_size(px(text::CAPTION)).text_color(theme.secondary).child(automations::when(run.at, now)))
                    .child(div().text_size(px(text::CAPTION)).text_color(theme.text).truncate().child(reason))
                    .when(run.settled_previous.is_some(), |d| d.child(page_kit::quiet_chip("settled prev", theme)))
                    .child(div().flex_1());
                if let Some(err) = &run.error {
                    r = r.child(
                        div()
                            .text_size(px(text::CAPTION))
                            .text_color(if st == RunStatus::Skipped { theme.tertiary } else { theme.error })
                            .truncate()
                            .child(err.clone()),
                    );
                } else {
                    r = r.when_some(dur, |d, s| d.child(div().flex_none().text_size(px(text::CAPTION)).text_color(theme.tertiary).child(s))).children(thread_el);
                }
                list = list.child(r);
            }
            list.into_any_element()
        };

        let cli_copy_btn = cli_cmd.clone();
        div()
            .id("automations-detail")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.detail_scroll)
            .px(px(space::L + 4.))
            .py(px(space::L))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .max_w(px(720.))
                    .flex()
                    .flex_col()
                    .gap(px(space::L))
                    .when(narrow, |d| {
                        d.child(
                            Button::new("auto-back")
                                .small()
                                .ghost()
                                .icon(icons::chevron_left())
                                .label("All Automations")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.narrow_list = true;
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(space::M))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .child(div().text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).text_color(theme.text).truncate().child(a.name.clone()))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .flex_wrap()
                                            .gap(px(6.))
                                            .child(if a.scope().is_global() {
                                                page_kit::chip("Global", theme.accent)
                                            } else {
                                                page_kit::quiet_chip(scope_label, theme)
                                            })
                                            .child(page_kit::quiet_chip(trigger_desc, theme))
                                            .child(page_kit::quiet_chip(policy.short_label(), theme)),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(
                                        Button::new(SharedString::from(format!("detail-run-{}", a.id)))
                                            .small()
                                            .outline()
                                            .icon(IconName::Play)
                                            .label("Run Now")
                                            .on_click(cx.listener(move |this, _, _, cx| this.run_now(id_run.clone(), cx))),
                                    )
                                    .child(
                                        Button::new(SharedString::from(format!("detail-edit-{}", a.id)))
                                            .small()
                                            .outline()
                                            .label("Edit…")
                                            .on_click(cx.listener(move |this, _, window, cx| this.open_form(Some(edit_a.clone()), None, window, cx))),
                                    )
                                    .child(menu)
                                    .child(
                                        Switch::new(SharedString::from(format!("detail-on-{}", a.id)))
                                            .checked(a.enabled)
                                            .disabled(done)
                                            .tooltip(if a.enabled { "Pause" } else { "Resume" })
                                            .on_change(cx.listener(move |this, on: &bool, _, cx| this.set_enabled(id_sw.clone(), *on, cx))),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(page_kit::section_title("Configuration", None, theme))
                            .child(cfg_card),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(page_kit::section_title("Prompt", None, theme))
                            .child(
                                page_kit::card(theme)
                                    .p(px(space::M))
                                    .text_size(px(text::BODY))
                                    .text_color(theme.text)
                                    .child(a.prompt.clone()),
                            ),
                    )
                    .when(matches!(trig, Trigger::Manual), |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(space::S))
                                .px(px(space::M))
                                .py(px(8.))
                                .rounded(px(radius::MEDIUM))
                                .bg(theme.selected)
                                .child(div().flex_1().font_family(MONO).text_size(px(text::CODE)).text_color(theme.secondary).child(cli_cmd))
                                .child(
                                    Button::new("copy-cli-cmd")
                                        .xsmall()
                                        .ghost()
                                        .label("Copy")
                                        .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(cli_copy_btn.clone()))),
                                ),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(page_kit::section_title("Run History", Some(a.runs.len()), theme))
                            .child(runs_card),
                    ),
            )
            .into_any_element()
    }
}

impl Render for AutomationsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(probe) = self.probe.take() {
            let editing = (probe == "edit").then(|| self.list.first().cloned()).flatten();
            cx.defer_in(window, move |p, window, cx| p.open_form(editing, None, window, cx));
        }
        let theme = Theme::get(cx);
        let now = automations::now();
        let compact = self.width.compact(window);
        let narrow = self.width.narrow(window);
        let ws = self.workspace.read(cx);
        let paused_all = ws.automations_paused();
        let active_n = self.list.iter().filter(|a| a.enabled).count();
        let events_n = self.list.iter().filter(|a| a.enabled && a.effective_trigger().is_event()).count();
        let next_due = self.list.iter().filter(|a| a.enabled).filter_map(|a| a.next_run).fold(None, |m: Option<f64>, t| Some(m.map_or(t, |m| m.min(t))));
        let subtitle: gpui_kit::AnyElement = if paused_all {
            div().text_color(theme.warning).child("All automations are paused — Resume All to re-arm schedules and Mac events").into_any_element()
        } else {
            let mut parts = vec![format!("{active_n} active")];
            if events_n > 0 {
                parts.push(format!("{events_n} event trigger{}", if events_n == 1 { "" } else { "s" }));
            }
            if let Some(next) = next_due {
                parts.push(format!("next run {}", automations::when(next, now)));
            }
            div().child(parts.join(" · ")).into_any_element()
        };

        let page_weak = cx.entity().downgrade();
        let templates_btn = Button::new("auto-templates")
            .small()
            .ghost()
            .icon(icons::sparkles())
            .when(!compact, |b| b.label("Templates"))
            .tooltip("Create from a starter template")
            .dropdown_menu(move |mut menu, _, _| {
                for tpl in automations::templates() {
                    let p = page_weak.clone();
                    let label = format!("{} — {}", tpl.name, tpl.summary);
                    menu = menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
                        let t = tpl.clone();
                        let _ = p.update(cx, |page, cx| page.open_form(None, Some(t), window, cx));
                    }));
                }
                menu
            });

        let ws_pause = self.workspace.clone();
        let pause_btn = Button::new("auto-pause-all")
            .small()
            .ghost()
            .icon(if paused_all { Icon::new(IconName::Play) } else { icons::circle_pause() })
            .when(!compact, |b| b.label(if paused_all { "Resume All" } else { "Pause All" }))
            .tooltip(if paused_all { "Resume all scheduled and event automations" } else { "Pause all scheduled and event automations" })
            .on_click(move |_, _, cx| ws_pause.update(cx, |ws, cx| ws.set_automations_paused(!paused_all, cx)));

        let new_btn = Button::new("auto-new-primary")
            .small()
            .primary()
            .icon(IconName::Plus)
            .when(!compact, |b| b.label("New Automation"))
            .tooltip("New Automation")
            .on_click(cx.listener(|this, _, window, cx| this.open_form(None, None, window, cx)));

        let toolbar = div().flex().items_center().gap(px(space::XS)).child(pause_btn).child(templates_btn).child(new_btn).into_any_element();

        // Scope filter tabs + project dropdown + search
        let (p1, p2, p3) = (cx.entity().downgrade(), cx.entity().downgrade(), cx.entity().downgrade());
        let projects: Vec<(String, String)> = ws
            .projects
            .iter()
            .filter(|p| ws.home_project().as_deref() != Some(p.id.as_str()))
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect();
        let active_proj_label = match &self.filter {
            ScopeFilter::Project(pid) => projects.iter().find(|(id, _)| id == pid).map(|(_, n)| n.clone()).unwrap_or_else(|| "Workspace".into()),
            _ => "Workspace".into(),
        };
        let is_proj_filter = matches!(self.filter, ScopeFilter::Project(_));
        let scope_tabs = page_kit::segmented(
            vec![
                Segment::new("scope-all", "All", self.filter == ScopeFilter::All, move |_, _, cx| {
                    let _ = p1.update(cx, |p, cx| {
                        p.filter = ScopeFilter::All;
                        cx.notify();
                    });
                }),
                Segment::new("scope-global", "Global", self.filter == ScopeFilter::Global, move |_, _, cx| {
                    let _ = p2.update(cx, |p, cx| {
                        p.filter = ScopeFilter::Global;
                        cx.notify();
                    });
                }),
            ],
            &theme,
        );
        let proj_picker = (!projects.is_empty()).then(|| {
            Button::new("scope-project-picker")
                .small()
                .when(is_proj_filter, |b| b.outline())
                .when(!is_proj_filter, |b| b.ghost())
                .icon(IconName::Folder)
                .label(active_proj_label)
                .dropdown_menu(move |mut menu, _, _| {
                    for (id, name) in projects.clone() {
                        let p = p3.clone();
                        menu = menu.item(PopupMenuItem::new(name).on_click(move |_, _, cx| {
                            let id = id.clone();
                            let _ = p.update(cx, |page, cx| {
                                page.filter = ScopeFilter::Project(id);
                                page.selected = None;
                                cx.notify();
                            });
                        }));
                    }
                    menu
                })
        });
        let filter_row = div()
            .w_full()
            .flex()
            .items_center()
            .gap(px(space::S))
            .child(scope_tabs)
            .children(proj_picker)
            .child(div().flex_1())
            .child(div().w(px(210.)).child(Input::new(&self.search).small()))
            .into_any_element();

        let filtered: Vec<Automation> = self.filtered(cx).into_iter().cloned().collect();
        let filtered_refs: Vec<&Automation> = filtered.iter().collect();
        let selected_auto = self
            .selected
            .as_deref()
            .and_then(|id| filtered.iter().find(|a| a.id == id))
            .or_else(|| filtered.first())
            .cloned();
        let selected_id = selected_auto.as_ref().map(|a| a.id.as_str());

        let body: gpui_kit::AnyElement = if self.list.is_empty() {
            // Welcoming empty state with starter templates.
            let mut tpl_row = div().flex().flex_wrap().justify_center().gap(px(8.)).max_w(px(640.));
            for tpl in automations::templates() {
                let t = tpl.clone();
                tpl_row = tpl_row.child(
                    Button::new(SharedString::from(format!("empty-tpl-{}", tpl.id)))
                        .small()
                        .outline()
                        .icon(icons::sparkles())
                        .label(tpl.name)
                        .tooltip(tpl.summary)
                        .on_click(cx.listener(move |this, _, window, cx| this.open_form(None, Some(t.clone()), window, cx))),
                );
            }
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(space::M))
                .p(px(space::XL))
                .child(page_kit::icon_tile(icons::zap(), 44., &theme))
                .child(div().text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).child("Automate Your Workflows"))
                .child(
                    div()
                        .max_w(px(440.))
                        .text_center()
                        .text_size(px(text::BODY))
                        .text_color(theme.secondary)
                        .child("Run prompts automatically on a schedule, when your Mac wakes or unlocks, when files change, on git/PR events, or from the CLI (yh trigger) — per workspace or globally."),
                )
                .child(
                    Button::new("empty-create-auto")
                        .primary()
                        .icon(IconName::Plus)
                        .label("Create Automation")
                        .on_click(cx.listener(|this, _, window, cx| this.open_form(None, None, window, cx))),
                )
                .child(div().pt(px(space::S)).text_size(px(text::CAPTION)).text_color(theme.tertiary).child("Or start from a template:"))
                .child(tpl_row)
                .into_any_element()
        } else if narrow {
            if self.narrow_list || selected_auto.is_none() {
                self.render_list(&filtered_refs, selected_id, paused_all, now, &theme, cx)
            } else if let Some(a) = &selected_auto {
                self.render_detail(a, paused_all, true, now, &theme, cx)
            } else {
                div().into_any_element()
            }
        } else {
            let list_pane = div()
                .w(px(276.))
                .flex_none()
                .h_full()
                .border_r_1()
                .border_color(theme.separator)
                .flex()
                .flex_col()
                .child(self.render_list(&filtered_refs, selected_id, paused_all, now, &theme, cx));
            let detail_pane = match &selected_auto {
                Some(a) => self.render_detail(a, paused_all, false, now, &theme, cx),
                None => div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.tertiary)
                    .child("Select an automation")
                    .into_any_element(),
            };
            div().flex_1().min_w_0().min_h_0().flex().child(list_pane).child(detail_pane).into_any_element()
        };

        div()
            .id("automations-page")
            .key_context("AutomationsPage")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &CloseAutomations, _, cx| {
                this.workspace.update(cx, |ws, cx| {
                    if ws.page == Some(crate::state::workspace::Page::Automations) {
                        ws.toggle_automations(cx);
                    }
                });
            }))
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(theme.canvas)
            .child(page_kit::page_header(icons::zap(), "Automations", subtitle, toolbar, Some(filter_row), &theme))
            .child(body)
            .child(self.width.probe(cx.entity().downgrade()))
    }
}

// MARK: Compact ProjectPage section

pub struct AutomationsSection {
    workspace: Entity<Workspace>,
    project: String,
    list: Vec<Automation>,
    revision: Option<u64>,
    probe: Option<String>,
    _observe: Subscription,
}

impl AutomationsSection {
    pub fn new(workspace: Entity<Workspace>, project: String, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&workspace, |s, _, cx| s.read(cx));
        let mut section = AutomationsSection {
            workspace,
            project,
            list: vec![],
            revision: None,
            probe: std::env::var("YH_AUTOMATION_FORM").ok(),
            _observe: observe,
        };
        section.read(cx);
        section
    }

    fn read(&mut self, cx: &mut Context<Self>) {
        let ws = self.workspace.read(cx);
        if self.revision == Some(ws.automations_revision) {
            return;
        }
        self.revision = Some(ws.automations_revision);
        self.list = ws.automations().into_iter().filter(|a| !a.is_global && a.project_id == self.project).collect();
        if logs() {
            for a in &self.list {
                eprintln!(
                    "AUTOMATION listed “{}” · {} · {} · next {:?} · runs {}",
                    a.name,
                    a.effective_trigger().describe(automations::now()),
                    if a.enabled { "on" } else { "off" },
                    a.next_run.map(|t| automations::when(t, automations::now())),
                    a.runs.len()
                );
            }
        }
        cx.notify();
    }

    pub fn open_form(&mut self, editing: Option<Automation>, window: &mut Window, cx: &mut Context<Self>) {
        let section = cx.entity().downgrade();
        let title = if editing.is_some() { "Edit Automation" } else { "New Automation" };
        let scope = Scope::Project { project_id: self.project.clone() };
        let form = cx.new(|cx| {
            AutomationForm::new(
                self.workspace.clone(),
                scope,
                editing,
                None,
                Some(Box::new(move |_, cx| {
                    let _ = section.update(cx, |_, cx| cx.notify());
                })),
                window,
                cx,
            )
        });
        window.open_dialog(cx, move |d, _, _| d.title(title).w(px(600.)).child(form.clone()));
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
        let done = a.is_done();
        let target = a.thread_policy().label();
        let ws = self.workspace.read(cx);
        let mut status: Vec<gpui_kit::AnyElement> = vec![];
        if let Some(next) = a.next_run.filter(|_| a.enabled) {
            status.push(div().child(format!("Next {}", automations::when(next, now))).into_any_element());
        } else if done {
            status.push(div().child("Done").into_any_element());
        } else if !a.enabled {
            status.push(div().child("Paused").into_any_element());
        } else {
            status.push(div().child("Armed").into_any_element());
        }
        if let Some(run) = a.last_run() {
            let mut last = div().flex().items_center().gap(px(4.)).min_w_0().child(format!("Last {}", automations::when(run.at, now)));
            if let Some(e) = &run.error {
                last = last.child(div().text_color(theme.error).truncate().child(format!("· {e}")));
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
        let menu = Button::new(SharedString::from(format!("auto-menu-{}", a.id)))
            .small()
            .ghost()
            .icon(Icon::new(IconName::Ellipsis).size(px(14.)).text_color(theme.secondary))
            .dropdown_menu(move |menu, _, _| {
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
        let trig_icon = if a.effective_trigger().is_event() { icons::zap() } else { icons::clock() };
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
                    .child(div().flex_none().text_color(if a.enabled { theme.accent } else { theme.tertiary }).child(trig_icon.size(px(14.))))
                    .child(div().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).child(a.name.clone()))
                    .child(page_kit::quiet_chip(a.effective_trigger().describe(now), theme))
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
        let ws_open = self.workspace.clone();
        let proj_open = self.project.clone();
        let mut column = div().flex().flex_col().gap(px(6.)).pb(px(space::XL)).child(
            div()
                .flex()
                .items_center()
                .gap(px(space::S))
                .child(div().flex_1().child(page_kit::section_title("Automations", Some(self.list.len()), &theme)))
                .child(
                    Button::new("automation-open-page")
                        .small()
                        .ghost()
                        .label("Open Automations →")
                        .on_click(move |_, _, cx| ws_open.update(cx, |ws, cx| ws.open_automations(Some(proj_open.clone()), cx))),
                )
                .child(
                    Button::new("automation-new")
                        .small()
                        .ghost()
                        .label("New")
                        .icon(IconName::Plus)
                        .on_click(cx.listener(|s, _, window, cx| s.open_form(None, window, cx))),
                ),
        );
        if self.list.is_empty() {
            column = column.child(
                div()
                    .text_size(px(text::FOOTNOTE))
                    .text_color(theme.secondary)
                    .child("Prompts this workspace runs by itself on a schedule, file/git/PR events, or Mac events. Make one here, in Automations (⇧⌘A), or ask in any thread."),
            );
        }
        let cards: Vec<gpui_kit::AnyElement> = self.list.clone().iter().map(|a| self.card(a, &theme, cx)).collect();
        column.children(cards)
    }
}

// MARK: Unified New / Edit Form

type OnSaved = Box<dyn Fn(Option<String>, &mut App) + 'static>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum FileRepoKind {
    FolderWatch,
    GitPr,
}

struct AutomationForm {
    workspace: Entity<Workspace>,
    editing: Option<Automation>,
    on_saved: Option<OnSaved>,
    // Scope
    is_global: bool,
    project_id: String,
    // Basics
    name: Entity<InputState>,
    prompt: Entity<TextareaState>,
    // Trigger
    trigger_cat: TriggerCategory,
    schedule: Entity<InputState>,
    sys_event: SystemEventKind,
    sys_app: Entity<InputState>,
    sys_power: Option<String>,
    file_repo_kind: FileRepoKind,
    watch_path: Entity<InputState>,
    watch_glob: Entity<InputState>,
    repo_event: RepoEventKind,
    repo_branch: Entity<InputState>,
    // Thread policy
    thread_policy: ThreadPolicy,
    // Agent & Permissions
    agent: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    mode: Option<String>,
    // Guardrails (collapsible)
    show_guardrails: bool,
    cooldown: Entity<InputState>,
    debounce: Entity<InputState>,
    overlap: OverlapPolicy,
    active_hours_on: bool,
    active_start: Entity<InputState>,
    active_end: Entity<InputState>,
    active_weekdays_only: bool,
    ac_power_only: bool,
    notify: NotifyPolicy,
    condition_cmd: Entity<InputState>,
    working_dir: Entity<InputState>,
    scroll: ScrollHandle,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

impl AutomationForm {
    fn new(
        workspace: Entity<Workspace>,
        default_scope: Scope,
        editing: Option<Automation>,
        template: Option<Template>,
        on_saved: Option<OnSaved>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let ws = workspace.read(cx);
        let first_project = ws
            .projects
            .iter()
            .find(|p| ws.home_project().as_deref() != Some(p.id.as_str()))
            .or_else(|| ws.projects.first())
            .map(|p| p.id.clone())
            .unwrap_or_default();

        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Morning PR review"));
        let prompt = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx).placeholder("What each run does, written to stand alone.");
            s.set_auto_grow(3, 10, cx);
            s
        });
        let schedule = cx.new(|cx| InputState::new(window, cx).placeholder("weekdays 9:00"));
        let sys_app = cx.new(|cx| InputState::new(window, cx).placeholder("Any app (or e.g. Xcode, Zoom)"));
        let watch_path = cx.new(|cx| InputState::new(window, cx).placeholder("Workspace root (or ~/Downloads)"));
        let watch_glob = cx.new(|cx| InputState::new(window, cx).placeholder("Optional: *.pdf, docs/**/*.md"));
        let repo_branch = cx.new(|cx| InputState::new(window, cx).placeholder("Any branch (or e.g. main, release/*)"));
        let cooldown = cx.new(|cx| InputState::new(window, cx).placeholder("0 (or e.g. 15m, 1h, 1d)"));
        let debounce = cx.new(|cx| InputState::new(window, cx).placeholder("2s"));
        let active_start = cx.new(|cx| InputState::new(window, cx).placeholder("08:00"));
        let active_end = cx.new(|cx| InputState::new(window, cx).placeholder("20:00"));
        let condition_cmd = cx.new(|cx| InputState::new(window, cx).placeholder("Optional shell command (exit 0 to run)"));
        let working_dir = cx.new(|cx| InputState::new(window, cx).placeholder("Optional working directory override"));

        let mut is_global = default_scope.is_global();
        let mut project_id = default_scope.project_id().map(str::to_string).unwrap_or(first_project.clone());
        let mut trigger_cat = TriggerCategory::Schedule;
        let mut sys_event = SystemEventKind::MacWake;
        let mut sys_power = None;
        let mut file_repo_kind = FileRepoKind::FolderWatch;
        let mut repo_event = RepoEventKind::PrChecksFailed;
        let mut thread_policy = ThreadPolicy::New;
        let mut agent = None;
        let mut model = None;
        let mut effort = None;
        let mut mode = None;
        let mut overlap = OverlapPolicy::Skip;
        let mut active_hours_on = false;
        let mut active_weekdays_only = false;
        let mut ac_power_only = false;
        let mut notify = NotifyPolicy::Always;
        let mut show_guardrails = false;

        let apply_trigger = |trig: &Trigger,
                             trigger_cat: &mut TriggerCategory,
                             sys_event: &mut SystemEventKind,
                             sys_power: &mut Option<String>,
                             file_repo_kind: &mut FileRepoKind,
                             repo_event: &mut RepoEventKind,
                             window: &mut Window,
                             cx: &mut Context<Self>| {
            *trigger_cat = trig.category();
            match trig {
                Trigger::Schedule { schedule: s } => {
                    schedule.update(cx, |i, cx| i.set_value(schedule_text(s), window, cx));
                }
                Trigger::System(s) => {
                    *sys_event = s.event;
                    *sys_power = s.power.clone();
                    if let Some(app) = &s.app {
                        sys_app.update(cx, |i, cx| i.set_value(app.clone(), window, cx));
                    }
                }
                Trigger::FileWatch(fw) => {
                    *file_repo_kind = FileRepoKind::FolderWatch;
                    watch_path.update(cx, |i, cx| i.set_value(fw.path.clone(), window, cx));
                    if let Some(g) = &fw.glob {
                        watch_glob.update(cx, |i, cx| i.set_value(g.clone(), window, cx));
                    }
                }
                Trigger::Repo(r) => {
                    *file_repo_kind = FileRepoKind::GitPr;
                    *repo_event = r.event;
                    if let Some(b) = &r.branch {
                        repo_branch.update(cx, |i, cx| i.set_value(b.clone(), window, cx));
                    }
                }
                Trigger::Manual => {}
            }
        };

        let apply_guardrails = |g: &Guardrails,
                                overlap: &mut OverlapPolicy,
                                active_hours_on: &mut bool,
                                active_weekdays_only: &mut bool,
                                ac_power_only: &mut bool,
                                show_guardrails: &mut bool,
                                window: &mut Window,
                                cx: &mut Context<Self>| {
            *overlap = g.overlap;
            *ac_power_only = g.ac_power_only;
            if g.cooldown_secs > 0 {
                cooldown.update(cx, |i, cx| i.set_value(automations::format_duration(g.cooldown_secs), window, cx));
            }
            if g.debounce_secs > 0 {
                debounce.update(cx, |i, cx| i.set_value(format!("{}s", g.debounce_secs), window, cx));
            }
            if let Some(h) = &g.active_hours {
                *active_hours_on = true;
                *active_weekdays_only = h.weekdays == vec![1, 2, 3, 4, 5];
                active_start.update(cx, |i, cx| i.set_value(format!("{:02}:{:02}", h.start_hour, h.start_minute), window, cx));
                active_end.update(cx, |i, cx| i.set_value(format!("{:02}:{:02}", h.end_hour, h.end_minute), window, cx));
            } else {
                active_start.update(cx, |i, cx| i.set_value("08:00", window, cx));
                active_end.update(cx, |i, cx| i.set_value("20:00", window, cx));
            }
            if let Some(cmd) = &g.condition_cmd {
                condition_cmd.update(cx, |i, cx| i.set_value(cmd.clone(), window, cx));
            }
            if !g.is_default() {
                *show_guardrails = true;
            }
        };

        if let Some(a) = &editing {
            is_global = a.scope().is_global();
            if let Some(pid) = a.scope().project_id() {
                project_id = pid.to_string();
            }
            name.update(cx, |s, cx| s.set_value(a.name.clone(), window, cx));
            prompt.update(cx, |s, cx| s.set_value(a.prompt.clone(), window, cx));
            apply_trigger(&a.effective_trigger(), &mut trigger_cat, &mut sys_event, &mut sys_power, &mut file_repo_kind, &mut repo_event, window, cx);
            thread_policy = a.thread_policy();
            agent = a.agent.clone();
            model = a.model.clone();
            effort = a.effort.clone();
            mode = a.mode.clone();
            notify = a.notify;
            if let Some(wd) = &a.working_dir {
                working_dir.update(cx, |i, cx| i.set_value(wd.clone(), window, cx));
                show_guardrails = true;
            }
            apply_guardrails(&a.guardrails, &mut overlap, &mut active_hours_on, &mut active_weekdays_only, &mut ac_power_only, &mut show_guardrails, window, cx);
        } else if let Some(tpl) = &template {
            is_global = tpl.is_global;
            name.update(cx, |s, cx| s.set_value(tpl.name.to_string(), window, cx));
            prompt.update(cx, |s, cx| s.set_value(tpl.prompt.to_string(), window, cx));
            apply_trigger(&tpl.trigger, &mut trigger_cat, &mut sys_event, &mut sys_power, &mut file_repo_kind, &mut repo_event, window, cx);
            thread_policy = tpl.thread_policy;
            apply_guardrails(&tpl.guardrails, &mut overlap, &mut active_hours_on, &mut active_weekdays_only, &mut ac_power_only, &mut show_guardrails, window, cx);
        } else {
            active_start.update(cx, |i, cx| i.set_value("08:00", window, cx));
            active_end.update(cx, |i, cx| i.set_value("20:00", window, cx));
        }

        let subs = vec![
            cx.subscribe(&schedule, |f, _, _: &InputEvent, cx| {
                f.error = None;
                cx.notify();
            }),
            cx.subscribe(&name, |f, _, _: &InputEvent, cx| {
                f.error = None;
                cx.notify();
            }),
        ];

        AutomationForm {
            workspace,
            editing,
            on_saved,
            is_global,
            project_id,
            name,
            prompt,
            trigger_cat,
            schedule,
            sys_event,
            sys_app,
            sys_power,
            file_repo_kind,
            watch_path,
            watch_glob,
            repo_event,
            repo_branch,
            thread_policy,
            agent,
            model,
            effort,
            mode,
            show_guardrails,
            cooldown,
            debounce,
            overlap,
            active_hours_on,
            active_start,
            active_end,
            active_weekdays_only,
            ac_power_only,
            notify,
            condition_cmd,
            working_dir,
            scroll: ScrollHandle::new(),
            error: None,
            _subs: subs,
        }
    }

    fn build_trigger(&self, now: f64, cx: &App) -> Result<Trigger, String> {
        match self.trigger_cat {
            TriggerCategory::Schedule => {
                let when = self.schedule.read(cx).value().trim().to_string();
                if when.is_empty() {
                    return Err("Give it a schedule (e.g. \"weekdays 9:00\" or \"every 2h\").".into());
                }
                let s = automations::parse(&when, now)?;
                Ok(Trigger::Schedule { schedule: s })
            }
            TriggerCategory::System => {
                let app = self.sys_app.read(cx).value().trim().to_string();
                Ok(Trigger::System(SystemTrigger {
                    event: self.sys_event,
                    app: (self.sys_event.needs_app_filter() && !app.is_empty()).then_some(app),
                    power: self.sys_event.needs_power_filter().then(|| self.sys_power.clone()).flatten(),
                }))
            }
            TriggerCategory::FileRepo => match self.file_repo_kind {
                FileRepoKind::FolderWatch => {
                    let path = self.watch_path.read(cx).value().trim().to_string();
                    if self.is_global && path.is_empty() {
                        return Err("Global folder watchers need a folder path (e.g. ~/Downloads).".into());
                    }
                    let glob = self.watch_glob.read(cx).value().trim().to_string();
                    Ok(Trigger::FileWatch(FileWatchTrigger {
                        path,
                        glob: (!glob.is_empty()).then_some(glob),
                        events: vec![],
                    }))
                }
                FileRepoKind::GitPr => {
                    let branch = self.repo_branch.read(cx).value().trim().to_string();
                    Ok(Trigger::Repo(RepoTrigger {
                        event: self.repo_event,
                        branch: (!branch.is_empty()).then_some(branch),
                    }))
                }
            },
            TriggerCategory::Manual => Ok(Trigger::Manual),
        }
    }

    fn build_guardrails(&self, cx: &App) -> Result<Guardrails, String> {
        let cd_str = self.cooldown.read(cx).value().trim().to_string();
        let cooldown_secs = if cd_str.is_empty() {
            0
        } else {
            automations::parse_duration_secs(&cd_str).ok_or_else(|| format!("Couldn't read cooldown \"{cd_str}\" (try 15m, 1h, 1d)."))?
        };
        let db_str = self.debounce.read(cx).value().trim().to_string();
        let debounce_secs = if db_str.is_empty() {
            0
        } else {
            automations::parse_duration_secs(&db_str).ok_or_else(|| format!("Couldn't read debounce \"{db_str}\" (try 2s, 10s)."))?
        };
        let active_hours = if self.active_hours_on {
            let parse_hm = |raw: &str| -> Option<(u32, u32)> {
                let (h, m) = raw.trim().split_once(':')?;
                let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
                (h < 24 && m < 60).then_some((h, m))
            };
            let (sh, sm) = parse_hm(self.active_start.read(cx).value().as_str()).ok_or("Active hours start must be HH:MM (e.g. 08:00).")?;
            let (eh, em) = parse_hm(self.active_end.read(cx).value().as_str()).ok_or("Active hours end must be HH:MM (e.g. 20:00).")?;
            Some(ActiveHours {
                start_hour: sh,
                start_minute: sm,
                end_hour: eh,
                end_minute: em,
                weekdays: if self.active_weekdays_only { vec![1, 2, 3, 4, 5] } else { vec![] },
            })
        } else {
            None
        };
        let cmd = self.condition_cmd.read(cx).value().trim().to_string();
        Ok(Guardrails {
            debounce_secs,
            cooldown_secs,
            active_hours,
            overlap: self.overlap,
            ac_power_only: self.ac_power_only,
            condition_cmd: (!cmd.is_empty()).then_some(cmd),
        })
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = automations::now();
        let name = self.name.read(cx).value().trim().to_string();
        let prompt = self.prompt.read(cx).value().trim().to_string();
        if name.is_empty() || prompt.is_empty() {
            self.error = Some("Give it a name and a prompt.".into());
            cx.notify();
            return;
        }
        let trigger = match self.build_trigger(now, cx) {
            Ok(t) => t,
            Err(e) => {
                self.error = Some(e);
                cx.notify();
                return;
            }
        };
        let guardrails = match self.build_guardrails(cx) {
            Ok(g) => g,
            Err(e) => {
                self.error = Some(e);
                cx.notify();
                return;
            }
        };
        let scope = if self.is_global {
            Scope::Global
        } else {
            if self.project_id.is_empty() {
                self.error = Some("Choose a workspace, or switch Scope to Global.".into());
                cx.notify();
                return;
            }
            Scope::Project { project_id: self.project_id.clone() }
        };
        let policy = self.thread_policy;
        let agent = self.agent.clone();
        let model = self.model.clone();
        let effort = self.effort.clone();
        let mode = self.mode.clone();
        let notify = self.notify;
        let wd = self.working_dir.read(cx).value().trim().to_string();
        let working_dir = (!wd.is_empty()).then_some(wd);

        let result: Result<Option<String>, String> = match self.editing.clone() {
            Some(old) => self.workspace.update(cx, |ws, cx| {
                ws.edit_automations(cx, |list| {
                    let Some(a) = list.iter_mut().find(|a| a.id == old.id) else {
                        return Err("It was deleted meanwhile.".to_string());
                    };
                    // Keep next_run intact when schedule text is unchanged.
                    let same_schedule = matches!((&a.effective_trigger(), &trigger), (Trigger::Schedule { schedule: s1 }, Trigger::Schedule { schedule: s2 }) if schedule_text(s1) == schedule_text(s2));
                    if !same_schedule {
                        a.set_trigger(trigger, now)?;
                    }
                    a.set_scope(scope);
                    a.name = name;
                    a.prompt = prompt;
                    a.set_thread_policy(policy);
                    a.agent = agent;
                    a.model = model;
                    a.effort = effort;
                    a.mode = mode;
                    a.notify = notify;
                    a.working_dir = working_dir;
                    a.guardrails = guardrails;
                    Ok(Some(a.id.clone()))
                })
            }),
            None => Automation::new_with_trigger(scope, &name, &prompt, trigger, now).map(|mut a| {
                a.set_thread_policy(policy);
                a.agent = agent;
                a.model = model;
                a.effort = effort;
                a.mode = mode;
                a.notify = notify;
                a.working_dir = working_dir;
                a.guardrails = guardrails;
                let id = a.id.clone();
                self.workspace.update(cx, |ws, cx| ws.edit_automations(cx, |list| list.push(a)));
                Some(id)
            }),
        };

        match result {
            Ok(id) => {
                let cb = self.on_saved.take();
                window.close_dialog(cx);
                if let Some(cb) = cb {
                    cb(id, cx);
                }
            }
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }

    fn choose_watch_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::platform::set_open_panel_message("Choose a folder to watch for changes");
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Watch".into()),
        });
        let input = self.watch_path.clone();
        cx.spawn_in(window, async move |_, cx| {
            if let Ok(Ok(Some(list))) = paths.await {
                if let Some(p) = list.first() {
                    let s = p.to_string_lossy().to_string();
                    let _ = cx.update(|window, cx| input.update(cx, |i, cx| i.set_value(s, window, cx)));
                }
            }
        })
        .detach();
    }
}

/// What the schedule field holds for a saved schedule: words `parse` reads back the same.
fn schedule_text(s: &Schedule) -> String {
    match s {
        Schedule::Once { at } => {
            let mut tm: libc::tm = unsafe { std::mem::zeroed() };
            let t = *at as libc::time_t;
            unsafe { libc::localtime_r(&t, &mut tm) };
            format!("{:04}-{:02}-{:02} {}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min)
        }
        Schedule::Every { minutes, .. } if minutes % 60 == 0 => format!("every {}h", minutes / 60),
        Schedule::Every { minutes, .. } => format!("every {minutes}m"),
        Schedule::Daily { hour, minute, weekdays } => {
            let days = match weekdays.as_slice() {
                [] => "daily".to_string(),
                [1, 2, 3, 4, 5] => "weekdays".into(),
                [0, 6] => "weekends".into(),
                [one] => format!("{}s", ["sunday", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday"][*one as usize % 7]),
                many => many.iter().map(|d| ["sun", "mon", "tue", "wed", "thu", "fri", "sat"][*d as usize % 7]).collect::<Vec<_>>().join(","),
            };
            format!("{days} {hour}:{minute:02}")
        }
        Schedule::Cron { expr } => format!("cron {expr}"),
    }
}

impl Render for AutomationForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let now = automations::now();
        let ws = self.workspace.read(cx);
        let projects: Vec<(String, String)> = ws
            .projects
            .iter()
            .filter(|p| ws.home_project().as_deref() != Some(p.id.as_str()))
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect();
        let models: Vec<(String, String)> = if self.agent.as_deref() == Some("antigravity") {
            ws.agent_models.iter().map(|m| (m.value.clone(), m.display_name.clone())).collect()
        } else {
            ws.models.iter().map(|m| (m.value.clone(), m.display_name.clone())).collect()
        };
        let agy_found = ws.agy_found;

        let field = |label: &str, el: gpui_kit::AnyElement| {
            div()
                .flex()
                .items_start()
                .gap(px(12.))
                .child(div().w(px(92.)).pt(px(5.)).flex_none().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(label.to_string()))
                .child(div().flex_1().min_w_0().child(el))
        };

        let form = cx.entity().downgrade();

        // Scope picker
        let (f_g, f_w, f_p) = (form.clone(), form.clone(), form.clone());
        let scope_seg = page_kit::segmented(
            vec![
                Segment::new("form-scope-global", "Global", self.is_global, move |_, _, cx| {
                    let _ = f_g.update(cx, |f, cx| {
                        f.is_global = true;
                        cx.notify();
                    });
                }),
                Segment::new("form-scope-ws", "Workspace", !self.is_global, move |_, _, cx| {
                    let _ = f_w.update(cx, |f, cx| {
                        f.is_global = false;
                        cx.notify();
                    });
                }),
            ],
            &theme,
        );
        let proj_name = projects.iter().find(|(id, _)| id == &self.project_id).map(|(_, n)| n.clone()).unwrap_or_else(|| "Select workspace".into());
        let scope_row = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(scope_seg)
            .when(!self.is_global, |d| {
                let projects = projects.clone();
                d.child(
                    Button::new("form-project-dd")
                        .small()
                        .outline()
                        .label(proj_name)
                        .dropdown_menu(move |mut menu, _, _| {
                            for (id, name) in projects.clone() {
                                let f = f_p.clone();
                                menu = menu.item(PopupMenuItem::new(name).on_click(move |_, _, cx| {
                                    let id = id.clone();
                                    let _ = f.update(cx, |f, cx| {
                                        f.project_id = id;
                                        cx.notify();
                                    });
                                }));
                            }
                            menu
                        }),
                )
            })
            .when(self.is_global, |d| d.child(div().text_size(px(text::CAPTION)).text_color(theme.tertiary).child("Runs in Home; reaches all workspaces")));

        // Trigger category segmented control
        let (ft1, ft2, ft3, ft4) = (form.clone(), form.clone(), form.clone(), form.clone());
        let trigger_seg = page_kit::segmented(
            vec![
                Segment::new("trig-sched", "Schedule", self.trigger_cat == TriggerCategory::Schedule, move |_, _, cx| {
                    let _ = ft1.update(cx, |f, cx| { f.trigger_cat = TriggerCategory::Schedule; cx.notify(); });
                }),
                Segment::new("trig-sys", "Mac Event", self.trigger_cat == TriggerCategory::System, move |_, _, cx| {
                    let _ = ft2.update(cx, |f, cx| { f.trigger_cat = TriggerCategory::System; cx.notify(); });
                }),
                Segment::new("trig-file", "File & Repo", self.trigger_cat == TriggerCategory::FileRepo, move |_, _, cx| {
                    let _ = ft3.update(cx, |f, cx| { f.trigger_cat = TriggerCategory::FileRepo; cx.notify(); });
                }),
                Segment::new("trig-manual", "Manual / CLI", self.trigger_cat == TriggerCategory::Manual, move |_, _, cx| {
                    let _ = ft4.update(cx, |f, cx| { f.trigger_cat = TriggerCategory::Manual; cx.notify(); });
                }),
            ],
            &theme,
        );

        let trigger_body: gpui_kit::AnyElement = match self.trigger_cat {
            TriggerCategory::Schedule => {
                let typed = self.schedule.read(cx).value().trim().to_string();
                let preview = (!typed.is_empty()).then(|| match automations::parse(&typed, now) {
                    Ok(s) => match s.next_after(now) {
                        Some(next) => (false, format!("{} · first run {}", s.describe(now), automations::when(next, now))),
                        None => (true, "That time has already passed.".to_string()),
                    },
                    Err(_) => (true, format!("Try {}", automations::FORMS)),
                });
                let mut presets = div().flex().flex_wrap().gap(px(4.));
                for (lbl, val) in [("Every 1h", "every 1h"), ("Daily 9:00", "daily 9:00"), ("Weekdays 9:00", "weekdays 9:00"), ("Fridays 17:00", "fridays 17:00"), ("Cron", "cron 0 9 * * 1-5")] {
                    let sched = self.schedule.clone();
                    presets = presets.child(
                        Button::new(SharedString::from(format!("preset-{lbl}")))
                            .xsmall()
                            .ghost()
                            .label(lbl)
                            .on_click(move |_, window, cx| sched.update(cx, |i, cx| i.set_value(val.to_string(), window, cx))),
                    );
                }
                div()
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .child(Input::new(&self.schedule))
                    .child(presets)
                    .when_some(preview, |d, (bad, words)| d.child(div().text_size(px(text::CAPTION)).text_color(if bad { theme.tertiary } else { theme.secondary }).child(words)))
                    .child(div().text_size(px(text::CAPTION)).text_color(theme.tertiary).child(format!("Local time ({}). Missed runs while closed catch up once.", automations::zone())))
                    .into_any_element()
            }
            TriggerCategory::System => {
                let f_ev = form.clone();
                let ev_picker = Button::new("sys-event-picker")
                    .small()
                    .outline()
                    .label(self.sys_event.label())
                    .dropdown_menu(move |mut menu, _, _| {
                        for k in SystemEventKind::ALL {
                            let f = f_ev.clone();
                            menu = menu.item(PopupMenuItem::new(k.label()).on_click(move |_, _, cx| {
                                let _ = f.update(cx, |f, cx| {
                                    f.sys_event = k;
                                    cx.notify();
                                });
                            }));
                        }
                        menu
                    });
                let (fp1, fp2, fp3) = (form.clone(), form.clone(), form.clone());
                let power_seg = page_kit::segmented(
                    vec![
                        Segment::new("pwr-any", "Any change", self.sys_power.is_none(), move |_, _, cx| {
                            let _ = fp1.update(cx, |f, cx| { f.sys_power = None; cx.notify(); });
                        }),
                        Segment::new("pwr-ac", "Plugged in", self.sys_power.as_deref() == Some("ac"), move |_, _, cx| {
                            let _ = fp2.update(cx, |f, cx| { f.sys_power = Some("ac".into()); cx.notify(); });
                        }),
                        Segment::new("pwr-bat", "On battery", self.sys_power.as_deref() == Some("battery"), move |_, _, cx| {
                            let _ = fp3.update(cx, |f, cx| { f.sys_power = Some("battery".into()); cx.notify(); });
                        }),
                    ],
                    &theme,
                );
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(div().flex().items_center().gap(px(8.)).child(ev_picker))
                    .when(self.sys_event.needs_app_filter(), |d| d.child(Input::new(&self.sys_app)))
                    .when(self.sys_event.needs_power_filter(), |d| d.child(div().flex().child(power_seg)))
                    .child(div().text_size(px(text::CAPTION)).text_color(theme.tertiary).child("Native macOS notification observer — zero privacy prompts, active while YourHarness is open."))
                    .into_any_element()
            }
            TriggerCategory::FileRepo => {
                let (ff1, ff2, fr) = (form.clone(), form.clone(), form.clone());
                let kind_seg = page_kit::segmented(
                    vec![
                        Segment::new("fr-watch", "Folder Watch", self.file_repo_kind == FileRepoKind::FolderWatch, move |_, _, cx| {
                            let _ = ff1.update(cx, |f, cx| { f.file_repo_kind = FileRepoKind::FolderWatch; cx.notify(); });
                        }),
                        Segment::new("fr-repo", "Git / PR Event", self.file_repo_kind == FileRepoKind::GitPr, move |_, _, cx| {
                            let _ = ff2.update(cx, |f, cx| { f.file_repo_kind = FileRepoKind::GitPr; cx.notify(); });
                        }),
                    ],
                    &theme,
                );
                let sub = match self.file_repo_kind {
                    FileRepoKind::FolderWatch => div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(div().flex_1().child(Input::new(&self.watch_path)))
                                .child(
                                    Button::new("choose-watch-dir")
                                        .small()
                                        .outline()
                                        .label("Choose…")
                                        .on_click(cx.listener(|f, _, window, cx| f.choose_watch_folder(window, cx))),
                                ),
                        )
                        .child(Input::new(&self.watch_glob))
                        .child(div().text_size(px(text::CAPTION)).text_color(theme.tertiary).child("Changed file paths are automatically passed into the prompt (or use {{event.files}})."))
                        .into_any_element(),
                    FileRepoKind::GitPr => div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div().flex().child(
                                Button::new("repo-event-picker")
                                    .small()
                                    .outline()
                                    .label(self.repo_event.label())
                                    .dropdown_menu(move |mut menu, _, _| {
                                        for k in RepoEventKind::ALL {
                                            let f = fr.clone();
                                            menu = menu.item(PopupMenuItem::new(k.label()).on_click(move |_, _, cx| {
                                                let _ = f.update(cx, |f, cx| { f.repo_event = k; cx.notify(); });
                                            }));
                                        }
                                        menu
                                    }),
                            ),
                        )
                        .child(Input::new(&self.repo_branch))
                        .into_any_element(),
                };
                div().flex().flex_col().gap(px(6.)).child(div().flex().child(kind_seg)).child(sub).into_any_element()
            }
            TriggerCategory::Manual => {
                let raw_name = self.name.read(cx).value().trim().to_string();
                let slug = if raw_name.is_empty() { "my-automation".to_string() } else { raw_name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect::<String>().trim_matches('-').to_string() };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(div().font_family(MONO).text_size(px(text::CODE)).px(px(8.)).py(px(6.)).rounded(px(radius::SMALL)).bg(theme.selected).child(format!("yh trigger {slug} --data \"...\"")))
                    .child(div().text_size(px(text::CAPTION)).text_color(theme.tertiary).child("Run via Run Now, Spotlight (⌘K), Apple Shortcuts, Raycast, or git hooks using `yh trigger`."))
                    .into_any_element()
            }
        };

        // Thread policy 3-way segmented control
        let (tp1, tp2, tp3) = (form.clone(), form.clone(), form.clone());
        let thread_seg = page_kit::segmented(
            vec![
                Segment::new("tp-new", "New thread", self.thread_policy == ThreadPolicy::New, move |_, _, cx| {
                    let _ = tp1.update(cx, |f, cx| { f.thread_policy = ThreadPolicy::New; cx.notify(); });
                }),
                Segment::new("tp-settle", "New & settle prev", self.thread_policy == ThreadPolicy::NewSettlePrevious, move |_, _, cx| {
                    let _ = tp2.update(cx, |f, cx| { f.thread_policy = ThreadPolicy::NewSettlePrevious; cx.notify(); });
                }),
                Segment::new("tp-same", "Same thread", self.thread_policy == ThreadPolicy::Same, move |_, _, cx| {
                    let _ = tp3.update(cx, |f, cx| { f.thread_policy = ThreadPolicy::Same; cx.notify(); });
                }),
            ],
            &theme,
        );

        // Agent / Model / Effort / Mode compact row
        let (fa, fm, fe, fmode) = (form.clone(), form.clone(), form.clone(), form.clone());
        let agent_label = match self.agent.as_deref() {
            Some("claude") => "Claude",
            Some("antigravity") => "Antigravity",
            _ => "Agent: Default",
        };
        let model_label = self
            .model
            .as_deref()
            .and_then(|v| models.iter().find(|(id, _)| id == v).map(|(_, d)| d.clone()).or_else(|| Some(v.to_string())))
            .unwrap_or_else(|| "Model: Default".into());
        let effort_label = match self.effort.as_deref() {
            Some(e) => format!("Effort: {e}"),
            None => "Effort: Default".into(),
        };
        let mode_label = match self.mode.as_deref() {
            Some("ask") => "Mode: Ask",
            Some("acceptEdits") => "Mode: Accept Edits",
            Some("plan") => "Mode: Plan",
            Some("bypass") => "Mode: Bypass",
            _ => "Mode: Project",
        };
        let agent_row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .when(agy_found, |d| {
                d.child(
                    Button::new("form-agent-dd")
                        .xsmall()
                        .outline()
                        .label(agent_label)
                        .dropdown_menu(move |menu, _, _| {
                            let (f1, f2, f3) = (fa.clone(), fa.clone(), fa.clone());
                            menu.item(PopupMenuItem::new("Default").on_click(move |_, _, cx| { let _ = f1.update(cx, |f, cx| { f.agent = None; f.model = None; cx.notify(); }); }))
                                .item(PopupMenuItem::new("Claude Code").on_click(move |_, _, cx| { let _ = f2.update(cx, |f, cx| { f.agent = Some("claude".into()); f.model = None; cx.notify(); }); }))
                                .item(PopupMenuItem::new("Antigravity").on_click(move |_, _, cx| { let _ = f3.update(cx, |f, cx| { f.agent = Some("antigravity".into()); f.model = None; cx.notify(); }); }))
                        }),
                )
            })
            .child(
                Button::new("form-model-dd")
                    .xsmall()
                    .outline()
                    .label(model_label)
                    .dropdown_menu(move |mut menu, _, _| {
                        let f0 = fm.clone();
                        menu = menu.item(PopupMenuItem::new("Default").on_click(move |_, _, cx| {
                            let _ = f0.update(cx, |f, cx| { f.model = None; cx.notify(); });
                        }));
                        for (val, disp) in models.clone() {
                            let f = fm.clone();
                            menu = menu.item(PopupMenuItem::new(disp).on_click(move |_, _, cx| {
                                let val = val.clone();
                                let _ = f.update(cx, |f, cx| { f.model = Some(val); cx.notify(); });
                            }));
                        }
                        menu
                    }),
            )
            .child(
                Button::new("form-effort-dd")
                    .xsmall()
                    .outline()
                    .label(effort_label)
                    .dropdown_menu(move |mut menu, _, _| {
                        let f0 = fe.clone();
                        menu = menu.item(PopupMenuItem::new("Default").on_click(move |_, _, cx| {
                            let _ = f0.update(cx, |f, cx| { f.effort = None; cx.notify(); });
                        }));
                        for e in ["low", "medium", "high", "xhigh", "max"] {
                            let f = fe.clone();
                            menu = menu.item(PopupMenuItem::new(e).on_click(move |_, _, cx| {
                                let _ = f.update(cx, |f, cx| { f.effort = Some(e.into()); cx.notify(); });
                            }));
                        }
                        menu
                    }),
            )
            .child(
                Button::new("form-mode-dd")
                    .xsmall()
                    .outline()
                    .label(mode_label)
                    .dropdown_menu(move |mut menu, _, _| {
                        let f0 = fmode.clone();
                        menu = menu.item(PopupMenuItem::new("Project default").on_click(move |_, _, cx| {
                            let _ = f0.update(cx, |f, cx| { f.mode = None; cx.notify(); });
                        }));
                        for (val, lbl) in [("ask", "Ask"), ("acceptEdits", "Accept Edits"), ("plan", "Plan"), ("bypass", "Bypass Permissions")] {
                            let f = fmode.clone();
                            menu = menu.item(PopupMenuItem::new(lbl).on_click(move |_, _, cx| {
                                let _ = f.update(cx, |f, cx| { f.mode = Some(val.into()); cx.notify(); });
                            }));
                        }
                        menu
                    }),
            );

        // Guardrails collapsible block
        let (fo1, fo2, fo3) = (form.clone(), form.clone(), form.clone());
        let overlap_seg = page_kit::segmented(
            vec![
                Segment::new("ov-skip", "Skip", self.overlap == OverlapPolicy::Skip, move |_, _, cx| {
                    let _ = fo1.update(cx, |f, cx| { f.overlap = OverlapPolicy::Skip; cx.notify(); });
                }),
                Segment::new("ov-queue", "Queue", self.overlap == OverlapPolicy::Queue, move |_, _, cx| {
                    let _ = fo2.update(cx, |f, cx| { f.overlap = OverlapPolicy::Queue; cx.notify(); });
                }),
                Segment::new("ov-allow", "Parallel", self.overlap == OverlapPolicy::Allow, move |_, _, cx| {
                    let _ = fo3.update(cx, |f, cx| { f.overlap = OverlapPolicy::Allow; cx.notify(); });
                }),
            ],
            &theme,
        );
        let (fn1, fn2, fn3) = (form.clone(), form.clone(), form.clone());
        let notify_seg = page_kit::segmented(
            vec![
                Segment::new("nt-all", "Always", self.notify == NotifyPolicy::Always, move |_, _, cx| {
                    let _ = fn1.update(cx, |f, cx| { f.notify = NotifyPolicy::Always; cx.notify(); });
                }),
                Segment::new("nt-attn", "Waiting only", self.notify == NotifyPolicy::NeedsAttention, move |_, _, cx| {
                    let _ = fn2.update(cx, |f, cx| { f.notify = NotifyPolicy::NeedsAttention; cx.notify(); });
                }),
                Segment::new("nt-silent", "Silent", self.notify == NotifyPolicy::Silent, move |_, _, cx| {
                    let _ = fn3.update(cx, |f, cx| { f.notify = NotifyPolicy::Silent; cx.notify(); });
                }),
            ],
            &theme,
        );

        let guardrails_section = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .id("toggle-guardrails")
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .cursor_pointer()
                    .text_size(px(text::FOOTNOTE))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.secondary)
                    .child(Icon::new(if self.show_guardrails { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(12.)))
                    .child("Guardrails & advanced options")
                    .on_click(cx.listener(|f, _, _, cx| {
                        f.show_guardrails = !f.show_guardrails;
                        cx.notify();
                    })),
            )
            .when(self.show_guardrails, |d| {
                d.child(
                    page_kit::card(&theme)
                        .p(px(space::M))
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .child(field("Cooldown", Input::new(&self.cooldown).small().into_any_element()))
                        .when(self.trigger_cat == TriggerCategory::FileRepo && self.file_repo_kind == FileRepoKind::FolderWatch, |c| {
                            c.child(field("Debounce", Input::new(&self.debounce).small().into_any_element()))
                        })
                        .child(field("If running", div().flex().child(overlap_seg).into_any_element()))
                        .child(field(
                            "Active hours",
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(8.))
                                        .child(
                                            Switch::new("form-active-hours").checked(self.active_hours_on).on_change(cx.listener(|f, on: &bool, _, cx| {
                                                f.active_hours_on = *on;
                                                cx.notify();
                                            })),
                                        )
                                        .when(self.active_hours_on, |r| {
                                            r.child(div().w(px(72.)).child(Input::new(&self.active_start).small()))
                                                .child("to")
                                                .child(div().w(px(72.)).child(Input::new(&self.active_end).small()))
                                                .child(
                                                    Button::new("form-weekdays-only")
                                                        .xsmall()
                                                        .when(self.active_weekdays_only, |b| b.outline())
                                                        .when(!self.active_weekdays_only, |b| b.ghost())
                                                        .label("Weekdays only")
                                                        .on_click(cx.listener(|f, _, _, cx| {
                                                            f.active_weekdays_only = !f.active_weekdays_only;
                                                            cx.notify();
                                                        })),
                                                )
                                        }),
                                )
                                .into_any_element(),
                        ))
                        .child(field(
                            "Power",
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(
                                    Switch::new("form-ac-only").checked(self.ac_power_only).on_change(cx.listener(|f, on: &bool, _, cx| {
                                        f.ac_power_only = *on;
                                        cx.notify();
                                    })),
                                )
                                .child(div().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child("Only run on AC power"))
                                .into_any_element(),
                        ))
                        .child(field("Notify", div().flex().child(notify_seg).into_any_element()))
                        .child(field("Pre-check", Input::new(&self.condition_cmd).small().into_any_element()))
                        .when(self.is_global, |c| c.child(field("Working dir", Input::new(&self.working_dir).small().into_any_element()))),
                )
            });

        let has_name_and_prompt = !self.name.read(cx).value().trim().is_empty() && !self.prompt.read(cx).value().trim().is_empty();

        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .text_size(px(text::BODY))
            .child(
                div()
                    .id("auto-form-scroll")
                    .max_h(px(540.))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .pr(px(4.))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(field("Scope", scope_row.into_any_element()))
                    .child(field("Name", Input::new(&self.name).into_any_element()))
                    .child(field(
                        "Prompt",
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(page_kit::card(&theme).px(px(space::S)).py(px(6.)).child(Textarea::new(&self.prompt).appearance(false)))
                            .when(self.trigger_cat != TriggerCategory::Schedule, |d| {
                                d.child(
                                    div()
                                        .text_size(px(text::CAPTION))
                                        .text_color(theme.tertiary)
                                        .child("Event details are appended automatically (or interpolate {{event.reason}}, {{event.files}}, {{event.app}}, {{event.branch}})."),
                                )
                            })
                            .into_any_element(),
                    ))
                    .child(field(
                        "Trigger",
                        div().flex().flex_col().gap(px(8.)).child(div().flex().child(trigger_seg)).child(trigger_body).into_any_element(),
                    ))
                    .child(field("Threads", div().flex().child(thread_seg).into_any_element()))
                    .child(field(
                        "Run with",
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(agent_row)
                            .when(self.mode.as_deref() == Some("bypass"), |d| {
                                d.child(
                                    div()
                                        .text_size(px(text::CAPTION))
                                        .text_color(theme.warning)
                                        .child("Bypass runs tools unattended without asking for permission."),
                                )
                            })
                            .into_any_element(),
                    ))
                    .child(guardrails_section),
            )
            .when_some(self.error.clone(), |d, e| d.child(div().text_size(px(text::FOOTNOTE)).text_color(theme.error).child(e)))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(Button::new("auto-cancel").outline().label("Cancel").on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(
                        Button::new("auto-save")
                            .primary()
                            .label(if self.editing.is_some() { "Save" } else { "Create" })
                            .disabled(!has_name_and_prompt)
                            .on_click(cx.listener(|f, _, window, cx| f.save(window, cx))),
                    ),
            )
    }
}
