//! `mcp__threads__…`: an agent looks through the app's threads (list, read, search), tidies them
//! (rename, settle, pin), brings in Claude Code sessions, the way you would from the sidebar
//! and ⌘K, and hands work to other threads: starts them, sends them messages, stops them and
//! waits for them, and answers what they wait on (permissions, questions, plans), as the phone
//! does (`Workspace::take_remote`); Settings → Threads turns that control off. It can't delete
//! anything. A thread in Home reaches every project; a thread anywhere
//! else only its own project's threads, and only sessions that ran in its folder. With title
//! updates on, it can have its own thread's title rewritten when the subject changes.
//! Names, schemas and replies are as 0.0.x had them: agents already use them.
//!
//! Calls run on a thread of their own: what only the main thread knows (which threads are
//! working, waiting, unread) is asked for over `ToolRequest`s; the database is read right here.

use serde_json::{json, Value};
use std::sync::Arc;
use yh_core::db::Database;
use yh_core::library::{Kind, ProjectMeta};
use yh_core::mcp::{schema, tool, ToolResult, ToolServer};

#[derive(Clone, Debug)]
pub struct ThreadInfo {
    pub id: String,
    pub title: String,
    pub project_id: String,
    pub running: bool,
    pub waiting: bool,
    pub unread: bool,
    pub pinned: bool,
    pub settled: bool,
    pub updated_at: f64,
    pub session_id: Option<String>,
    /// Messages queued behind its turn.
    pub queued: usize,
    /// What it waits for the user on, oldest first.
    pub requests: Vec<Ask>,
    /// The thread whose agent started it.
    pub parent_id: Option<String>,
}

/// A request waiting in a thread's composer.
#[derive(Clone, Debug)]
pub struct Ask {
    pub id: String,
    /// `permission`, `question` (AskUserQuestion), `plan` (ExitPlanMode) or `mcp` (an MCP server's question).
    pub kind: &'static str,
    /// What it's about, for the agent to decide on.
    pub detail: String,
}

/// What the calling thread reaches, as the main thread sees it right now.
pub struct Snapshot {
    /// None: it's in Home and reaches everything.
    pub scope: Option<ProjectMeta>,
    pub projects: Vec<ProjectMeta>,
    pub threads: Vec<ThreadInfo>,
    /// Settings → Threads lets agents control other threads, right now.
    pub controls: bool,
}

pub enum ToolRequest {
    Snapshot { caller: String, reply: flume::Sender<Option<Snapshot>> },
    Rename { id: String, title: String, reply: flume::Sender<()> },
    Settle { id: String, on: bool, reply: flume::Sender<()> },
    Pin { id: String, on: bool, reply: flume::Sender<()> },
    /// Reply: the new thread's id, or why not.
    Import { caller: String, session: String, project: Option<String>, reply: flume::Sender<Result<(String, usize), String>> },
    /// `messages`: the conversation's user and assistant messages, oldest first; `sent`: how many
    /// the user sent, so a title is asked for once per message at most.
    /// An agent's memory tools changed the memory: pages showing it read it again.
    MemoryChanged,
    /// A control request to the caller's `claude` (`mcp_status`, `mcp_authenticate`).
    Mcp { caller: String, request: serde_json::Value, reply: flume::Sender<Result<serde_json::Value, String>> },
    /// Shows a sign-in card in the caller's thread (None clears it).
    SignInCard { caller: String, ask: Option<crate::state::thread::SignInAsk> },
    /// A page for you, in your default browser (`YH_MCP_BROWSER=log` prints it instead).
    OpenUrl(String),
    /// Start, send or stop, taken as the phone's are. Reply: a new thread's id, or why not.
    Control { caller: String, action: yh_server::Action, reply: flume::Sender<Result<Option<String>, String>> },
    /// A permission denied with a reason the thread's Claude reads.
    Deny { caller: String, thread: String, request_id: String, message: String, reply: flume::Sender<()> },
    UpdateTitle { caller: String, messages: Vec<(bool, String)>, sent: usize, hint: Option<String>, reply: flume::Sender<String> },
}

pub struct ThreadTools {
    pub caller: String,
    pub requests: flume::Sender<ToolRequest>,
    pub db: Option<Arc<Database>>,
    pub updates_title: bool,
    /// Start, message, stop and answer other threads (Settings → Threads).
    pub controls: bool,
}

/// What the agent is told about the thread tools.
pub fn instructions(project: Option<&str>, updates_title: bool, controls: bool) -> String {
    let reach = match project {
        Some(p) => format!("From this thread you reach only the threads of the project {p}, and only Claude Code sessions that ran in its folder; for anything wider, the user can ask from a thread in Home."),
        None => "This thread is in Home, so you reach every project's threads and every Claude Code session.".into(),
    };
    let mut text = format!("The user's conversations in YourHarness are threads, grouped by project in the sidebar. The mcp__threads__ tools work with them: list_threads, read_thread and search_threads find and read them; rename_thread, settle_thread and pin_thread tidy them; list_claude_code_sessions and import_claude_code_session bring in sessions from Claude Code run in a terminal or an editor.{{control}} {reach}\nReading and searching are fine whenever they help, like picking up what an earlier thread worked out. Changing threads is the user's call: rename, settle, pin or import only what the user asked for, or a list they agreed to. Before changing more than two threads, list exactly what you'll change and wait for a yes. Leave threads that are working or waiting for the user alone unless the user named them. You can't delete threads; if asked, tell the user to do it from the sidebar. Other threads can hold private details or secrets: bring into this conversation only what the task needs. When you're done, say what you changed.");
    let control = if controls { " start_thread, send_to_thread, stop_thread, wait_for_threads and answer_thread hand work to other threads, follow it and answer what they wait on." } else { "" };
    text = text.replace("{control}", control);
    if controls {
        text += "\n\nDelegating: when the user asks you to hand work off, start_thread gives each piece a thread of its own, with a message that stands alone (the new thread knows nothing of this conversation: give it the goal, the files, what done means and what to leave alone). Then wait_for_threads and read its reply, or read_thread for more; send_to_thread follows up. When a thread you started waits on a permission, a question or a plan, answer_thread answers it as the user would: allow what serves the task the user gave you and stays inside it, deny with a reason what doesn't (deleting or overwriting what isn't the task's, pushing, publishing, spending money, anything the user didn't ask for), and ask the user when you aren't sure. Send to, stop or answer only threads you started or the user named, never one the user is working in. Settle the threads you started once their work is done and the user has what they need.";
    }
    if updates_title {
        text += "\n\nKeeping this thread's title right is part of your job; a separate title model writes it, you tell it when to look again. When the user's latest message turns the thread to a different subject than it has been about (a new, unrelated task or question), or a vague start (\"fix this\", a bare link) has turned out to be about something specific, call mcp__threads__update_title before you answer, with a few words on the new subject as the hint. Not for progress on the same subject (exploring, planning, implementing, reviewing, fixing CI, merging), and once per change at most. Don't use rename_thread on this thread unless the user asks for a particular name, and don't mention the title to the user.";
    }
    text
}

