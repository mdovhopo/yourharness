//! A project's page, from clicking its name: its pull requests and repos at a glance (the
//! Swift app's WorkspaceView), with Memory one tab over. Everything's discovered from git and
//! the forges' CLIs (`yh_core::workspace`): read on a copy off the main thread, swapped in
//! whole, again every minute while it shows. Open PRs come grouped into stacks, each a card
//! drawn as a staircase (`StackGraph`); then each repo, with its worktrees and the threads
//! working in them.

use crate::state::workspace::Workspace;
use crate::theme::{Theme, MONO};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::Sizable;
use gpui_kit::{div, prelude::*, px, Context, Entity, FontWeight, Hsla, SharedString, Task, Window};
use std::sync::Arc;
use std::time::Duration;
use yh_core::forge::PullRequest;
use yh_core::stacks::{Lanes, PullRequestStack, Segment, StackGraph};
use yh_core::workspace::{LayoutKind, RemovalRisk, WorkspaceTools};

pub struct ProjectPage {
    workspace: Entity<Workspace>,
    pub project: String,
    core: Option<yh_core::workspace::Workspace>,
    tools: Arc<WorkspaceTools>,
    refreshing: bool,
    /// What a stack command or a worktree change is doing, or its error.
    activity: Option<String>,
    /// Repos opened to show their worktrees.
    open_repos: std::collections::HashSet<String>,
    /// New Worktree: for which repo, and the branch typed.
    new_worktree: Option<(String, Entity<InputState>)>,
    _poll: Option<Task<()>>,
}

fn green() -> Hsla {
    gpui_kit::rgb(0x34c759).into()
}

fn purple() -> Hsla {
    gpui_kit::rgb(0xaf52de).into()
}

impl ProjectPage {
    pub fn new(workspace: Entity<Workspace>, project: String, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tools = Arc::new(WorkspaceTools::find(std::env::var("YH_PATH").ok().as_deref()));
        let mut page = ProjectPage { workspace, project, core: None, tools, refreshing: false, activity: None, open_repos: Default::default(), new_worktree: None, _poll: None };
        page.refresh(true, cx);
        page._poll = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(60)).await;
            if this.update(cx, |p, cx| p.refresh(false, cx)).is_err() {
                break;
            }
        }));
        page
    }

    fn root(&self, cx: &gpui_kit::App) -> Option<std::path::PathBuf> {
        self.workspace.read(cx).project(&self.project).map(|p| std::path::PathBuf::from(&p.path))
    }

    /// Reads it all again on a copy, off the main thread; `rescan` looks for repos again too.
    pub fn refresh(&mut self, rescan: bool, cx: &mut Context<Self>) {
        if self.refreshing {
            return;
        }
        let Some(root) = self.root(cx) else { return };
        let mut copy = self.core.clone().unwrap_or_else(|| yh_core::workspace::Workspace::new(&root));
        let tools = self.tools.clone();
        self.refreshing = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let started = std::time::Instant::now();
            let done = cx
                .background_executor()
                .spawn(async move {
                    copy.refresh(&tools, rescan, Duration::from_secs(30));
                    copy
                })
                .await;
            if std::env::var("YH_DATA_DIR").is_ok() {
                eprintln!(
                    "PROJECT read {} repos, {} worktrees, {} PRs in {} ms",
                    done.cloned_repos().len(),
                    done.all_worktrees().len(),
                    done.pull_requests().len(),
                    started.elapsed().as_millis()
                );
            }
            let _ = this.update(cx, |p, cx| {
                p.core = Some(done);
                p.refreshing = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// Runs a command (a `gh stack` one, or a worktree change) off the main thread, says
    /// how it went, and reads everything again now and a few seconds later.
    fn run(&mut self, what: String, job: impl FnOnce(Arc<WorkspaceTools>) -> Result<(), String> + Send + 'static, cx: &mut Context<Self>) {
        self.activity = Some(format!("{what}…"));
        cx.notify();
        let tools = self.tools.clone();
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { job(tools) }).await;
            let _ = this.update(cx, |p, cx| {
                p.activity = Some(match &result {
                    Ok(()) => format!("{what}: done."),
                    Err(e) => format!("{what} failed: {e}"),
                });
                if std::env::var("YH_DATA_DIR").is_ok() {
                    eprintln!("PROJECT {}", p.activity.clone().unwrap_or_default());
                }
                p.refresh(false, cx);
                cx.notify();
            });
            // Forges take a moment to show what a merge or a push did.
            for wait in [4, 15] {
                cx.background_executor().timer(Duration::from_secs(wait)).await;
                let _ = this.update(cx, |p, cx| p.refresh(false, cx));
            }
        })
        .detach();
    }

    /// Merges a GitHub stack up to a PR (all of it with None), with `gh stack merge`.
    pub fn merge_stack(&mut self, stack: Option<u64>, up_to: Option<u64>, repo: std::path::PathBuf, cx: &mut Context<Self>) {
        let what = match up_to {
            Some(n) => format!("Merging up to #{n}"),
            None => "Merging the stack".to_string(),
        };
        self.run(what, move |t| t.stacks.as_ref().ok_or("gh stack isn't installed.".to_string())?.merge(stack, up_to, &repo), cx);
    }

    /// Opens the PRs of a stack made with `gh stack` that isn't on GitHub yet.
    pub fn submit_stack(&mut self, checkout: std::path::PathBuf, cx: &mut Context<Self>) {
        self.run("Submitting the stack".into(), move |t| t.stacks.as_ref().ok_or("gh stack isn't installed.".to_string())?.submit(&checkout), cx);
    }

    fn sync_stack(&mut self, checkout: std::path::PathBuf, cx: &mut Context<Self>) {
        self.run("Syncing the stack".into(), move |t| t.stacks.as_ref().ok_or("gh stack isn't installed.".to_string())?.sync(&checkout), cx);
    }

    /// A branch checked out in a folder of its own beside the repo (new when it doesn't exist).
    pub fn add_worktree(&mut self, repo_id: String, branch: String, cx: &mut Context<Self>) {
        let Some(core) = &self.core else { return };
        let Some(repo) = core.repos().iter().find(|r| r.id() == repo_id).cloned() else { return };
        let name: String = branch.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
        let path = repo.path.with_file_name(format!("{}-{name}", repo.path.file_name().unwrap_or_default().to_string_lossy()));
        self.new_worktree = None;
        self.run(format!("Adding a worktree for {branch}"), move |t| {
            let git = t.git.as_ref().ok_or("git isn't installed.".to_string())?;
            let exists = git.branch_exists(&branch, &repo.path);
            git.add_worktree(&repo.path, &path, if exists { None } else { Some(&branch) }, None, if exists { Some(&branch) } else { None })
        }, cx);
    }

    fn remove_worktree(&mut self, worktree: yh_core::git::Worktree, repo_path: std::path::PathBuf, force: bool, cx: &mut Context<Self>) {
        self.run("Removing the worktree".into(), move |t| t.git.as_ref().ok_or("git isn't installed.".to_string())?.remove_worktree(&worktree, &repo_path, force), cx);
    }
}

