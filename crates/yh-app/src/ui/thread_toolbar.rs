//! The open thread's header buttons, the Swift app's `SelectedThreadToolbar`: the checkouts it
//! works in (a popover with each one's state, and running the thread in another), its pull
//! requests, and the right sidebar. Built from what the workspace already holds: nothing here
//! runs git or reads files.

use crate::state::workspace::{Panel, Workspace};
use crate::theme::{Theme, MONO};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::Sizable;
use gpui_kit::{div, prelude::*, px, AnyElement, App, Entity, FontWeight, SharedString};
use yh_core::git::RepoStatus;

/// One checkout as the popover lists it.
#[derive(Clone)]
struct Row {
    name: String,
    of_repo: Option<String>,
    path: String,
    runs_here: bool,
    state: String,
    attention: bool,
}

/// What a checkout's status says, in a line: its branch, what's not committed, what's not pushed.
fn state_line(status: Option<&RepoStatus>) -> (String, bool) {
    let Some(s) = status else { return ("Reading its state…".into(), false) };
    let mut parts = vec![s.branch.clone().unwrap_or_else(|| "detached HEAD".into())];
    let changed = s.staged + s.modified + s.untracked;
    if s.conflicted > 0 {
        parts.push(format!("{} in conflict", s.conflicted));
    }
    if changed > 0 {
        parts.push(format!("{changed} uncommitted"));
    }
    if s.ahead > 0 {
        parts.push(format!("{} to push", s.ahead));
    }
    if s.behind > 0 {
        parts.push(format!("{} behind", s.behind));
    }
    if s.upstream_gone {
        parts.push("its remote branch is gone".into());
    }
    (parts.join(" · "), s.conflicted > 0 || changed > 0 || s.ahead > 0)
}

fn rows(workspace: &Workspace, cx: &App) -> (Vec<Row>, bool) {
    let Some(thread) = workspace.selected_thread() else { return (vec![], false) };
    let t = thread.read(cx);
    let Some(scan) = workspace.scans.get(&t.meta.project_id) else { return (vec![], false) };
    let running = t.meta.working_directory.as_deref().map(yh_core::workspace::real_path);
    let rows = workspace
        .checkouts_of(t)
        .into_iter()
        .map(|c| {
            let path = c.path();
            let status = match &c.worktree {
                Some(w) => scan.status.worktree_statuses.get(&w.path),
                None => scan.status.statuses.get(c.repo.id()),
            };
            let (state, attention) = state_line(status);
            Row {
                name: c.worktree.as_ref().map_or_else(|| c.repo.name(), |w| w.path.rsplit('/').next().unwrap_or("").to_string()),
                of_repo: c.worktree.as_ref().map(|_| c.repo.name()),
                runs_here: running.as_deref() == Some(path.as_str()),
                path,
                state,
                attention,
            }
        })
        .collect();
    (rows, t.meta.working_directory.is_some())
}