/// At most this many threads working at once in a project, counting those the user started:
/// a delegating agent can't run away.
const MAX_RUNNING: usize = 8;
/// How deep threads may start threads: the user's thread starts workers, they may start their
/// own, and those can't. With commands going only down the tree (`other`), nothing loops.
const MAX_DEPTH: usize = 2;
/// Threads one thread may start in all (settled ones too): a start-settle-start loop ends.
const MAX_STARTED: usize = 20;
const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
/// The modes an agent may approve a plan into (it becomes the project's mode, as the card's
/// does). Not Bypass: that one is the user's to choose.
const PLAN_MODES: [&str; 3] = ["auto", "acceptEdits", "ask"];

fn one_line(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        format!("{}…", flat.chars().take(max).collect::<String>())
    }
}

fn clip(text: &str, max: usize) -> String {
    let n = text.chars().count();
    if n <= max {
        text.to_string()
    } else {
        format!("{}… ({} more characters)", text.chars().take(max).collect::<String>(), n - max)
    }
}

/// "5 minutes ago", "yesterday", "3 days ago".
pub fn ago(ms: f64) -> String {
    let secs = ((yh_core::library::now_ms() - ms) / 1000.0).max(0.0) as i64;
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} minute{} ago", secs / 60, if secs / 60 == 1 { "" } else { "s" }),
        3600..=86_399 => format!("{} hour{} ago", secs / 3600, if secs / 3600 == 1 { "" } else { "s" }),
        86_400..=172_799 => "yesterday".into(),
        _ => format!("{} days ago", secs / 86_400),
    }
}

fn words(q: Option<&str>) -> Vec<String> {
    q.unwrap_or("").to_lowercase().split_whitespace().map(str::to_string).collect()
}

impl ThreadTools {
    fn ask<T>(&self, make: impl FnOnce(flume::Sender<T>) -> ToolRequest) -> Option<T> {
        let (tx, rx) = flume::bounded(1);
        self.requests.send(make(tx)).ok()?;
        rx.recv_timeout(std::time::Duration::from_secs(30)).ok()
    }

    fn snapshot(&self) -> Result<Snapshot, ToolResult> {
        self.ask(|reply| ToolRequest::Snapshot { caller: self.caller.clone(), reply }).flatten().ok_or_else(|| ToolResult::error("This thread is gone."))
    }

    fn project_name(s: &Snapshot, t: &ThreadInfo) -> String {
        s.projects.iter().find(|p| p.id == t.project_id).map_or("its project".into(), |p| p.name.clone())
    }

    fn short_id(s: &Snapshot, t: &ThreadInfo) -> String {
        let full = t.id.to_lowercase();
        let short: String = full.chars().take(8).collect();
        let shared = s.threads.iter().any(|o| o.id.to_lowercase() != full && o.id.to_lowercase().starts_with(&short));
        if shared { full } else { short }
    }

    fn describe(&self, s: &Snapshot, t: &ThreadInfo, listed: bool) -> String {
        let mut states = vec![];
        if t.id == self.caller {
            states.push("this thread");
        }
        if t.running {
            states.push("working");
        }
        if t.waiting {
            states.push("waiting for the user");
        }
        let queued = format!("{} queued", t.queued);
        if t.queued > 0 {
            states.push(&queued);
        }
        if t.unread {
            states.push("unread");
        }
        if t.pinned {
            states.push("pinned");
        }
        if t.settled {
            states.push("settled");
        }
        let started_by = t.parent_id.as_ref().and_then(|p| s.threads.iter().find(|o| &o.id == p)).map(|p| {
            if p.id == self.caller { "started by this thread".to_string() } else { format!("started by {}", Self::short_id(s, p)) }
        });
        if let Some(by) = &started_by {
            states.push(by);
        }
        let mut line = format!("{}{} · “{}” · {}", if listed { "- " } else { "" }, Self::short_id(s, t), one_line(&t.title, 100), Self::project_name(s, t));
        if !states.is_empty() {
            line += &format!(" · {}", states.join(", "));
        }
        line + &format!(" · {}", ago(t.updated_at))
    }

