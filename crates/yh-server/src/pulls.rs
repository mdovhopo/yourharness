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

// One PR in full, for its review page on the phone: `gh pr view` for the PR, its checks,
// comments and reviews, `gh repo view` for how the repo merges, and `gh api` for comments on
// lines of code (`glab` for GitLab: the MR, its pipeline and notes). Merging is `gh pr merge`.

use serde_json::Value;
use yh_core::forge::Link;
use yh_proto::{PullCheck, PullComment, PullDetail};

const FIELDS: &str = "number,title,url,state,isDraft,author,body,headRefName,baseRefName,additions,deletions,changedFiles,commits,createdAt,updatedAt,mergedAt,labels,reviewDecision,mergeStateStatus,autoMergeRequest,statusCheckRollup,comments,reviews";
const METHODS: &[&str] = &["squash", "merge", "rebase"];

/// A PR by its URL, read now. Blocks while the forge answers: a second or two.
pub fn detail(url: &str) -> ToApp {
    let read = Link::parse(url).ok_or_else(|| "That isn't a pull request's address.".to_string()).and_then(|link| match mock(&link) {
        Some(found) => found,
        None if link.gitlab => gitlab_detail(&link),
        None => github_detail(&link),
    });
    match read {
        Ok(pull) => ToApp::Pull { url: url.to_string(), pull: Some(pull), error: None },
        Err(error) => ToApp::Pull { url: url.to_string(), pull: None, error: Some(error) },
    }
}

/// Merges a PR (`method`: `squash`, `merge`, `rebase`; `auto`: once checks and reviews allow).
pub fn merge(url: &str, method: &str, auto: bool) -> Result<(), String> {
    let link = Link::parse(url).ok_or_else(|| "That isn't a pull request's address.".to_string())?;
    if !METHODS.contains(&method) {
        return Err(format!("No merge method named {method}."));
    }
    if std::env::var("YH_MOCK_PULLS").is_ok_and(|p| !p.is_empty()) {
        eprintln!("PULL merge {} {method}{}", link.url, if auto { " auto" } else { "" });
        return Ok(());
    }
    let mut args: Vec<String> = if link.gitlab {
        let mut a = vec!["mr".into(), "merge".into(), link.url.clone(), "--yes".into()];
        match method {
            "squash" => a.push("--squash".into()),
            "rebase" => a.push("--rebase".into()),
            _ => {}
        }
        a
    } else {
        vec!["pr".into(), "merge".into(), link.url.clone(), format!("--{method}")]
    };
    if auto {
        args.push(if link.gitlab { "--auto-merge".into() } else { "--auto".into() });
    }
    run(if link.gitlab { "glab" } else { "gh" }, &args).map(|_| ())
}

/// `tool args…` in the login shell's environment (`gh`'s token, Homebrew's PATH), its output.
fn run(tool: &str, args: &[String]) -> Result<Vec<u8>, String> {
    let mut command = std::process::Command::new(tool);
    command.args(args).current_dir(std::env::temp_dir());
    yh_core::shell_env::apply(&mut command);
    if let Ok(p) = std::env::var("YH_PATH") {
        command.env("PATH", p);
    }
    let output = command.output().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("`{tool}` isn't installed on the Mac."),
        _ => format!("{tool}: {e}"),
    })?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if said.is_empty() { format!("{tool} failed.") } else { said });
    }
    Ok(output.stdout)
}

