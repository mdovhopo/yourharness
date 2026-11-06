//! Claude Code's session files (`~/.claude/projects/<folder>/<session>.jsonl`), read the way
//! HarnessCore's `ClaudeSessionArchive` reads them: the live branch only (the chain back from
//! the last message, plus tool results hanging off it), CLI bookkeeping skipped.

use crate::library::{head, EntryData, Kind, TOOL_TEXT_LIMIT};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;

/// The folder and first real message of a session, from the start of its file only.
pub fn peek(path: &Path) -> (Option<String>, Option<String>) {
    let Ok(file) = std::fs::File::open(path) else { return (None, None) };
    let mut data = Vec::with_capacity(256 * 1024);
    let _ = file.take(256 * 1024).read_to_end(&mut data);
    let (mut cwd, mut title) = (None, None);
    for line in data.split(|b| *b == b'\n') {
        let Ok(object) = serde_json::from_slice::<Value>(line) else { continue };
        if cwd.is_none() {
            cwd = object["cwd"].as_str().map(str::to_string);
        }
        if title.is_none() && object["type"] == "user" && object["isMeta"] != true {
            let content = &object["message"]["content"];
            let text = content.as_str().map(str::to_string).or_else(|| {
                content.as_array()?.iter().find(|b| b["type"] == "text")?["text"].as_str().map(str::to_string)
            });
            if let Some(text) = text {
                if !text.starts_with('<') && !text.starts_with("Caveat:") && !text.trim().is_empty() {
                    title = Some(text.lines().next().unwrap_or("").chars().take(80).collect());
                }
            }
        }
        if cwd.is_some() && title.is_some() {
            break;
        }
    }
    (cwd, title)
}

struct Record {
    line: usize,
    range: std::ops::Range<usize>,
    uuid: String,
    parent: Option<String>,
    kind: String,
    boundary: bool,
    tool_result_only: bool,
}