    fn reachable<'a>(&self, s: &'a Snapshot) -> Vec<&'a ThreadInfo> {
        s.threads.iter().filter(|t| s.scope.as_ref().is_none_or(|p| p.id == t.project_id)).collect()
    }

    fn thread<'a>(&self, s: &'a Snapshot, a: &Value) -> Result<&'a ThreadInfo, ToolResult> {
        let given = a["thread_id"].as_str().unwrap_or("").trim().to_lowercase();
        if let Some(exact) = s.threads.iter().find(|t| t.id.to_lowercase() == given) {
            return self.reachable(s).into_iter().find(|t| t.id == exact.id).ok_or_else(|| ToolResult::error(format!("No thread {given} that you can reach. list_threads shows them.")));
        }
        if given.chars().count() < 6 {
            return Err(ToolResult::error("Give thread_id: the id from list_threads (8 characters are enough)."));
        }
        let matches: Vec<&ThreadInfo> = self.reachable(s).into_iter().filter(|t| t.id.to_lowercase().starts_with(&given)).collect();
        match matches.len() {
            0 => Err(ToolResult::error(format!("No thread {given} that you can reach. list_threads shows them."))),
            1 => Ok(matches[0]),
            n => Err(ToolResult::error(format!("{given} matches {n} threads; give more of the id."))),
        }
    }

    fn list(&self, a: &Value) -> Result<ToolResult, ToolResult> {
        let s = self.snapshot()?;
        let project = match a["project"].as_str().map(str::trim).filter(|p| !p.is_empty()) {
            None => None,
            Some(given) => {
                let found = s
                    .projects
                    .iter()
                    .find(|p| given.len() >= 6 && p.id.to_lowercase().starts_with(&given.to_lowercase()))
                    .or_else(|| s.projects.iter().find(|p| p.name.eq_ignore_ascii_case(given)));
                let Some(found) = found else {
                    return Err(ToolResult::error(format!("No project {given}. Projects: {}.", s.projects.iter().map(|p| p.name.clone()).collect::<Vec<_>>().join(", "))));
                };
                if let Some(own) = &s.scope {
                    if own.id != found.id {
                        return Err(ToolResult::error(format!("From a thread in {} you can only reach {}. Work across projects from a thread in Home.", own.name, own.name)));
                    }
                }
                Some(found.clone())
            }
        };
        let w = words(a["query"].as_str());
        let settled = a["include_settled"].as_bool().unwrap_or(false);
        let limit = a["limit"].as_u64().unwrap_or(50).clamp(1, 200) as usize;
        let mut found: Vec<&ThreadInfo> = self
            .reachable(&s)
            .into_iter()
            .filter(|t| project.as_ref().is_none_or(|p| p.id == t.project_id))
            .filter(|t| settled || !t.settled)
            .filter(|t| w.iter().all(|x| t.title.to_lowercase().contains(x)))
            .collect();
        found.sort_by(|a, b| b.updated_at.total_cmp(&a.updated_at));
        if found.is_empty() {
            return Ok(ToolResult::text("No threads match."));
        }
        let header = match (&project, &s.scope) {
            (Some(p), _) => format!("{}:", p.name),
            (None, Some(p)) => format!("{}:", p.name),
            (None, None) => "All projects:".into(),
        };
        let mut lines = vec![format!(
            "{header} {} thread{}{}:",
            found.len(),
            if found.len() == 1 { "" } else { "s" },
            if found.len() > limit { format!(", the {limit} most recent") } else { String::new() }
        )];
        lines.extend(found.iter().take(limit).map(|t| self.describe(&s, t, true)));
        Ok(ToolResult::text(lines.join("\n")))
    }

    fn read(&self, a: &Value) -> Result<ToolResult, ToolResult> {
        let s = self.snapshot()?;
        let t = self.thread(&s, a)?;
        let Some(db) = &self.db else { return Ok(ToolResult::error("No history here.")) };
        let total = db.transcript_count(&t.id);
        let limit = a["limit"].as_u64().unwrap_or(30).clamp(1, 100) as usize;
        let start = a["offset"].as_u64().map(|o| (o as usize).min(total)).unwrap_or(total.saturating_sub(limit));
        let end = (start + limit).min(total);
        let rows = db.read_rows(&t.id, start, end - start);
        let mut lines = vec![
            self.describe(&s, t, true),
            format!("Entries {start}–{} of {total}{}:", end.saturating_sub(1).max(start), if start > 0 { " (offset to read earlier ones)" } else { "" }),
        ];
        let mut budget: i64 = 60_000;
        for (n, row) in rows.into_iter().enumerate() {
            let index = start + n;
            let Some(e) = yh_core::library::EntryData::from_row(row) else { continue };
            let line = match e.kind {
                Kind::User => format!("[{index}] user: {}", clip(&e.text, 4000)),
                Kind::Assistant => format!("[{index}] claude: {}", clip(&e.text, 4000)),
                Kind::Thinking => format!("[{index}] (thinking, {} characters)", e.text.chars().count()),
                Kind::Tool => {
                    let mut l = format!("[{index}] tool {}: {}", e.tool_name.as_deref().unwrap_or("?"), one_line(e.tool_input.as_deref().unwrap_or(""), 300));
                    if let Some(r) = &e.tool_result {
                        l += &format!("\n      → {}{}", if e.is_error { "error: " } else { "" }, one_line(r, 400));
                    }
                    l
                }
                Kind::Notice => format!("[{index}] notice: {}", one_line(&e.text, 400)),
                Kind::Command => format!("[{index}] command output: {}", clip(&e.text, 1500)),
            };
            budget -= line.len() as i64;
            if budget < 0 {
                lines.push(format!("… stopped at entry {index} to keep this short; read on with offset {index}."));
                break;
            }
            lines.push(line);
        }
        Ok(ToolResult::text(lines.join("\n")))
    }

    fn search(&self, a: &Value) -> Result<ToolResult, ToolResult> {
        let Some(query) = a["query"].as_str().filter(|q| !q.trim().is_empty()) else { return Err(ToolResult::error("Give query: the words to find.")) };
        let s = self.snapshot()?;
        let limit = a["limit"].as_u64().unwrap_or(20).clamp(1, 50) as usize;
        let hits = self.db.as_ref().map(|db| db.search(query, limit * 3)).unwrap_or_default();
        let reach = self.reachable(&s);
        let mut lines = vec![];
        let mut last = String::new();
        let mut kept = 0;
        for hit in hits {
            let Some(t) = reach.iter().find(|t| t.id == hit.thread) else { continue };
            if t.id != last {
                lines.push(self.describe(&s, t, true));
                last = t.id.clone();
            }
            lines.push(format!("    entry {}: {}", hit.seq, hit.snippet.replace('\n', " ")));
            kept += 1;
            if kept >= limit {
                break;
            }
        }
        if lines.is_empty() {
            return Ok(ToolResult::text(format!("Nothing found for “{query}”.")));
        }
        Ok(ToolResult::text(lines.join("\n")))
    }

    fn rename(&self, a: &Value) -> Result<ToolResult, ToolResult> {
        let s = self.snapshot()?;
        let t = self.thread(&s, a)?;
        let title = a["title"].as_str().unwrap_or("").trim().to_string();
        if title.is_empty() {
            return Err(ToolResult::error("Give title: the new name."));
        }
        if title.chars().count() > 120 {
            return Err(ToolResult::error(format!("That title is {} characters; keep it under 120.", title.chars().count())));
        }
        self.ask(|reply| ToolRequest::Rename { id: t.id.clone(), title: title.clone(), reply });
        Ok(ToolResult::text(format!("Renamed “{}” to “{title}”.", t.title)))
    }

    fn settle(&self, a: &Value) -> Result<ToolResult, ToolResult> {
        let s = self.snapshot()?;
        let t = self.thread(&s, a)?;
        let on = a["settled"].as_bool().unwrap_or(true);
        if t.settled == on {
            return Ok(ToolResult::text(format!("“{}” is already {}.", t.title, if on { "settled" } else { "active" })));
        }
        self.ask(|reply| ToolRequest::Settle { id: t.id.clone(), on, reply });
        let p = Self::project_name(&s, t);
        Ok(ToolResult::text(if on {
            format!("Settled “{}”: it's on the Settled shelf of {p}, and comes back when the user sends it a message.", t.title)
        } else {
            format!("Brought “{}” back to the active threads of {p}.", t.title)
        }))
    }

    fn pin(&self, a: &Value) -> Result<ToolResult, ToolResult> {
        let s = self.snapshot()?;
        let t = self.thread(&s, a)?;
        let on = a["pinned"].as_bool().unwrap_or(true);
        if t.pinned == on {
            return Ok(ToolResult::text(format!("“{}” is already {}.", t.title, if on { "pinned" } else { "unpinned" })));
        }
        if on && t.settled {
            return Err(ToolResult::error(format!("“{}” is settled; bring it back with settle_thread first.", t.title)));
        }
        self.ask(|reply| ToolRequest::Pin { id: t.id.clone(), on, reply });
        Ok(ToolResult::text(format!("{} “{}”.", if on { "Pinned" } else { "Unpinned" }, t.title)))
    }

    fn control(&self, action: yh_server::Action) -> Result<Option<String>, ToolResult> {
        self.ask(|reply| ToolRequest::Control { caller: self.caller.clone(), action, reply }).ok_or_else(|| ToolResult::error("The app didn't answer."))?.map_err(ToolResult::error)
    }

    /// The threads above this one: the one that started it, the one that started that…
    fn ancestors(s: &Snapshot, id: &str) -> Vec<String> {
        let mut out: Vec<String> = vec![];
        let mut at = id.to_string();
        while let Some(parent) = s.threads.iter().find(|t| t.id == at).and_then(|t| t.parent_id.clone()) {
            if out.contains(&parent) || parent == id {
                break;
            }
            out.push(parent.clone());
            at = parent;
        }
        out
    }

    /// A thread this one may send to, stop, answer or wait on: not itself, nor any thread above
    /// it (the one that started it, and so on), and of another thread's workers only its own.
    /// Commands go only down, so no two threads drive each other in a loop.
    fn other<'a>(&self, s: &'a Snapshot, a: &Value) -> Result<&'a ThreadInfo, ToolResult> {
        let t = self.thread(s, a)?;
        if t.id == self.caller {
            return Err(ToolResult::error("That's this thread. Give another thread's id."));
        }
        if Self::ancestors(s, &self.caller).contains(&t.id) {
            return Err(ToolResult::error(format!("“{}” started this thread (or one above it): a thread can't drive the threads above it. Reply in your own conversation; it reads that.", t.title)));
        }
        if t.parent_id.is_some() && !Self::ancestors(s, &t.id).contains(&self.caller) {
            return Err(ToolResult::error(format!("“{}” was started by another thread, which drives it. Leave it to that one.", t.title)));
        }
        Ok(t)
    }

    fn start(&self, a: &Value) -> Result<ToolResult, ToolResult> {
        let message = a["message"].as_str().unwrap_or("").trim().to_string();
        if message.is_empty() {
            return Err(ToolResult::error("Give message: what the new thread should do, written so it stands alone."));
        }
        let s = self.snapshot()?;
        let project = match (&s.scope, a["project"].as_str().map(str::trim).filter(|p| !p.is_empty())) {
            (Some(own), None) => own.clone(),
            (Some(own), Some(given)) => {
                if !(own.name.eq_ignore_ascii_case(given) || (given.len() >= 6 && own.id.to_lowercase().starts_with(&given.to_lowercase()))) {
                    return Err(ToolResult::error(format!("From a thread in {} you can only start threads in {}.", own.name, own.name)));
                }
                own.clone()
            }
            (None, given) => {
                let caller_project = s.threads.iter().find(|t| t.id == self.caller).map(|t| t.project_id.clone());
                let found = match given {
                    Some(given) => s
                        .projects
                        .iter()
                        .find(|p| given.len() >= 6 && p.id.to_lowercase().starts_with(&given.to_lowercase()))
                        .or_else(|| s.projects.iter().find(|p| p.name.eq_ignore_ascii_case(given))),
                    None => s.projects.iter().find(|p| Some(&p.id) == caller_project.as_ref()),
                };
                match found {
                    Some(p) => p.clone(),
                    None => return Err(ToolResult::error(format!("No project {}. Projects: {}.", given.unwrap_or(""), s.projects.iter().map(|p| p.name.clone()).collect::<Vec<_>>().join(", ")))),
                }
            }
        };
        let depth = Self::ancestors(&s, &self.caller).len();
        if depth >= MAX_DEPTH {
            return Err(ToolResult::error("This thread was started by a thread that was itself started by one: threads this deep can't start more. Do the work here, or say in your reply what should be handed off."));
        }
        let started = s.threads.iter().filter(|t| t.parent_id.as_deref() == Some(self.caller.as_str())).count();
        if started >= MAX_STARTED {
            return Err(ToolResult::error(format!("This thread has started {started} threads already, the most one may. Reuse one with send_to_thread.")));
        }
        let running = self.reachable(&s).into_iter().filter(|t| t.running && t.project_id == project.id).count();
        if running >= MAX_RUNNING {
            return Err(ToolResult::error(format!("{running} threads are already working in {}; wait for some to finish first.", project.name)));
        }
        let title = a["title"].as_str().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
        if title.as_ref().is_some_and(|t| t.chars().count() > 120) {
            return Err(ToolResult::error("Keep title under 120 characters."));
        }
        let model = a["model"].as_str().map(str::trim).filter(|m| !m.is_empty()).map(str::to_string);
        let effort = a["effort"].as_str().map(str::trim).filter(|e| !e.is_empty()).map(str::to_string);
        if let Some(e) = &effort {
            if !EFFORTS.contains(&e.as_str()) {
                return Err(ToolResult::error(format!("effort is one of {}.", EFFORTS.join(", "))));
            }
        }
        let id = self
            .control(yh_server::Action::NewThread { project: project.id.clone(), text: Some(message), model, effort, mode: None })?
            .ok_or_else(|| ToolResult::error("Couldn't start a thread."))?;
        if let Some(title) = &title {
            self.ask(|reply| ToolRequest::Rename { id: id.clone(), title: title.clone(), reply });
        }
        let s = self.snapshot()?;
        let Some(t) = s.threads.iter().find(|t| t.id == id) else { return Err(ToolResult::error("Couldn't start a thread.")) };
        Ok(ToolResult::text(format!(
            "Started {}\nIt's working on your message; the user isn't taken to it. wait_for_threads with thread_ids [\"{}\"] tells you when it's done.",
            self.describe(&s, t, false),
            Self::short_id(&s, t)
        )))
    }

    fn send_to(&self, a: &Value) -> Result<ToolResult, ToolResult> {
        let message = a["message"].as_str().unwrap_or("").trim().to_string();
        if message.is_empty() {
            return Err(ToolResult::error("Give message: what to send."));
        }
        let s = self.snapshot()?;
        let t = self.other(&s, a)?;
        let now = a["now"].as_bool().unwrap_or(false);
        self.control(yh_server::Action::Send { thread: t.id.clone(), text: message, now, images: vec![] })?;
        Ok(ToolResult::text(if !t.running {
            format!("Sent to “{}”; it's working on it.", t.title)
        } else if now {
            format!("Sent to “{}”, which was working: it reads it at its next step.", t.title)
        } else {
            format!("Queued in “{}”, which is working: it goes when the turn ends.", t.title)
        }))
    }

    fn stop_other(&self, a: &Value) -> Result<ToolResult, ToolResult> {
        let s = self.snapshot()?;
        let t = self.other(&s, a)?;
        if !t.running {
            return Ok(ToolResult::text(format!("“{}” isn't working.", t.title)));
        }
        self.control(yh_server::Action::Stop { thread: t.id.clone() })?;
        Ok(ToolResult::text(format!("Stopped “{}”{}.", t.title, if t.queued > 0 { "; its queued messages went back to its composer" } else { "" })))
    }

    fn answer(&self, a: &Value) -> Result<ToolResult, ToolResult> {
        let s = self.snapshot()?;
        let t = self.other(&s, a)?;
        let asked = a["request_id"].as_str().map(str::trim).filter(|r| !r.is_empty());
        let Some(r) = (match asked {
            Some(id) => t.requests.iter().find(|r| r.id == id),
            None => t.requests.first(),
        }) else {
            return Err(ToolResult::error(match asked {
                Some(id) => format!("“{}” has no request {id} waiting (any more). wait_for_threads shows what it waits on.", t.title),
                None => format!("“{}” isn't waiting on anything.", t.title),
            }));
        };
        let decision = a["decision"].as_str().unwrap_or("").trim().to_lowercase();
        let message = a["message"].as_str().map(str::trim).filter(|m| !m.is_empty()).map(str::to_string);
        let (thread, request_id) = (t.id.clone(), r.id.clone());
        let done = match (r.kind, decision.as_str()) {
            ("question", "allow") => {
                let Some(answers) = a["answers"].as_object().filter(|o| !o.is_empty()) else {
                    return Err(ToolResult::error("Give answers: each question's text to the chosen option's label (or your own words)."));
                };
                let answers = answers.iter().map(|(q, v)| (q.clone(), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))).collect();
                self.control(yh_server::Action::Answer { thread, request_id, answers })?;
                "Answered its question"
            }
            ("question", "deny") => {
                self.control(yh_server::Action::SkipQuestion { thread, request_id })?;
                "Skipped its question"
            }
            ("plan", "allow") => {
                let mode = a["mode"].as_str().unwrap_or("acceptEdits").to_string();
                if !PLAN_MODES.contains(&mode.as_str()) {
                    return Err(ToolResult::error(format!("mode is one of {}.", PLAN_MODES.join(", "))));
                }
                self.control(yh_server::Action::ApprovePlan { thread, request_id, mode, feedback: message })?;
                "Approved its plan"
            }
            ("plan", "deny") => {
                self.control(yh_server::Action::KeepPlanning { thread, request_id, feedback: message })?;
                "Sent it back to planning"
            }
            ("mcp", d @ ("allow" | "deny")) => {
                self.control(yh_server::Action::AnswerQuestion { thread, request_id, accept: d == "allow" })?;
                if d == "allow" { "Accepted the MCP server's question with its defaults" } else { "Declined the MCP server's question" }
            }
            (_, "allow") => {
                self.control(yh_server::Action::Respond { thread, request_id, allow: true })?;
                "Allowed"
            }
            (_, "deny") => {
                match message {
                    Some(message) => {
                        self.ask(|reply| ToolRequest::Deny { caller: self.caller.clone(), thread, request_id, message, reply }).ok_or_else(|| ToolResult::error("The app didn't answer."))?;
                    }
                    None => {
                        self.control(yh_server::Action::Respond { thread, request_id, allow: false })?;
                    }
                }
                "Denied"
            }
            _ => return Err(ToolResult::error("Give decision: allow or deny.")),
        };
        Ok(ToolResult::text(format!("{done} in “{}” ({} {}). It carries on; wait_for_threads again to follow it.", t.title, r.kind, r.id)))
    }

    /// Its last turn's last reply, as saved (a turn's rows are saved before it shows as over).
    fn last_reply(&self, id: &str) -> Option<String> {
        let db = self.db.as_ref()?;
        let total = db.transcript_count(id);
        let start = total.saturating_sub(80);
        let rows: Vec<_> = db.read_rows(id, start, total - start).into_iter().filter_map(yh_core::library::EntryData::from_row).collect();
        rows.iter().rev().take_while(|e| e.kind != Kind::User).find(|e| e.kind == Kind::Assistant && !e.text.trim().is_empty()).map(|e| clip(e.text.trim(), 3000))
    }

    fn wait(&self, a: &Value) -> Result<ToolResult, ToolResult> {
        let given: Vec<String> = match &a["thread_ids"] {
            Value::Array(ids) => ids.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
            Value::String(one) => vec![one.clone()],
            _ => vec![],
        };
        if given.is_empty() {
            return Err(ToolResult::error("Give thread_ids: the threads to wait for."));
        }
        let any = a["any"].as_bool().unwrap_or(false);
        let timeout = a["timeout_seconds"].as_u64().unwrap_or(300).clamp(1, 600);
        let s = self.snapshot()?;
        let mut ids = vec![];
        for g in &given {
            let t = self.other(&s, &json!({ "thread_id": g }))?;
            ids.push(t.id.clone());
        }
        let busy = |t: &ThreadInfo| t.running && !t.waiting;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout);
        let s = loop {
            let s = self.snapshot()?;
            let states: Vec<bool> = ids.iter().map(|id| s.threads.iter().find(|t| &t.id == id).is_none_or(|t| !busy(t))).collect();
            let done = if any { states.iter().any(|d| *d) } else { states.iter().all(|d| *d) };
            // Stopped meanwhile: nobody waits for the answer any more.
            let caller_gone = !s.threads.iter().any(|t| t.id == self.caller && t.running);
            if done || caller_gone || std::time::Instant::now() >= deadline {
                break s;
            }
            std::thread::sleep(std::time::Duration::from_millis(1000));
        };
        let mut lines = vec![];
        let mut still = 0;
        for id in &ids {
            let Some(t) = s.threads.iter().find(|t| &t.id == id) else {
                lines.push(format!("- {id}: gone (deleted)"));
                continue;
            };
            lines.push(self.describe(&s, t, true));
            if t.waiting {
                if t.requests.is_empty() {
                    lines.push("    waits for the user: an MCP sign-in, which only the user can do".into());
                }
                for r in &t.requests {
                    lines.push(format!("    waits on {} {}: {}", r.kind, r.id, r.detail.replace('\n', "\n      ")));
                }
            } else if busy(t) {
                still += 1;
            } else if let Some(reply) = self.last_reply(&t.id) {
                lines.push(format!("    last reply:\n{}", reply.lines().map(|l| format!("    {l}")).collect::<Vec<_>>().join("\n")));
            }
        }
        let timed_out = std::time::Instant::now() >= deadline;
        let head = match (still, timed_out) {
            (0, _) => "Done waiting:".to_string(),
            (n, true) => format!("After {timeout} s, {n} still working (call again to keep waiting):"),
            (n, false) => format!("One is done or needs an answer; {n} still working:"),
        };
        Ok(ToolResult::text(format!("{head}\n{}", lines.join("\n"))))
    }

    fn sessions(&self, a: &Value) -> Result<ToolResult, ToolResult> {
        let s = self.snapshot()?;
        let w = words(a["query"].as_str());
        let include_imported = a["include_imported"].as_bool().unwrap_or(false);
        let limit = a["limit"].as_u64().unwrap_or(20).clamp(1, 50) as usize;
        let imported: std::collections::HashSet<String> = s.threads.iter().filter_map(|t| t.session_id.as_ref().map(|x| x.to_lowercase())).collect();
        let folder = s.scope.as_ref().map(|p| p.path.clone());
        let mut lines = vec![];
        for file in yh_core::library::session_files(&yh_core::history::config_directory()).into_iter().take(300) {
            if lines.len() >= limit {
                break;
            }
            if !include_imported && imported.contains(&file.session_id.to_lowercase()) {
                continue;
            }
            let (cwd, title) = yh_core::history::peek(&file.path);
            if let Some(f) = &folder {
                if !cwd.as_deref().is_some_and(|c| c == f || c.starts_with(&format!("{}/", f.trim_end_matches('/')))) {
                    continue;
                }
            }
            let title = title.unwrap_or_else(|| "Untitled".into());
            if !w.iter().all(|x| title.to_lowercase().contains(x)) {
                continue;
            }
            let mut line = format!("- {} · “{}” · {} · in {}", file.session_id, one_line(&title, 90), ago(file.modified), cwd.unwrap_or_else(|| "an unknown folder".into()));
            if imported.contains(&file.session_id.to_lowercase()) {
                line += " · already imported";
            }
            lines.push(line);
        }
        if lines.is_empty() {
            return Ok(ToolResult::text(match folder {
                Some(f) => format!("No Claude Code sessions to import from {f}."),
                None => "No Claude Code sessions to import.".into(),
            }));
        }
        Ok(ToolResult::text(format!("{} Claude Code session{}, most recent first:\n{}", lines.len(), if lines.len() == 1 { "" } else { "s" }, lines.join("\n"))))
    }

    fn import(&self, a: &Value) -> Result<ToolResult, ToolResult> {
        let Some(session) = a["session_id"].as_str().and_then(crate::state::workspace::session_id_in) else {
            return Err(ToolResult::error("Give session_id: one from list_claude_code_sessions."));
        };
        let project = a["project"].as_str().map(str::trim).filter(|p| !p.is_empty()).map(str::to_string);
        match self.ask(|reply| ToolRequest::Import { caller: self.caller.clone(), session, project, reply }) {
            Some(Ok((id, count))) => {
                let s = self.snapshot()?;
                let Some(t) = s.threads.iter().find(|t| t.id == id) else { return Err(ToolResult::error("Couldn't import that session.")) };
                if count == usize::MAX {
                    return Ok(ToolResult::text(format!("Already imported: {}", self.describe(&s, t, false))));
                }
                let messages = if count == 1 { "1 message is".to_string() } else { format!("{count} messages are") };
                Ok(ToolResult::text(format!("Imported as {}\n{messages} in the thread; it continues the session when the user sends it a message.", self.describe(&s, t, false))))
            }
            Some(Err(e)) => Err(ToolResult::error(e)),
            None => Err(ToolResult::error("Couldn't import that session.")),
        }
    }
}