fn json(bytes: Vec<u8>) -> Result<Value, String> {
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

fn github_detail(link: &Link) -> Result<PullDetail, String> {
    let repo = if link.host == "github.com" { link.path.clone() } else { format!("{}/{}", link.host, link.path) };
    let (pr, settings, code) = std::thread::scope(|s| {
        let settings = s.spawn(|| run("gh", &["repo".into(), "view".into(), repo.clone(), "--json".into(), "squashMergeAllowed,mergeCommitAllowed,rebaseMergeAllowed,viewerDefaultMergeMethod".into()]).and_then(json));
        let code = s.spawn(|| {
            let mut args = vec!["api".into(), format!("repos/{}/pulls/{}/comments?per_page=100", link.path, link.number)];
            if link.host != "github.com" {
                args.extend(["--hostname".into(), link.host.clone()]);
            }
            run("gh", &args).and_then(json)
        });
        let pr = run("gh", &["pr".into(), "view".into(), link.url.clone(), "--json".into(), FIELDS.into()]).and_then(json);
        (pr, settings.join().unwrap_or_else(|_| Err("".into())), code.join().unwrap_or_else(|_| Err("".into())))
    });
    Ok(from_github(link, &pr?, settings.ok().as_ref(), code.ok().as_ref()))
}

/// The PR as `gh pr view --json` prints it; how the repo merges (`gh repo view`) and its code
/// comments (`gh api …/comments`) when they could be read.
pub fn from_github(link: &Link, v: &Value, settings: Option<&Value>, code: Option<&Value>) -> PullDetail {
    let text = |v: &Value| v.as_str().map(str::to_string);
    let mut checks: Vec<PullCheck> = v["statusCheckRollup"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            let name = text(&c["name"]).or_else(|| text(&c["context"])).unwrap_or_default();
            let said = c["conclusion"].as_str().filter(|s| !s.is_empty()).or(c["state"].as_str()).or(c["status"].as_str()).unwrap_or("").to_uppercase();
            let state = match said.as_str() {
                "SUCCESS" => "passing",
                "FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED" | "STARTUP_FAILURE" => "failing",
                "SKIPPED" | "NEUTRAL" | "STALE" => "skipped",
                _ => "pending",
            };
            PullCheck { name, workflow: text(&c["workflowName"]).filter(|w| !w.is_empty()), state: state.into(), url: text(&c["detailsUrl"]).or_else(|| text(&c["targetUrl"])).filter(|u| !u.is_empty()) }
        })
        .collect();
    let rank = |s: &str| ["failing", "pending", "passing", "skipped"].iter().position(|x| *x == s).unwrap_or(4);
    checks.sort_by(|a, b| rank(&a.state).cmp(&rank(&b.state)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));

    let mut comments: Vec<PullComment> = vec![];
    for c in v["comments"].as_array().into_iter().flatten().filter(|c| !c["isMinimized"].as_bool().unwrap_or(false)) {
        comments.push(PullComment { author: text(&c["author"]["login"]), body: text(&c["body"]).unwrap_or_default(), at: text(&c["createdAt"]), kind: "comment".into(), url: text(&c["url"]), ..Default::default() });
    }
    for r in v["reviews"].as_array().into_iter().flatten() {
        let body = text(&r["body"]).unwrap_or_default();
        let verdict = match r["state"].as_str().unwrap_or("") {
            "APPROVED" => "approved",
            "CHANGES_REQUESTED" => "changesRequested",
            "DISMISSED" => "dismissed",
            "PENDING" => continue,
            _ => "commented",
        };
        // A review that only holds code comments: those come on their own.
        if verdict == "commented" && body.trim().is_empty() {
            continue;
        }
        comments.push(PullComment { author: text(&r["author"]["login"]), body, at: text(&r["submittedAt"]), kind: "review".into(), verdict: Some(verdict.into()), ..Default::default() });
    }
    for c in code.and_then(Value::as_array).into_iter().flatten() {
        comments.push(PullComment {
            author: text(&c["user"]["login"]),
            body: text(&c["body"]).unwrap_or_default(),
            at: text(&c["created_at"]),
            kind: "code".into(),
            path: text(&c["path"]),
            line: c["line"].as_u64().or(c["original_line"].as_u64()),
            url: text(&c["html_url"]),
            ..Default::default()
        });
    }
    comments.sort_by(|a, b| a.at.cmp(&b.at));

    let mut merge_methods: Vec<String> = match settings {
        Some(s) => [("squash", "squashMergeAllowed"), ("merge", "mergeCommitAllowed"), ("rebase", "rebaseMergeAllowed")].iter().filter(|(_, k)| s[*k].as_bool().unwrap_or(true)).map(|(m, _)| m.to_string()).collect(),
        None => METHODS.iter().map(|m| m.to_string()).collect(),
    };
    if let Some(default) = settings.and_then(|s| s["viewerDefaultMergeMethod"].as_str()).map(|m| m.to_lowercase()) {
        if let Some(at) = merge_methods.iter().position(|m| *m == default) {
            let m = merge_methods.remove(at);
            merge_methods.insert(0, m);
        }
    }

    PullDetail {
        repo: link.path.clone(),
        number: link.number,
        title: text(&v["title"]).unwrap_or_default(),
        url: text(&v["url"]).unwrap_or_else(|| link.url.clone()),
        state: v["state"].as_str().unwrap_or("OPEN").to_lowercase(),
        draft: v["isDraft"].as_bool().unwrap_or(false),
        author: text(&v["author"]["login"]),
        body: text(&v["body"]).unwrap_or_default(),
        head: text(&v["headRefName"]).unwrap_or_default(),
        base: text(&v["baseRefName"]).unwrap_or_default(),
        additions: v["additions"].as_u64().unwrap_or(0),
        deletions: v["deletions"].as_u64().unwrap_or(0),
        changed_files: v["changedFiles"].as_u64().unwrap_or(0),
        commits: v["commits"].as_array().map(|c| c.len() as u64).unwrap_or(0),
        created_at: text(&v["createdAt"]),
        updated_at: text(&v["updatedAt"]),
        merged_at: text(&v["mergedAt"]).filter(|m| !m.is_empty()),
        labels: v["labels"].as_array().into_iter().flatten().filter_map(|l| text(&l["name"])).collect(),
        review: match v["reviewDecision"].as_str() {
            Some("APPROVED") => "approved",
            Some("CHANGES_REQUESTED") => "changesRequested",
            Some("REVIEW_REQUIRED") => "required",
            _ => "",
        }
        .into(),
        merge_status: text(&v["mergeStateStatus"]),
        auto_merge: !v["autoMergeRequest"].is_null(),
        checks,
        comments,
        merge_methods,
    }
}

