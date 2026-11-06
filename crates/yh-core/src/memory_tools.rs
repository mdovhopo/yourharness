//! `mcp__memory__*`: a workspace's memory as tools for its agents. Every call is recorded (what
//! was searched and found, what was read), which is what the Memory page's activity shows. The
//! port of the Swift app's `MemoryToolServer` (its agent tools), with its descriptions and replies.

use crate::mcp::{schema, tool, ToolResult, ToolServer};
use crate::memory::{day, problem, Actor, Memory, Step, Store, KINDS};
use serde_json::{json, Value};

pub struct MemoryTools {
    pub store: Store,
    pub scope: String,
    pub actor: Actor,
    /// Called after a call changed something, from the call's thread.
    pub on_change: Box<dyn Fn() + Send + Sync>,
}

fn kinds() -> Value {
    json!({ "type": "string", "enum": KINDS, "description": "user, feedback, project or reference. Tasks are kept with `track`." })
}

fn clean(v: &Value) -> Option<String> {
    v.as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// IDs as a list, or a list some models send as one string.
fn id_list(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        Value::String(s) => s.split(|c: char| ",; \n[]\"".contains(c)).filter(|x| !x.is_empty()).map(str::to_string).collect(),
        _ => vec![],
    }
}

impl MemoryTools {
    fn resolve(&self, v: &Value) -> Option<String> {
        v.as_str().and_then(|s| self.store.resolve(s, &self.scope))
    }

    fn not_found(&self, v: &Value) -> ToolResult {
        ToolResult::error(format!("No memory with ID {} in this workspace. IDs are in the index, or use `search`.", v.as_str().unwrap_or("?")))
    }

    fn search(&self, a: &Value) -> ToolResult {
        let Some(query) = clean(&a["query"]) else { return ToolResult::error("Say what to search for in `query`.") };
        let limit = a["limit"].as_u64().unwrap_or(10).clamp(1, 30) as usize;
        let hits = self.store.find(&query, Some(&self.scope), a["include_archived"] == true, limit);
        self.store.note_search(&self.scope, &query, hits.iter().map(|(m, _)| m.id.clone()).collect(), &self.actor);
        (self.on_change)();
        if hits.is_empty() {
            return ToolResult::text(format!("No memory matches \"{query}\"."));
        }
        let lines: Vec<String> = hits
            .iter()
            .map(|(m, snippet)| {
                let mut line = format!("- {} [{}] {}", m.short_id(), m.kind, m.title);
                if !m.description.is_empty() {
                    line += &format!(" — {}", m.description);
                }
                if m.status != "active" {
                    line += &format!(" ({})", m.status);
                }
                let snippet = snippet.replace('\n', " ");
                if !snippet.is_empty() && !m.description.contains(&snippet) {
                    line += &format!("\n  {snippet}");
                }
                line
            })
            .collect();
        ToolResult::text(format!(
            "{} match{} for \"{query}\":\n{}\nRead the ones that matter with `read`.",
            hits.len(),
            if hits.len() == 1 { "" } else { "es" },
            lines.join("\n")
        ))
    }

