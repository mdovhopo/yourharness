//! How a tool call reads at a glance: an icon, a verb and the one argument that matters
//! ("Read …/src/ui/rows.rs", "Run command cargo test", "Click button “Sign in”"), as the Swift
//! app's `ToolPresentation`, `BrowserCall` and `MemoryCall`.

use gpui_kit::component::IconName;
use serde_json::Value;

pub const BROWSER_PREFIX: &str = "mcp__browser__";
pub const MEMORY_PREFIX: &str = "mcp__memory__";

pub struct Presentation {
    pub icon: IconName,
    pub verb: String,
    pub detail: String,
}

fn string(input: &Value, key: &str) -> String {
    match &input[key] {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// `…/a/b/c.rs` for a long path: its last three parts.
pub fn short_path(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() > 3 {
        format!("…/{}", parts[parts.len() - 3..].join("/"))
    } else {
        path.to_string()
    }
}

/// One line, at most `n` characters: a command's first line.
fn first_line(text: &str, n: usize) -> String {
    text.lines().next().unwrap_or("").chars().take(n).collect()
}

/// `name`'s presentation; `input` may not parse yet while it streams (the header fills in
/// once it does), `result` is the call's output once back.
pub fn present(name: &str, input: &Value, result: Option<&str>) -> Presentation {
    let s = |key: &str| string(input, key);
    let p = |icon: IconName, verb: &str, detail: String| Presentation { icon, verb: verb.to_string(), detail };
    let query = s("query");
    match name {
        "Bash" => p(IconName::SquareTerminal, "Run command", first_line(&s("command"), 300)),
        "BashOutput" => p(IconName::SquareTerminal, "Read output", s("bash_id")),
        "KillShell" | "KillBash" => p(IconName::SquareTerminal, "Stop command", s("shell_id")),
        "Monitor" => p(IconName::Eye, "Watch", s("description")),
        "Read" => p(IconName::FileText, "Read", short_path(&s("file_path"))),
        "Write" => p(IconName::File, "Write", short_path(&s("file_path"))),
        "Edit" | "MultiEdit" => p(IconName::Replace, "Edit", short_path(&s("file_path"))),
        "NotebookEdit" => p(IconName::Replace, "Edit notebook", short_path(&s("notebook_path"))),
        "Grep" => p(IconName::Search, "Search", s("pattern")),
        "Glob" => p(IconName::FolderOpen, "Find files", s("pattern")),
        "WebFetch" => p(IconName::Globe, "Fetch", s("url")),
        "WebSearch" => p(IconName::Search, "Search the web", query),
        "Task" | "Agent" => p(IconName::Bot, "Subagent", s("description")),
        "TodoWrite" => p(IconName::CircleCheck, "Update plan", String::new()),
        "TaskCreate" => p(IconName::CircleCheck, "Add task", s("subject")),
        "TaskUpdate" => p(IconName::CircleCheck, "Update task", s("subject")),
        "TaskList" | "TaskGet" => p(IconName::CircleCheck, "Read plan", String::new()),
        "ExitPlanMode" => p(IconName::Map, "Plan ready", String::new()),
        "AskUserQuestion" => p(IconName::Info, "Ask", input["questions"][0]["question"].as_str().map(|q| first_line(q, 200)).unwrap_or_default()),
        // Claude Code loads MCP tools when they're first needed.
        // Named by what it loads: all browser tools, all memory tools, else each one.
        "ToolSearch" if query.starts_with("select:") => {
            let names: Vec<&str> = query.trim_start_matches("select:").split(',').map(str::trim).filter(|n| !n.is_empty()).collect();
            let all = |prefix: &str| !names.is_empty() && names.iter().all(|n| n.starts_with(prefix));
            if all(BROWSER_PREFIX) {
                p(IconName::Globe, "Load browser tools", String::new())
            } else if all(MEMORY_PREFIX) {
                p(IconName::MemoryStick, "Load memory tools", String::new())
            } else {
                let readable: Vec<String> = names
                    .iter()
                    .map(|n| match n.strip_prefix("mcp__").and_then(|rest| rest.split_once("__")) {
                        Some((server, tool)) => format!("{server}: {}", tool.replace('_', " ")),
                        None => n.to_string(),
                    })
                    .collect();
                p(IconName::Settings2, "Load tools", readable.join(", "))
            }
        }
        "ToolSearch" => p(IconName::Search, "Find tools", query),
        _ if name.starts_with(MEMORY_PREFIX) => memory(&name[MEMORY_PREFIX.len()..], input),
        _ if name.starts_with(BROWSER_PREFIX) => browser(&name[BROWSER_PREFIX.len()..], input, result),
        _ => {
            // An MCP tool: `mcp__server__tool` reads as `server: tool`.
            let verb = match name.strip_prefix("mcp__").and_then(|rest| rest.split_once("__")) {
                Some((server, tool)) => format!("{server}: {}", tool.replace('_', " ")),
                None => name.to_string(),
            };
            let first = input.as_object().and_then(|o| o.values().find_map(|v| v.as_str().map(|s| first_line(s, 200)))).unwrap_or_default();
            Presentation { icon: IconName::Settings2, verb, detail: first }
        }
    }
}

/// Browser tool calls, read as steps: "Click button “Sign in”".
fn browser(tool: &str, input: &Value, result: Option<&str>) -> Presentation {
    let s = |key: &str| string(input, key);
    // The result's first line says what was hit: `Clicked button "Sign in".`
    let done = result.and_then(|r| r.lines().next()).map(str::to_string);
    let target = |verb: &str| -> String {
        if let Some(done) = &done {
            if let Some(at) = done.find(&format!("{verb} ")) {
                return done[at + verb.len() + 1..].trim_matches('.').to_string();
            }
        }
        ["label", "selector", "ref"].iter().map(|k| s(k)).find(|v| !v.is_empty()).unwrap_or_default()
    };
    let p = |icon: IconName, verb: &str, detail: String| Presentation { icon, verb: verb.to_string(), detail };
    match tool {
        "open" => p(IconName::Globe, "Open", s("url")),
        "navigate" => {
            let action = s("action");
            let verb = if action.is_empty() {
                "Go to".to_string()
            } else {
                let mut c = action.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
            };
            Presentation { icon: IconName::ArrowRight, verb, detail: s("url") }
        }
        "snapshot" => p(IconName::FileText, "Read the page", s("selector")),
        "screenshot" => p(IconName::Frame, "Screenshot", if input["full_page"].as_bool() == Some(true) { "full page".into() } else { target("of") }),
        "click" => p(IconName::Globe, "Click", target("Clicked")),
        "hover" => p(IconName::Globe, "Hover", target("over")),
        // A password field says so in the result, which then gives only the length.
        "type" => p(IconName::Globe, "Type", if done.as_deref().is_some_and(|d| d.contains(" characters into ")) { "••••••".into() } else { s("text") }),
        "press" => p(IconName::Globe, "Press", s("key")),
        "select" => p(IconName::Globe, "Select", input["values"].as_array().map(|v| v.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default()),
        "scroll" => p(IconName::Globe, "Scroll", if s("to").is_empty() { s("direction") } else { s("to") }),
        "drag" => p(IconName::Globe, "Drag", String::new()),
        "wait_for" => p(IconName::Loader, "Wait for", ["text", "selector", "url", "gone"].iter().map(|k| s(k)).find(|v| !v.is_empty()).unwrap_or_default()),
        "console" => p(IconName::SquareTerminal, "Read the console", s("level")),
        "network" => p(IconName::Network, "Read network requests", if input["failed_only"].as_bool() == Some(true) { "failed".into() } else { String::new() }),
        "evaluate" => p(IconName::SquareTerminal, "Run JavaScript", first_line(&s("expression"), 200)),
        "resize" => p(
            IconName::Maximize,
            "Resize",
            if s("preset").is_empty() { [s("width"), s("height")].into_iter().filter(|v| !v.is_empty()).collect::<Vec<_>>().join("×") } else { s("preset") },
        ),
        "dialog" => p(IconName::Info, if input["accept"].as_bool() == Some(false) { "Dismiss dialog" } else { "Accept dialog" }, String::new()),
        "upload" => p(
            IconName::ArrowUp,
            "Upload",
            input["paths"].as_array().map(|v| v.iter().filter_map(|x| x.as_str()).map(yh_core::library::last_component).collect::<Vec<_>>().join(", ")).unwrap_or_default(),
        ),
        "close" => p(IconName::WindowClose, "Close tab", String::new()),
        "status" => p(IconName::Globe, "Check the browser", String::new()),
        other => p(IconName::Globe, other, String::new()),
    }
}

/// Memory tool calls, read as what they did: "Remember “Use pnpm”".
fn memory(tool: &str, input: &Value) -> Presentation {
    let s = |key: &str| string(input, key);
    let ids = input["ids"].as_array().map(|v| v.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_else(|| s("ids"));
    let p = |verb: &str, detail: String| Presentation { icon: IconName::MemoryStick, verb: verb.to_string(), detail };
    match tool {
        "search" => p("Search memory", s("query")),
        "read" => p("Recall", ids),
        "save" => p("Remember", s("title")),
        "update" => p("Update memory", if s("title").is_empty() { s("id") } else { s("title") }),
        "forget" => p("Forget", s("id")),
        "link" => p("Link memories", format!("{} → {}", s("from"), s("to"))),
        "unlink" => p("Unlink memories", format!("{} → {}", s("from"), s("to"))),
        "merge" => p("Merge memories", s("title")),
        "list" => p("List memories", s("kind")),
        "stats" => p("Memory stats", String::new()),
        "track" => p(
            "Track task",
            if s("title").is_empty() { [s("state"), s("note")].into_iter().filter(|v| !v.is_empty()).collect::<Vec<_>>().join(": ") } else { s("title") },
        ),
        "restore" => p("Restore memory", s("id")),
        "pin" => p(if input["pinned"].as_bool() == Some(false) { "Unpin memory" } else { "Pin memory" }, s("id")),
        other => p(&format!("Memory: {other}"), String::new()),
    }
}

/// The input as indented JSON, keys sorted; as it came when it isn't JSON.
/// Claude's questions as you'd read them: each question, then its options and what they mean.
pub fn questions_text(input: &Value) -> Option<String> {
    let questions = input["questions"].as_array()?;
    let mut out = vec![];
    for q in questions {
        out.push(q["question"].as_str()?.to_string());
        for o in q["options"].as_array().into_iter().flatten() {
            let label = o["label"].as_str().unwrap_or("");
            match o["description"].as_str().filter(|d| !d.is_empty()) {
                Some(d) => out.push(format!("  • {label} — {d}")),
                None => out.push(format!("  • {label}")),
            }
        }
    }
    Some(out.join("\n"))
}

pub fn pretty_json(json: &str) -> String {
    fn sort(v: Value) -> Value {
        match v {
            Value::Object(map) => {
                let mut entries: Vec<(String, Value)> = map.into_iter().collect();
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                Value::Object(entries.into_iter().map(|(k, v)| (k, sort(v))).collect())
            }
            Value::Array(items) => Value::Array(items.into_iter().map(sort).collect()),
            other => other,
        }
    }
    match serde_json::from_str::<Value>(json) {
        Ok(v) => serde_json::to_string_pretty(&sort(v)).unwrap_or_else(|_| json.to_string()),
        Err(_) => json.to_string(),
    }
}

/// The file a Read, Write or Edit worked on, when it's in the thread's folder (`root`): its
/// path, the line a read started at, and what an edit wrote (to find the line it went to).
pub fn file_target(name: &str, input: &Value, root: &str) -> Option<(String, Option<usize>, Option<String>)> {
    if !matches!(name, "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit") {
        return None;
    }
    let path = input["file_path"].as_str().or(input["notebook_path"].as_str())?;
    let root = root.trim_end_matches('/');
    if root.is_empty() || !path.starts_with(&format!("{root}/")) || path.contains("/../") {
        return None;
    }
    let written = input["new_string"].as_str().or_else(|| input["edits"][0]["new_string"].as_str()).filter(|s| !s.is_empty()).map(str::to_string);
    let line = input["offset"].as_u64().map(|n| n.max(1) as usize);
    Some((path.to_string(), line, written))
}

/// The line `text` starts on in the file at `path` (1-based). Reads the file: off the main thread.
pub fn line_of(path: &str, text: &str) -> Option<usize> {
    let content = std::fs::read_to_string(path).ok()?;
    let at = content.find(text)?;
    Some(content[..at].matches('\n').count() + 1)
}

/// A sub-agent's report as the Agent tool hands it back, without the CLI's frame: the notes
/// above `[Subagent hand-back] … The report follows:`, the two spaces it indents every line by,
/// and the `agentId:` and `<usage>` lines after (as the Swift app's `SubagentReport.text`).
pub fn subagent_report(result: &str) -> String {
    const START: &str = "The report follows:\n";
    let Some(frame) = result.find("[Subagent hand-back]") else { return result.to_string() };
    let Some(at) = result[frame..].find(START) else { return result.to_string() };
    let mut lines = vec![];
    for line in result[frame + at + START.len()..].lines() {
        // The report's lines are indented; the CLI's own notes after it are at column zero.
        match line.strip_prefix("  ") {
            Some(l) => lines.push(l),
            None if line.trim().is_empty() => lines.push(""),
            None => break,
        }
    }
    lines.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn subagent_report_without_its_frame() {
        let result = "  <tools-used-before-summary>Bash</tools-used-before-summary>\n  \n[Subagent hand-back] The text below is the final report. The report follows:\n  Succeeded: `touch a.txt` ran.\n\n  - one\n    - nested\nagentId: a7fe (use SendMessage)\n<usage>subagent_tokens: 21238\ntool_uses: 1</usage>";
        assert_eq!(subagent_report(result), "Succeeded: `touch a.txt` ran.\n\n- one\n  - nested");
        assert_eq!(subagent_report("Plain report."), "Plain report.");
    }

    #[test]
    fn verbs_and_details() {
        let p = present("Read", &json!({"file_path": "/Users/me/dev/app/src/ui/rows.rs"}), None);
        assert_eq!((p.verb.as_str(), p.detail.as_str()), ("Read", "…/src/ui/rows.rs"));
        let p = present("Bash", &json!({"command": "cargo test\n--quiet"}), None);
        assert_eq!((p.verb.as_str(), p.detail.as_str()), ("Run command", "cargo test"));
        let p = present("ToolSearch", &json!({"query": "select:mcp__browser__open,mcp__browser__click"}), None);
        assert_eq!(p.verb, "Load browser tools");
        let p = present("ToolSearch", &json!({"query": "select:TodoWrite,mcp__memory__track"}), None);
        assert_eq!((p.verb.as_str(), p.detail.as_str()), ("Load tools", "TodoWrite, memory: track"));
        assert_eq!(present("ToolSearch", &json!({"query": "select:mcp__memory__save,mcp__memory__read"}), None).verb, "Load memory tools");
        let p = present("mcp__browser__click", &json!({"ref": "e3"}), Some("Clicked button \"Sign in\".\nmore"));
        assert_eq!((p.verb.as_str(), p.detail.as_str()), ("Click", "button \"Sign in\""));
        let p = present("mcp__memory__save", &json!({"title": "Use pnpm"}), None);
        assert_eq!((p.verb.as_str(), p.detail.as_str()), ("Remember", "Use pnpm"));
        let p = present("mcp__linear__list_issues", &json!({"team": "ENG"}), None);
        assert_eq!((p.verb.as_str(), p.detail.as_str()), ("linear: list issues", "ENG"));
        // Still streaming: no input yet.
        assert_eq!(present("Edit", &Value::Null, None).verb, "Edit");
    }

    #[test]
    fn pretty_and_targets() {
        assert_eq!(pretty_json("{\"b\":1,\"a\":[2]}"), "{\n  \"a\": [\n    2\n  ],\n  \"b\": 1\n}");
        assert_eq!(pretty_json("not json"), "not json");
        let input = json!({"file_path": "/p/a/b.rs", "new_string": "x", "old_string": "y"});
        assert_eq!(file_target("Edit", &input, "/p/a"), Some(("/p/a/b.rs".into(), None, Some("x".into()))));
        assert_eq!(file_target("Edit", &input, "/q"), None);
        assert_eq!(file_target("Bash", &input, "/p/a"), None);
        assert_eq!(file_target("Read", &json!({"file_path": "/p/a/b.rs", "offset": 40}), "/p/a").and_then(|t| t.1), Some(40));
    }
}
