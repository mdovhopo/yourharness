//! A project's page, from clicking its name: its pull requests and repos at a glance,
//! with Memory one tab over. Everything's discovered from git and
//! the forges' CLIs (`yh_core::workspace`): read on a copy off the main thread, swapped in
//! whole, again every minute while it shows. Open PRs come grouped into stacks, each a card
//! drawn as a staircase (`StackGraph`) with what can be done with it (merge, submit, add a
//! branch, sync, make it one of GitHub's); then each repo, with its worktrees, what state
//! each is in and the threads working in them. Anything that loses work asks first.

use crate::state::workspace::Workspace;
use crate::theme::{radius, space, text, Theme, MONO};
use crate::ui::page_icons as icons;
use crate::ui::page_kit::{self, Segment as Tab};
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use crate::ui::thread_toolbar::icon_button;
use gpui_kit::component::{Disableable, Icon, IconName, Sizable, WindowExt};
use gpui_kit::{div, prelude::*, px, App, ClipboardItem, Context, Entity, FontWeight, Hsla, SharedString, Task, WeakEntity, Window};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use yh_core::forge::PullRequest;
use yh_core::git::{Forge, RepoStatus, Worktree};
use yh_core::stacks::{Lanes, PullRequestStack, Segment, StackGraph};
use yh_core::workspace::{LayoutKind, RemovalRisk, WorkspaceRepo, WorkspaceTools};

pub struct ProjectPage {
    workspace: Entity<Workspace>,
    pub project: String,
    core: Option<yh_core::workspace::Workspace>,
    tools: Arc<WorkspaceTools>,
    refreshing: bool,
    /// Asked to read again while a read was going: done once it's in (true: rescan too).
    again: Option<bool>,
    /// What a stack command or a worktree change is doing, or its error.
    activity: Option<String>,
    /// Repos whose worktrees were folded or unfolded by hand (else open with three or fewer).
    toggled_repos: HashSet<String>,
    /// Whether `gh stack` is there; None until asked (once, off the main thread).
    stacks_installed: Option<bool>,
    installing: bool,
    /// The threads working in each checkout (by its path), newest first: worked out when the
    /// repos were read, not on every draw.
    threads_here: HashMap<String, Vec<String>>,
    /// Big workspaces: repos, worktrees and threads filtered by name.
    filter: Entity<InputState>,
    /// Claude Code's own memory here: (on, its files), read aside, for the nudge.
    claude_memory: Option<(bool, usize)>,
    show_others: bool,
    show_uncloned: bool,
    /// `YH_PROJECT_PROBE`: what to do once the repos have been read (see `run_probe`).
    probe: Option<String>,
    scroll: gpui_kit::ScrollHandle,
    /// The page's own width, measured where it's painted.
    width: page_kit::PageWidth,
    /// A narrow page (from the last render): the toolbar keeps to its buttons.
    compact: bool,
    /// Narrower still: PR rows leave out when they changed, for their branches' room.
    narrow: bool,
    _poll: Option<Task<()>>,
}

fn green() -> Hsla {
    gpui_kit::rgb(0x34c759).into()
}

fn purple() -> Hsla {
    gpui_kit::rgb(0xaf52de).into()
}

fn orange() -> Hsla {
    gpui_kit::rgb(0xff9500).into()
}

fn copy(text: String, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(text));
}

fn open_in_terminal(path: &str) {
    let _ = std::process::Command::new("/usr/bin/open").args(["-a", "Terminal", path]).spawn();
}

fn tilde(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    match path.strip_prefix(&home) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => path.to_string(),
    }
}

fn logs() -> bool {
    std::env::var("YH_DATA_DIR").is_ok()
}

/// A merge asked for: the open layers it takes, bottom first, and the layer it stops at
/// (None: the whole stack).
#[derive(Clone)]
struct MergeRequest {
    number: u64,
    pulls: Vec<(u64, String)>,
    base: String,
    up_to: Option<u64>,
    repo: PathBuf,
}