impl ThreadTools {
    /// The caller's user and assistant messages as saved, oldest first: what a title is
    /// written from.
    fn title_messages(&self) -> Vec<(bool, String)> {
        let Some(db) = &self.db else { return vec![] };
        let total = db.transcript_count(&self.caller);
        db.read_rows(&self.caller, 0, total)
            .into_iter()
            .filter_map(yh_core::library::EntryData::from_row)
            .filter_map(|e| match e.kind {
                Kind::User => Some((true, e.text)),
                Kind::Assistant => Some((false, e.text)),
                _ => None,
            })
            .collect()
    }
}

impl ToolServer for ThreadTools {
    fn name(&self) -> &str {
        "threads"
    }

    fn tools(&self) -> Vec<Value> {
        let thread_id = json!({ "type": "string", "description": "The thread's id from list_threads or search_threads (its first 8 characters are enough)." });
        let mut tools = vec![
            tool("list_threads", "List the user's threads in YourHarness that you can reach, most recent first, with their id, project, state and when they last changed.",
                schema(json!({
                    "query": { "type": "string", "description": "Only threads whose title has all these words." },
                    "project": { "type": "string", "description": "Only this project's threads (name or id). From Home only." },
                    "include_settled": { "type": "boolean", "description": "Include settled threads (finished work on the shelf). Default false." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 200, "description": "Default 50." },
                }), &[]), true, false),
            tool("read_thread", "Read a thread's conversation: messages, Claude's replies and its tool calls, numbered. Without offset, the last entries.",
                schema(json!({
                    "thread_id": thread_id,
                    "offset": { "type": "integer", "minimum": 0, "description": "The entry number to start at (from a search hit, say)." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "description": "How many entries. Default 30." },
                }), &["thread_id"]), true, false),
            tool("search_threads", "Search what was said in the threads you can reach (messages, replies, commands). Each hit gives the thread and entry number for read_thread.",
                schema(json!({
                    "query": { "type": "string", "description": "Words to find; the last one may be a prefix." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "description": "Default 20." },
                }), &["query"]), true, false),
            tool("rename_thread", "Rename a thread, as the user would in the sidebar.",
                schema(json!({ "thread_id": thread_id, "title": { "type": "string", "description": "The new title: short, says what the thread is about." } }), &["thread_id", "title"]), false, false),
            tool("settle_thread", "Settle a thread (move finished work to its project's Settled shelf; the conversation stays and can be picked up again), or bring a settled one back.",
                schema(json!({ "thread_id": thread_id, "settled": { "type": "boolean", "description": "False brings it back. Default true." } }), &["thread_id"]), false, false),
            tool("pin_thread", "Pin a thread to the top of its project in the sidebar, or unpin it.",
                schema(json!({ "thread_id": thread_id, "pinned": { "type": "boolean", "description": "False unpins. Default true." } }), &["thread_id"]), false, false),
            tool("list_claude_code_sessions", "List sessions from Claude Code's own history (claude in a terminal or an editor) that you can import, most recent first, with the folder each ran in.",
                schema(json!({
                    "query": { "type": "string", "description": "Only sessions whose title or first message has all these words." },
                    "include_imported": { "type": "boolean", "description": "Include sessions that already have a thread. Default false." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "description": "Default 20." },
                }), &[]), true, false),
            tool("import_claude_code_session", "Import a Claude Code session as a thread: its history shows in the thread, which continues the session (claude --resume) when the user sends a message. The user isn't taken to it.",
                schema(json!({
                    "session_id": { "type": "string", "description": "From list_claude_code_sessions." },
                    "project": { "type": "string", "description": "From Home only: the project to put it in (name or id). Default: the project for the folder it ran in, else Home." },
                }), &["session_id"]), false, false),
        ];
        if self.controls {
            tools.extend([
            tool("start_thread", "Start a new thread in this project and send it a first message, to hand it a piece of work. It runs on its own, in the project's permission mode; the user isn't taken to it. Returns its id for wait_for_threads, read_thread and send_to_thread.",
                schema(json!({
                    "message": { "type": "string", "description": "Its first message. It knows nothing of this conversation: say the goal, where to look, what done means and what to leave alone." },
                    "title": { "type": "string", "description": "Its sidebar title. Default: written from the message, as for any thread." },
                    "model": { "type": "string", "description": "haiku, sonnet, opus or a full model id. Default: the user's default." },
                    "effort": { "type": "string", "enum": EFFORTS, "description": "Default: the user's default." },
                    "project": { "type": "string", "description": "From Home only: the project to start it in (name or id). Default: Home." },
                }), &["message"]), false, false),
            tool("send_to_thread", "Send a message to another thread, as if the user typed it in its composer: it starts a turn, or queues behind the one running.",
                schema(json!({
                    "thread_id": thread_id,
                    "message": { "type": "string" },
                    "now": { "type": "boolean", "description": "If it's working, have it read this at its next step instead of after the turn. Default false." },
                }), &["thread_id", "message"]), false, false),
            tool("stop_thread", "Stop another thread's turn, as its Stop button does.",
                schema(json!({ "thread_id": thread_id }), &["thread_id"]), false, false),
            tool("wait_for_threads", "Wait until threads finish their turns or need the user, then get each one's state and last reply (or what it waits for). Returns after timeout_seconds with the ones still working; call again to keep waiting.",
                schema(json!({
                    "thread_ids": { "type": "array", "items": { "type": "string" }, "description": "The threads' ids." },
                    "any": { "type": "boolean", "description": "Return as soon as one of them is done. Default false: when all are." },
                    "timeout_seconds": { "type": "integer", "minimum": 1, "maximum": 600, "description": "Default 300." },
                }), &["thread_ids"]), true, false),
            tool("answer_thread", "Answer what another thread waits on, as the user would from its composer: a tool permission (allow or deny, with a reason it reads), a question it asks (answers), a plan (approve into a mode, or send back with feedback), an MCP server's question. wait_for_threads lists what each thread waits on with its request id.",
                schema(json!({
                    "thread_id": thread_id,
                    "request_id": { "type": "string", "description": "From wait_for_threads. Default: the oldest waiting." },
                    "decision": { "type": "string", "enum": ["allow", "deny"], "description": "allow: allow the tool, answer the question, approve the plan, accept the MCP question. deny: deny, skip, keep planning, decline." },
                    "message": { "type": "string", "description": "Denying a permission: why, for it to read. A plan: feedback sent with it." },
                    "answers": { "type": "object", "additionalProperties": { "type": "string" }, "description": "Answering a question: each question's text to the chosen option's label, or your own words." },
                    "mode": { "type": "string", "enum": PLAN_MODES, "description": "Approving a plan: the mode it then works in; it becomes the project's mode, as the plan card's does. Default acceptEdits." },
                }), &["thread_id", "decision"]), false, false),
            ]);
        }
        if self.updates_title {
            tools.push(tool("update_title", "Have this thread's title rewritten because what the thread is about has changed: the user turned to a different task, or a vague start turned out to be about something specific. YourHarness's title model writes it from the conversation, in the background, and keeps the current title if it's still accurate. Not for progress on the same subject, and not for other threads.",
                schema(json!({ "hint": { "type": "string", "description": "What the thread is about now, in a few words: a hint for the title model, not the title itself." } }), &[]), false, true));
        }
        tools
    }

