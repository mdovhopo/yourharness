//! A thread's pull requests: the ones its tool calls changed (opened, merged, commented on),
//! and the ones that only came up (links pasted, PRs Claude looked at), each as its forge's
//! CLI tells it now. All of it off the main thread.

use std::path::Path;
use yh_core::db::Database;
use yh_core::forge::{self, Link, PullRequest, Target};
use yh_core::library::{EntryData, Kind};

#[derive(Clone, Debug)]
pub struct ThreadPull {
    pub link: Link,
    /// Its tool calls changed it, rather than only naming it.
    pub worked: bool,
    pub pull: Result<PullRequest, String>,
}

/// The thread's PR links: worked on first (in the order they came up), then the rest.
pub fn links(db: &Database, thread: &str, folder: &str, path_env: Option<&str>) -> Vec<(Link, bool)> {
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

/// Each link looked up at once, on threads of its own.
pub fn look_up(links: Vec<(Link, bool)>, path_env: Option<String>) -> Vec<ThreadPull> {
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
    handles.into_iter().filter_map(|h| h.join().ok()).collect()
}
