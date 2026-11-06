//! Thread titles: a one-shot, tool-less request through the official CLI on a small model
//! (Haiku), in an empty temporary folder so it sees only the prompt. The prompt and rules are the
//! Swift app's (`ThreadTitlePrompt`, ported from T3 Code), verbatim.

use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};

pub const SCHEMA: &str = r#"{"type":"object","properties":{"title":{"type":"string"},"needsRefinement":{"type":"boolean"}},"required":["title","needsRefinement"],"additionalProperties":false}"#;

const INITIAL_FRAME: &str = r#"Generate a title that will help the user recognize this YourHarness thread weeks later.
Return JSON with keys title and needsRefinement.
Set needsRefinement to true only if the subject is still unknown, such as an unresolved link, "fix this", or an unexplained attachment. Otherwise set it to false.
"#;

pub const DEFAULT_RULES: &str = r#"Before answering, silently reduce the request to:
- Subject: What system, feature, or problem is this really about?
- Outcome: What does the user ultimately want to understand or change?
- Incidental instructions: What only describes how the agent should do the work?

Title the subject and outcome. Discard incidental instructions.

Editorial rules:
- 3-8 words, fewer than 40 characters.
- Use a compact noun phrase or clear action phrase.
- Capture the umbrella goal when the request lists several symptoms or steps.
- Name the product change, not the mock, plan, report, branch, or PR used to produce it.
- Models, subagents, tools, output formats, and monitoring instructions do not belong in the title unless they are themselves the topic.
- For reviews, name what is being reviewed and the relevant concern. Avoid generic titles such as "Review PR 123" when linked context reveals the subject.
- For research, name the question domain rather than the requested research process.
- Do not claim the work is complete.
- Do not copy and truncate the user's message.
- Avoid project names already visible in the UI, quotes, labels, filler, and trailing punctuation.
- Local git history is not evidence of what a linked PR or issue is about. Never title the thread after branch names, commit messages, or merged commits.
- If a linked PR or issue cannot be read, fall back to the user's stated action plus its number, such as "Take Over PR 8588". This is the one case where a PR or issue number belongs in the title.
"#;

const TRUNCATED: &str = "\n[Content truncated]\n";

/// Keeps the start and end of a long message, which say the most about it.
fn limit(text: &str, budget: usize) -> String {
    let count = text.chars().count();
    if count <= budget {
        return text.to_string();
    }
    let available = budget.saturating_sub(TRUNCATED.chars().count());
    let head = available.div_ceil(2);
    let tail = available - head;
    let start: String = text.chars().take(head).collect();
    let end: String = text.chars().skip(count - tail).collect();
    format!("{start}{TRUNCATED}{end}")
}

/// The prompt for a thread's first message. `rules` (from Settings) replace the built-in ones.
pub fn initial_prompt(message: &str, rules: Option<&str>) -> String {
    let rules = rules.map(str::trim).filter(|r| !r.is_empty()).unwrap_or(DEFAULT_RULES);
    format!("{INITIAL_FRAME}\n\n{rules}\n\nUser message:\n{}", limit(message, 8_000))
}

/// One line, unquoted, whitespace collapsed, at most 120 characters.
pub fn sanitize(raw: &str) -> String {
    let title = serde_json::from_str::<Value>(raw).ok().and_then(|v| v["title"].as_str().map(str::to_string)).unwrap_or_else(|| raw.to_string());
    let first = title.trim().lines().next().unwrap_or("").to_string();
    let unquoted = first.trim_matches(|c: char| c == '\'' || c == '"' || c == '`' || c.is_whitespace());
    let normalized = unquoted.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return "New Thread".into();
    }
    if normalized.chars().count() <= 120 {
        return normalized;
    }
    let cut: String = normalized.chars().take(117).collect();
    format!("{}…", cut.trim_end())
}

/// Runs the request; blocking, so off the main thread. `executable` is `claude`.
pub fn generate(executable: &std::path::Path, path_env: Option<&str>, model: &str, prompt: &str) -> Result<String, String> {
    let folder = std::env::temp_dir().join(format!("yourharness-title-{}", crate::library::new_uuid()));
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let mut command = Command::new(executable);
    command
        .args(["-p", "--output-format", "json", "--json-schema", SCHEMA, "--model", model])
        .args(["--settings", r#"{"disableAllHooks":true}"#, "--tools", "", "--disable-slash-commands", "--strict-mcp-config"])
        .args(["--permission-mode", "dontAsk", "--no-session-persistence"])
        .current_dir(&folder)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = path_env {
        command.env("PATH", path);
    }
    let result = (|| {
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        child.stdin.take().unwrap().write_all(prompt.as_bytes()).map_err(|e| e.to_string())?;
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        let json: Value = serde_json::from_slice(&out.stdout).map_err(|_| String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("no answer").to_string())?;
        let result = match &json {
            Value::Array(list) => list.iter().rev().find(|m| m["type"] == "result").cloned().unwrap_or(Value::Null),
            other => other.clone(),
        };
        if result["is_error"] == true || !out.status.success() {
            return Err(result["result"].as_str().unwrap_or("Claude couldn't write a title").to_string());
        }
        let title = result["structured_output"]["title"]
            .as_str()
            .map(str::to_string)
            .or_else(|| result["result"].as_str().and_then(|t| serde_json::from_str::<Value>(t).ok()).and_then(|v| v["title"].as_str().map(str::to_string)))
            .ok_or("Claude returned an unexpected answer.")?;
        Ok(sanitize(&title))
    })();
    let _ = std::fs::remove_dir_all(&folder);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_like_the_swift_app() {
        assert_eq!(sanitize("  \"Fix   the parser\"\nmore"), "Fix the parser");
        assert_eq!(sanitize(r#"{"title":"Plan the Rewrite"}"#), "Plan the Rewrite");
        assert_eq!(sanitize(""), "New Thread");
    }

    #[test]
    fn long_messages_keep_both_ends() {
        let p = initial_prompt(&"a".repeat(9000), None);
        assert!(p.contains("[Content truncated]"));
        assert!(p.contains("Editorial rules"));
    }
}