    fn read(&self, a: &Value) -> ToolResult {
        let requested = id_list(if a["ids"].is_null() { &a["id"] } else { &a["ids"] });
        if requested.is_empty() {
            return ToolResult::error("Give the memory IDs in `ids`.");
        }
        let (mut found, mut missing) = (vec![], vec![]);
        for id in requested.iter().take(20) {
            match self.store.resolve(id, &self.scope).and_then(|r| self.store.memory(&r)) {
                Some(m) => found.push(m),
                None => missing.push(id.clone()),
            }
        }
        self.store.note_recall(found.iter().map(|m| m.id.clone()).collect(), &self.actor);
        if found.is_empty() {
            return ToolResult::error(format!("No memory with ID {} here.", missing.join(", ")));
        }
        (self.on_change)();
        let all = self.store.memories(Some(&self.scope), None);
        let mut text = found
            .iter()
            .map(|m| {
                let mut block = format!("## {}\nid: {} · kind: {} · updated {}", m.title, m.short_id(), m.kind, day(m.updated_at));
                if m.status != "active" {
                    block += &format!(" · {}", m.status);
                }
                if let Some(state) = &m.task_state {
                    block += &format!(" · task: {}", crate::memory::state_label(state).to_lowercase());
                }
                if let Some(merged) = &m.merged_into {
                    block += &format!(" · merged into {}", &merged[..8.min(merged.len())]);
                }
                if !m.description.is_empty() {
                    block += &format!("\n{}", m.description);
                }
                if !m.steps.is_empty() {
                    block += &format!("\n\nSteps ({} of {} done):\n", m.steps_done(), m.steps.len());
                    block += &m.steps.iter().map(|s| format!("- [{}] {}", if s.done { "x" } else { " " }, s.text)).collect::<Vec<_>>().join("\n");
                }
                if !m.body.is_empty() {
                    block += &format!("\n\n{}", m.body);
                }
                let links: Vec<String> = m
                    .links
                    .iter()
                    .filter_map(|(to, rel)| {
                        all.iter().find(|x| &x.id == to).map(|x| format!("{} {}{}", x.short_id(), x.title, rel.as_ref().map(|r| format!(" ({r})")).unwrap_or_default()))
                    })
                    .collect();
                if !links.is_empty() {
                    block += &format!("\n\nLinked: {}", links.join("; "));
                }
                block
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        if !missing.is_empty() {
            text += &format!("\n\nNot found: {}.", missing.join(", "));
        }
        ToolResult::text(text)
    }

    fn save(&self, a: &Value, tool_use_id: Option<&str>) -> ToolResult {
        let (Some(title), Some(body)) = (clean(&a["title"]), clean(&a["body"])) else { return ToolResult::error("A memory needs a `title` and a `body`.") };
        let description = clean(&a["description"]).unwrap_or_default();
        if a["kind"] == "task" {
            return ToolResult::error("Keep tasks with `track`.");
        }
        let Some(kind) = a["kind"].as_str().filter(|k| KINDS.contains(k)) else {
            return ToolResult::error("`kind` must be one of: user, feedback, project, reference.");
        };
        if let Some(p) = problem(&title, &description, &body) {
            return ToolResult::error(p);
        }
        if let Some(same) = self.store.memories(Some(&self.scope), Some("active")).into_iter().find(|m| m.title.eq_ignore_ascii_case(&title)) {
            return ToolResult::error(format!("Memory {} already has this title. Update it with `update` instead of saving another.", same.short_id()));
        }
        let mut m = Memory::new(&self.scope, kind, &title, &description, &body, &origin(&self.actor));
        if let (Some(thread), Some(entry)) = (&self.actor.thread, tool_use_id) {
            m.sources.push((thread.clone(), entry.to_string()));
        }
        m.links = id_list(&a["links"]).iter().filter_map(|l| self.store.resolve(l, &self.scope)).map(|to| (to, None)).collect();
        self.store.save(m.clone(), &self.actor, None, None);
        (self.on_change)();
        let mut text = format!("Saved as {}.", m.short_id());
        let similar: Vec<String> =
            self.store.find(&title, Some(&self.scope), false, 4).into_iter().filter(|(x, _)| x.id != m.id).map(|(x, _)| format!("{} {}", x.short_id(), x.title)).collect();
        if !similar.is_empty() {
            text += &format!(" Similar memories: {}. If one says the same thing, merge them by updating it and forgetting this one.", similar.join("; "));
        }
        ToolResult::text(text)
    }

    fn track(&self, a: &Value, tool_use_id: Option<&str>) -> ToolResult {
        let mut task = if let Some(raw) = a["id"].as_str() {
            let Some(found) = self.store.resolve(raw, &self.scope).and_then(|id| self.store.memory(&id)) else { return self.not_found(&a["id"]) };
            if !found.is_task() {
                return ToolResult::error(format!("{} isn't a task; use `update` for memories.", found.short_id()));
            }
            found
        } else {
            let Some(title) = clean(&a["title"]) else { return ToolResult::error("Give the task a `title` (or an `id` to update one).") };
            let open = self.store.memories(Some(&self.scope), Some("active")).into_iter().find(|m| {
                m.is_task() && m.task_state.as_deref() != Some("done") && m.title.eq_ignore_ascii_case(&title)
            });
            open.unwrap_or_else(|| {
                let mut t = Memory::new(&self.scope, "task", &title, "", "", &origin(&self.actor));
                t.task_state = Some("active".into());
                t
            })
        };
        // A title finds the task when there's no ID; with one, it renames it.
        if a["id"].is_string() {
            if let Some(title) = clean(&a["title"]) {
                task.title = title;
            }
        }
        if let Some(d) = a["description"].as_str() {
            task.description = d.trim().to_string();
        }
        if let Some(state) = a["state"].as_str() {
            if !["todo", "active", "blocked", "done"].contains(&state) {
                return ToolResult::error("`state` must be todo, active, blocked or done.");
            }
            task.task_state = Some(state.into());
        }
        if task.task_state.is_none() {
            task.task_state = Some("active".into());
        }
        if let Some(steps) = a["steps"].as_array() {
            task.steps = steps
                .iter()
                .filter_map(|s| {
                    if let Some(t) = s.as_str().map(str::trim).filter(|t| !t.is_empty()) {
                        return Some(Step { text: t.into(), done: false });
                    }
                    let t = s["text"].as_str()?.trim();
                    (!t.is_empty()).then(|| Step { text: t.into(), done: s["done"].as_bool().unwrap_or(false) })
                })
                .collect();
            if task.steps.len() > 40 {
                return ToolResult::error("At most 40 steps: group smaller ones.");
            }
        }
        if let Some(note) = clean(&a["note"]) {
            let line = format!("- {}: {}", day(crate::library::now_ms() / 1000.0), note.replace('\n', " "));
            task.body = if task.body.is_empty() { line } else { format!("{}\n{line}", task.body) };
        }
        if let Some(p) = problem(&task.title, &task.description, &task.body) {
            return ToolResult::error(p);
        }
        if let Some(thread) = &self.actor.thread {
            if !task.sources.iter().any(|(t, _)| t == thread) {
                task.sources.push((thread.clone(), tool_use_id.unwrap_or("").to_string()));
            }
        }
        task.updated_at = crate::library::now_ms() / 1000.0;
        if task.status == "archived" {
            task.status = "active".into();
        }
        self.store.save(task.clone(), &self.actor, None, None);
        (self.on_change)();
        let state = task.task_state.as_deref().unwrap_or("active");
        let mut text = format!("Tracked {} {}: {}", task.short_id(), task.title, crate::memory::state_label(state).to_lowercase());
        if !task.steps.is_empty() {
            text += &format!(", {} of {} steps done", task.steps_done(), task.steps.len());
        }
        if let (Some(next), false) = (task.next_step(), state == "done") {
            text += &format!(". Next: {next}");
        }
        ToolResult::text(text + ".")
    }

    fn update(&self, a: &Value) -> ToolResult {
        let Some(mut m) = self.resolve(&a["id"]).and_then(|id| self.store.memory(&id)) else { return self.not_found(&a["id"]) };
        let mut changed = false;
        if let Some(t) = clean(&a["title"]).filter(|t| *t != m.title) {
            m.title = t;
            changed = true;
        }
        if let Some(d) = a["description"].as_str().map(|d| d.trim().to_string()).filter(|d| *d != m.description) {
            m.description = d;
            changed = true;
        }
        if let Some(b) = clean(&a["body"]).filter(|b| *b != m.body) {
            m.body = b;
            changed = true;
        }
        if let Some(k) = a["kind"].as_str() {
            if !KINDS.contains(&k) {
                return ToolResult::error("`kind` must be one of: user, feedback, project, reference.");
            }
            if k != m.kind {
                m.kind = k.into();
                changed = true;
            }
        }
        if !changed {
            return ToolResult::text(format!("Nothing to change in {}.", m.short_id()));
        }
        if let Some(p) = problem(&m.title, &m.description, &m.body) {
            return ToolResult::error(p);
        }
        m.updated_at = crate::library::now_ms() / 1000.0;
        self.store.save(m.clone(), &self.actor, None, None);
        (self.on_change)();
        ToolResult::text(format!("Updated {}.", m.short_id()))
    }

    fn forget(&self, a: &Value) -> ToolResult {
        let Some(m) = self.resolve(&a["id"]).and_then(|id| self.store.memory(&id)) else { return self.not_found(&a["id"]) };
        self.store.set_status(&m.id, "archived", &self.actor, a["reason"].as_str().map(str::to_string));
        (self.on_change)();
        ToolResult::text(format!("Archived {} {}. Agents no longer get it; it can be restored.", m.short_id(), m.title))
    }

    fn link(&self, a: &Value) -> ToolResult {
        let Some(from) = self.resolve(&a["from"]) else { return self.not_found(&a["from"]) };
        let Some(to) = self.resolve(&a["to"]) else { return self.not_found(&a["to"]) };
        if from == to {
            return ToolResult::error("A memory can't link to itself.");
        }
        self.store.link(&from, &to, a["relation"].as_str().map(str::to_string), &self.actor);
        (self.on_change)();
        ToolResult::text(format!("Linked {} → {}.", from[..8].to_lowercase(), to[..8].to_lowercase()))
    }
}

fn origin(actor: &Actor) -> String {
    match actor.kind.as_str() {
        "importer" => "import",
        "reconciler" => "reconciler",
        "user" => "user",
        _ => "agent",
    }
    .into()
}

impl ToolServer for MemoryTools {
    fn name(&self) -> &str {
        "memory"
    }

    fn tools(&self) -> Vec<Value> {
        let ids = json!({ "type": "array", "items": { "type": "string" }, "minItems": 1, "description": "Memory IDs (the 8-character ones in the index are enough)." });
        vec![
            tool("search", "Search this workspace's memory: words in titles, descriptions and bodies, best match first. Use it when something you should know about isn't in the memory index, or to check before saving a new memory.",
                schema(json!({
                    "query": { "type": "string", "description": "What to look for, in a few words." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 30, "description": "At most this many (10 by default)." },
                    "include_archived": { "type": "boolean", "description": "Also archived memories." },
                }), &["query"]), true, false),
            tool("read", "Read memories in full: their body, kind, links and when they were last updated.", schema(json!({ "ids": ids }), &["ids"]), true, false),
            tool("save", "Save a new memory for every later session in this workspace: one fact, preference, decision or pointer. Check the index or search first; if one already covers it, update that one instead.",
                schema(json!({
                    "title": { "type": "string", "description": "A few words, e.g. \"Release only on request\"." },
                    "description": { "type": "string", "description": "One line on what it says and when it matters: the index shows it." },
                    "body": { "type": "string", "description": "The memory itself, short and self-contained. For feedback and project memories, add **Why:** and **How to apply:** lines." },
                    "kind": kinds(),
                    "links": { "type": "array", "items": { "type": "string" }, "description": "IDs of related memories." },
                }), &["title", "description", "body", "kind"]), false, false),
            tool("update", "Change a memory: give only what changes. The old version is kept.",
                schema(json!({ "id": { "type": "string" }, "title": { "type": "string" }, "description": { "type": "string" }, "body": { "type": "string" }, "kind": kinds() }), &["id"]), false, false),
            tool("forget", "Archive a memory that's wrong or no longer true: agents stop getting it. It can be restored.",
                schema(json!({ "id": { "type": "string" }, "reason": { "type": "string", "description": "Why, in a few words." } }), &["id"]), false, false),
            tool("link", "Link two related memories, so reading one points at the other.",
                schema(json!({ "from": { "type": "string" }, "to": { "type": "string" }, "relation": { "type": "string", "description": "Optional: how they relate, e.g. \"refines\", \"see also\", \"contradicts\"." } }), &["from", "to"]), false, false),
            tool("track", "Keep one of the user's asks as a task, with its steps and where it stands, so this and later sessions know how far it's got. Without `id`, starts a task (or picks up the open one with the same title); with `id`, updates it. Give the whole `steps` list each time, ticking what's done.",
                schema(json!({
                    "id": { "type": "string", "description": "The task to update; leave out to start one." },
                    "title": { "type": "string", "description": "What was asked, in a few words, e.g. \"Move billing to Stripe Checkout\"." },
                    "description": { "type": "string", "description": "One line: the goal and anything that defines done." },
                    "state": { "type": "string", "enum": ["todo", "active", "blocked", "done"], "description": "todo (not started), active (in progress; the default), blocked (waiting on something: say what in `note`), done." },
                    "steps": { "type": "array", "description": "Every step, in order, with what's done ticked.", "items": { "type": "object", "properties": { "text": { "type": "string" }, "done": { "type": "boolean" } }, "required": ["text"] } },
                    "note": { "type": "string", "description": "What just happened, in a line: kept in the task's log with today's date." },
                }), &[]), false, false),
        ]
    }

    fn call(&self, tool: &str, a: &Value, tool_use_id: Option<&str>) -> ToolResult {
        match tool {
            "search" => self.search(a),
            "read" => self.read(a),
            "save" => self.save(a, tool_use_id),
            "track" => self.track(a, tool_use_id),
            "update" => self.update(a),
            "forget" => self.forget(a),
            "link" => self.link(a),
            other => ToolResult::error(format!("No tool named {other}.")),
        }
    }
}
