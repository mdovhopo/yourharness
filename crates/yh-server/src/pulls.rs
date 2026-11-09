//! A project's open PRs for a client, as the project's page on the Mac lists them: every repo
//! in its folder, read from the forges (`gh`, `glab`; `YH_MOCK_PULLS` in tests), in stacks, split
//! into what waits for your review, yours and everyone else's.

use yh_core::workspace::{Workspace, WorkspaceTools};
use yh_proto::{PullInfo, ToApp};

/// Blocks while the forges answer: a second or two.
pub fn read(project: &str, folder: &str) -> ToApp {
    let tools = WorkspaceTools::find(std::env::var("YH_PATH").ok().as_deref());
    let mut ws = Workspace::new(std::path::Path::new(folder));
    ws.refresh(&tools, true, std::time::Duration::ZERO);
    let sections = ws.pull_request_sections();
    let mut pulls = vec![];
    for (name, stacks) in [("review", &sections.needs_review), ("mine", &sections.mine), ("others", &sections.others)] {
        for stack in stacks {
            let bottom = format!("{}#{}", stack.pull_request.link.path, stack.pull_request.number());
            for (p, depth) in stack.flattened() {
                pulls.push(PullInfo {
                    repo: p.link.path.clone(),
                    number: p.number(),
                    title: p.title.clone(),
                    url: p.link.url.clone(),
                    author: p.author.clone(),
                    draft: p.draft,
                    review: p.review.clone(),
                    checks: p.checks.clone(),
                    merge_status: p.merge_status.clone(),
                    auto_merge: p.auto_merge,
                    in_merge_queue: p.in_merge_queue,
                    updated_at: p.updated_at.clone(),
                    head: p.head.clone(),
                    base: p.base.clone(),
                    mine: p.is_mine,
                    section: name.into(),
                    stack: bottom.clone(),
                    depth: depth as u32,
                });
            }
        }
    }
    let errors = ws.pulls.errors.values().cloned().collect();
    let accounts = ws.pulls.accounts.values().cloned().collect();
    ToApp::Pulls { project: project.to_string(), pulls, errors, accounts }
}