/// The whole session as rows. Long tool text is cut to a head.
pub fn load(path: &Path) -> Vec<EntryData> {
    let Ok(data) = std::fs::read(path) else { return vec![] };
    let mut records: HashMap<String, Record> = HashMap::new();
    let mut last_message: Option<String> = None;
    let mut start = 0;
    for (line_number, line) in data.split(|b| *b == b'\n').enumerate() {
        let range = start..start + line.len();
        start += line.len() + 1;
        if line.is_empty() {
            continue;
        }
        let Ok(object) = serde_json::from_slice::<Value>(line) else { continue };
        let Some(kind) = object["type"].as_str() else { continue };
        if matches!(kind, "custom-title" | "ai-title" | "summary") || object["isSidechain"] == true {
            continue;
        }
        let Some(uuid) = object["uuid"].as_str() else { continue };
        let boundary = kind == "system" && object["subtype"] == "compact_boundary";
        let parent = object["parentUuid"]
            .as_str()
            .or_else(|| if boundary { object["logicalParentUuid"].as_str() } else { None })
            .map(str::to_string);
        let tool_result_only = kind == "user"
            && object["message"]["content"]
                .as_array()
                .is_some_and(|b| !b.is_empty() && b.iter().all(|x| x["type"] == "tool_result"));
        if kind == "user" || kind == "assistant" {
            last_message = Some(uuid.to_string());
        }
        records.insert(
            uuid.to_string(),
            Record {
                line: line_number,
                range,
                uuid: uuid.to_string(),
                parent,
                kind: kind.to_string(),
                boundary,
                tool_result_only,
            },
        );
    }

    let mut live = HashSet::new();
    let mut cursor = last_message;
    while let Some(uuid) = cursor {
        if live.contains(&uuid) {
            break;
        }
        let Some(record) = records.get(&uuid) else { break };
        cursor = record.parent.clone();
        live.insert(uuid);
    }
    let mut kept: Vec<&Record> = records
        .values()
        .filter(|r| live.contains(&r.uuid) || (r.tool_result_only && r.parent.as_ref().is_some_and(|p| live.contains(p))))
        .collect();
    kept.sort_by_key(|r| r.line);

    let mut entries: Vec<EntryData> = vec![];
    let mut tools: HashMap<String, usize> = HashMap::new();
    for record in kept {
        let id = format!("h{}", entries.len());
        if record.boundary {
            entries.push(EntryData::new(id, Kind::Notice, "Conversation compacted"));
            continue;
        }
        if record.kind != "user" && record.kind != "assistant" {
            continue;
        }
        let Ok(object) = serde_json::from_slice::<Value>(&data[record.range.clone()]) else { continue };
        let message = &object["message"];
        if record.kind == "assistant" {
            let synthetic = message["model"] == "<synthetic>";
            for block in message["content"].as_array().into_iter().flatten() {
                match block["type"].as_str() {
                    Some("text") => {
                        let text = block["text"].as_str().unwrap_or("").trim();
                        // The CLI's stand-in reply for a turn that was stopped, not Claude's.
                        if text.is_empty() || (synthetic && text == "No response requested.") {
                            continue;
                        }
                        entries.push(EntryData::new(format!("h{}", entries.len()), Kind::Assistant, text));
                    }
                    Some("thinking") => {
                        // Usually redacted to a signature; only real text is worth showing.
                        let text = block["thinking"].as_str().unwrap_or("");
                        if !text.is_empty() {
                            entries.push(EntryData::new(format!("h{}", entries.len()), Kind::Thinking, text));
                        }
                    }
                    Some("tool_use") => {
                        let tool_id = block["id"].as_str().unwrap_or("").to_string();
                        let mut entry = EntryData::new(
                            if tool_id.is_empty() { format!("h{}", entries.len()) } else { tool_id.clone() },
                            Kind::Tool,
                            "",
                        );
                        entry.tool_name = block["name"].as_str().map(str::to_string);
                        entry.tool_input = Some(head(&block["input"].to_string(), TOOL_TEXT_LIMIT));
                        tools.insert(tool_id, entries.len());
                        entries.push(entry);
                    }
                    _ => {}
                }
            }
            continue;
        }
        // User messages: what you typed, tool results, and CLI bookkeeping to skip.
        if object["isMeta"] == true || object["isCompactSummary"] == true {
            continue;
        }
        let mut texts: Vec<String> = vec![];
        let mut has_image = false;
        match &message["content"] {
            Value::String(s) => texts.push(s.clone()),
            Value::Array(blocks) => {
                for block in blocks {
                    match block["type"].as_str() {
                        Some("text") => texts.push(block["text"].as_str().unwrap_or("").to_string()),
                        Some("image") => has_image = true,
                        Some("tool_result") => {
                            let Some(&index) = block["tool_use_id"].as_str().and_then(|id| tools.get(id)) else { continue };
                            let content = tool_result_text(&block["content"]);
                            let entry = &mut entries[index];
                            entry.result_len = content.len();
                            entry.tool_result = Some(head(&content, TOOL_TEXT_LIMIT));
                            entry.is_error = block["is_error"] == true;
                        }
                        _ => {}
                    }
                }
            }
            _ => continue,
        }
        let text = texts.iter().map(|t| visible_text(t)).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n\n");
        let id = format!("h{}", entries.len());
        if text.starts_with("[Request interrupted by user") {
            entries.push(EntryData::new(id, Kind::Notice, "Interrupted"));
        } else if let Some(command) = slash_command(&text) {
            entries.push(EntryData::new(id, Kind::Notice, command));
        } else if !text.is_empty() || has_image {
            entries.push(EntryData::new(id, Kind::User, if text.is_empty() { "[Image]".to_string() } else { text }));
        }
    }
    entries
}

/// A tool result's text: a string, or a list of text blocks.
pub fn tool_result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Strips what the CLI wraps around a prompt (`<system-reminder>`, local command output).
fn visible_text(text: &str) -> String {
    let mut text = text.to_string();
    for tag in [
        "system-reminder",
        "local-command-stdout",
        "local-command-stderr",
        "local-command-caveat",
        "bash-stdout",
        "bash-stderr",
    ] {
        let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
        while let Some(start) = text.find(&open) {
            let Some(end) = text[start..].find(&close) else { break };
            text.replace_range(start..start + end + close.len(), "");
        }
    }
    if let Some(command) = tag_contents("bash-input", &text) {
        return format!("! {command}");
    }
    text.trim().to_string()
}

/// "/compact" for a slash command run in the terminal.
fn slash_command(text: &str) -> Option<String> {
    let name = tag_contents("command-name", text)?;
    let args = tag_contents("command-args", text).unwrap_or_default();
    Some(if args.is_empty() { name } else { format!("{name} {args}") })
}

fn tag_contents(tag: &str, text: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&format!("</{tag}>"))?;
    Some(text[start..start + end].trim().to_string())
}
