//! `mcp__threads__…`: an agent looks through the app's threads (list, read, search), tidies them
//! (rename, settle, pin) and brings in Claude Code sessions, the way you would from the sidebar
//! and ⌘K. It can't delete anything. A thread in Home reaches every project; a thread anywhere
//! else only its own project's threads, and only sessions that ran in its folder. With title
//! updates on, it can have its own thread's title rewritten when the subject changes.
//! Names, schemas and replies are the Swift app's: agents already use them.
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
}

/// What the calling thread reaches, as the main thread sees it right now.
pub struct Snapshot {
    /// None: it's in Home and reaches everything.
    pub scope: Option<ProjectMeta>,
    pub projects: Vec<ProjectMeta>,
    pub threads: Vec<ThreadInfo>,
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
    UpdateTitle { caller: String, messages: Vec<(bool, String)>, sent: usize, hint: Option<String>, reply: flume::Sender<String> },
}

pub struct ThreadTools {
    pub caller: String,
    pub requests: flume::Sender<ToolRequest>,
    pub db: Option<Arc<Database>>,
    pub updates_title: bool,
}

/// What the agent is told about the thread tools.
pub fn instructions(project: Option<&str>, updates_title: bool) -> String {
    let reach = match project {
        Some(p) => format!("From this thread you reach only the threads of the project {p}, and only Claude Code sessions that ran in its folder; for anything wider, the user can ask from a thread in Home."),
        None => "This thread is in Home, so you reach every project's threads and every Claude Code session.".into(),
    };
    let mut text = format!("The user's conversations in YourHarness are threads, grouped by project in the sidebar. The mcp__threads__ tools work with them: list_threads, read_thread and search_threads find and read them; rename_thread, settle_thread and pin_thread tidy them; list_claude_code_sessions and import_claude_code_session bring in sessions from Claude Code run in a terminal or an editor. {reach}\nReading and searching are fine whenever they help, like picking up what an earlier thread worked out. Changing threads is the user's call: rename, settle, pin or import only what the user asked for, or a list they agreed to. Before changing more than two threads, list exactly what you'll change and wait for a yes. Leave threads that are working or waiting for the user alone unless the user named them. You can't delete threads; if asked, tell the user to do it from the sidebar. Other threads can hold private details or secrets: bring into this conversation only what the task needs. When you're done, say what you changed.");
    if updates_title {
        text += "\n\nKeeping this thread's title right is part of your job; a separate title model writes it, you tell it when to look again. When the user's latest message turns the thread to a different subject than it has been about (a new, unrelated task or question), or a vague start (\"fix this\", a bare link) has turned out to be about something specific, call mcp__threads__update_title before you answer, with a few words on the new subject as the hint. Not for progress on the same subject (exploring, planning, implementing, reviewing, fixing CI, merging), and once per change at most. Don't use rename_thread on this thread unless the user asks for a particular name, and don't mention the title to the user.";
    }
    text
}

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
        if t.unread {
            states.push("unread");
        }
        if t.pinned {
            states.push("pinned");
        }
        if t.settled {
            states.push("settled");
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
        if self.updates_title {
            tools.push(tool("update_title", "Have this thread's title rewritten because what the thread is about has changed: the user turned to a different task, or a vague start turned out to be about something specific. YourHarness's title model writes it from the conversation, in the background, and keeps the current title if it's still accurate. Not for progress on the same subject, and not for other threads.",
                schema(json!({ "hint": { "type": "string", "description": "What the thread is about now, in a few words: a hint for the title model, not the title itself." } }), &[]), false, true));
        }
        tools
    }

    fn call(&self, tool: &str, a: &Value, _: Option<&str>) -> ToolResult {
        let result = match tool {
            "list_threads" => self.list(a),
            "read_thread" => self.read(a),
            "search_threads" => self.search(a),
            "rename_thread" => self.rename(a),
            "settle_thread" => self.settle(a),
            "pin_thread" => self.pin(a),
            "list_claude_code_sessions" => self.sessions(a),
            "import_claude_code_session" => self.import(a),
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