fn gitlab_detail(link: &Link) -> Result<PullDetail, String> {
    let notes_path = format!("projects/{}/merge_requests/{}/notes?sort=asc&per_page=100", link.path.replace('/', "%2F"), link.number);
    let (mr, notes) = std::thread::scope(|s| {
        let notes = s.spawn(|| run("glab", &["api".into(), notes_path.clone()]).and_then(json));
        let mr = run("glab", &["mr".into(), "view".into(), link.url.clone(), "--output".into(), "json".into()]).and_then(json);
        (mr, notes.join().unwrap_or_else(|_| Err("".into())))
    });
    Ok(from_gitlab(link, &mr?, notes.ok().as_ref()))
}

/// The MR as `glab mr view --output json` prints it, and its notes when they could be read.
pub fn from_gitlab(link: &Link, v: &Value, notes: Option<&Value>) -> PullDetail {
    let text = |v: &Value| v.as_str().map(str::to_string);
    let pipeline = if v["head_pipeline"].is_object() { &v["head_pipeline"] } else { &v["pipeline"] };
    let checks = pipeline["status"].as_str().map(|status| PullCheck {
        name: "Pipeline".into(),
        workflow: None,
        state: match status {
            "success" => "passing",
            "failed" | "canceled" => "failing",
            "skipped" | "manual" => "skipped",
            _ => "pending",
        }
        .into(),
        url: text(&pipeline["web_url"]),
    });
    let comments = notes
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|n| !n["system"].as_bool().unwrap_or(false))
        .map(|n| PullComment {
            author: text(&n["author"]["username"]),
            body: text(&n["body"]).unwrap_or_default(),
            at: text(&n["created_at"]),
            kind: if n["position"].is_object() { "code" } else { "comment" }.into(),
            path: text(&n["position"]["new_path"]),
            line: n["position"]["new_line"].as_u64(),
            ..Default::default()
        })
        .collect();
    let mut merge_methods = vec!["merge".to_string()];
    if v["squash"].as_bool().unwrap_or(false) || v["squash_on_merge"].as_bool().unwrap_or(false) {
        merge_methods.insert(0, "squash".into());
    } else {
        merge_methods.push("squash".into());
    }
    merge_methods.push("rebase".into());
    PullDetail {
        repo: link.path.clone(),
        number: link.number,
        title: text(&v["title"]).unwrap_or_default(),
        url: text(&v["web_url"]).unwrap_or_else(|| link.url.clone()),
        state: match v["state"].as_str() {
            Some("merged") => "merged",
            Some("closed") => "closed",
            _ => "open",
        }
        .into(),
        draft: v["draft"].as_bool().or(v["work_in_progress"].as_bool()).unwrap_or(false),
        author: text(&v["author"]["username"]),
        body: text(&v["description"]).unwrap_or_default(),
        head: text(&v["source_branch"]).unwrap_or_default(),
        base: text(&v["target_branch"]).unwrap_or_default(),
        changed_files: v["changes_count"].as_str().and_then(|c| c.trim_end_matches('+').parse().ok()).unwrap_or(0),
        created_at: text(&v["created_at"]),
        updated_at: text(&v["updated_at"]),
        merged_at: text(&v["merged_at"]),
        labels: v["labels"].as_array().into_iter().flatten().filter_map(|l| text(l).or_else(|| text(&l["name"]))).collect(),
        merge_status: match v["detailed_merge_status"].as_str() {
            Some("mergeable") => Some("CLEAN".into()),
            Some("conflict") => Some("DIRTY".into()),
            Some("need_rebase") => Some("BEHIND".into()),
            Some("draft_status") => Some("DRAFT".into()),
            Some(_) => Some("BLOCKED".into()),
            None => None,
        },
        checks: checks.into_iter().collect(),
        comments,
        merge_methods,
        ..Default::default()
    }
}