impl ProjectPage {
    pub fn new(workspace: Entity<Workspace>, project: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tools = Arc::new(WorkspaceTools::find(std::env::var("YH_PATH").ok().as_deref()));
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter repos, worktrees, threads"));
        cx.subscribe(&filter, |_, _, _: &gpui_kit::component::input::InputEvent, cx| cx.notify()).detach();
        if let Ok(words) = std::env::var("YH_PROJECT_FILTER") {
            filter.update(cx, |f, cx| f.set_value(words, window, cx));
        }
        let mut page = ProjectPage {
            workspace,
            project,
            core: None,
            tools,
            refreshing: false,
            again: None,
            activity: None,
            toggled_repos: Default::default(),
            stacks_installed: None,
            installing: false,
            threads_here: Default::default(),
            filter,
            claude_memory: None,
            show_others: false,
            show_uncloned: false,
            probe: std::env::var("YH_PROJECT_PROBE").ok(),
            scroll: gpui_kit::ScrollHandle::new(),
            width: Default::default(),
            compact: false,
            narrow: false,
            _poll: None,
        };
        page.refresh(true, cx);
        page.check_stacks(cx);
        page.read_claude_memory(cx);
        page._poll = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(60)).await;
            if this.update(cx, |p, cx| p.refresh(false, cx)).is_err() {
                break;
            }
        }));
        page
    }

    /// Whether Claude Code keeps its own memory here too, read aside (the nudge's question).
    fn read_claude_memory(&mut self, cx: &mut Context<Self>) {
        let ws = self.workspace.read(cx);
        let on = ws.library.setting("memoryEnabled").and_then(|v| v.as_bool()).unwrap_or(true);
        let dismissed = ws.library.setting("claudeMemoryNudgeDismissed").and_then(|v| v.as_array().map(|a| a.iter().any(|x| x.as_str() == Some(self.project.as_str())))).unwrap_or(false);
        let Some(folder) = ws.project(&self.project).map(|p| p.path.clone()) else { return };
        if !on || dismissed {
            self.claude_memory = None;
            return;
        }
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    let config = yh_core::history::config_directory();
                    let setting = yh_core::memory_archive::claude_code::setting(&folder, &config);
                    (!setting.is_off(), yh_core::memory_archive::claude_code::files(&folder, &config).len())
                })
                .await;
            let _ = this.update(cx, |p, cx| {
                p.claude_memory = Some(found);
                cx.notify();
            });
        })
        .detach();
    }

    /// "Claude Code keeps its own memory here": compact, with the way to one memory on the
    /// Memory page.
    fn claude_memory_nudge(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        let (on, files) = self.claude_memory?;
        if !on && files == 0 {
            return None;
        }
        let text = match (on, files) {
            (true, 0) => "Claude Code also keeps its own memory when you run claude here outside YourHarness, so two memories would drift apart.".to_string(),
            (_, n) => format!("Claude Code keeps its own memory for this folder too ({} file{}): bring it in and keep one.", n, if n == 1 { "" } else { "s" }),
        };
        let (ws, project) = (self.workspace.clone(), self.project.clone());
        let (ws2, project2) = (self.workspace.clone(), self.project.clone());
        Some(
            div()
                .mb(px(20.))
                .px(px(space::M + 2.))
                .py(px(space::S + 2.))
                .rounded(px(radius::LARGE))
                .bg(theme.warning.opacity(0.07))
                .border_1()
                .border_color(theme.warning.opacity(0.28))
                .flex()
                .items_center()
                .gap(px(space::M))
                .child(crate::ui::icons::layers().size(px(15.)).text_color(theme.warning))
                .child(div().flex_1().min_w_0().text_size(px(text::FOOTNOTE)).child(text))
                .child(Button::new("claude-memory-open").small().outline().label("Use One Memory…").on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.open_memory(&project, cx))))
                .child(
                    icon_button("claude-memory-dismiss", Icon::new(IconName::Close), "Not now", theme).on_click(cx.listener(move |p, _, _, cx| {
                        let lib = ws2.read(cx).library.clone();
                        let mut list = lib.setting("claudeMemoryNudgeDismissed").and_then(|v| v.as_array().cloned()).unwrap_or_default();
                        list.push(serde_json::Value::from(project2.clone()));
                        lib.set_setting("claudeMemoryNudgeDismissed", &serde_json::Value::Array(list));
                        p.claude_memory = None;
                        cx.notify();
                    })),
                )
                .into_any_element(),
        )
    }

    /// The filter's words, lowercased; empty when there's no filter.
    fn filter_words(&self, cx: &App) -> String {
        self.filter.read(cx).value().trim().to_lowercase()
    }

    /// Whether a worktree matches the filter: its folder, its branch, or a thread working there.
    fn worktree_matches(&self, wt: &Worktree, words: &str, cx: &App) -> bool {
        words.is_empty()
            || wt.path.to_lowercase().contains(words)
            || wt.branch.as_deref().is_some_and(|b| b.to_lowercase().contains(words))
            || self.threads_here.get(&wt.path).is_some_and(|ids| ids.iter().any(|id| self.workspace.read(cx).thread(id).is_some_and(|t| t.read(cx).title.to_lowercase().contains(words))))
    }

    fn root(&self, cx: &App) -> Option<PathBuf> {
        self.workspace.read(cx).project(&self.project).map(|p| PathBuf::from(&p.path))
    }

    /// Whether `gh stack` is installed: `gh extension list`, off the main thread.
    fn check_stacks(&mut self, cx: &mut Context<Self>) {
        let Some(stacks) = self.tools.stacks.clone() else {
            self.stacks_installed = Some(false);
            return;
        };
        cx.spawn(async move |this, cx| {
            let installed = cx.background_executor().spawn(async move { stacks.is_installed() }).await;
            let _ = this.update(cx, |p, cx| {
                p.stacks_installed = Some(installed);
                cx.notify();
            });
        })
        .detach();
    }

    /// Reads it all again on a copy, off the main thread; `rescan` looks for repos again too.
    pub fn refresh(&mut self, rescan: bool, cx: &mut Context<Self>) {
        if self.refreshing {
            self.again = Some(self.again.unwrap_or(false) || rescan);
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
            if logs() {
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
                p.count_threads(cx);
                if let Some(rescan) = p.again.take() {
                    p.refresh(rescan, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Which threads of the project work in which checkout, from the folders they touched.
    fn count_threads(&mut self, cx: &App) {
        let Some(core) = &self.core else { return };
        let ws = self.workspace.read(cx);
        let metas: Vec<_> = ws.threads.iter().map(|t| t.read(cx)).filter(|t| t.meta.project_id == self.project).map(|t| (t.id().to_string(), &t.meta)).collect();
        let folders: Vec<yh_core::workspace::ThreadFolders> = metas
            .iter()
            .map(|(id, m)| yh_core::workspace::ThreadFolders { id, footprint: &m.footprint, written: &m.written_footprint, working_directory: m.working_directory.as_deref(), last_active: m.updated_at })
            .collect();
        self.threads_here = core.threads_by_checkout(&folders, false);
        if logs() {
            let mut counts: Vec<String> = self.threads_here.iter().map(|(k, v)| format!("{}={}", tilde(k), v.len())).collect();
            counts.sort();
            eprintln!("PROJECT threads by checkout: {}", counts.join(", "));
        }
    }

    /// Runs a command (a `gh stack` one, or a worktree change) off the main thread, says how it
    /// went, and reads everything again now and a few seconds later. A failure with `alert`
    /// set also puts up an alert with that title.
    fn run(&mut self, what: String, alert: Option<&'static str>, job: impl FnOnce(Arc<WorkspaceTools>) -> Result<(), String> + Send + 'static, window: &mut Window, cx: &mut Context<Self>) {
        self.activity = Some(format!("{what}…"));
        cx.notify();
        let tools = self.tools.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_executor().spawn(async move { job(tools) }).await;
            let _ = this.update_in(cx, |p, window, cx| {
                p.activity = Some(match &result {
                    Ok(()) => format!("{what}: done."),
                    Err(e) => format!("{what} failed: {e}"),
                });
                if logs() {
                    eprintln!("PROJECT {}", p.activity.clone().unwrap_or_default());
                }
                if let (Err(e), Some(title)) = (&result, alert) {
                    let e = e.clone();
                    window.open_alert_dialog(cx, move |a, _, _| a.title(title).description(e.clone()).ok_text("OK"));
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

    // MARK: Stacks

    /// Asks first: a merge on GitHub can't be undone.
    fn request_merge(&mut self, request: MergeRequest, window: &mut Window, cx: &mut Context<Self>) {
        let title = if request.pulls.len() == 1 {
            format!("Squash and merge #{} into {}?", request.pulls[0].0, request.base)
        } else {
            format!("Squash and merge {} pull requests into {}?", request.pulls.len(), request.base)
        };
        let list: Vec<String> = request.pulls.iter().map(|(n, t)| format!("#{n} {t}")).collect();
        let how = if request.pulls.len() == 1 { "GitHub squashes it into one commit." } else { "GitHub merges them bottom up, each squashed into one commit." };
        let message = format!("{}\n\n{how} This can't be undone.", list.join("\n"));
        if logs() {
            eprintln!("PROJECT asks: {title} | {}", message.replace('\n', " / "));
        }
        let page = cx.entity().downgrade();
        let r1 = request.clone();
        window.open_alert_dialog(cx, move |a, _, _| {
            let (page, request) = (page.clone(), r1.clone());
            a.title(title.clone()).description(message.clone()).ok_text("Squash and Merge").show_cancel(true).on_ok(move |_, window, cx| {
                let request = request.clone();
                let _ = page.update(cx, |p, cx| p.merge_stack(request, window, cx));
                true
            })
        });
        if std::env::var("YH_PROJECT_PROBE_ANSWER").as_deref() == Ok("ok") {
            // The probe says yes, as clicking Squash and Merge does (sandbox repos only).
            window.close_dialog(cx);
            self.merge_stack(request, window, cx);
        }
    }

    /// Merges a GitHub stack up to a PR (all of it with None), with `gh stack merge`.
    fn merge_stack(&mut self, r: MergeRequest, window: &mut Window, cx: &mut Context<Self>) {
        let what = match r.up_to {
            Some(n) => format!("Merging up to #{n}"),
            None => format!("Merging stack #{}", r.number),
        };
        self.run(what, None, move |t| t.stacks.as_ref().ok_or("gh isn't installed.".to_string())?.merge(Some(r.number), r.up_to, &r.repo), window, cx);
    }

    fn submit_stack(&mut self, checkout: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.run("Submitting the stack".into(), None, move |t| t.stacks.as_ref().ok_or("gh isn't installed.".to_string())?.submit(&checkout), window, cx);
    }

    fn sync_stack(&mut self, checkout: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.run("Syncing the stack".into(), None, move |t| t.stacks.as_ref().ok_or("gh isn't installed.".to_string())?.sync(&checkout), window, cx);
    }

    fn link_stack(&mut self, urls: Vec<String>, repo: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.run("Making it a GitHub stack".into(), None, move |t| t.stacks.as_ref().ok_or("gh isn't installed.".to_string())?.link(&urls, &repo), window, cx);
    }

    fn add_branch(&mut self, branch: String, checkout: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.run(format!("Adding {branch}"), None, move |t| t.stacks.as_ref().ok_or("gh isn't installed.".to_string())?.add(&branch, &checkout), window, cx);
    }

    fn install_stacks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.installing = true;
        let _ = window;
        let Some(stacks) = self.tools.stacks.clone() else { return };
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { stacks.install() }).await;
            let _ = this.update(cx, |p, cx| {
                p.installing = false;
                p.stacks_installed = Some(result.is_ok());
                if let Err(e) = result {
                    p.activity = Some(format!("Installing gh stack failed: {e}"));
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Add Branch on Top…: a name, then `gh stack add` in the checkout on the top branch.
    fn ask_branch_on_top(&mut self, checkout: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Branch"));
        let page = cx.entity().downgrade();
        let theme = Theme::get(cx);
        window.open_dialog(cx, move |dialog, _, _| {
            let (input, page, checkout) = (input.clone(), page.clone(), checkout.clone());
            dialog.title("Add a Branch on Top").w(px(420.)).child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(div().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child("A new branch at the top branch's latest commit, checked out there. Commit to it, then Submit."))
                    .child(Input::new(&input))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(Button::new("branch-cancel").outline().label("Cancel").on_click(|_, window, cx| window.close_dialog(cx)))
                            .child(Button::new("branch-add").primary().label("Add").on_click(move |_, window, cx| {
                                let branch = input.read(cx).value().trim().to_string();
                                window.close_dialog(cx);
                                if !branch.is_empty() {
                                    let checkout = checkout.clone();
                                    let _ = page.update(cx, |p, cx| p.add_branch(branch, checkout, window, cx));
                                }
                            })),
                    ),
            )
        });
    }

    // MARK: Worktrees

    /// Safe: just remove it. Work in it: ask, saying what happens to it.
    fn request_removal(&mut self, wt: Worktree, repo: WorkspaceRepo, window: &mut Window, cx: &mut Context<Self>) {
        let Some(core) = &self.core else { return };
        let risk = core.removal_risk(&wt, &repo);
        if risk == RemovalRisk::None {
            self.remove_worktree(wt, repo, window, cx);
            return;
        }
        let name = Path::new(&wt.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let loses = matches!(risk, RemovalRisk::LosesWork(_));
        let title = if loses { format!("Discard the work in “{name}”?") } else { format!("Remove the worktree “{name}”?") };
        let mut parts = vec![];
        if let RemovalRisk::KeepsWork(why) | RemovalRisk::LosesWork(why) = &risk {
            parts.push(why.clone());
        }
        parts.push("Its folder is deleted.".to_string());
        match self.threads_here.get(&wt.path).map_or(0, Vec::len) {
            0 => {}
            1 => parts.push("The thread that worked in it keeps its conversation.".into()),
            n => parts.push(format!("The {n} threads that worked in it keep their conversations.")),
        }
        let message = parts.join(" ");
        if logs() {
            eprintln!("PROJECT asks: {title} | {message}");
        }
        let page = cx.entity().downgrade();
        let (w1, r1) = (wt.clone(), repo.clone());
        window.open_alert_dialog(cx, move |a, _, _| {
            let (page, wt, repo) = (page.clone(), w1.clone(), r1.clone());
            a.title(title.clone())
                .description(message.clone())
                .ok_text(if loses { "Remove and Discard" } else { "Remove" })
                .ok_variant(ButtonVariant::Danger)
                .show_cancel(true)
                .on_ok(move |_, window, cx| {
                    let (wt, repo) = (wt.clone(), repo.clone());
                    let _ = page.update(cx, |p, cx| p.remove_worktree(wt, repo, window, cx));
                    true
                })
        });
        if std::env::var("YH_PROJECT_PROBE_ANSWER").as_deref() == Ok("ok") {
            window.close_dialog(cx);
            self.remove_worktree(wt, repo, window, cx);
        }
    }

    /// `git worktree remove` (forced with uncommitted changes, which git refuses otherwise), or
    /// for a folder that's gone, `git worktree prune`. A failure puts up an alert.
    fn remove_worktree(&mut self, wt: Worktree, repo: WorkspaceRepo, window: &mut Window, cx: &mut Context<Self>) {
        let dirty = self.core.as_ref().and_then(|c| c.status.worktree_statuses.get(&wt.path)).is_some_and(|s| s.is_dirty());
        if let Some(core) = &mut self.core {
            // Gone from the list at once; a failure brings it back with the next read.
            core.forget_worktree(&wt, &repo);
        }
        let repo_path = repo.path.clone();
        self.run(
            if wt.is_missing { "Forgetting the worktree".into() } else { "Removing the worktree".into() },
            Some("Couldn't remove the worktree"),
            move |t| {
                let git = t.git.as_ref().ok_or("git isn't installed.".to_string())?;
                if wt.is_missing { git.prune_worktrees(&repo_path) } else { git.remove_worktree(&wt, &repo_path, dirty) }
            },
            window,
            cx,
        );
    }

    fn prune(&mut self, repo: WorkspaceRepo, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(core) = &mut self.core {
            core.forget_missing_worktrees(&repo);
        }
        let path = repo.path.clone();
        self.run("Forgetting missing worktrees".into(), Some("Couldn't forget the worktrees"), move |t| t.git.as_ref().ok_or("git isn't installed.".to_string())?.prune_worktrees(&path), window, cx);
    }

    /// A thread of the project that works in `folder` from its first message.
    fn new_thread_in(&mut self, folder: Option<String>, cx: &mut Context<Self>) {
        let project = self.project.clone();
        let project_path = self.root(cx).map(|p| p.to_string_lossy().into_owned());
        self.workspace.update(cx, |ws, cx| {
            let id = ws.new_thread(&project, cx);
            if let Some(f) = folder.filter(|f| Some(f) != project_path.as_ref()) {
                ws.set_working_directory(&id, Some(f), cx);
            }
        });
    }

    fn open_new_worktree(&mut self, repo: WorkspaceRepo, window: &mut Window, cx: &mut Context<Self>) {
        let base = self.core.as_ref().and_then(|c| c.status.default_branches.get(repo.id()).cloned());
        let data = self.workspace.read(cx).data_dir.clone();
        let page = cx.entity().downgrade();
        let form = cx.new(|cx| NewWorktreeForm::new(page, repo.clone(), base, data, self.tools.clone(), window, cx));
        let f = form.clone();
        window.open_dialog(cx, move |d, _, _| d.title(format!("New Worktree of {}", repo.name())).w(px(520.)).child(f.clone()));
        if std::env::var("YH_PROJECT_PROBE_ANSWER").as_deref() == Ok("ok") {
            // The probe clicks Create (with `YH_NEW_WORKTREE_BRANCH` typed).
            form.update(cx, |f, cx| f.create(window, cx));
        }
    }

    fn open_new_stack(&mut self, repo: WorkspaceRepo, window: &mut Window, cx: &mut Context<Self>) {
        let Some(core) = &self.core else { return };
        let page = cx.entity().downgrade();
        let core = core.clone();
        let installed = self.stacks_installed;
        let form = cx.new(|cx| NewStackForm::new(page, repo.clone(), core, self.tools.clone(), installed, window, cx));
        let f = form.clone();
        window.open_dialog(cx, move |d, _, _| d.title(format!("New Stack in {}", repo.name())).w(px(620.)).child(f.clone()));
        // `YH_NEW_STACK_BRANCHES=<a:b>` picks them, bottom first, Submit off; with
        // `YH_PROJECT_PROBE_ANSWER=ok` the probe clicks Create Stack.
        if let Ok(branches) = std::env::var("YH_NEW_STACK_BRANCHES") {
            form.update(cx, |f, cx| {
                f.chosen = branches.split(':').map(str::to_string).collect();
                f.submit = false;
                if std::env::var("YH_PROJECT_PROBE_ANSWER").as_deref() == Ok("ok") {
                    f.create(window, cx);
                }
            });
        }
    }

    /// `YH_PROJECT_PROBE`, once the repos have been read: `remove:<worktree folder name>` asks to
    /// remove it as its button does, `merge` (or `merge:<up to #>`) the first numbered stack,
    /// `new-worktree` and `new-stack` open their sheets for the first repo, `add-branch` the
    /// first stack's Add Branch on Top, `expand` unfolds every repo's worktrees, `repos` scrolls down to the repos; several with commas.
    /// `YH_PROJECT_PROBE_ANSWER=ok` then says yes.
    fn run_probe(&mut self, probe: String, window: &mut Window, cx: &mut Context<Self>) {
        if probe.contains(',') {
            for p in probe.split(',') {
                self.run_probe(p.to_string(), window, cx);
            }
            return;
        }
        let Some(core) = self.core.clone() else { return };
        let first_repo = core.cloned_repos().first().map(|r| (*r).clone());
        eprintln!("PROJECT probe {probe}");
        let (verb, arg) = probe.split_once(':').map_or((probe.as_str(), None), |(a, b)| (a, Some(b)));
        match verb {
            "remove" => {
                let target = core.cloned_repos().into_iter().find_map(|r| {
                    core.worktrees_of(r).iter().find(|w| !w.is_main && Path::new(&w.path).file_name().is_some_and(|n| Some(n.to_string_lossy().as_ref()) == arg)).map(|w| (w.clone(), r.clone()))
                });
                match target {
                    Some((wt, repo)) => self.request_removal(wt, repo, window, cx),
                    None => eprintln!("PROJECT probe: no worktree named {arg:?}"),
                }
            }
            "merge" => {
                let sections = core.pull_request_sections();
                let stack = sections.needs_review.iter().chain(&sections.mine).chain(&sections.others).find(|s| s.number().is_some()).cloned();
                match stack.and_then(|s| self.merge_request(&s, arg.and_then(|a| a.parse().ok()))) {
                    Some(r) => self.request_merge(r, window, cx),
                    None => eprintln!("PROJECT probe: no stack to merge"),
                }
            }
            "new-worktree" => {
                if let Some(r) = first_repo {
                    self.open_new_worktree(r, window, cx);
                }
            }
            "new-stack" => {
                if let Some(r) = first_repo {
                    self.open_new_stack(r, window, cx);
                }
            }
            // Every repo's worktrees shown.
            "expand" => {
                for r in core.cloned_repos() {
                    if core.worktrees_of(r).iter().filter(|w| !w.is_main).count() > 3 {
                        self.toggled_repos.insert(r.id().to_string());
                    }
                }
                cx.notify();
            }
            // Scrolled to the repos, for a snapshot of them.
            "repos" => {
                self.scroll.scroll_to_bottom();
                cx.notify();
            }
            "add-branch" => self.ask_branch_on_top(core.root.clone(), window, cx),
            _ => eprintln!("PROJECT probe: unknown {probe}"),
        }
    }

    /// The merge a stack's button asks for: its open layers up to the first draft, up to
    /// `up_to` when given.
    fn merge_request(&self, stack: &PullRequestStack, up_to: Option<u64>) -> Option<MergeRequest> {
        let core = self.core.as_ref()?;
        let number = stack.number()?;
        let open: Vec<&PullRequest> = stack.all().into_iter().filter(|p| p.is_open()).collect();
        let mergeable: Vec<&PullRequest> = open.iter().take_while(|p| !p.draft).copied().collect();
        if mergeable.is_empty() {
            return None;
        }
        let end = up_to.and_then(|n| mergeable.iter().position(|p| p.number() == n)).unwrap_or(mergeable.len() - 1);
        let pulls: Vec<&PullRequest> = mergeable[..=end].to_vec();
        let whole = pulls.len() == open.len();
        Some(MergeRequest {
            number,
            pulls: pulls.iter().map(|p| (p.number(), p.title.clone())).collect(),
            base: pulls[0].base.clone(),
            up_to: if whole { None } else { Some(pulls[pulls.len() - 1].number()) },
            repo: core.repo_for(&stack.pull_request)?.path.clone(),
        })
    }
}

// MARK: Drawing

fn chip(label: &str, color: Hsla) -> gpui_kit::Div {
    page_kit::chip(label.to_string(), color)
}

/// A coloured dot and words: a worktree's or repo's state.
fn state_badge(label: impl Into<SharedString>, color: Hsla, theme: &Theme) -> gpui_kit::Div {
    div().flex_none().flex().items_center().gap(px(5.)).text_size(px(text::CAPTION)).font_weight(FontWeight::MEDIUM).text_color(theme.secondary).child(page_kit::dot(color, 6.)).child(label.into())
}

/// A PR's check or review state: an icon in its colour, the words quiet beside it.
fn status(icon: gpui_kit::component::Icon, color: Hsla, label: &str, theme: &Theme) -> gpui_kit::Div {
    div().flex_none().flex().items_center().gap(px(3.)).text_color(theme.secondary).child(icon.size(px(11.)).text_color(color)).child(label.to_string())
}

fn age(updated: Option<&str>) -> String {
    let Some(t) = updated.and_then(yh_core::library::parse_iso) else { return String::new() };
    // parse_iso gives seconds; ago takes milliseconds.
    crate::state::thread_tools::ago(t * 1000.)
}

fn section_title(title: &str, count: usize, theme: &Theme) -> gpui_kit::Div {
    div()
        .flex()
        .items_baseline()
        .gap(px(6.))
        .child(div().text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).text_color(theme.text).child(title.to_string()))
        .when(count > 0, |d| d.child(div().text_size(px(text::FOOTNOTE)).text_color(theme.tertiary).child(count.to_string())))
}

fn group_label(title: &str, count: usize, theme: &Theme) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(px(text::FOOTNOTE))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.secondary)
        .child(title.to_string())
        .child(div().font_weight(FontWeight::NORMAL).text_color(theme.tertiary).child(count.to_string()))
}

/// Ahead and behind its upstream, or that it has none.
fn sync_label(s: &RepoStatus, theme: &Theme) -> Option<gpui_kit::AnyElement> {
    if s.branch.is_some() && s.upstream.is_none() {
        return Some(div().id("local-only").text_size(px(text::CAPTION)).text_color(theme.tertiary).child("Local only").tooltip_text("This branch isn't pushed").into_any_element());
    }
    if s.ahead == 0 && s.behind == 0 {
        return None;
    }
    let mut d = div().flex().gap(px(6.)).text_size(px(text::CAPTION)).font_weight(FontWeight::MEDIUM).text_color(theme.secondary);
    if s.ahead > 0 {
        d = d.child(div().flex().items_center().gap(px(1.)).child(icons::arrow_up().size(px(11.))).child(s.ahead.to_string()));
    }
    if s.behind > 0 {
        d = d.child(div().flex().items_center().gap(px(1.)).child(icons::arrow_down().size(px(11.))).child(s.behind.to_string()));
    }
    let tip = [(s.ahead, "to push"), (s.behind, "to pull (as of the last fetch)")]
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, w)| format!("{n} commit{} {w}", if *n == 1 { "" } else { "s" }))
        .collect::<Vec<_>>()
        .join(", ");
    Some(div().id("sync").child(d).tooltip_text(tip).into_any_element())
}

/// Conflicts in red, other changes in orange.
fn changes_label(s: &RepoStatus, theme: &Theme) -> Option<gpui_kit::AnyElement> {
    if s.conflicted > 0 {
        return Some(state_badge(format!("{} conflicted", s.conflicted), theme.error, theme).into_any_element());
    }
    if !s.is_dirty() {
        return None;
    }
    let detail = [(s.staged, "staged"), (s.modified, "modified"), (s.untracked, "untracked")].iter().filter(|(n, _)| *n > 0).map(|(n, w)| format!("{n} {w}")).collect::<Vec<_>>().join(", ");
    Some(div().id("changes").child(state_badge(format!("{} changed", s.changed_files()), orange(), theme)).tooltip_text(detail).into_any_element())
}

trait TooltipText {
    fn tooltip_text(self, text: impl Into<SharedString>) -> Self;
}

impl TooltipText for gpui_kit::Stateful<gpui_kit::Div> {
    fn tooltip_text(self, text: impl Into<SharedString>) -> Self {
        let text: SharedString = text.into();
        self.tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(text.clone()).build(window, cx))
    }
}

impl ProjectPage {
    fn header(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = self.workspace.read(cx);
        let name = ws.project(&self.project).map(|p| p.name.clone()).unwrap_or_default();
        let path = ws.project(&self.project).map(|p| p.path.clone()).unwrap_or_default();
        let subtitle = match self.core.as_ref().and_then(|c| c.layout.as_ref().map(|l| (c, l))) {
            None => "Looking for repositories…".to_string(),
            Some((_, l)) if l.kind == LayoutKind::Plain => "Folder".to_string(),
            Some((c, l)) => {
                let mut parts = if l.kind == LayoutKind::Repo {
                    vec![format!("Git repository{}", c.status.statuses.get("").and_then(|s| s.branch.clone()).map(|b| format!(" · {b}")).unwrap_or_default())]
                } else {
                    let (cloned, uncloned) = (c.cloned_repos().len(), c.uncloned_repos().len());
                    let mut p = vec!["Workspace".to_string(), if cloned == 1 { "1 repo".into() } else { format!("{cloned} repos") }];
                    if uncloned > 0 {
                        p.push(format!("{uncloned} not cloned"));
                    }
                    p
                };
                // "3 worktrees", and what needs a look: not on a branch, done with, missing.
                let all: Vec<(&WorkspaceRepo, &Worktree)> = c.cloned_repos().into_iter().flat_map(|r| c.worktrees_of(r).iter().filter(|w| !w.is_main).map(move |w| (r, w))).collect();
                if !all.is_empty() {
                    parts.push(if all.len() == 1 { "1 worktree".into() } else { format!("{} worktrees", all.len()) });
                    let detached = all.iter().filter(|(_, w)| w.is_detached() && !w.is_missing).count();
                    let finished = all.iter().filter(|(r, w)| c.is_finished(w, r)).count();
                    let missing = all.iter().filter(|(_, w)| w.is_missing).count();
                    for (n, what) in [(detached, "not on a branch"), (finished, "done with"), (missing, "missing")] {
                        if n > 0 {
                            parts.push(format!("{n} {what}"));
                        }
                    }
                }
                parts.join(" · ")
            }
        };
        let updated = self.core.as_ref().and_then(|c| c.pulls_updated_at.or(c.status_updated_at)).map(|t| {
            let secs = t.elapsed().as_secs();
            if secs < 60 { "Updated just now".to_string() } else { format!("Updated {} min ago", secs / 60) }
        });
        let (ws1, project) = (self.workspace.clone(), self.project.clone());
        let (p1, p2) = (path.clone(), path.clone());
        let workspace_kind = self.core.as_ref().and_then(|c| c.layout.as_ref()).is_some_and(|l| l.is_workspace());
        let tabs = page_kit::segmented(
            vec![
                Tab::new("tab-overview", "Overview", true, |_, _, _| {}).icon(gpui_kit::component::Icon::new(IconName::LayoutDashboard)),
                Tab::new("tab-memory", "Memory", false, move |_, _, cx| ws1.update(cx, |ws, cx| ws.open_memory(&project, cx))).icon(icons::brain()),
            ],
            theme,
        )
        .into_any_element();
        let toolbar = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .when_some(self.activity.clone(), |d, a| d.child(div().mr(px(space::S)).max_w(px(300.)).text_size(px(text::FOOTNOTE)).text_color(theme.secondary).line_clamp(2).text_ellipsis().child(a)))
            .when_some(updated.filter(|_| !self.compact), |d, u| d.child(div().mr(px(space::XS)).flex_none().text_size(px(text::CAPTION)).text_color(theme.tertiary).child(u)))
            .child(
                icon_button("project-refresh", Icon::new(IconName::RefreshCw), "Refresh repos and pull requests", theme)
                    .loading(self.refreshing)
                    .disabled(self.refreshing)
                    .on_click(cx.listener(|p, _, _, cx| p.refresh(true, cx))),
            )
            .child(icon_button("project-finder", Icon::new(IconName::FolderOpen), "Show in Finder", theme).on_click(move |_, _, cx| cx.reveal_path(Path::new(&p2))))
            .child(div().ml(px(space::XS)).child(Button::new("project-new-thread").small().primary().label("New Thread").icon(IconName::Plus).on_click(cx.listener(|p, _, _, cx| p.new_thread_in(None, cx)))))
            .into_any_element();
        // A stack for a workspace of repos, a folder otherwise.
        let icon = if workspace_kind { crate::ui::icons::layers() } else { gpui_kit::component::Icon::new(IconName::Folder) };
        div()
            .id("project-header")
            .context_menu(move |menu, _, _| {
                let p = p1.clone();
                menu.item(PopupMenuItem::new("Show in Finder").icon(IconName::Folder).on_click(move |_, _, cx| cx.reveal_path(Path::new(&p))))
            })
            .child(page_kit::page_header(icon, name, div().truncate().child(subtitle).into_any_element(), toolbar, Some(tabs), theme))
    }

    /// One repo (or none read yet): PR rows needn't say which repo.
    fn single_repo(&self) -> bool {
        self.core.as_ref().is_none_or(|c| c.repos().len() <= 1)
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
            "approved" => Some((icons::circle_check(), "Approved", green())),
            "changesRequested" => Some((gpui_kit::component::Icon::new(IconName::CircleX), "Changes requested", theme.error)),
            "required" => Some((gpui_kit::component::Icon::new(IconName::Eye), "Review required", theme.tertiary)),
            _ => None,
        };
        let checks = match pr.checks.as_str() {
            "passing" => Some((icons::check(), "Checks pass", green())),
            "failing" => Some((gpui_kit::component::Icon::new(IconName::CircleX), "Checks fail", theme.error)),
            "pending" => Some((icons::clock(), "Checks running", theme.warning)),
            _ => None,
        };
        let blocker = match pr.merge_status.as_deref().filter(|_| pr.state == "open") {
            Some("DIRTY") => Some("Conflicts"),
            Some("BEHIND") => Some("Behind base"),
            Some("BLOCKED") => Some("Blocked"),
            _ => None,
        };
        let (u1, u2, head) = (url.clone(), url.clone(), pr.head.clone());
        div()
            .id(SharedString::from(format!("pr-{}", pr.link.id())))
            .flex()
            .items_stretch()
            .min_h(px(48.))
            .px(px(space::S + 2.))
            .cursor_pointer()
            .hover(|d| d.bg(theme.hover))
            .on_click(move |_, _, cx| cx.open_url(&url))
            .context_menu(move |menu, _, _| {
                let (u1, u2, head) = (u1.clone(), u2.clone(), head.clone());
                menu.item(PopupMenuItem::new("Open in Browser").icon(IconName::ExternalLink).on_click(move |_, _, cx| cx.open_url(&u1)))
                    .item(PopupMenuItem::new("Copy Link").icon(IconName::Copy).on_click(move |_, _, cx| copy(u2.clone(), cx)))
                    .item(PopupMenuItem::new("Copy Branch Name").on_click(move |_, _, cx| copy(head.clone(), cx)))
            })
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
                    .child(div().flex().items_center().gap(px(6.)).child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().font_weight(FontWeight::MEDIUM).text_color(theme.text).child(pr.title.clone())).when_some(state, |d, s| d.child(chip(s, dot))))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S + 2.))
                            .text_size(px(text::CAPTION))
                            .text_color(theme.secondary)
                            .child(div().flex_none().child(if self.single_repo() {
                                format!("#{}", pr.link.number)
                            } else {
                                format!("{} #{}", pr.link.path.rsplit('/').next().unwrap_or(&pr.link.path), pr.link.number)
                            }))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap(px(3.))
                                    .child(icons::git_branch().size(px(11.)).text_color(theme.tertiary))
                                    .child(div().min_w_0().font_family(MONO).overflow_hidden().whitespace_nowrap().text_ellipsis().child(format!("{} → {}", pr.head, pr.base))),
                            )
                            .when_some(pr.author.clone().filter(|_| !pr.is_mine), |d, a| d.child(div().flex_none().child(a)))
                            .when_some(blocker, |d, b| d.child(status(gpui_kit::component::Icon::new(IconName::TriangleAlert), theme.warning, b, theme)))
                            .when_some(review, |d, (i, t, c)| d.child(status(i, c, t, theme)))
                            .when_some(checks, |d, (i, t, c)| d.child(status(i, c, t, theme)))
                            .child(div().flex_1())
                            .when(!self.narrow, |d| d.child(div().flex_none().text_color(theme.tertiary).child(age(pr.updated_at.as_deref())))),
                    ),
            )
            .into_any_element()
    }

    /// Above a stack's PRs: which it is, how far along, and what can be done with it.
    fn stack_header(&self, stack: &PullRequestStack, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(core) = &self.core else { return div().into_any_element() };
        let top = &stack.pull_request;
        let all = stack.all();
        let open: Vec<&PullRequest> = all.iter().filter(|p| p.is_open()).copied().collect();
        let number = stack.number();
        let clone = core.repo_for(top).cloned();
        let github = top.remote().forge == Some(Forge::GitHub);
        let title = match number {
            None => format!("Stack of {}", all.len()),
            Some(n) => {
                let size = top.stack.as_ref().map_or(0, |s| s.size as usize).max(all.len());
                let merged = all.iter().filter(|p| p.state == "merged").count();
                if merged > 0 {
                    format!("Stack #{n} · {merged} of {size} merged")
                } else if open.len() < size {
                    format!("Stack #{n} · {} of {size} open", open.len())
                } else {
                    format!("Stack #{n} · {size} PRs")
                }
            }
        };
        let sid = stack.id();
        let mut actions = div().flex().items_center().gap(px(6.));
        let not_installed = self.stacks_installed == Some(false);
        if (number.is_some() || (github && !open.is_empty())) && not_installed {
            actions = actions.child(self.install_button(cx));
        } else if let Some(n) = number {
            // Branches gh stack has locally with no PR yet: Submit opens them.
            let local = core.local_stack(n, &top.remote()).cloned();
            if let (Some(local), Some(repo)) = (&local, &clone) {
                let pending = local.layers.iter().filter(|l| l.pull_request_number.is_none()).count();
                if pending > 0 {
                    let branches: Vec<String> = local.layers.iter().map(|l| l.branch.clone()).collect();
                    let checkout = core.checkout_on_any_of(repo, &branches);
                    let tip = if checkout.is_none() {
                        "Check out one of the stack's branches to submit it".to_string()
                    } else if pending == 1 {
                        "Push the stack and open a PR for its new branch".to_string()
                    } else {
                        format!("Push the stack and open PRs for its {pending} new branches")
                    };
                    actions = actions.child(Button::new(SharedString::from(format!("submit-{sid}"))).small().outline().icon(icons::upload()).label("Submit").disabled(checkout.is_none()).tooltip(tip).on_click(cx.listener(move |p, _, window, cx| {
                        if let Some(c) = checkout.clone() {
                            p.submit_stack(c, window, cx);
                        }
                    })));
                }
            }
            actions = actions.child(self.merge_control(stack, &open, clone.as_ref(), cx));
            // More: Add Branch on Top (in the checkout on the top branch), Sync with GitHub.
            let branches: Vec<String> = local.as_ref().map(|l| l.layers.iter().map(|l| l.branch.clone()).collect()).unwrap_or_else(|| all.iter().map(|p| p.head.clone()).collect());
            let top_checkout = clone.as_ref().and_then(|r| branches.last().and_then(|b| core.checkout_on_any_of(r, std::slice::from_ref(b))));
            let any_checkout = clone.as_ref().and_then(|r| core.checkout_on_any_of(r, &all.iter().map(|p| p.head.clone()).collect::<Vec<_>>()));
            let page = cx.entity().downgrade();
            actions = actions.child(icon_button(SharedString::from(format!("more-{sid}")), Icon::new(IconName::Ellipsis), "More for this stack", theme).dropdown_menu(move |menu, _, _| {
                let (p1, p2) = (page.clone(), page.clone());
                let (t, a) = (top_checkout.clone(), any_checkout.clone());
                let (t2, a2) = (t.clone(), a.clone());
                let menu = menu
                    .item(PopupMenuItem::new("Add Branch on Top…").disabled(t.is_none()).on_click(move |_, window, cx| {
                        if let Some(c) = t2.clone() {
                            let _ = p1.update(cx, |p, cx| p.ask_branch_on_top(c, window, cx));
                        }
                    }))
                    .item(PopupMenuItem::new("Sync with GitHub").icon(IconName::RefreshCw).disabled(a.is_none()).on_click(move |_, window, cx| {
                        if let Some(c) = a2.clone() {
                            let _ = p2.update(cx, |p, cx| p.sync_stack(c, window, cx));
                        }
                    }));
                if t.is_none() || a.is_none() {
                    menu.separator().item(crate::ui::page_kit::menu_note("Check out one of the stack's branches (its top one, to add) to use these"))
                } else {
                    menu
                }
            }));
        } else if github && !open.is_empty() {
            // A stack only its branches know about: one click makes it one of GitHub's.
            let tip = if !stack.is_line() {
                "GitHub stacks are one line of PRs; this one forks".to_string()
            } else if clone.is_none() {
                format!("This runs gh stack in a clone of {}; there isn't one in this project", top.remote().name())
            } else {
                "Make these PRs one of GitHub's stacked pull requests, bottom up, so they merge as a stack. Branches and PRs stay as they are.".to_string()
            };
            let urls: Vec<String> = all.iter().map(|p| p.link.url.clone()).collect();
            let repo = clone.as_ref().map(|r| r.path.clone());
            actions = actions.child(Button::new(SharedString::from(format!("link-{sid}"))).small().outline().icon(icons::layers()).label("Make GitHub Stack").disabled(!stack.is_line() || repo.is_none()).tooltip(tip).on_click(cx.listener(move |p, _, window, cx| {
                if let Some(r) = repo.clone() {
                    p.link_stack(urls.clone(), r, window, cx);
                }
            })));
        }
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .pl(px(space::M + 2.))
            .pr(px(space::S + 2.))
            .py(px(space::S))
            .border_b_1()
            .border_color(theme.separator)
            .child(icons::layers().size(px(13.)).text_color(theme.secondary))
            .child(div().flex_1().min_w_0().text_size(px(text::FOOTNOTE)).font_weight(FontWeight::SEMIBOLD).text_color(theme.secondary).truncate().child(title))
            .child(actions)
            .into_any_element()
    }

    fn install_button(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        Button::new("install-gh-stack")
            .xsmall()
            .label("Install gh stack")
            .loading(self.installing)
            .disabled(self.installing || self.tools.stacks.is_none())
            .tooltip(format!("Stacks are merged and submitted with GitHub's gh stack extension: {}", yh_core::github::STACK_INSTALL_COMMAND))
            .on_click(cx.listener(|p, _, window, cx| p.install_stacks(window, cx)))
            .into_any_element()
    }

    /// Merge, Merge #N, or Merge Stack with Merge Up to #N in its menu; disabled with why.
    fn merge_control(&self, stack: &PullRequestStack, open: &[&PullRequest], clone: Option<&WorkspaceRepo>, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let sid = stack.id();
        if open.is_empty() {
            return div().into_any_element();
        }
        let mergeable: Vec<&PullRequest> = open.iter().take_while(|p| !p.draft).copied().collect();
        let disabled = |tip: String| Button::new(SharedString::from(format!("merge-{sid}"))).small().outline().icon(icons::git_merge()).label("Merge").disabled(true).tooltip(tip).into_any_element();
        if clone.is_none() {
            return disabled(format!("Merging runs gh stack in a clone of {}; there isn't one in this project", stack.pull_request.remote().name()));
        }
        if mergeable.is_empty() {
            return disabled("The bottom PR is a draft: GitHub won't merge it".into());
        }
        let whole = self.merge_request(stack, None);
        if mergeable.len() == 1 {
            let label = if open.len() == 1 { "Merge".to_string() } else { format!("Merge #{}", mergeable[0].number()) };
            return Button::new(SharedString::from(format!("merge-{sid}")))
                .small()
                .outline()
                .icon(icons::git_merge())
                .label(label)
                .tooltip("Squash and merge on GitHub")
                .on_click(cx.listener(move |p, _, window, cx| {
                    if let Some(r) = whole.clone() {
                        p.request_merge(r, window, cx);
                    }
                }))
                .into_any_element();
        }
        let label = if mergeable.len() == open.len() { "Merge Stack".to_string() } else { format!("Merge {}", mergeable.len()) };
        let tip = if mergeable.len() == open.len() { "Squash and merge the whole stack on GitHub, bottom up" } else { "Squash and merge the layers below the first draft" };
        let partial: Vec<(u64, Option<MergeRequest>)> = mergeable[..mergeable.len() - 1].iter().map(|p| (p.number(), self.merge_request(stack, Some(p.number())))).collect();
        let page = cx.entity().downgrade();
        div()
            .flex()
            .child(Button::new(SharedString::from(format!("merge-{sid}"))).small().outline().icon(icons::git_merge()).label(label).tooltip(tip).on_click(cx.listener(move |p, _, window, cx| {
                if let Some(r) = whole.clone() {
                    p.request_merge(r, window, cx);
                }
            })))
            .child(Button::new(SharedString::from(format!("merge-menu-{sid}"))).small().ghost().icon(IconName::ChevronDown).tooltip("Merge part of the stack").dropdown_menu(move |mut menu: PopupMenu, _, _| {
                for (n, r) in &partial {
                    let (page, r) = (page.clone(), r.clone());
                    menu = menu.item(PopupMenuItem::new(format!("Merge Up to #{n}")).on_click(move |_, window, cx| {
                        if let Some(r) = r.clone() {
                            let _ = page.update(cx, |p, cx| p.request_merge(r, window, cx));
                        }
                    }));
                }
                menu
            }))
            .into_any_element()
    }

    /// One card a stack (a lone PR is a stack of one), drawn as a staircase.
    fn stack_card(&self, stack: &PullRequestStack, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let graph = StackGraph::layout(stack, Lanes::Depth);
        let is_stack = stack.is_stacked() || stack.number().is_some();
        let mut card = div().flex().flex_col().pb(px(2.)).rounded(px(radius::LARGE)).overflow_hidden().bg(theme.card).border_1().border_color(theme.separator);
        if is_stack {
            card = card.child(self.stack_header(stack, theme, cx));
        }
        for row in &graph.rows {
            card = card.child(self.pull_row(&row.pull_request, is_stack.then_some((row, graph.lane_count)), theme));
        }
        // GitHub stack branches gh stack has locally, no PR yet: on top of the PRs.
        if let (Some(core), Some(n)) = (&self.core, stack.number()) {
            if let Some(local) = core.local_stack(n, &stack.pull_request.remote()) {
                let below = graph.rows.len();
                for (i, l) in local.layers.iter().enumerate().filter(|(_, l)| l.pull_request_number.is_none()) {
                    card = card.child(unsubmitted_row(&l.branch, (i + 1).max(below + 1), theme));
                }
            }
        }
        card.into_any_element()
    }

    fn pulls_section(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(core) = &self.core else { return div().into_any_element() };
        if !core.has_forge_remotes() && core.pulls.errors.is_empty() {
            return div().into_any_element();
        }
        let sections = core.pull_request_sections();
        let mut column = div().flex().flex_col().gap(px(8.)).child(section_title("Pull Requests", core.pull_requests().len(), theme));
        // Repos the signed-in account can't see, so their PRs are missing from the list.
        let unreachable = core.pulls.unreachable.len();
        let forge_remotes: HashSet<_> = core.remotes.values().filter(|r| r.forge.is_some()).collect();
        if unreachable > 0 {
            let total = forge_remotes.len();
            let forges: HashSet<_> = core.pulls.unreachable.iter().filter_map(|r| r.forge).collect();
            let who = if forges.len() == 1 { forges.iter().next().and_then(|f| core.pulls.accounts.get(f)).map(|a| format!("@{a}")) } else { None }.unwrap_or_else(|| "your account".into());
            let repos = if unreachable == total {
                if unreachable == 1 { "This repo isn't".to_string() } else { "These repos aren't".to_string() }
            } else {
                format!("{unreachable} of {total} repos aren't")
            };
            column = column.child(div().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(format!("{repos} visible to {who}, so their pull requests can't be shown.")));
        }
        for (forge, e) in &core.pulls.errors {
            let install = e.starts_with("Install").then(|| forge.install_url().to_string());
            column = column.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .p(px(12.))
                    .rounded(px(radius::LARGE))
                    .bg(orange().opacity(0.08))
                    .border_1()
                    .border_color(orange().opacity(0.3))
                    .child(gpui_kit::component::Icon::new(IconName::TriangleAlert).text_color(orange()))
                    .child(div().flex_1().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(e.clone()))
                    .when_some(install, |d, url| d.child(Button::new(SharedString::from(format!("get-{}", forge.cli_name()))).xsmall().ghost().label(format!("Get {}", forge.cli_name())).on_click(move |_, _, cx| cx.open_url(&url)))),
            );
        }
        let empty = sections.needs_review.is_empty() && sections.mine.is_empty() && sections.others.is_empty();
        if empty && core.pulls_updated_at.is_some() && core.pulls.errors.is_empty() && unreachable < forge_remotes.len().max(1) {
            column = column.child(div().text_color(theme.secondary).child("No open pull requests."));
        } else if empty && self.refreshing {
            column = column.child(div().text_color(theme.secondary).child("Loading pull requests…"));
        }
        // Stacks made with gh stack, not on GitHub yet.
        let unsubmitted: Vec<_> = core.unsubmitted_stacks().into_iter().filter(|(_, s)| s.number.is_none()).collect();
        if !unsubmitted.is_empty() {
            column = column.child(div().pt(px(6.)).child(group_label("Not Submitted", unsubmitted.len(), theme)));
        }
        for (repo, local) in unsubmitted {
            let branches: Vec<String> = local.layers.iter().map(|l| l.branch.clone()).collect();
            let checkout = core.checkout_on_any_of(repo, &branches);
            let sid = format!("local:{}:{}", repo.id(), local.id());
            let action = if self.stacks_installed == Some(false) {
                self.install_button(cx)
            } else {
                let tip = if checkout.is_none() { "Check out one of the stack's branches to submit it".to_string() } else { format!("Push its branches and open a PR for each, into {}", local.trunk) };
                Button::new(SharedString::from(format!("submit-{sid}")))
                    .small()
                    .outline()
                    .icon(icons::upload())
                    .label("Submit")
                    .disabled(checkout.is_none())
                    .tooltip(tip)
                    .on_click(cx.listener(move |p, _, window, cx| {
                        if let Some(c) = checkout.clone() {
                            p.submit_stack(c, window, cx);
                        }
                    }))
                    .into_any_element()
            };
            let mut card = div().flex().flex_col().pb(px(3.)).rounded(px(radius::LARGE)).overflow_hidden().bg(theme.card).border_1().border_color(theme.separator).child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(12.))
                    .pt(px(8.))
                    .pb(px(2.))
                    .child(div().flex_1().text_size(px(text::CAPTION)).font_weight(FontWeight::SEMIBOLD).text_color(theme.secondary).child(format!("{} · stack of {}, not submitted", repo.name(), local.layers.len())))
                    .child(action),
            );
            for (i, l) in local.layers.iter().enumerate() {
                card = card.child(unsubmitted_row(&l.branch, i + 1, theme));
            }
            column = column.child(card);
        }
        for (title, stacks) in [("Needs Your Review", &sections.needs_review), ("Yours", &sections.mine)] {
            if stacks.is_empty() {
                continue;
            }
            column = column.child(div().pt(px(6.)).child(group_label(title, stacks.iter().map(|s| s.all().len()).sum(), theme)));
            for s in stacks.iter() {
                column = column.child(self.stack_card(s, theme, cx));
            }
        }
        if !sections.others.is_empty() {
            // Busy repos have plenty of PRs that aren't yours: out of the way unless there's nothing else.
            let expanded = self.show_others || (sections.needs_review.is_empty() && sections.mine.is_empty());
            column = column.child(
                div()
                    .id("others-toggle")
                    .pt(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .cursor_pointer()
                    .on_click(cx.listener(|p, _, _, cx| {
                        p.show_others = !p.show_others;
                        cx.notify();
                    }))
                    .child(gpui_kit::component::Icon::new(if expanded { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(12.)).text_color(theme.secondary))
                    .child(group_label("Everyone Else", sections.others.iter().map(|s| s.all().len()).sum(), theme)),
            );
            if expanded {
                for s in sections.others.iter() {
                    column = column.child(self.stack_card(s, theme, cx));
                }
            }
        }
        column.child(div().h(px(16.))).into_any_element()
    }

    /// The threads working in a checkout, as one pill: the title of a single active one, else
    /// how many are active and settled. Click for the list, and open one from it.
    fn thread_chips(&self, key: &str, checkout: &str, theme: &Theme, cx: &App) -> Option<gpui_kit::AnyElement> {
        let ids = self.threads_here.get(checkout)?;
        let ws = self.workspace.read(cx);
        let mut active = vec![];
        let mut settled = vec![];
        for id in ids {
            let Some(t) = ws.thread(id) else { continue };
            let t = t.read(cx);
            let row = (id.clone(), t.title.clone(), t.running);
            if t.meta.settled_at.is_some() { settled.push(row) } else { active.push(row) }
        }
        if active.is_empty() && settled.is_empty() {
            return None;
        }
        let only_settled = active.is_empty();
        let label = if active.len() == 1 && settled.is_empty() {
            active[0].1.chars().take(28).collect::<String>()
        } else {
            [(active.len(), "active"), (settled.len(), "settled")].iter().filter(|(n, _)| *n > 0).map(|(n, w)| format!("{n} {w}")).collect::<Vec<_>>().join(" · ")
        };
        let working = active.iter().any(|(_, _, r)| *r);
        let tip = [(active.len(), "active thread"), (settled.len(), "settled")]
            .iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, w)| if *w == "settled" { format!("{n} settled") } else { format!("{n} {w}{}", if *n == 1 { "" } else { "s" }) })
            .collect::<Vec<_>>()
            .join(", ")
            + " work here. Click to see them.";
        let ws = self.workspace.downgrade();
        let accent = theme.accent;
        Some(
            Button::new(SharedString::from(format!("threads-{key}")))
                .xsmall()
                .ghost()
                .label(label)
                .when(working, |b| b.child(div().w(px(6.)).h(px(6.)).rounded_full().bg(accent)))
                .when(only_settled, |b| b.opacity(0.6))
                .tooltip(tip)
                .dropdown_menu(move |mut menu, _, _| {
                    for (title, rows) in [("Active", &active), ("Settled", &settled)] {
                        if rows.is_empty() {
                            continue;
                        }
                        menu = menu.label(title);
                        for (id, name, running) in rows.iter() {
                            let (ws, id) = (ws.clone(), id.clone());
                            let text = if *running { format!("{name} (working)") } else { name.clone() };
                            menu = menu.item(PopupMenuItem::new(text).on_click(move |_, _, cx| {
                                let _ = ws.update(cx, |ws, cx| ws.select(&id, cx));
                            }));
                        }
                    }
                    menu
                })
                .into_any_element(),
        )
    }

    fn repos_section(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(core) = &self.core else { return div().into_any_element() };
        let mut repos: Vec<&WorkspaceRepo> = core.cloned_repos();
        // Most used first (by threads working in them), then by name; a shell's own repo on top.
        let shell = core.layout.as_ref().is_some_and(|l| l.kind == LayoutKind::Shell);
        let usage = |r: &WorkspaceRepo| self.threads_here.get(&r.real_path()).map_or(0, Vec::len) + core.worktrees_of(r).iter().map(|w| self.threads_here.get(&w.path).map_or(0, Vec::len)).sum::<usize>();
        repos.sort_by_key(|r| (!(shell && r.is_root()), std::cmp::Reverse(usage(r)), r.name().to_lowercase()));
        let total_worktrees: usize = repos.iter().map(|r| core.worktrees_of(r).iter().filter(|w| !w.is_main).count()).sum();
        let filterable = repos.len() > 6 || total_worktrees > 8;
        let words = if filterable { self.filter_words(cx) } else { String::new() };
        let shown: Vec<&WorkspaceRepo> = repos
            .iter()
            .copied()
            .filter(|r| {
                words.is_empty()
                    || r.name().to_lowercase().contains(&words)
                    || self.threads_here.get(&r.real_path()).is_some_and(|ids| ids.iter().any(|id| self.workspace.read(cx).thread(id).is_some_and(|t| t.read(cx).title.to_lowercase().contains(&words))))
                    || core.worktrees_of(r).iter().any(|w| !w.is_main && self.worktree_matches(w, &words, cx))
            })
            .collect();
        // Expand All / Collapse All: every repo with worktrees, open or folded.
        let with_worktrees: Vec<(String, usize)> = repos.iter().map(|r| (r.id().to_string(), core.worktrees_of(r).iter().filter(|w| !w.is_main).count())).filter(|(_, n)| *n > 0).collect();
        let is_open = |id: &str, n: usize| (n <= 3) != self.toggled_repos.contains(id);
        let any_open = with_worktrees.iter().any(|(id, n)| is_open(id, *n));
        let title_row = div()
            .flex()
            .items_center()
            .gap(px(space::M))
            .child(div().flex_1().child(section_title("Repositories", repos.len(), theme)))
            .when(filterable, |d| d.child(div().w(px(220.)).child(Input::new(&self.filter).small().cleanable(true))))
            .when(!with_worktrees.is_empty(), |d| {
                let list = with_worktrees.clone();
                d.child(
                    Button::new("repos-expand-all")
                        .small()
                        .ghost()
                        .label(if any_open { "Collapse All" } else { "Expand All" })
                        .tooltip(if any_open { "Fold every repo's worktrees" } else { "Show every repo's worktrees" })
                        .on_click(cx.listener(move |p, _, _, cx| {
                            for (id, n) in &list {
                                // Open is (n <= 3) != toggled; make it !any_open for each.
                                let want_open = !any_open;
                                let toggled = (*n <= 3) != want_open;
                                if toggled {
                                    p.toggled_repos.insert(id.clone());
                                } else {
                                    p.toggled_repos.remove(id);
                                }
                            }
                            cx.notify();
                        })),
                )
            });
        let mut column = div().flex().flex_col().gap(px(6.)).child(title_row);
        if shown.is_empty() && !words.is_empty() {
            column = column.child(div().py(px(space::S)).text_color(theme.secondary).child(format!("Nothing matches “{}”.", self.filter.read(cx).value().trim())));
        }
        let page = cx.entity().downgrade();
        for repo in shown {
            column = column.child(self.repo_card(repo, &page, theme, cx));
        }
        let uncloned = core.uncloned_repos();
        if !uncloned.is_empty() {
            let open = self.show_uncloned;
            column = column.child(
                div()
                    .id("uncloned-toggle")
                    .pt(px(14.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .cursor_pointer()
                    .on_click(cx.listener(|p, _, _, cx| {
                        p.show_uncloned = !p.show_uncloned;
                        cx.notify();
                    }))
                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Referenced by this workspace (for example in its compose file) but not on disk yet").build(window, cx))
                    .child(gpui_kit::component::Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(12.)).text_color(theme.secondary))
                    .child(group_label("Not Cloned", uncloned.len(), theme)),
            );
            if open {
                let mut grid = div().flex().flex_wrap().gap(px(6.)).pl(px(18.));
                for r in uncloned {
                    grid = grid.child(div().w(px(190.)).text_size(px(text::FOOTNOTE)).text_color(theme.secondary).truncate().child(r.name()));
                }
                column = column.child(grid);
            }
        }
        column.into_any_element()
    }

    fn repo_card(&self, repo: &WorkspaceRepo, page: &WeakEntity<Self>, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(core) = &self.core else { return div().into_any_element() };
        let id = repo.id().to_string();
        let status = core.status.statuses.get(&id);
        let remote = core.remotes.get(&id).cloned();
        let words = self.filter_words(cx);
        let all_worktrees: Vec<Worktree> = core.worktrees_of(repo).iter().filter(|w| !w.is_main).cloned().collect();
        // Filtering: the repo's name matching shows all of them, else only those that match.
        let worktrees: Vec<Worktree> = if words.is_empty() || repo.name().to_lowercase().contains(&words) { all_worktrees } else { all_worktrees.into_iter().filter(|w| self.worktree_matches(w, &words, cx)).collect() };
        let open = (worktrees.len() <= 3) != self.toggled_repos.contains(&id) || !words.is_empty();
        let prs = core.pull_requests_for(repo).len();
        let unreachable = remote.as_ref().is_some_and(|r| core.pulls.unreachable.contains(r));
        let forge_name = remote.as_ref().map(|r| r.forge.map_or(r.host.clone(), |f| f.display_name().to_string()));
        let web = remote.as_ref().map(|r| r.web_url());
        let needs_look = worktrees.iter().any(|w| w.is_missing || w.is_detached() || core.status.worktree_statuses.get(&w.path).is_some_and(|s| s.is_dirty()));
        let folded_summary = {
            let mut parts = vec![if worktrees.len() == 1 { "1 worktree".to_string() } else { format!("{} worktrees", worktrees.len()) }];
            let changed = worktrees.iter().filter(|w| !w.is_missing && core.status.worktree_statuses.get(&w.path).is_some_and(|s| s.is_dirty())).count();
            let detached = worktrees.iter().filter(|w| w.is_detached() && !w.is_missing).count();
            let finished = worktrees.iter().filter(|w| core.is_finished(w, repo)).count();
            let missing = worktrees.iter().filter(|w| w.is_missing).count();
            for (n, what) in [(changed, "with changes"), (detached, "not on a branch"), (finished, "done with"), (missing, "missing")] {
                if n > 0 {
                    parts.push(format!("{n} {what}"));
                }
            }
            parts.join(" · ") + ". Click to show them."
        };
        let branch_text = match status {
            None => "—".to_string(),
            Some(s) => match (&s.branch, &s.head_oid) {
                (Some(b), _) => b.clone(),
                (None, Some(oid)) => format!("Detached at {}", &oid[..oid.len().min(7)]),
                (None, None) => "No commits yet".into(),
            },
        };
        let (toggle1, toggle2) = (fold_listener(id.clone(), cx), fold_listener(id.clone(), cx));
        let menu = {
            let (page, repo) = (page.clone(), repo.clone());
            let (web, forge_name, branch) = (web.clone(), forge_name.clone(), status.and_then(|s| s.branch.clone()));
            let github = remote.as_ref().and_then(|r| r.forge) == Some(Forge::GitHub);
            let has_missing = worktrees.iter().any(|w| w.is_missing);
            move |menu: PopupMenu, _: &mut Window, _: &mut Context<PopupMenu>| {
                let path = repo.path.to_string_lossy().into_owned();
                let (p1, p2, p3) = (path.clone(), path.clone(), path.clone());
                let mut menu = menu.item(PopupMenuItem::new("Show in Finder").icon(IconName::Folder).on_click(move |_, _, cx| cx.reveal_path(Path::new(&p1))));
                if let (Some(w), Some(f)) = (web.clone(), forge_name.clone()) {
                    menu = menu.item(PopupMenuItem::new(format!("Open on {f}")).icon(IconName::ExternalLink).on_click(move |_, _, cx| cx.open_url(&w)));
                }
                menu = menu
                    .item(PopupMenuItem::new("Open in Terminal").icon(IconName::SquareTerminal).on_click(move |_, _, _| open_in_terminal(&p2)))
                    .item(PopupMenuItem::new("Copy Path").icon(IconName::Copy).on_click(move |_, _, cx| copy(p3.clone(), cx)));
                if let Some(b) = branch.clone() {
                    menu = menu.item(PopupMenuItem::new("Copy Branch Name").on_click(move |_, _, cx| copy(b.clone(), cx)));
                }
                let (pg1, pg2, pg3, pg4) = (page.clone(), page.clone(), page.clone(), page.clone());
                let (r1, r2, r3) = (repo.clone(), repo.clone(), repo.clone());
                menu = menu.separator().item(PopupMenuItem::new("New Worktree…").icon(IconName::Plus).on_click(move |_, window, cx| {
                    let _ = pg1.update(cx, |p, cx| p.open_new_worktree(r1.clone(), window, cx));
                }));
                if github {
                    menu = menu.item(PopupMenuItem::new("New Stack from Branches…").on_click(move |_, window, cx| {
                        let _ = pg2.update(cx, |p, cx| p.open_new_stack(r2.clone(), window, cx));
                    }));
                }
                let here = path.clone();
                menu = menu.item(PopupMenuItem::new("New Thread Here").on_click(move |_, _, cx| {
                    let _ = pg3.update(cx, |p, cx| p.new_thread_in(Some(here.clone()), cx));
                }));
                if has_missing {
                    menu = menu.item(PopupMenuItem::new("Forget Missing Worktrees").icon(icons::trash()).on_click(move |_, window, cx| {
                        let _ = pg4.update(cx, |p, cx| p.prune(r3.clone(), window, cx));
                    }));
                }
                menu
            }
        };
        let threads = self.thread_chips(&id, &repo.real_path(), theme, cx);
        let refreshing = self.refreshing;
        let header = div()
            .id(SharedString::from(format!("repo-{id}")))
            .group("repo-row")
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .py(px(8.))
            .context_menu(menu)
            .child(
                div().flex_none().w(px(14.)).when(!worktrees.is_empty(), |d| {
                    d.child(
                        div()
                            .id(SharedString::from(format!("fold-{id}")))
                            .cursor_pointer()
                            .on_click(toggle1)
                            .tooltip({
                                let t: SharedString = if open { "Hide its worktrees".into() } else { format!("Show its {} worktree{}", worktrees.len(), if worktrees.len() == 1 { "" } else { "s" }).into() };
                                move |window, cx| gpui_kit::component::tooltip::Tooltip::new(t.clone()).build(window, cx)
                            })
                            .child(gpui_kit::component::Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(12.)).text_color(theme.secondary)),
                    )
                }),
            )
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
                            .items_center()
                            .gap(px(6.))
                            .child(icons::folder_git_2().size(px(14.)).text_color(theme.secondary))
                            .child(div().min_w_0().truncate().font_weight(FontWeight::MEDIUM).text_color(theme.text).child(repo.name()))
                            .when(unreachable, |d| {
                                let tip: SharedString = format!("{} couldn't find this repo with your account", forge_name.clone().unwrap_or_else(|| "The host".into())).into();
                                d.child(div().id("unreachable").text_size(px(text::CAPTION)).text_color(theme.tertiary).child("not found").tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)))
                            })
                            .when(!open && !worktrees.is_empty(), |d| {
                                let tip: SharedString = folded_summary.clone().into();
                                d.child(
                                    div()
                                        .id(SharedString::from(format!("wt-count-{id}")))
                                        .flex()
                                        .items_center()
                                        .gap(px(3.))
                                        .px(px(6.))
                                        .rounded(px(8.))
                                        .bg(theme.selected)
                                        .text_size(px(text::CAPTION))
                                        .text_color(theme.secondary)
                                        .cursor_pointer()
                                        .on_click(toggle2)
                                        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                                        .child(format!("{} worktree{}", worktrees.len(), if worktrees.len() == 1 { "" } else { "s" }))
                                        .when(needs_look, |d| d.child(div().w(px(5.)).h(px(5.)).rounded_full().bg(orange()))),
                                )
                            }),
                    )
                    .child(div().text_size(px(text::CAPTION)).font_family(MONO).text_color(theme.secondary).truncate().child(branch_text)),
            )
            .when_some(threads, |d, t| d.child(t))
            .when_some(status.and_then(|s| sync_label(s, theme)), |d, l| d.child(l))
            .when_some(status.and_then(|s| changes_label(s, theme)), |d, l| d.child(l))
            .when(status.is_none() && refreshing, |d| d.child(div().text_size(px(text::CAPTION)).text_color(theme.secondary).child("…")))
            .when(prs > 0, |d| d.child(chip(&format!("{prs} PR{}", if prs == 1 { "" } else { "s" }), theme.secondary)))
            .when_some(web.zip(forge_name), |d, (w, f)| {
                d.child(div().invisible().group_hover("repo-row", |s| s.visible()).child(icon_button(SharedString::from(format!("web-{id}")), Icon::new(IconName::ExternalLink), format!("Open on {f}"), theme).xsmall().on_click(move |_, _, cx| cx.open_url(&w))))
            });
        let mut card = div().flex().flex_col().rounded(px(radius::LARGE)).overflow_hidden().bg(theme.card).border_1().border_color(theme.separator).child(header);
        if open {
            for wt in &worktrees {
                card = card.child(self.worktree_row(wt, repo, page, theme, cx));
            }
        }
        card.into_any_element()
    }

    /// A linked worktree under its repo: where it is, its branch (or that it's on none), what's
    /// in it, its PR, the threads working in it, and Remove (asking first when work's at stake).
    fn worktree_row(&self, wt: &Worktree, repo: &WorkspaceRepo, page: &WeakEntity<Self>, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(core) = &self.core else { return div().into_any_element() };
        let st = core.status.worktree_statuses.get(&wt.path);
        let pr = core.pull_request_for(wt, repo).cloned();
        let risk = core.removal_risk(wt, repo);
        let finished = core.is_finished(wt, repo);
        let no_work = core.has_no_work(wt, repo);
        let base = core.status.default_branches.get(repo.id()).cloned().unwrap_or_else(|| "main".into());
        let base = base.strip_prefix("origin/").unwrap_or(&base).to_string();
        let name = Path::new(&wt.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        // Inside the project: relative; elsewhere: with ~ for your home folder.
        let location = {
            let folder = Path::new(&wt.path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let root = core.root.to_string_lossy().into_owned();
            if folder == root {
                String::new()
            } else if let Some(rest) = folder.strip_prefix(&format!("{root}/")) {
                format!("{rest}/")
            } else {
                format!("{}/", tilde(&folder))
            }
        };
        let branch: gpui_kit::AnyElement = match &wt.branch {
            Some(b) => div().text_size(px(text::CAPTION)).font_family(MONO).text_color(theme.secondary).truncate().child(b.clone()).into_any_element(),
            None => {
                let oid = wt.head_oid.as_deref().map(|o| format!(" · {}", &o[..o.len().min(7)])).unwrap_or_default();
                div()
                    .id(SharedString::from(format!("detached-{}", wt.path)))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .text_size(px(text::CAPTION))
                    .font_family(MONO)
                    .text_color(orange())
                    .child(gpui_kit::component::Icon::new(IconName::TriangleAlert).size(px(10.)))
                    .child(format!("Not on a branch{oid}"))
                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("HEAD is detached: new commits here aren't on any branch, and are easy to lose. Check out or create a branch in it.").build(window, cx))
                    .into_any_element()
            }
        };
        let state: Option<gpui_kit::AnyElement> = if wt.is_missing {
            Some(div().id("missing").child(state_badge("Folder missing", orange(), theme)).tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Its folder was moved or deleted. Forget Missing Worktrees (on the repo) cleans it up.").build(window, cx)).into_any_element())
        } else if let Some(s) = st {
            if finished {
                let text = if s.upstream_gone { "Remote branch deleted".to_string() } else { format!("Merged into {base}") };
                Some(div().id("landed").child(state_badge(text, green(), theme)).tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Its work has landed. If nothing in it is still needed, it can be removed.").build(window, cx)).into_any_element())
            } else if no_work && !s.is_dirty() {
                let tip: SharedString = format!("Its branch points at a commit of {base}: nothing has been committed on it").into();
                Some(div().id("no-commits").text_size(px(text::CAPTION)).font_weight(FontWeight::MEDIUM).text_color(theme.tertiary).child("No commits yet").tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)).into_any_element())
            } else {
                sync_label(s, theme)
            }
        } else {
            None
        };
        let changes = st.filter(|_| !wt.is_missing).and_then(|s| changes_label(s, theme));
        let threads = self.thread_chips(&wt.path, &wt.path, theme, cx);
        let remove_tip = if wt.is_locked {
            format!("Locked{}. Unlock it with git worktree unlock to remove it.", wt.lock_reason.as_ref().map(|r| format!(": {r}")).unwrap_or_default())
        } else if wt.is_missing {
            "Forget this worktree: its folder is already gone".to_string()
        } else {
            match &risk {
                RemovalRisk::None => "Remove this worktree: nothing in it would be lost".to_string(),
                RemovalRisk::KeepsWork(_) => "Remove this worktree (asks first: its branch has work in progress)".to_string(),
                RemovalRisk::LosesWork(_) => "Remove this worktree (asks first: something in it would be lost)".to_string(),
            }
        };
        let safe = risk == RemovalRisk::None;
        let (w1, r1, w2, r2) = (wt.clone(), repo.clone(), wt.clone(), repo.clone());
        let group = SharedString::from(format!("wt-{}", wt.path));
        let menu = {
            let (page, wt) = (page.clone(), wt.clone());
            let pr = pr.as_ref().map(|p| (p.number(), p.link.url.clone()));
            let label = if safe { "Remove Worktree" } else { "Remove Worktree…" };
            move |menu: PopupMenu, _: &mut Window, _: &mut Context<PopupMenu>| {
                let (p1, p2, p3) = (wt.path.clone(), wt.path.clone(), wt.path.clone());
                let mut menu = menu
                    .item(PopupMenuItem::new("Show in Finder").icon(IconName::Folder).disabled(wt.is_missing).on_click(move |_, _, cx| cx.reveal_path(Path::new(&p1))))
                    .item(PopupMenuItem::new("Open in Terminal").icon(IconName::SquareTerminal).disabled(wt.is_missing).on_click(move |_, _, _| open_in_terminal(&p2)))
                    .item(PopupMenuItem::new("Copy Path").icon(IconName::Copy).on_click(move |_, _, cx| copy(p3.clone(), cx)));
                if let Some(b) = wt.branch.clone() {
                    menu = menu.item(PopupMenuItem::new("Copy Branch Name").on_click(move |_, _, cx| copy(b.clone(), cx)));
                }
                if let Some((n, url)) = pr.clone() {
                    menu = menu.item(PopupMenuItem::new(format!("Open #{n}")).icon(IconName::ExternalLink).on_click(move |_, _, cx| cx.open_url(&url)));
                }
                menu = menu.separator();
                if !wt.is_missing {
                    let (pg, here) = (page.clone(), wt.path.clone());
                    menu = menu.item(PopupMenuItem::new("New Thread Here").on_click(move |_, _, cx| {
                        let _ = pg.update(cx, |p, cx| p.new_thread_in(Some(here.clone()), cx));
                    }));
                }
                let (pg, w, r) = (page.clone(), w2.clone(), r2.clone());
                menu.item(PopupMenuItem::new(label).icon(icons::trash()).disabled(wt.is_locked).on_click(move |_, window, cx| {
                    let _ = pg.update(cx, |p, cx| p.request_removal(w.clone(), r.clone(), window, cx));
                }))
            }
        };
        div()
            .id(SharedString::from(format!("wt-row-{}", wt.path)))
            .group(group.clone())
            .flex()
            .items_center()
            .gap(px(8.))
            .pl(px(34.))
            .pr(px(12.))
            .py(px(6.))
            .border_t_1()
            .border_color(theme.separator)
            .context_menu(menu)
            .child(gpui_kit::component::Icon::new(if wt.is_missing { IconName::CircleAlert } else { IconName::FolderClosed }).size(px(14.)).text_color(if wt.is_missing { orange() } else { theme.tertiary }))
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
                            .items_center()
                            .gap(px(6.))
                            .child(div().flex_none().font_weight(FontWeight::MEDIUM).child(name))
                            .when(!location.is_empty(), |d| d.child(div().min_w_0().truncate().text_size(px(text::CAPTION)).text_color(theme.tertiary).child(location)))
                            .when(wt.is_locked, |d| d.child(div().text_size(px(text::CAPTION)).text_color(theme.tertiary).child("locked"))),
                    )
                    .child(branch),
            )
            .when_some(threads, |d, t| d.child(t))
            .when_some(state, |d, s| d.child(s))
            .when_some(changes, |d, c| d.child(c))
            .when_some(pr, |d, p| {
                let url = p.link.url.clone();
                d.child(Button::new(SharedString::from(format!("wt-pr-{}", wt.path))).xsmall().ghost().label(format!("#{}", p.number())).tooltip(p.title.clone()).on_click(move |_, _, cx| cx.open_url(&url)))
            })
            .child(
                // Always there when it's safe to remove; on hover otherwise.
                div().when(!safe, |d| d.invisible().group_hover(group, |s| s.visible())).child(
                    icon_button(SharedString::from(format!("rm-wt-{}", wt.path)), icons::trash(), remove_tip, theme)
                        .xsmall()
                        .disabled(wt.is_locked)
                        .on_click(cx.listener(move |p, _, window, cx| p.request_removal(w1.clone(), r1.clone(), window, cx))),
                ),
            )
            .into_any_element()
    }
}