/// The checkouts button, when the thread has touched any.
pub fn checkouts(workspace: &Entity<Workspace>, theme: &Theme, cx: &App) -> Option<AnyElement> {
    let ws = workspace.read(cx);
    let (rows, elsewhere) = rows(ws, cx);
    if rows.is_empty() {
        return None;
    }
    let thread = ws.selected.clone()?;
    let project = ws.selected_thread().map(|t| t.read(cx).meta.project_id.clone())?;
    let count = rows.len();
    let theme = theme.clone();
    let workspace = workspace.clone();
    Some(
        Popover::new("thread-checkouts")
            // `YH_OPEN_CHECKOUTS=1` opens it, for checking it without a click.
            .default_open(std::env::var("YH_OPEN_CHECKOUTS").is_ok())
            .trigger(
                Button::new("thread-checkouts-button")
                    .small()
                    .ghost()
                    .label(format!("⎇ {count}"))
                    .tooltip(if count == 1 { "The checkout this thread works in".to_string() } else { format!("The {count} checkouts this thread works in") }),
            )
            .content(move |_, _, _| {
                let mut list = div().w(px(400.)).flex().flex_col().child(div().px(px(14.)).pt(px(12.)).pb(px(8.)).font_weight(FontWeight::SEMIBOLD).child("Works In"));
                for (i, row) in rows.iter().enumerate() {
                    let (path, path2, path3, path4) = (row.path.clone(), row.path.clone(), row.path.clone(), row.path.clone());
                    let (ws, thread) = (workspace.clone(), thread.clone());
                    let runs_here = row.runs_here;
                    list = list.child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(10.))
                            .px(px(14.))
                            .py(px(8.))
                            .when(i > 0, |d| d.border_t_1().border_color(theme.separator))
                            .child(div().w(px(16.)).text_color(theme.secondary).child(if row.of_repo.is_some() { "↳" } else { "▣" }))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(3.))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.))
                                            .child(div().font_weight(FontWeight::MEDIUM).child(row.name.clone()))
                                            .when_some(row.of_repo.clone(), |d, repo| d.child(div().text_color(theme.secondary).child(format!("of {repo}"))))
                                            .when(row.runs_here, |d| {
                                                d.child(div().px(px(5.)).rounded(px(6.)).bg(theme.accent.opacity(0.15)).text_size(px(10.)).font_weight(FontWeight::SEMIBOLD).child("Runs here"))
                                            }),
                                    )
                                    .child(div().text_size(px(11.)).text_color(if row.attention { theme.warning } else { theme.secondary }).child(row.state.clone()))
                                    .child(div().text_size(px(10.5)).font_family(MONO).text_color(theme.secondary).truncate().child(row.path.clone())),
                            )
                            .child(Button::new(SharedString::from(format!("checkout-menu-{i}"))).xsmall().ghost().label("⋯").dropdown_menu(move |menu, _, _| {
                                let (p1, p2, p3, p4) = (path.clone(), path2.clone(), path3.clone(), path4.clone());
                                let (ws, thread) = (ws.clone(), thread.clone());
                                let menu = menu
                                    .item(PopupMenuItem::new("Show in Finder").on_click(move |_, _, cx| cx.reveal_path(std::path::Path::new(&p1))))
                                    .item(PopupMenuItem::new("Open in Terminal").on_click(move |_, _, _| {
                                        let _ = std::process::Command::new("/usr/bin/open").args(["-a", "Terminal", &p2]).spawn();
                                    }))
                                    .item(PopupMenuItem::new("Copy Path").on_click(move |_, _, cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(p3.clone()))));
                                if runs_here {
                                    menu
                                } else {
                                    menu.separator().item(PopupMenuItem::new("Run This Thread Here").on_click(move |_, _, cx| {
                                        ws.update(cx, |ws, cx| ws.set_working_directory(&thread, Some(p4.clone()), cx));
                                    }))
                                }
                            })),
                    );
                }
                let (ws_back, ws_all, thread, project) = (workspace.clone(), workspace.clone(), thread.clone(), project.clone());
                list.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(px(14.))
                        .py(px(10.))
                        .border_t_1()
                        .border_color(theme.separator)
                        .child(if elsewhere {
                            Button::new("checkouts-back")
                                .small()
                                .ghost()
                                .label("Back to the Project Folder")
                                .tooltip("The next message starts a new Claude session there")
                                .on_click(move |_, _, cx| ws_back.update(cx, |ws, cx| ws.set_working_directory(&thread, None, cx)))
                                .into_any_element()
                        } else {
                            div().into_any_element()
                        })
                        .child(Button::new("checkouts-all").small().ghost().label("All Worktrees").on_click(move |_, _, cx| ws_all.update(cx, |ws, cx| ws.open_project(&project, cx)))),
                )
            })
            .into_any_element(),
    )
}

/// Pull requests and the right sidebar.
pub fn buttons(workspace: &Entity<Workspace>, cx: &App) -> AnyElement {
    let ws = workspace.read(cx);
    let shows = ws.shows_inspector;
    let (w1, w2) = (workspace.clone(), workspace.clone());
    div()
        .flex()
        .items_center()
        .gap(px(2.))
        .child(Button::new("thread-pulls").small().ghost().label("Pull Requests").tooltip("Pull requests this thread worked on or mentioned").on_click(move |_, _, cx| {
            w1.update(cx, |ws, cx| ws.show_panel(Panel::PullRequests, cx));
        }))
        .when(!shows, |d| {
            d.child(Button::new("thread-sidebar").small().ghost().label("◧").tooltip("Show the right sidebar (⌥⌘0)").on_click(move |_, _, cx| {
                w2.update(cx, |ws, cx| ws.set_shows_inspector(true, cx));
            }))
        })
        .into_any_element()
}