// MARK: Drawing

fn chip(label: &str, color: Hsla) -> gpui_kit::Div {
    div().flex_none().px(px(6.)).rounded(px(8.)).text_size(px(10.)).font_weight(FontWeight::MEDIUM).text_color(color).bg(color.opacity(0.14)).child(label.to_string())
}

fn age(updated: Option<&str>) -> String {
    let Some(t) = updated.and_then(yh_core::library::parse_iso) else { return String::new() };
    // parse_iso gives seconds; ago takes milliseconds.
    crate::state::thread_tools::ago(t * 1000.)
}

impl ProjectPage {
    fn header(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = self.workspace.read(cx);
        let name = ws.project(&self.project).map(|p| p.name.clone()).unwrap_or_default();
        let subtitle = match self.core.as_ref().and_then(|c| c.layout.as_ref().map(|l| (c, l))) {
            None => "Looking for repositories…".to_string(),
            Some((_, l)) if l.kind == LayoutKind::Plain => "Folder".to_string(),
            Some((c, l)) if l.kind == LayoutKind::Repo => {
                let branch = c.status.statuses.get("").and_then(|s| s.branch.clone()).map(|b| format!(" · {b}")).unwrap_or_default();
                let wt = c.all_worktrees().iter().filter(|w| !w.is_main).count();
                format!("Git repository{branch}{}", if wt > 0 { format!(" · {wt} worktrees") } else { String::new() })
            }
            Some((c, _)) => {
                let (cloned, uncloned) = (c.cloned_repos().len(), c.uncloned_repos().len());
                let mut parts = vec!["Workspace".to_string(), if cloned == 1 { "1 repo".into() } else { format!("{cloned} repos") }];
                if uncloned > 0 {
                    parts.push(format!("{uncloned} not cloned"));
                }
                parts.join(" · ")
            }
        };
        let (ws1, project) = (self.workspace.clone(), self.project.clone());
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(28.))
            .py(px(16.))
            .border_b_1()
            .border_color(theme.separator)
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.)).child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).child(name)).child(div().text_color(theme.secondary).child(subtitle)))
            .when_some(self.activity.clone(), |d, a| d.child(div().max_w(px(360.)).text_size(px(12.)).text_color(theme.secondary).line_clamp(2).child(a)))
            .child(Button::new("project-refresh").small().ghost().label(if self.refreshing { "Refreshing…" } else { "Refresh" }).on_click(cx.listener(|p, _, _, cx| p.refresh(true, cx))))
            .child(div().flex().gap(px(2.)).child(Button::new("tab-overview").small().primary().label("Overview")).child(Button::new("tab-memory").small().ghost().label("Memory").on_click(move |_, _, cx| {
                ws1.update(cx, |ws, cx| ws.open_memory(&project, cx))
            })))
    }

    /// A PR's row: its graph slice (in a stack), state, title, branches, what blocks it.
    fn pull_row(&self, pr: &PullRequest, graph: Option<(&yh_core::stacks::GraphRow, usize)>, theme: &Theme) -> gpui_kit::AnyElement {
        let (dot, state): (Hsla, Option<&str>) = match pr.state.as_str() {
            "merged" => (purple(), Some("Merged")),
            "closed" => (theme.error, Some("Closed")),
            _ if pr.in_merge_queue => (purple(), Some("In merge queue")),
            _ if pr.auto_merge => (purple(), Some("Auto-merge")),
            _ if pr.draft => (theme.secondary, Some("Draft")),
            _ => (green(), None),
        };
        let lane_w = 22.;
        let graph_el = graph.map(|(row, lanes)| {
            let segments: Vec<Segment> = row.segments.iter().cloned().collect();
            let lane = row.lane;
            let line = theme.secondary.opacity(0.5);
            gpui_kit::canvas(
                |_, _, _| {},
                move |b, _, window, _| {
                    let x = |l: usize| f32::from(b.origin.x) + 12. + l as f32 * lane_w;
                    let (top, mid, bottom) = (f32::from(b.origin.y), f32::from(b.origin.y) + f32::from(b.size.height) / 2., f32::from(b.origin.y) + f32::from(b.size.height));
                    let mut seg = |x1: f32, y1: f32, x2: f32, y2: f32| {
                        let mut p = gpui_kit::PathBuilder::stroke(px(1.5));
                        p.move_to(gpui_kit::point(px(x1), px(y1)));
                        p.line_to(gpui_kit::point(px(x2), px(y2)));
                        if let Ok(p) = p.build() {
                            window.paint_path(p, line);
                        }
                    };
                    for s in &segments {
                        match *s {
                            Segment::Through(l) => seg(x(l), top, x(l), bottom),
                            Segment::FromAbove(l) => seg(x(l), top, x(l), mid),
                            Segment::ToBelow(l) => seg(x(l), mid, x(l), bottom),
                            Segment::Branch { from, to } => {
                                seg(x(from), top, x(from), mid - 6.);
                                seg(x(from), mid - 6., x(to), mid);
                            }
                        }
                    }
                    let r = 5.;
                    window.paint_quad(gpui_kit::fill(gpui_kit::Bounds::new(gpui_kit::point(px(x(lane) - r), px(mid - r)), gpui_kit::size(px(r * 2.), px(r * 2.))), dot).corner_radii(px(r)));
                },
            )
            .w(px(12. + lanes as f32 * lane_w))
            .h_full()
            .flex_none()
        });
        let url = pr.link.url.clone();
        let review = match pr.review.as_str() {
            "approved" => Some(("Approved", green())),
            "changesRequested" => Some(("Changes requested", theme.error)),
            "required" => Some(("Review required", theme.secondary)),
            _ => None,
        };
        let checks = match pr.checks.as_str() {
            "passing" => Some(("Checks pass", green())),
            "failing" => Some(("Checks fail", theme.error)),
            "pending" => Some(("Checks running", theme.warning)),
            _ => None,
        };
        let blocker = match pr.merge_status.as_deref().filter(|_| pr.state == "open") {
            Some("DIRTY") => Some("Conflicts"),
            Some("BEHIND") => Some("Behind base"),
            Some("BLOCKED") => Some("Blocked"),
            _ => None,
        };
        div()
            .id(SharedString::from(format!("pr-{}", pr.link.id())))
            .flex()
            .items_stretch()
            .min_h(px(48.))
            .px(px(10.))
            .hover(|d| d.bg(theme.selected))
            .on_click(move |_, _, cx| cx.open_url(&url))
            .when_some(graph_el, |d, g| d.child(g))
            .when(graph.is_none(), |d| d.child(div().flex_none().w(px(22.)).flex().items_center().child(div().w(px(9.)).h(px(9.)).rounded_full().bg(dot))))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .py(px(7.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().flex().items_center().gap(px(6.)).child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().font_weight(FontWeight::MEDIUM).child(pr.title.clone())).when_some(state, |d, s| d.child(chip(s, dot))))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .text_size(px(11.))
                            .text_color(theme.secondary)
                            .child(format!("{} #{}", pr.link.path.rsplit('/').next().unwrap_or(&pr.link.path), pr.link.number))
                            .child(div().font_family(MONO).overflow_hidden().whitespace_nowrap().text_ellipsis().child(format!("{} → {}", pr.head, pr.base)))
                            .when_some(pr.author.clone().filter(|_| !pr.is_mine), |d, a| d.child(a))
                            .when_some(blocker, |d, b| d.child(div().text_color(theme.warning).child(b)))
                            .when_some(review, |d, (t, c)| d.child(div().text_color(c).child(t)))
                            .when_some(checks, |d, (t, c)| d.child(div().text_color(c).child(t)))
                            .child(div().flex_1())
                            .child(age(pr.updated_at.as_deref())),
                    ),
            )
            .into_any_element()
    }

    /// One card a stack (a lone PR is a stack of one), drawn as a staircase.
    fn stack_card(&self, stack: &PullRequestStack, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let graph = StackGraph::layout(stack, Lanes::Depth);
        let is_stack = stack.is_stacked() || stack.number().is_some();
        let mut card = div().flex().flex_col().py(px(3.)).rounded(px(10.)).bg(theme.card).border_1().border_color(theme.separator);
        if is_stack {
            let top = &stack.pull_request;
            let open: Vec<u64> = stack.all().iter().filter(|p| p.is_open()).map(|p| p.link.number).collect();
            let repo_path = self.core.as_ref().and_then(|c| c.repo_for(top).map(|r| r.path.clone()));
            let number = stack.number();
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(12.))
                    .pt(px(8.))
                    .pb(px(2.))
                    .text_size(px(11.))
                    .text_color(theme.secondary)
                    .child(div().flex_1().child(format!("Stack{} · {} PRs into {}", number.map(|n| format!(" {n}")).unwrap_or_default(), stack.all().len(), top.base)))
                    // GitHub's own stacks (gh stack) merge from the bottom up to a layer.
                    .when(number.is_some() && !open.is_empty() && repo_path.is_some(), |d| {
                        let repo = repo_path.clone().unwrap_or_default();
                        d.child(Button::new(SharedString::from(format!("merge-stack-{}", stack.id()))).xsmall().ghost().label("Merge Stack").on_click(cx.listener(move |p, _, _, cx| p.merge_stack(number, None, repo.clone(), cx))))
                    }),
            );
        }
        for row in &graph.rows {
            card = card.child(self.pull_row(&row.pull_request, is_stack.then_some((row, graph.lane_count)), theme));
        }
        card.into_any_element()
    }

    fn pulls_section(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(core) = &self.core else { return div().into_any_element() };
        let sections = core.pull_request_sections();
        let mut column = div().flex().flex_col().gap(px(8.));
        let errors: Vec<String> = core.pulls.errors.values().cloned().collect();
        for e in errors {
            column = column.child(div().text_size(px(12.)).text_color(theme.warning).child(e));
        }
        for (title, stacks) in [("Needs your review", &sections.needs_review), ("Yours", &sections.mine), ("Others", &sections.others)] {
            if stacks.is_empty() {
                continue;
            }
            column = column.child(div().pt(px(8.)).text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(theme.secondary).child(format!("{} · {}", title.to_uppercase(), stacks.iter().map(|s| s.all().len()).sum::<usize>())));
            for s in stacks.iter() {
                column = column.child(self.stack_card(s, theme, cx));
            }
        }
        // Stacks made with gh stack, not on GitHub yet.
        for (repo, local) in core.unsubmitted_stacks() {
            let branches: Vec<String> = local.layers.iter().map(|l| l.branch.clone()).collect();
            let checkout = core.checkout_on_any_of(repo, &branches).unwrap_or(repo.path.clone());
            let (c1, c2) = (checkout.clone(), checkout);
            column = column.child(
                div()
                    .p(px(12.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(theme.separator)
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().flex_1().flex().flex_col().child(div().font_weight(FontWeight::MEDIUM).child(format!("A stack in {}, not on GitHub yet", repo.name()))).child(div().text_size(px(11.)).font_family(MONO).text_color(theme.secondary).child(format!("{} ← {}", local.trunk, branches.join(" ← ")))))
                    .child(Button::new(SharedString::from(format!("sync-{}", local.id()))).xsmall().ghost().label("Sync").on_click(cx.listener(move |p, _, _, cx| p.sync_stack(c1.clone(), cx))))
                    .child(Button::new(SharedString::from(format!("submit-{}", local.id()))).xsmall().primary().label("Submit").on_click(cx.listener(move |p, _, _, cx| p.submit_stack(c2.clone(), cx)))),
            );
        }
        if sections.needs_review.is_empty() && sections.mine.is_empty() && sections.others.is_empty() && core.has_forge_remotes() && core.pulls_updated_at.is_some() {
            column = column.child(div().text_color(theme.secondary).child("No open pull requests."));
        }
        column.into_any_element()
    }

    fn repos_section(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(core) = &self.core else { return div().into_any_element() };
        let mut column = div().flex().flex_col().gap(px(6.)).child(div().pt(px(16.)).text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(theme.secondary).child("REPOSITORIES"));
        // The threads working in each folder, from where they run.
        let threads: Vec<(String, String, String)> = self
            .workspace
            .read(cx)
            .threads
            .iter()
            .filter_map(|t| {
                let t = t.read(cx);
                (t.meta.project_id == self.project && t.meta.settled_at.is_none()).then(|| (t.id().to_string(), t.title.clone(), self.workspace.read(cx).folder_of(t)))
            })
            .collect();
        let mut repos: Vec<&yh_core::workspace::WorkspaceRepo> = core.repos().iter().collect();
        repos.sort_by_key(|r| (!r.is_root(), r.name()));
        for repo in repos {
            let id = repo.id().to_string();
            let status = core.status.statuses.get(&id);
            let worktrees: Vec<_> = core.worktrees_of(repo).iter().filter(|w| !w.is_main).cloned().collect();
            let open = self.open_repos.contains(&id) || worktrees.len() <= 3;
            let prs = core.pull_requests_for(repo).len();
            let mut detail = vec![];
            if let Some(s) = status {
                detail.push(s.branch.clone().unwrap_or_else(|| "no branch".into()));
                if s.is_dirty() {
                    detail.push(format!("{} changed", s.staged + s.modified + s.untracked));
                }
                if s.ahead > 0 {
                    detail.push(format!("{} to push", s.ahead));
                }
                if s.behind > 0 {
                    detail.push(format!("{} behind", s.behind));
                }
            }
            if !repo.is_cloned {
                detail.push("not cloned".into());
            }
            if prs > 0 {
                detail.push(format!("{prs} open PR{}", if prs == 1 { "" } else { "s" }));
            }
            let here: Vec<&(String, String, String)> = threads.iter().filter(|(_, _, f)| *f == repo.real_path() || *f == repo.path.to_string_lossy()).collect();
            let (toggle_id, add_id) = (id.clone(), id.clone());
            let mut card = div().flex().flex_col().rounded(px(10.)).bg(theme.card).border_1().border_color(theme.separator).child(
                div()
                    .id(SharedString::from(format!("repo-{id}")))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(12.))
                    .py(px(8.))
                    .child(div().flex_1().min_w_0().flex().flex_col().child(div().font_weight(FontWeight::MEDIUM).child(repo.name())).child(div().text_size(px(11.)).text_color(theme.secondary).child(detail.join(" · "))))
                    .when(!here.is_empty(), |d| d.child(chip(&if here.len() == 1 { here[0].1.chars().take(28).collect::<String>() } else { format!("{} threads", here.len()) }, theme.accent)))
                    .when(worktrees.len() > 3, |d| {
                        d.child(Button::new(SharedString::from(format!("fold-{id}"))).xsmall().ghost().label(if open { "Hide worktrees".to_string() } else { format!("{} worktrees", worktrees.len()) }).on_click(cx.listener(move |p, _, _, cx| {
                            if !p.open_repos.remove(&toggle_id) {
                                p.open_repos.insert(toggle_id.clone());
                            }
                            cx.notify();
                        })))
                    })
                    .when(repo.is_cloned, |d| {
                        d.child(Button::new(SharedString::from(format!("add-wt-{id}"))).xsmall().ghost().label("New Worktree…").on_click(cx.listener(move |p, _, window, cx| {
                            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Branch: an existing one, or a new name"));
                            p.new_worktree = Some((add_id.clone(), input));
                            cx.notify();
                        })))
                    }),
            );
            if let Some((for_repo, input)) = &self.new_worktree {
                if *for_repo == id {
                    let (rid, inp) = (id.clone(), input.clone());
                    card = card.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .px(px(12.))
                            .pb(px(8.))
                            .child(div().flex_1().child(Input::new(input).small()))
                            .child(Button::new("wt-cancel").xsmall().ghost().label("Cancel").on_click(cx.listener(|p, _, _, cx| {
                                p.new_worktree = None;
                                cx.notify();
                            })))
                            .child(Button::new("wt-add").xsmall().primary().label("Add").on_click(cx.listener(move |p, _, _, cx| {
                                let branch = inp.read(cx).value().trim().to_string();
                                if !branch.is_empty() {
                                    p.add_worktree(rid.clone(), branch, cx);
                                }
                            }))),
                    );
                }
            }
            if open {
                for wt in &worktrees {
                    let st = core.status.worktree_statuses.get(&wt.path);
                    let pr = core.pull_request_for(wt, repo);
                    let risk = core.removal_risk(wt, repo);
                    let here: Vec<&(String, String, String)> = threads.iter().filter(|(_, _, f)| *f == wt.path || f.starts_with(&format!("{}/", wt.path))).collect();
                    let home = std::env::var("HOME").unwrap_or_default();
                    let shown_path = wt.path.strip_prefix(&home).map(|p| format!("~{p}")).unwrap_or(wt.path.clone());
                    let mut bits = vec![wt.branch.clone().unwrap_or_else(|| "no branch".into())];
                    if wt.is_missing {
                        bits.push("folder missing".into());
                    }
                    if let Some(s) = st.filter(|s| s.is_dirty()) {
                        bits.push(format!("{} changed", s.staged + s.modified + s.untracked));
                    }
                    let (w1, r1) = (wt.clone(), repo.path.clone());
                    let force = risk != RemovalRisk::None;
                    let (label, tip) = match &risk {
                        RemovalRisk::None => ("Remove", "Nothing in it that isn't elsewhere.".to_string()),
                        RemovalRisk::KeepsWork(why) => ("Remove…", why.clone()),
                        RemovalRisk::LosesWork(why) => ("Remove (loses work)", why.clone()),
                    };
                    card = card.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .pl(px(28.))
                            .pr(px(12.))
                            .py(px(6.))
                            .border_t_1()
                            .border_color(theme.separator)
                            .child(div().flex_1().min_w_0().flex().flex_col().child(div().text_size(px(12.)).font_family(MONO).overflow_hidden().whitespace_nowrap().text_ellipsis().child(shown_path)).child(div().text_size(px(11.)).text_color(theme.secondary).child(bits.join(" · "))))
                            .when_some(pr.map(|p| format!("#{}", p.link.number)), |d, n| d.child(chip(&n, green())))
                            .when(!here.is_empty(), |d| d.child(chip(&format!("{} thread{}", here.len(), if here.len() == 1 { "" } else { "s" }), theme.accent)))
                            .child(
                                Button::new(SharedString::from(format!("rm-wt-{}", wt.path)))
                                    .xsmall()
                                    .ghost()
                                    .label(label)
                                    .tooltip(tip)
                                    .on_click(cx.listener(move |p, _, _, cx| p.remove_worktree(w1.clone(), r1.clone(), force, cx))),
                            ),
                    );
                }
            }
            column = column.child(card);
        }
        column.into_any_element()
    }
}

impl Render for ProjectPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let plain = self.core.as_ref().and_then(|c| c.layout.as_ref()).is_some_and(|l| l.kind == LayoutKind::Plain);
        let body = if plain {
            div().p(px(40.)).text_center().text_color(theme.secondary).child("No Git repositories. Repos and pull requests show up here once this folder, or folders inside it, are git repositories.").into_any_element()
        } else {
            div().flex().flex_col().child(self.pulls_section(&theme, cx)).child(self.repos_section(&theme, cx)).into_any_element()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.header(&theme, cx))
            .child(div().id("project-body").flex_1().min_h_0().overflow_y_scroll().child(div().max_w(px(880.)).w_full().mx_auto().px(px(32.)).py(px(20.)).child(body)))
    }
}