/// Folds or unfolds a repo's worktrees.
fn fold_listener(id: String, cx: &mut Context<ProjectPage>) -> impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static {
    cx.listener(move |p: &mut ProjectPage, _: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut Context<ProjectPage>| {
        if !p.toggled_repos.remove(&id) {
            p.toggled_repos.insert(id.clone());
        }
        cx.notify();
    })
}

/// A branch of a stack that has no pull request yet.
fn unsubmitted_row(branch: &str, layer: usize, theme: &Theme) -> gpui_kit::AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(12.))
        .py(px(7.))
        .child(div().w(px(16.)).h(px(16.)).rounded_full().border_1().border_dashed().border_color(theme.secondary).flex().items_center().justify_center().text_size(px(text::CAPTION)).font_weight(FontWeight::BOLD).text_color(theme.secondary).child(layer.to_string()))
        .child(div().flex_1().min_w_0().truncate().text_size(px(text::CAPTION)).font_family(MONO).child(branch.to_string()))
        .child(chip("Not submitted", theme.secondary))
        .into_any_element()
}

impl Render for ProjectPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.core.as_ref().is_some_and(|c| c.status_updated_at.is_some()) && !self.refreshing {
            if let Some(probe) = self.probe.take() {
                cx.defer_in(window, move |p, window, cx| p.run_probe(probe, window, cx));
            }
        }
        self.compact = self.width.compact(window);
        self.narrow = self.width.narrow(window);
        let theme = Theme::get(cx);
        let plain = self.core.as_ref().and_then(|c| c.layout.as_ref()).is_some_and(|l| l.kind == LayoutKind::Plain);
        let body = if plain {
            div()
                .pt(px(40.))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(space::S))
                .child(Icon::new(IconName::Folder).size(px(36.)).text_color(theme.tertiary))
                .child(div().mt(px(space::S)).text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).child("No Git Repositories"))
                .child(div().max_w(px(380.)).text_center().text_color(theme.secondary).child("Repos and pull requests show up here once this folder, or folders inside it, are git repositories."))
                .into_any_element()
        } else {
            div().flex().flex_col().child(self.pulls_section(&theme, cx)).child(self.repos_section(&theme, cx)).into_any_element()
        };
        let pad = if self.narrow { space::L } else { 32. };
        let nudge = self.claude_memory_nudge(&theme, cx);
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .child(self.header(&theme, cx))
            .child(div().id("project-body").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.scroll).child(div().max_w(px(880.)).w_full().mx_auto().px(px(pad)).py(px(20.)).children(nudge).child(body)))
            .child(self.width.probe(cx.entity().downgrade()))
    }
}

