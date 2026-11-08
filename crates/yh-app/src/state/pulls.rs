//! A thread's pull requests: the ones its tool calls changed (opened, merged, commented on),
//! the open ones from the branches of every checkout it wrote to (each repo and worktree, as
//! the project's scan last saw them, GitHub and GitLab alike), and the ones that only came up
//! (links pasted, PRs Claude looked at), each as its forge's CLI tells it now. All of it off
//! the main thread. Claude touching PRs (`gh pr …`, `gh stack`, `glab mr`, a push) has them
//! looked at again a moment later (`note_results`), as the Swift app's `pullRequestActivity`.

use crate::state::thread::{EntryRef, ThreadState};
use std::path::Path;
use yh_core::claude::ToolResult;
use yh_core::db::Database;
use yh_core::forge::{self, Link, PullRequest, Target};
use yh_core::github::LocalStack;
use yh_core::library::{EntryData, Kind};

#[derive(Clone, Debug)]
pub struct ThreadPull {
    pub link: Link,
    /// Its tool calls changed it, rather than only naming it.
    pub worked: bool,
    pub pull: Result<PullRequest, String>,
}

/// A thread's PRs as last looked up, and the `gh stack` stacks not submitted yet on its
/// branches: (repo name, stack).
#[derive(Clone, Debug, Default)]
pub struct ThreadPulls {
    pub pulls: Vec<ThreadPull>,
    pub local: Vec<(String, LocalStack)>,
}

/// What the project's scan says about a thread's branches, taken on the main thread (the scan
/// lives there) for the lookup aside.
#[derive(Clone, Debug, Default)]
pub struct BranchPulls {
    /// Open PRs whose head is the branch of a checkout the thread changed something in (not
    /// a default branch).
    pub pulls: Vec<PullRequest>,
    /// The workspace's open PRs: where each sits in a GitHub stack, which a lookup of one
    /// doesn't say.
    pub open: Vec<PullRequest>,
    /// Stacks made with `gh stack` and not on GitHub yet, on a branch the thread works on.
    pub local: Vec<(String, LocalStack)>,
}

/// The thread's branches' PRs, from the project's scan.
pub fn branch_pulls(scan: &yh_core::workspace::Workspace, thread: &yh_core::workspace::ThreadFolders) -> BranchPulls {
    let mut out = BranchPulls { open: scan.pull_requests().to_vec(), ..Default::default() };
    for checkout in scan.written_checkouts(thread) {
        let Some(branch) = scan.branch_of(&checkout) else { continue };
        if scan.status.default_branches.get(checkout.repo.id()) == Some(&branch) {
            continue;
        }
        for p in scan.pull_requests_for(&checkout.repo) {
            if p.head == branch && !out.pulls.iter().any(|x| x.id() == p.id()) {
                out.pulls.push(p.clone());
            }
        }
    }
    // A branch's PR in a GitHub stack brings the rest of its stack: the card shows it whole.
    let stacks: Vec<(String, u64)> = out.pulls.iter().filter_map(|p| p.stack.map(|s| (p.link.path.to_lowercase(), s.number))).collect();
    for p in scan.pull_requests() {
        if p.stack.is_some_and(|s| stacks.contains(&(p.link.path.to_lowercase(), s.number))) && !out.pulls.iter().any(|x| x.id() == p.id()) {
            out.pulls.push(p.clone());
        }
    }
    let mut seen = std::collections::HashSet::new();
    for checkout in scan.checkouts_for(thread) {
        let Some(branch) = scan.branch_of(&checkout) else { continue };
        for stack in scan.status.local_stacks.get(checkout.repo.id()).into_iter().flatten() {
            if stack.number.is_none() && stack.layers.iter().any(|l| l.branch == branch) && seen.insert(format!("{}{}", checkout.repo.id(), stack.trunk)) {
                out.local.push((checkout.repo.name(), stack.clone()));
            }
        }
    }
    out
}

/// The thread's PR links: worked on first (in the order they came up), then the rest.
/// `branches` are the open PRs of its branches (worked on); `saved` the links its record keeps
/// (`workedPullRequests`, `pullRequests`, as the Swift app saved them), so a branch's PR
/// that merged and left the open list still shows.
pub fn links(db: &Database, thread: &str, folder: &str, path_env: Option<&str>, branches: &[PullRequest], saved: (&[String], &[String])) -> Vec<(Link, bool)> {
    let total = db.transcript_count(thread);
    let rows: Vec<EntryData> = db.read_rows(thread, 0, total).into_iter().filter_map(EntryData::from_row).collect();
    let folder = Path::new(folder);
    let mut worked: Vec<Link> = vec![];
    let mut mentioned: Vec<Link> = vec![];
    let mut origin = None;
    let add = |list: &mut Vec<Link>, link: Link| {
        if !list.iter().any(|l| l.id() == link.id()) {
            list.push(link);
        }
    };
    for p in branches {
        add(&mut worked, p.link.clone());
    }
    for url in saved.0 {
        if let Some(l) = Link::find(url).into_iter().next() {
            add(&mut worked, l);
        }
    }
    for e in &rows {
        match e.kind {
            Kind::Tool => {
                let name = e.tool_name.as_deref().unwrap_or("");
                let input = e.tool_input.as_deref().unwrap_or("");
                let result = e.tool_result.as_deref().unwrap_or("");
                if let Some(targets) = forge::activity(name, input, result) {
                    for t in targets {
                        let link = match t {
                            Target::Link(l) => Some(l),
                            Target::Number(n, repo) => {
                                let (host, path, gitlab) = match repo {
                                    Some(r) => ("github.com".to_string(), r, false),
                                    None => match origin.get_or_insert_with(|| forge::origin(folder)) {
                                        Some(o) => o.clone(),
                                        None => continue,
                                    },
                                };
                                let kind = if gitlab { "/-/merge_requests/" } else { "/pull/" };
                                Some(Link { url: format!("https://{host}/{path}{kind}{n}"), host, path, number: n, gitlab })
                            }
                            Target::CurrentBranch => branch_pull(folder, path_env),
                        };
                        if let Some(l) = link {
                            add(&mut worked, l);
                        }
                    }
                } else {
                    for l in Link::find(&format!("{input}\n{result}")) {
                        add(&mut mentioned, l);
                    }
                }
            }
            Kind::User | Kind::Assistant => {
                for l in Link::find(&e.text) {
                    add(&mut mentioned, l);
                }
            }
            _ => {}
        }
    }
    for url in saved.1 {
        if let Some(l) = Link::find(url).into_iter().next() {
            add(&mut mentioned, l);
        }
    }
    let mut all: Vec<(Link, bool)> = worked.iter().cloned().map(|l| (l, true)).collect();
    for l in mentioned {
        if !worked.iter().any(|w| w.id() == l.id()) {
            all.push((l, false));
        }
    }
    all
}