    fn call(&self, tool: &str, a: &Value, _: Option<&str>) -> ToolResult {
        // Turned off since this thread's `claude` started: its tools list still has them.
        if matches!(tool, "start_thread" | "send_to_thread" | "stop_thread" | "wait_for_threads" | "answer_thread") && !(self.controls && self.snapshot().is_ok_and(|s| s.controls)) {
            return ToolResult::error("The user turned off controlling other threads (Settings → Threads).");
        }
        let result = match tool {
            "list_threads" => self.list(a),
            "read_thread" => self.read(a),
            "search_threads" => self.search(a),
            "rename_thread" => self.rename(a),
            "settle_thread" => self.settle(a),
            "pin_thread" => self.pin(a),
            "list_claude_code_sessions" => self.sessions(a),
            "import_claude_code_session" => self.import(a),
            "start_thread" => self.start(a),
            "send_to_thread" => self.send_to(a),
            "stop_thread" => self.stop_other(a),
            "wait_for_threads" => self.wait(a),
            "answer_thread" => self.answer(a),
            "update_title" => {
                let hint = a["hint"].as_str().map(|h| one_line(h, 300));
                let messages = self.title_messages();
                let sent = messages.iter().filter(|m| m.0).count();
                Ok(ToolResult::text(
                    self.ask(|reply| ToolRequest::UpdateTitle { caller: self.caller.clone(), messages, sent, hint, reply })
                        .unwrap_or_else(|| "This thread is gone.".into()),
                ))
            }
            other => Err(ToolResult::error(format!("No tool named {other}."))),
        };
        result.unwrap_or_else(|e| e)
    }
}