// MARK: New Worktree

/// New Worktree…: a branch (new, or one that exists) checked out in a folder of its own, from a
/// base, and a thread started in it if you like.
struct NewWorktreeForm {
    page: WeakEntity<ProjectPage>,
    repo: WorkspaceRepo,
    branch: Entity<InputState>,
    base: Entity<InputState>,
    default_base: Option<String>,
    data_dir: PathBuf,
    /// Chosen with Change…; else under the data folder's `worktrees/<repo>/<branch>`.
    location: Option<PathBuf>,
    start_thread: bool,
    working: bool,
    error: Option<String>,
    tools: Arc<WorkspaceTools>,
    _sub: gpui_kit::Subscription,
}

impl NewWorktreeForm {
    fn new(page: WeakEntity<ProjectPage>, repo: WorkspaceRepo, default_base: Option<String>, data_dir: PathBuf, tools: Arc<WorkspaceTools>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let branch = cx.new(|cx| InputState::new(window, cx).placeholder("feature/login"));
        let base = cx.new(|cx| InputState::new(window, cx).placeholder(default_base.clone().unwrap_or_else(|| "HEAD".into())));
        if let Ok(b) = std::env::var("YH_NEW_WORKTREE_BRANCH") {
            branch.update(cx, |i, cx| i.set_value(b, window, cx));
        }
        let sub = cx.subscribe(&branch, |_, _, _: &gpui_kit::component::input::InputEvent, cx| cx.notify());
        NewWorktreeForm { page, repo, branch, base, default_base, data_dir, location: None, start_thread: true, working: false, error: None, tools, _sub: sub }
    }