/// `YH_MOCK_PULLS`: a URL's entry in the file's object form, read as `gh pr view` prints it
/// (with `"mergeMethods"` and `"codeComments"`, as `gh repo view` and `gh api` would answer);
/// in its list form, what the list says. None when there's no mock file.
fn mock(link: &Link) -> Option<Result<PullDetail, String>> {
    let path = std::env::var("YH_MOCK_PULLS").ok().filter(|p| !p.is_empty())?;
    let file: Value = match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string())) {
        Ok(v) => v,
        Err(e) => return Some(Err(e)),
    };
    if let Some(map) = file.as_object() {
        let entry = map.iter().find(|(url, _)| Link::parse(url).is_some_and(|l| l.id() == link.id()));
        return Some(match entry {
            Some((_, v)) if link.gitlab => Ok(from_gitlab(link, v, v.get("notes"))),
            Some((_, v)) => {
                let methods = v["mergeMethods"].as_array().map(|m| {
                    let allowed = |name: &str| m.iter().any(|x| x.as_str() == Some(name));
                    serde_json::json!({"squashMergeAllowed": allowed("squash"), "mergeCommitAllowed": allowed("merge"), "rebaseMergeAllowed": allowed("rebase"), "viewerDefaultMergeMethod": m.first().and_then(Value::as_str).unwrap_or("").to_uppercase()})
                });
                Ok(from_github(link, v, methods.as_ref(), v.get("codeComments")))
            }
            None => Err("not in the mock file".into()),
        });
    }
    let pulls = match yh_core::forge::mock_pulls()? {
        Ok(p) => p,
        Err(e) => return Some(Err(e)),
    };
    Some(pulls.into_iter().find(|p| link.matches(p)).ok_or_else(|| "not in the mock file".to_string()).map(|p| PullDetail {
        repo: link.path.clone(),
        number: link.number,
        title: p.title,
        url: link.url.clone(),
        state: p.state,
        draft: p.draft,
        author: p.author,
        head: p.head,
        base: p.base,
        updated_at: p.updated_at,
        review: p.review,
        merge_status: p.merge_status,
        auto_merge: p.auto_merge,
        checks: if p.checks.is_empty() { vec![] } else { vec![PullCheck { name: "Checks".into(), workflow: None, state: p.checks, url: None }] },
        merge_methods: METHODS.iter().map(|m| m.to_string()).collect(),
        ..Default::default()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link() -> Link {
        Link::parse("https://github.com/o/r/pull/7").unwrap()
    }

    #[test]
    fn reads_a_github_pr() {
        let v = serde_json::json!({
            "title": "Fix it", "state": "OPEN", "isDraft": false, "author": {"login": "ann"}, "body": "Why",
            "headRefName": "fix", "baseRefName": "main", "additions": 3, "deletions": 1, "changedFiles": 2,
            "commits": [{}, {}], "labels": [{"name": "bug"}], "reviewDecision": "CHANGES_REQUESTED",
            "mergeStateStatus": "BLOCKED", "autoMergeRequest": null,
            "statusCheckRollup": [
                {"__typename": "CheckRun", "name": "test", "workflowName": "CI", "status": "COMPLETED", "conclusion": "SUCCESS", "detailsUrl": "https://ci/1"},
                {"__typename": "CheckRun", "name": "lint", "status": "IN_PROGRESS", "conclusion": ""},
                {"__typename": "StatusContext", "context": "deploy", "state": "FAILURE", "targetUrl": "https://ci/2"}
            ],
            "comments": [{"author": {"login": "bob"}, "body": "Hi", "createdAt": "2026-01-02T00:00:00Z", "url": "https://c/1"},
                         {"author": {"login": "spam"}, "body": "x", "createdAt": "2026-01-01T00:00:00Z", "isMinimized": true}],
            "reviews": [{"author": {"login": "cat"}, "body": "", "state": "COMMENTED", "submittedAt": "2026-01-03T00:00:00Z"},
                        {"author": {"login": "cat"}, "body": "Change this", "state": "CHANGES_REQUESTED", "submittedAt": "2026-01-04T00:00:00Z"}]
        });
        let settings = serde_json::json!({"squashMergeAllowed": true, "mergeCommitAllowed": false, "rebaseMergeAllowed": true, "viewerDefaultMergeMethod": "REBASE"});
        let code = serde_json::json!([{"user": {"login": "cat"}, "body": "Typo", "created_at": "2026-01-03T00:00:01Z", "path": "a.rs", "line": 4, "html_url": "https://c/2"}]);
        let p = from_github(&link(), &v, Some(&settings), Some(&code));
        assert_eq!((p.title.as_str(), p.state.as_str(), p.commits, p.labels.clone()), ("Fix it", "open", 2, vec!["bug".to_string()]));
        assert_eq!(p.review, "changesRequested");
        assert_eq!(p.checks.iter().map(|c| (c.name.as_str(), c.state.as_str())).collect::<Vec<_>>(), [("deploy", "failing"), ("lint", "pending"), ("test", "passing")]);
        assert_eq!(p.checks[2].workflow.as_deref(), Some("CI"));
        assert_eq!(p.comments.iter().map(|c| (c.kind.as_str(), c.body.as_str())).collect::<Vec<_>>(), [("comment", "Hi"), ("code", "Typo"), ("review", "Change this")]);
        assert_eq!(p.comments[1].line, Some(4));
        assert_eq!(p.merge_methods, ["rebase", "squash"]);
    }

    #[test]
    fn unknown_merge_settings_offer_every_method() {
        let p = from_github(&link(), &serde_json::json!({"title": "T"}), None, None);
        assert_eq!(p.merge_methods, ["squash", "merge", "rebase"]);
        assert!(p.checks.is_empty() && p.comments.is_empty());
    }

    #[test]
    fn reads_a_gitlab_mr() {
        let link = Link::parse("https://gitlab.com/g/r/-/merge_requests/3").unwrap();
        let v = serde_json::json!({"title": "MR", "state": "opened", "description": "D", "source_branch": "f", "target_branch": "main",
            "head_pipeline": {"status": "failed", "web_url": "https://p/1"}, "detailed_merge_status": "conflict", "changes_count": "5"});
        let notes = serde_json::json!([{"author": {"username": "a"}, "body": "ok", "system": false}, {"body": "added 1 commit", "system": true}]);
        let p = from_gitlab(&link, &v, Some(&notes));
        assert_eq!((p.state.as_str(), p.changed_files, p.merge_status.as_deref()), ("open", 5, Some("DIRTY")));
        assert_eq!(p.checks[0].state, "failing");
        assert_eq!(p.comments.len(), 1);
    }

    #[test]
    fn refuses_what_isnt_a_pr() {
        assert!(matches!(detail("https://example.com/x"), ToApp::Pull { error: Some(_), .. }));
        assert!(merge("https://github.com/o/r/pull/1", "yolo", false).is_err());
    }
}