/// The PR of the branch the folder is on, if it has one.
fn branch_pull(folder: &Path, path_env: Option<&str>) -> Option<Link> {
    let branch = forge::branch(folder)?;
    let mut command = std::process::Command::new("gh");
    command.args(["pr", "view", &branch, "--json", "url"]).current_dir(folder);
    if let Some(p) = path_env {
        command.env("PATH", p);
    }
    let out = command.output().ok()?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    Link::find(v["url"].as_str()?).into_iter().next()
}

/// Each link looked up at once, on threads of its own. `open` is the workspace's open PRs: a
/// PR in a GitHub stack gets its place in it from there.
pub fn look_up(links: Vec<(Link, bool)>, path_env: Option<String>, open: &[PullRequest]) -> Vec<ThreadPull> {
    let handles: Vec<_> = links
        .into_iter()
        .map(|(link, worked)| {
            let path = path_env.clone();
            std::thread::spawn(move || {
                let pull = forge::look_up(&link, path.as_deref());
                ThreadPull { link, worked, pull }
            })
        })
        .collect();
    let mut found: Vec<ThreadPull> = handles.into_iter().filter_map(|h| h.join().ok()).collect();
    for t in &mut found {
        if let Ok(p) = &mut t.pull {
            if p.stack.is_none() {
                p.stack = open.iter().find(|o| o.id() == p.id()).and_then(|o| o.stack);
            }
        }
    }
    found
}

// MARK: Claude touching PRs

/// Whether a command changes PRs: merging, opening, pushing to one (its state is about to
/// change), as the Swift app's `touchesPullRequests`.
pub fn touches_pull_requests(input_json: &str) -> bool {
    let v: serde_json::Value = serde_json::from_str(input_json).unwrap_or_default();
    let command = v["command"].as_str().unwrap_or("");
    ["gh pr", "gh stack", "glab mr", "git push", "gh api"].iter().any(|c| command.contains(c))
}

fn activity() -> &'static (flume::Sender<String>, flume::Receiver<String>) {
    static CHANNEL: std::sync::OnceLock<(flume::Sender<String>, flume::Receiver<String>)> = std::sync::OnceLock::new();
    CHANNEL.get_or_init(flume::unbounded)
}

/// Threads whose `claude` just touched PRs, as they come: the right sidebar looks their PRs up
/// again 3 and 12 seconds later.
pub fn activity_feed() -> flume::Receiver<String> {
    activity().1.clone()
}

/// From the thread's event handling, for each batch of tool results: a `Bash` call (the
/// thread's own, or a sub-agent's: its work is the thread's) that touched PRs and didn't fail
/// sends the thread to `activity_feed`. `entry` finds a tool call's row by its id.
pub fn note_results(thread: &ThreadState, parent: Option<&str>, results: &[ToolResult], entry: impl Fn(&str) -> Option<EntryRef>) {
    let touched = results.iter().filter(|r| !r.is_error).any(|r| {
        let input = match parent {
            None => entry(&r.id).and_then(|e| {
                let b = e.borrow();
                (b.data.tool_name.as_deref() == Some("Bash")).then(|| b.data.tool_input.clone()).flatten()
            }),
            Some(p) => entry(p).and_then(|e| e.borrow().sub.as_ref().and_then(|s| s.log.iter().find(|s| s.id == r.id && s.tool.as_deref() == Some("Bash")).map(|s| s.text.clone()))),
        };
        input.is_some_and(|i| touches_pull_requests(&i))
    });
    if touched {
        if std::env::var("YH_PR_PANEL_LOG").is_ok() {
            eprintln!("PRS activity in {}", thread.id());
        }
        let _ = activity().0.send(thread.id().to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_commands_that_touch_pull_requests() {
        assert!(touches_pull_requests(r#"{"command":"gh pr merge 12 --squash"}"#));
        assert!(touches_pull_requests(r#"{"command":"git push -u origin feature"}"#));
        assert!(touches_pull_requests(r#"{"command":"glab mr create"}"#));
        assert!(!touches_pull_requests(r#"{"command":"git status"}"#));
        assert!(!touches_pull_requests("not json"));
    }
}