    fn slug(&self, cx: &App) -> String {
        let b = self.branch.read(cx).value().trim().to_string();
        if b.is_empty() { "branch".into() } else { b.replace(['/', ' '], "-") }
    }

    fn target(&self, cx: &App) -> PathBuf {
        match &self.location {
            Some(folder) => folder.join(self.slug(cx)),
            None => self.data_dir.join("worktrees").join(self.repo.path.file_name().unwrap_or_default()).join(self.slug(cx)),
        }
    }

    fn choose_folder(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Put It Here".into()) });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut p))) = paths.await {
                if let Some(folder) = p.pop() {
                    let _ = this.update(cx, |f, cx| {
                        f.location = Some(folder);
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let branch = self.branch.read(cx).value().trim().to_string();
        if branch.is_empty() || self.working {
            return;
        }
        let typed = self.base.read(cx).value().trim().to_string();
        let base = if typed.is_empty() { self.default_base.clone() } else { Some(typed) };
        let folder = self.target(cx);
        let (repo, tools) = (self.repo.path.clone(), self.tools.clone());
        self.working = true;
        self.error = None;
        cx.notify();
        let f2 = folder.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let git = tools.git.as_ref().ok_or("git isn't installed.".to_string())?;
                    if let Some(parent) = f2.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let exists = git.branch_exists(&branch, &repo);
                    git.add_worktree(&repo, &f2, (!exists).then_some(branch.as_str()), if exists { None } else { base.as_deref() }, exists.then_some(branch.as_str()))
                })
                .await;
            let _ = this.update_in(cx, |f, window, cx| {
                f.working = false;
                eprintln!("PROJECT new worktree at {}: {:?}", folder.display(), result);
                match result {
                    Err(e) => f.error = Some(e),
                    Ok(()) => {
                        window.close_dialog(cx);
                        let (start, path) = (f.start_thread, folder.to_string_lossy().into_owned());
                        let _ = f.page.update(cx, |p, cx| {
                            p.refresh(false, cx);
                            if start {
                                p.new_thread_in(Some(path), cx);
                            }
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for NewWorktreeForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let empty = self.branch.read(cx).value().trim().is_empty();
        let field = |label: &str, el: gpui_kit::AnyElement| div().flex().items_center().gap(px(12.)).child(div().w(px(110.)).flex_none().text_color(theme.secondary).child(label.to_string())).child(div().flex_1().min_w_0().child(el));
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .text_size(px(text::BODY))
            .child(div().text_size(px(text::FOOTNOTE)).line_height(px(17.)).text_color(theme.secondary).child("Another checkout of the repo, on its own branch, so work there doesn't touch this one. A thread can work in as many as it needs."))
            .child(field("Branch", Input::new(&self.branch).into_any_element()))
            .child(field("Starting from", Input::new(&self.base).into_any_element()))
            .child(div().pl(px(122.)).text_size(px(text::CAPTION)).text_color(theme.secondary).child("Used for a new branch. An existing branch is checked out as it is."))
            .child(field(
                "Folder",
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(text::FOOTNOTE)).font_family(MONO).child(tilde(&self.target(cx).to_string_lossy())))
                    .child(Button::new("wt-folder").xsmall().label("Change…").on_click(cx.listener(|f, _, _, cx| f.choose_folder(cx))))
                    .into_any_element(),
            ))
            .child(field(
                "",
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(Switch::new("wt-start-thread").checked(self.start_thread).on_change(cx.listener(|f, on: &bool, _, cx| {
                        f.start_thread = *on;
                        cx.notify();
                    })))
                    .child("Start a thread in it")
                    .into_any_element(),
            ))
            .when_some(self.error.clone(), |d, e| d.child(div().text_color(theme.error).child(e)))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(Button::new("wt-cancel").outline().label("Cancel").on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(Button::new("wt-create").primary().label("Create").loading(self.working).disabled(empty || self.working).on_click(cx.listener(|f, _, window, cx| f.create(window, cx)))),
            )
    }
}

// MARK: New Stack

/// New Stack from Branches…: existing branches of a repo made a GitHub stack, bottom first,
/// with `gh stack init`, and submitted if you like.
struct NewStackForm {
    page: WeakEntity<ProjectPage>,
    repo: WorkspaceRepo,
    core: yh_core::workspace::Workspace,
    tools: Arc<WorkspaceTools>,
    installed: Option<bool>,
    branches: Option<Vec<String>>,
    /// Bottom first.
    chosen: Vec<String>,
    base: Entity<InputState>,
    submit: bool,
    working: bool,
    error: Option<String>,
}

impl NewStackForm {
    fn new(page: WeakEntity<ProjectPage>, repo: WorkspaceRepo, core: yh_core::workspace::Workspace, tools: Arc<WorkspaceTools>, installed: Option<bool>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let default = core.status.default_branches.get(repo.id()).cloned().unwrap_or_else(|| "main".into());
        let base = cx.new(|cx| InputState::new(window, cx).placeholder(default));
        let (path, t) = (repo.path.clone(), tools.clone());
        cx.spawn(async move |this, cx| {
            let list = cx.background_executor().spawn(async move { t.git.as_ref().map(|g| g.local_branches(&path)).unwrap_or_default() }).await;
            let _ = this.update(cx, |f, cx| {
                f.branches = Some(list);
                cx.notify();
            });
        })
        .detach();
        NewStackForm { page, repo, core, tools, installed, branches: None, chosen: vec![], base, submit: true, working: false, error: None }
    }

    fn trunk(&self, cx: &App) -> String {
        let typed = self.base.read(cx).value().trim().to_string();
        if typed.is_empty() { self.core.status.default_branches.get(self.repo.id()).cloned().unwrap_or_else(|| "main".into()) } else { typed }
    }

    /// Where `gh stack` runs: a checkout already on one of the branches, else the repo's own.
    fn checkout(&self) -> PathBuf {
        self.core.checkout_on_any_of(&self.repo, &self.chosen).unwrap_or_else(|| self.repo.path.clone())
    }

    fn checkout_is_dirty(&self) -> bool {
        let c = self.checkout();
        if c == self.repo.path {
            self.core.status.statuses.get(self.repo.id()).is_some_and(|s| s.is_dirty())
        } else {
            self.core.status.worktree_statuses.get(&c.to_string_lossy().into_owned()).is_some_and(|s| s.is_dirty())
        }
    }

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.chosen.is_empty() || self.working {
            return;
        }
        let typed = self.base.read(cx).value().trim().to_string();
        let (branches, checkout, submit, tools) = (self.chosen.clone(), self.checkout(), self.submit, self.tools.clone());
        self.working = true;
        self.error = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let stacks = tools.stacks.as_ref().ok_or("gh isn't installed.".to_string())?;
                    stacks.create(&branches, (!typed.is_empty()).then_some(typed.as_str()), &checkout)?;
                    if submit {
                        stacks.submit(&checkout).map_err(|e| format!("The stack was made, but submitting it failed: {e}"))?;
                    }
                    Ok::<(), String>(())
                })
                .await;
            let _ = this.update_in(cx, |f, window, cx| {
                f.working = false;
                eprintln!("PROJECT new stack: {result:?}");
                match result {
                    Err(e) => f.error = Some(e),
                    Ok(()) => {
                        window.close_dialog(cx);
                        let _ = f.page.update(cx, |p, cx| p.refresh(false, cx));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for NewStackForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let trunk = self.trunk(cx);
        let list_box = || div().id("list").flex_1().h(px(220.)).overflow_y_scroll().rounded(px(radius::MEDIUM + 1.)).border_1().border_color(theme.separator).bg(theme.card).p(px(4.)).flex().flex_col();
        let mut available = list_box();
        match &self.branches {
            None => available = available.child(div().p(px(8.)).text_color(theme.secondary).child("Reading branches…")),
            Some(all) => {
                for b in all.iter().filter(|b| !self.chosen.contains(b) && **b != trunk) {
                    let b2 = b.clone();
                    available = available.child(
                        div()
                            .id(SharedString::from(format!("add-{b}")))
                            .px(px(8.))
                            .py(px(3.))
                            .rounded(px(5.))
                            .hover(|d| d.bg(theme.hover))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(text::FOOTNOTE))
                            .child(gpui_kit::component::Icon::new(IconName::Plus).size(px(12.)).text_color(theme.tertiary))
                            .child(div().min_w_0().truncate().font_family(MONO).child(b.clone()))
                            .on_click(cx.listener(move |f, _, _, cx| {
                                f.chosen.push(b2.clone());
                                cx.notify();
                            })),
                    );
                }
            }
        }
        let mut chosen = list_box();
        if self.chosen.is_empty() {
            chosen = chosen.child(div().p(px(8.)).text_center().text_color(theme.tertiary).child("Click branches to add them, bottom first"));
        }
        // Shown top first.
        for (i, b) in self.chosen.iter().enumerate().rev() {
            let n = self.chosen.len();
            chosen = chosen.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(6.))
                    .py(px(2.))
                    .child(div().w(px(16.)).text_size(px(text::CAPTION)).text_color(theme.secondary).child((i + 1).to_string()))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(text::FOOTNOTE)).font_family(MONO).child(b.clone()))
                    .child(icon_button(SharedString::from(format!("up-{i}")), Icon::new(IconName::ChevronUp), "Higher in the stack", &theme).xsmall().disabled(i + 1 == n).on_click(cx.listener(move |f, _, _, cx| {
                        f.chosen.swap(i, i + 1);
                        cx.notify();
                    })))
                    .child(icon_button(SharedString::from(format!("down-{i}")), Icon::new(IconName::ChevronDown), "Lower in the stack", &theme).xsmall().disabled(i == 0).on_click(cx.listener(move |f, _, _, cx| {
                        f.chosen.swap(i, i - 1);
                        cx.notify();
                    })))
                    .child(icon_button(SharedString::from(format!("rm-{i}")), Icon::new(IconName::Minus), "Remove", &theme).xsmall().on_click(cx.listener(move |f, _, _, cx| {
                        f.chosen.remove(i);
                        cx.notify();
                    }))),
            );
        }
        let dirty = !self.chosen.is_empty() && self.checkout_is_dirty();
        let not_installed = self.installed == Some(false);
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .text_size(px(text::BODY))
            .child(div().text_size(px(text::FOOTNOTE)).line_height(px(17.)).text_color(theme.secondary).child(format!("Existing branches, each a pull request on the one below it. Pick them bottom first: the first goes into {trunk}.")))
            .child(div().flex().gap(px(12.)).child(available).child(chosen))
            .child(div().flex().items_center().gap(px(12.)).child(div().w(px(60.)).text_color(theme.secondary).child("Into")).child(div().flex_1().child(Input::new(&self.base))))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(Switch::new("stack-submit").checked(self.submit).on_change(cx.listener(|f, on: &bool, _, cx| {
                        f.submit = *on;
                        cx.notify();
                    })))
                    .child("Submit to GitHub now (push and open the PRs)"),
            )
            .when(!self.chosen.is_empty(), |d| d.child(div().text_size(px(text::CAPTION)).text_color(theme.secondary).child(format!("Runs in {}, which ends up on {}", tilde(&self.checkout().to_string_lossy()), self.chosen.last().cloned().unwrap_or_default()))))
            .when(dirty, |d| d.child(div().text_color(orange()).child("That checkout has uncommitted changes. Commit or stash them first.")))
            .when(not_installed, |d| d.child(div().text_color(orange()).child(format!("gh stack isn't installed: {}", yh_core::github::STACK_INSTALL_COMMAND))))
            .when_some(self.error.clone(), |d, e| d.child(div().text_color(theme.error).child(e)))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(Button::new("stack-cancel").outline().label("Cancel").on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(Button::new("stack-create").primary().label("Create Stack").loading(self.working).disabled(self.chosen.is_empty() || self.working || dirty || not_installed).on_click(cx.listener(|f, _, window, cx| f.create(window, cx)))),
            )
    }
}
