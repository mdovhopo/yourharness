//! A workspace's memory as files: JSON that brings back everything (for backups and moving
//! between Macs), or a Markdown folder in Claude Code's own memory format (a file a memory with
//! frontmatter, and a `MEMORY.md` index) that people and other tools can read. Both come back in
//! without an agent; anything else goes through the importer. The files are byte for
//! byte the ones 0.0.x writes, so exports move between versions both ways.

use crate::library::{iso, new_uuid, parse_iso};
use crate::memory::{Actor, Memory, Step, Store};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const FORMAT: &str = "yourharness-memory";
pub const VERSION: i64 = 1;

/// Memory kinds in the order the export lists them (as 0.0.x does).
const KINDS: &[&str] = &["user", "feedback", "project", "reference", "task"];

/// `MemoryKind.of`: an unknown kind (from a newer version, or an import) reads as a project memory.
fn kind_of(raw: &str) -> &'static str {
    KINDS.iter().find(|k| **k == raw).copied().unwrap_or("project")
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "user" => "About You",
        "feedback" => "Feedback",
        "project" => "Project",
        "reference" => "Reference",
        _ => "Task",
    }
}

fn status_of(raw: Option<&str>) -> String {
    match raw {
        Some(s @ ("active" | "proposed" | "archived")) => s.to_string(),
        _ => "active".into(),
    }
}

fn task_state_of(raw: Option<&str>) -> Option<String> {
    raw.filter(|s| matches!(*s, "todo" | "active" | "blocked" | "done")).map(str::to_string)
}

fn now() -> f64 {
    crate::library::now_ms() / 1000.0
}

/// A date as the export writes it (0.0.x's format): whole seconds (truncated), UTC, `Z`.
fn date_string(t: f64) -> String {
    iso(t.floor() as i64)
}

// MARK: - JSON

/// The JSON export, in 0.0.x's format: ISO 8601 dates, pretty printed (`" : "` between key and
/// value), sorted keys, slashes not escaped.
pub fn json(memories: &[Memory], workspace: &str, path: Option<&str>, now: f64) -> Vec<u8> {
    let items: Vec<Value> = memories
        .iter()
        .map(|m| {
            let mut o = Map::new();
            o.insert("id".into(), json!(m.id));
            o.insert("kind".into(), json!(m.kind));
            o.insert("title".into(), json!(m.title));
            o.insert("description".into(), json!(m.description));
            o.insert("body".into(), json!(m.body));
            o.insert("status".into(), json!(m.status));
            if m.pinned {
                o.insert("pinned".into(), json!(true));
            }
            if let Some(origin) = &m.origin {
                o.insert("origin".into(), json!(origin));
            }
            o.insert("createdAt".into(), json!(date_string(m.created_at)));
            o.insert("updatedAt".into(), json!(date_string(m.updated_at)));
            o.insert("recallCount".into(), json!(m.recall_count));
            if let Some(t) = m.last_recalled_at {
                o.insert("lastRecalledAt".into(), json!(date_string(t)));
            }
            if !m.links.is_empty() {
                let links = m
                    .links
                    .iter()
                    .map(|(to, relation)| {
                        let mut l = Map::new();
                        l.insert("to".into(), json!(to));
                        if let Some(r) = relation {
                            l.insert("relation".into(), json!(r));
                        }
                        Value::Object(l)
                    })
                    .collect();
                o.insert("links".into(), Value::Array(links));
            }
            if !m.sources.is_empty() {
                let sources = m.sources.iter().map(|(thread, entry)| json!({"threadID": thread, "entryID": entry})).collect();
                o.insert("sources".into(), Value::Array(sources));
            }
            if let Some(state) = &m.task_state {
                o.insert("taskState".into(), json!(state));
            }
            if !m.steps.is_empty() {
                o.insert("steps".into(), Value::Array(m.steps.iter().map(|s| json!({"text": s.text, "done": s.done})).collect()));
            }
            Value::Object(o)
        })
        .collect();
    let mut ws = Map::new();
    ws.insert("name".into(), json!(workspace));
    if let Some(path) = path {
        ws.insert("path".into(), json!(path));
    }
    let document = json!({
        "format": FORMAT,
        "version": VERSION,
        "exportedAt": date_string(now),
        "workspace": Value::Object(ws),
        "memories": Value::Array(items),
    });
    swift_pretty(&document).into_bytes()
}

/// JSON as Foundation pretty prints it with sorted keys: two spaces, `"key" : value`, empty
/// collections as `[\n\n]` at the parent's indent, slashes as they are.
pub fn swift_pretty(value: &Value) -> String {
    let mut out = String::new();
    write_value(value, 0, &mut out);
    out
}

fn write_value(value: &Value, level: usize, out: &mut String) {
    let pad = |n: usize| "  ".repeat(n);
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(",\n");
                }
                out.push_str(&pad(level + 1));
                write_value(item, level + 1, out);
            }
            out.push('\n');
            out.push_str(&pad(level));
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push_str("{\n");
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push_str(",\n");
                }
                out.push_str(&pad(level + 1));
                write_string(key, out);
                out.push_str(" : ");
                write_value(&map[*key], level + 1, out);
            }
            out.push('\n');
            out.push_str(&pad(level));
            out.push('}');
        }
    }
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadError {
    NotAnExport,
    NewerVersion(i64),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::NotAnExport => write!(f, "This isn't a YourHarness memory export."),
            ReadError::NewerVersion(v) => write!(f, "This export is from a newer YourHarness (format {v})."),
        }
    }
}

impl std::error::Error for ReadError {}

/// The memories in an export, for `scope`.
pub fn memories_from_json(data: &[u8], scope: &str) -> Result<Vec<Memory>, ReadError> {
    let document: Value = serde_json::from_slice(data).map_err(|_| ReadError::NotAnExport)?;
    // What 0.0.x needs to read one: a format, a version, when it was exported and the list.
    let format = document["format"].as_str().ok_or(ReadError::NotAnExport)?;
    let version = document["version"].as_i64().ok_or(ReadError::NotAnExport)?;
    document["exportedAt"].as_str().and_then(parse_iso).ok_or(ReadError::NotAnExport)?;
    let items = document["memories"].as_array().ok_or(ReadError::NotAnExport)?;
    if format != FORMAT {
        return Err(ReadError::NotAnExport);
    }
    if version > VERSION {
        return Err(ReadError::NewerVersion(version));
    }
    let date = |v: &Value| v.as_str().and_then(parse_iso);
    let mut memories = vec![];
    for item in items {
        let text = |key: &str| item[key].as_str().map(str::to_string);
        let (Some(id), Some(kind), Some(title), Some(body)) = (text("id"), text("kind"), text("title"), text("body")) else {
            return Err(ReadError::NotAnExport);
        };
        let list = |key: &str| item[key].as_array().cloned().unwrap_or_default();
        memories.push(Memory {
            id,
            scope: scope.into(),
            kind,
            title,
            description: text("description").unwrap_or_default(),
            body,
            status: status_of(item["status"].as_str()),
            pinned: item["pinned"].as_bool().unwrap_or(false),
            origin: text("origin"),
            created_at: date(&item["createdAt"]).unwrap_or_else(now),
            updated_at: date(&item["updatedAt"]).unwrap_or_else(now),
            recall_count: item["recallCount"].as_i64().unwrap_or(0),
            surfaced_count: 0,
            last_recalled_at: date(&item["lastRecalledAt"]),
            merged_into: None,
            task_state: task_state_of(item["taskState"].as_str()),
            steps: list("steps")
                .iter()
                .filter_map(|s| Some(Step { text: s["text"].as_str()?.to_string(), done: s["done"].as_bool()? }))
                .collect(),
            sources: list("sources")
                .iter()
                .filter_map(|s| Some((s["threadID"].as_str()?.to_string(), s["entryID"].as_str()?.to_string())))
                .collect(),
            links: list("links")
                .iter()
                .filter_map(|l| Some((l["to"].as_str()?.to_string(), l["relation"].as_str().map(str::to_string))))
                .collect(),
        });
    }
    Ok(memories)
}

// MARK: - Markdown

/// Characters as people count them (grapheme clusters), near enough: a base scalar with the marks, joiners and selectors that
/// go with it.
fn graphemes(s: &str) -> Vec<&str> {
    fn extends(c: char) -> bool {
        matches!(c as u32,
            0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05BD | 0x0610..=0x061A | 0x064B..=0x065F
            | 0x0900..=0x0903 | 0x093A..=0x094F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x200C | 0x200D
            | 0x20D0..=0x20FF | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F | 0x1F3FB..=0x1F3FF | 0xE0020..=0xE007F | 0xE0100..=0xE01EF)
    }
    let mut out = vec![];
    let (mut start, mut after_joiner) = (None::<usize>, false);
    for (i, c) in s.char_indices() {
        if let Some(st) = start {
            if !(extends(c) || after_joiner) {
                out.push(&s[st..i]);
                start = Some(i);
            }
        } else {
            start = Some(i);
        }
        after_joiner = c == '\u{200D}';
    }
    if let Some(st) = start {
        out.push(&s[st..]);
    }
    out
}

/// File name for each memory (by ID): its title in kebab case, made unique.
pub fn slugs(memories: &[Memory]) -> HashMap<String, String> {
    let (mut used, mut slugs) = (HashSet::new(), HashMap::new());
    for m in memories {
        let lower = m.title.to_lowercase();
        let mapped: String = graphemes(&lower)
            .into_iter()
            .map(|g| {
                let c = g.chars().next().unwrap_or('-');
                if c.is_alphabetic() || c.is_numeric() { g } else { "-" }
            })
            .collect();
        let mut base = mapped.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
        if base.is_empty() {
            base = "memory".into();
        }
        base = graphemes(&base).into_iter().take(60).collect();
        let (mut slug, mut n) = (base.clone(), 2);
        while !used.insert(slug.clone()) {
            slug = format!("{base}-{n}");
            n += 1;
        }
        slugs.insert(m.id.clone(), slug);
    }
    slugs
}

/// The files of a Markdown export, by name: `MEMORY.md` first, then one a memory.
pub fn markdown_files(memories: &[Memory], workspace: &str) -> Vec<(String, String)> {
    let slugs = slugs(memories);
    let line = |m: &Memory| format!("- [{}]({}.md) — {}", m.title, slugs[&m.id], m.description);
    let mut index = format!("# {workspace} memory\n\nExported from YourHarness. One file a memory; links are [[file-name]].\n\n");
    for kind in KINDS {
        let group: Vec<String> = memories.iter().filter(|m| kind_of(&m.kind) == *kind && m.status == "active").map(line).collect();
        if group.is_empty() {
            continue;
        }
        index += &format!("## {}\n\n{}\n\n", kind_label(kind), group.join("\n"));
    }
    let archived: Vec<String> = memories.iter().filter(|m| m.status != "active").map(line).collect();
    if !archived.is_empty() {
        index += &format!("## Archived\n\n{}\n", archived.join("\n"));
    }
    let mut files = vec![("MEMORY.md".to_string(), index)];
    for m in memories {
        let slug = &slugs[&m.id];
        let mut text = format!("---\nname: {slug}\ndescription: {}\nmetadata:\n  type: {}\n", yaml(&m.description), m.kind);
        text += &format!("  title: {}\n  id: {}\n", yaml(&m.title), m.id);
        text += &format!("  created: {}\n  updated: {}\n", date_string(m.created_at), date_string(m.updated_at));
        if m.status != "active" {
            text += &format!("  status: {}\n", m.status);
        }
        if m.pinned {
            text += "  pinned: true\n";
        }
        if let Some(state) = &m.task_state {
            text += &format!("  state: {state}\n");
        }
        text += &format!("---\n\n{}\n", m.body.trim());
        if !m.steps.is_empty() {
            let steps: Vec<String> = m.steps.iter().map(|s| format!("- [{}] {}", if s.done { "x" } else { " " }, s.text)).collect();
            text += &format!("\n## Steps\n\n{}\n", steps.join("\n"));
        }
        let links: Vec<String> = m.links.iter().filter_map(|(to, _)| slugs.get(to)).map(|s| format!("[[{s}]]")).collect();
        if !links.is_empty() {
            text += &format!("\nRelated: {}\n", links.join(" "));
        }
        files.push((format!("{slug}.md"), text));
    }
    files
}

fn yaml(value: &str) -> String {
    let flat = value.replace('\n', " ");
    let needs_quotes = flat.is_empty()
        || flat.contains(": ")
        || flat.contains('#')
        || flat.chars().next().is_some_and(|c| "\"'[]{}>|*&!%@`-".contains(c));
    if needs_quotes {
        format!("\"{}\"", flat.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        flat
    }
}

/// `CharacterSet.whitespaces`: spaces and tabs, not new lines.
fn is_space(c: char) -> bool {
    c == '\t' || (c.is_whitespace() && !matches!(c, '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}'))
}

/// Memories from Markdown files with memory frontmatter (`name`, `description`, a type), as
/// Claude Code and a Markdown export write them. Files without it (a CLAUDE.md, notes) are left
/// out: those need the importer. Titles come from `MEMORY.md` when it names them.
pub fn memories_from_markdown(files: &BTreeMap<String, String>, scope: &str, now: f64) -> Vec<Memory> {
    let titles = index_titles(files.get("MEMORY.md").map(String::as_str).unwrap_or(""));
    let mut parsed: Vec<(String, Memory, Vec<String>)> = vec![];
    for (file, text) in files {
        if file == "MEMORY.md" || !file.ends_with(".md") {
            continue;
        }
        let Some((front, body)) = frontmatter(text) else { continue };
        let name = front.get("name").cloned().unwrap_or_else(|| file[..file.len() - 3].to_string());
        let Some(kind) = front.get("metadata.type").or(front.get("type")) else { continue };
        if !front.contains_key("description") && !front.contains_key("metadata.type") {
            continue;
        }
        let mut clean = body.clone();
        let mut link_names = wiki_links(&body);
        // Our own export's footer comes back as links, not text.
        if let Some(i) = clean.rfind("\nRelated: [[") {
            clean.truncate(i);
        }
        let mut seen = HashSet::new();
        link_names.retain(|n| seen.insert(n.clone()));
        let mut steps = vec![];
        if let Some(i) = clean.find("\n## Steps\n") {
            for line in clean[i + "\n## Steps\n".len()..].split('\n') {
                let trimmed = line.trim_matches(is_space);
                if let Some(rest) = trimmed.strip_prefix("- [x] ").or(trimmed.strip_prefix("- [X] ")) {
                    steps.push(Step { text: rest.into(), done: true });
                } else if let Some(rest) = trimmed.strip_prefix("- [ ] ") {
                    steps.push(Step { text: rest.into(), done: false });
                }
            }
            clean.truncate(i);
        }
        let file_name = file.split('/').filter(|p| !p.is_empty()).next_back().unwrap_or(file);
        let title = front.get("metadata.title").or(titles.get(file_name)).cloned().unwrap_or_else(|| humanize(&name));
        let date = |key: &str| front.get(key).and_then(|s| parse_iso(s));
        let memory = Memory {
            id: front.get("metadata.id").cloned().unwrap_or_else(new_uuid),
            scope: scope.into(),
            kind: kind_of(kind).into(),
            title,
            description: front.get("description").cloned().unwrap_or_default(),
            body: clean.trim().to_string(),
            status: status_of(front.get("metadata.status").map(String::as_str)),
            pinned: front.get("metadata.pinned").is_some_and(|p| p == "true"),
            origin: Some("import".into()),
            created_at: date("metadata.created").unwrap_or(now),
            updated_at: date("metadata.updated").or(date("metadata.modified")).unwrap_or(now),
            recall_count: 0,
            surfaced_count: 0,
            last_recalled_at: None,
            merged_into: None,
            task_state: task_state_of(front.get("metadata.state").map(String::as_str)),
            steps,
            sources: vec![],
            links: vec![],
        };
        parsed.push((name, memory, link_names));
    }
    let mut ids: HashMap<&str, &str> = HashMap::new();
    for (name, m, _) in &parsed {
        ids.entry(name.as_str()).or_insert(m.id.as_str());
    }
    let links: Vec<Vec<(String, Option<String>)>> = parsed
        .iter()
        .map(|(_, m, names)| {
            names.iter().filter_map(|n| ids.get(n.as_str())).filter(|id| **id != m.id).map(|id| (id.to_string(), None)).collect()
        })
        .collect();
    parsed.into_iter().zip(links).map(|((_, mut m, _), links)| {
        m.links = links;
        m
    }).collect()
}

/// Every `.md` file in a folder (not below it), by name.
pub fn read_markdown_folder(folder: &Path) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(folder) else { return files };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".md") {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(entry.path()) {
            files.insert(name, text);
        }
    }
    files
}

/// A file's frontmatter (nested keys as `parent.key`) and the text after it.
pub fn frontmatter(text: &str) -> Option<(HashMap<String, String>, String)> {
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.first()?.trim_matches(is_space) != "---" {
        return None;
    }
    let end = lines.iter().skip(1).position(|l| l.trim_matches(is_space) == "---")? + 1;
    let (mut values, mut parent) = (HashMap::new(), None::<String>);
    for line in &lines[1..end] {
        if line.trim_matches(is_space).is_empty() {
            continue;
        }
        let indented = line.starts_with(' ') || line.starts_with('\t');
        let trimmed = line.trim_matches(is_space);
        let Some(colon) = trimmed.find(':') else { continue };
        let key = trimmed[..colon].trim_matches(is_space).to_string();
        let value = unquote(trimmed[colon + 1..].trim_matches(is_space));
        match &parent {
            Some(p) if indented => {
                values.insert(format!("{p}.{key}"), value);
            }
            _ if value.is_empty() => parent = Some(key),
            _ => {
                parent = None;
                values.insert(key, value);
            }
        }
    }
    Some((values, lines[end + 1..].join("\n")))
}

fn unquote(value: &str) -> String {
    let mut chars = value.chars();
    let (Some(first), Some(last)) = (chars.next(), value.chars().next_back()) else { return value.into() };
    if value.chars().count() < 2 || first != last || (first != '"' && first != '\'') {
        return value.into();
    }
    value[1..value.len() - 1].replace("\\\"", "\"").replace("\\\\", "\\")
}

/// `[[name]]` and `[[name|label]]`, the names.
fn wiki_links(text: &str) -> Vec<String> {
    let b = text.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i] == b'[' && b[i + 1] == b'[' {
            let start = i + 2;
            let mut j = start;
            while j < b.len() && b[j] != b']' && b[j] != b'|' {
                j += 1;
            }
            if j > start && j < b.len() {
                let name_end = j;
                if b[j] == b'|' {
                    while j < b.len() && b[j] != b']' {
                        j += 1;
                    }
                }
                if j + 1 < b.len() && b[j] == b']' && b[j + 1] == b']' {
                    out.push(text[start..name_end].trim_matches(is_space).to_string());
                    i = j + 2;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// `[Title](file.md)` links in an index, title by file name.
fn index_titles(index: &str) -> HashMap<String, String> {
    let b = index.as_bytes();
    let mut titles = HashMap::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'[' {
            let start = i + 1;
            let mut j = start;
            while j < b.len() && b[j] != b']' {
                j += 1;
            }
            if j > start && j + 1 < b.len() && b[j + 1] == b'(' {
                let (title_end, file_start) = (j, j + 2);
                let k = file_start
                    + index[file_start..].find(|c: char| c == ')' || c.is_whitespace()).unwrap_or(index.len() - file_start);
                let file = &index[file_start..k];
                if k < b.len() && b[k] == b')' && file.len() > 3 && file.ends_with(".md") {
                    let name = file.split('/').filter(|p| !p.is_empty()).next_back().unwrap_or("");
                    titles.insert(name.to_string(), index[start..title_end].to_string());
                    i = k + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    titles
}

fn humanize(name: &str) -> String {
    let words: Vec<&str> = name.split(['-', '_']).filter(|w| !w.is_empty()).collect();
    let Some(first) = words.first() else { return name.into() };
    let mut chars = first.chars();
    let head: String = chars.next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    std::iter::once(format!("{head}{}", chars.as_str())).chain(words[1..].iter().map(|w| w.to_string())).collect::<Vec<_>>().join(" ")
}

// MARK: - Bringing memories in

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub added: usize,
    pub updated: usize,
    /// Already there, the same.
    pub unchanged: usize,
}

impl ImportReport {
    pub fn total(&self) -> usize {
        self.added + self.updated + self.unchanged
    }
}

/// Adds `incoming` to `scope`: a memory it already has (same ID, or same title) is updated when
/// the incoming one is newer; one whose ID another workspace uses gets a new ID.
pub fn merge(incoming: Vec<Memory>, scope: &str, store: &Store, actor: &Actor) -> ImportReport {
    let existing = store.memories(Some(scope), None);
    let mut by_id: HashMap<&str, &Memory> = HashMap::new();
    for m in &existing {
        by_id.entry(m.id.as_str()).or_insert(m);
    }
    // In use first: a title that's also on an archived memory matches the one in use.
    let mut ordered: Vec<&Memory> = existing.iter().collect();
    ordered.sort_by_key(|m| if m.status == "active" { 0 } else { 1 });
    let mut by_title: HashMap<String, &Memory> = HashMap::new();
    for m in ordered {
        by_title.entry(m.title.to_lowercase()).or_insert(m);
    }
    let (mut report, mut remap, mut to_save) = (ImportReport::default(), HashMap::<String, String>::new(), Vec::<Memory>::new());
    for mut memory in incoming {
        memory.scope = scope.into();
        let current = by_id.get(memory.id.as_str()).or(by_title.get(&memory.title.to_lowercase())).copied();
        if let Some(current) = current {
            remap.insert(memory.id.clone(), current.id.clone());
            // One you archived stays archived: an import doesn't bring it back.
            if current.status == "archived" && current.id != memory.id {
                report.unchanged += 1;
                continue;
            }
            let same = current.title == memory.title
                && current.description == memory.description
                && current.body == memory.body
                && current.kind == memory.kind
                && current.status == memory.status
                && current.task_state == memory.task_state
                && current.steps == memory.steps;
            if same || memory.updated_at <= current.updated_at {
                report.unchanged += 1;
                continue;
            }
            let mut updated = current.clone();
            updated.title = memory.title;
            updated.description = memory.description;
            updated.body = memory.body;
            updated.kind = memory.kind;
            updated.status = memory.status;
            updated.task_state = memory.task_state;
            updated.steps = memory.steps;
            updated.pinned = memory.pinned || current.pinned;
            updated.updated_at = memory.updated_at.max(current.updated_at);
            for link in memory.links {
                if !updated.links.contains(&link) {
                    updated.links.push(link);
                }
            }
            to_save.push(updated);
            report.updated += 1;
        } else {
            if store.memory(&memory.id).is_some() {
                let fresh = new_uuid();
                remap.insert(memory.id.clone(), fresh.clone());
                memory.id = fresh;
            }
            to_save.push(memory);
            report.added += 1;
        }
    }
    for m in &mut to_save {
        let mut links: Vec<(String, Option<String>)> = vec![];
        for (to, relation) in &m.links {
            let to = remap.get(to).cloned().unwrap_or_else(|| to.clone());
            if to != m.id && !links.iter().any(|(t, _)| *t == to) {
                links.push((to, relation.clone()));
            }
        }
        m.links = links;
    }
    store.save_all(to_save, actor);
    if report.total() > 0 {
        store.record_event(
            scope,
            "imported",
            actor,
            Some((report.added + report.updated) as i64),
            Some(format!("{} added, {} updated, {} unchanged", report.added, report.updated, report.unchanged)),
        );
    }
    report
}

/// Claude Code's own memory for a folder: `~/.claude/projects/<folder as a name>/memory`.
pub mod claude_code {
    use super::*;

    pub const SETTING_KEY: &str = "autoMemoryEnabled";

    /// Claude Code names a project's folder after its path, every character that isn't a letter
    /// or digit (ASCII) a dash. The real path, as Claude Code sees its working directory
    /// (`/private/tmp`, not `/tmp`).
    pub fn directory(folder: &str, config_dir: &Path) -> PathBuf {
        let path = std::fs::canonicalize(folder).map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| folder.to_string());
        let name: String = graphemes(&path)
            .into_iter()
            .map(|g| if g.len() == 1 && g.as_bytes()[0].is_ascii_alphanumeric() { g } else { "-" })
            .collect();
        config_dir.join("projects").join(name).join("memory")
    }

    /// The memory files there, not counting the index.
    pub fn files(folder: &str, config_dir: &Path) -> Vec<PathBuf> {
        let directory = directory(folder, config_dir);
        let mut names: Vec<String> = std::fs::read_dir(&directory)
            .map(|e| e.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect())
            .unwrap_or_default();
        names.retain(|n| n.ends_with(".md") && n != "MEMORY.md");
        names.sort();
        names.into_iter().map(|n| directory.join(n)).collect()
    }

    /// Whether Claude Code keeps its own memory for a folder, as its settings say
    /// (`autoMemoryEnabled`): the folder's `.claude/settings.local.json` and
    /// `.claude/settings.json`, then your own `settings.json`.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Setting {
        /// Off in the folder's own settings.
        pub off_here: bool,
        /// Off in your Claude Code settings, for every folder.
        pub off_everywhere: bool,
        /// The folder's settings turn it back on, whatever yours say.
        pub on_here: bool,
    }

    impl Setting {
        pub fn is_off(&self) -> bool {
            self.off_here || (self.off_everywhere && !self.on_here)
        }
    }

    /// Where a folder's own setting goes: the local file, which isn't shared with the repo.
    pub fn local_settings(folder: &str) -> PathBuf {
        Path::new(folder).join(".claude").join("settings.local.json")
    }

    /// `.` and `..` worked out, as `standardizedFileURL` does.
    fn standardized(path: &Path) -> PathBuf {
        let mut out = PathBuf::new();
        for part in path.components() {
            match part {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    out.pop();
                }
                other => out.push(other),
            }
        }
        out
    }

    pub fn setting(folder: &str, config_dir: &Path) -> Setting {
        let claude = Path::new(folder).join(".claude");
        // A folder that is your config's own parent (your home folder) has no settings of its own.
        let own: Vec<PathBuf> = if standardized(&claude) == standardized(config_dir) {
            vec![]
        } else {
            vec![claude.join("settings.local.json"), claude.join("settings.json")]
        };
        let here = own.iter().find_map(|p| value(p));
        let everywhere = value(&config_dir.join("settings.json"));
        Setting { off_here: here == Some(false), off_everywhere: everywhere == Some(false), on_here: here == Some(true) }
    }

    fn value(path: &Path) -> Option<bool> {
        let data = std::fs::read(path).ok()?;
        let object: Value = serde_json::from_slice(&data).ok()?;
        match object.as_object()?.get(SETTING_KEY)? {
            Value::Bool(b) => Some(*b),
            // Foundation reads 0 and 1 as booleans too.
            Value::Number(n) => match n.as_f64()? {
                0.0 => Some(false),
                1.0 => Some(true),
                _ => None,
            },
            _ => None,
        }
    }

    /// Turns Claude Code's own memory off in `path` (a settings file), keeping everything else
    /// in it. A file that isn't JSON is left alone, and said so.
    pub fn turn_off(path: &Path) -> Result<(), String> {
        let mut object = Map::new();
        if let Ok(data) = std::fs::read(path) {
            if !data.is_empty() {
                match serde_json::from_slice::<Value>(&data) {
                    Ok(Value::Object(existing)) => object = existing,
                    _ => return Err(format!("{} isn't valid JSON, so it was left alone.", path.display())),
                }
            }
        }
        object.insert(SETTING_KEY.into(), Value::Bool(false));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let text = swift_pretty(&Value::Object(object)) + "\n";
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, text).and_then(|_| std::fs::rename(&temporary, path)).map_err(|e| e.to_string())
    }

    /// Keeps a folder's local settings out of git, in the repo's own `.git/info/exclude` (never a
    /// file that's committed), when the folder is a repo's root.
    pub fn exclude_from_git(folder: &str) {
        let git = Path::new(folder).join(".git");
        if !git.is_dir() {
            return;
        }
        let (info, line) = (git.join("info"), ".claude/settings.local.json");
        let exclude = info.join("exclude");
        let current = std::fs::read_to_string(&exclude).unwrap_or_default();
        if current.split('\n').any(|l| l.trim_matches(is_space) == line) {
            return;
        }
        let _ = std::fs::create_dir_all(&info);
        let text = if current.is_empty() || current.ends_with('\n') { format!("{current}{line}\n") } else { format!("{current}\n{line}\n") };
        let _ = std::fs::write(&exclude, text);
    }

    /// Memory files there that `memories` doesn't have (by title or what they say), changed
    /// after `since` (seconds since 1970).
    pub fn unimported(folder: &str, config_dir: &Path, memories: &[Memory], since: Option<f64>) -> Vec<PathBuf> {
        let files = files(folder, config_dir);
        if files.is_empty() {
            return vec![];
        }
        let titles: HashSet<String> = memories.iter().map(|m| m.title.to_lowercase()).collect();
        let bodies: HashSet<&str> = memories.iter().map(|m| m.body.trim()).collect();
        let index = std::fs::read_to_string(directory(folder, config_dir).join("MEMORY.md")).unwrap_or_default();
        files
            .into_iter()
            .filter(|path| {
                if let Some(since) = since {
                    let modified = std::fs::metadata(path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs_f64());
                    if modified.is_some_and(|m| m <= since) {
                        return false;
                    }
                }
                let (Ok(text), Some(name)) = (std::fs::read_to_string(path), path.file_name()) else { return false };
                let one = BTreeMap::from([(name.to_string_lossy().to_string(), text), ("MEMORY.md".to_string(), index.clone())]);
                let Some(memory) = memories_from_markdown(&one, "", 0.0).into_iter().next() else { return false };
                !titles.contains(&memory.title.to_lowercase()) && !bodies.contains(memory.body.as_str())
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn store(name: &str) -> Store {
        let dir = std::env::temp_dir().join(format!("yh-memory-archive-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Store { db: Arc::new(crate::db::Database::open(&dir).unwrap()) }
    }

    fn actor() -> Actor {
        Actor { kind: "importer".into(), thread: None }
    }

    fn sample() -> Vec<Memory> {
        let mut a = Memory::new("project:P", "feedback", "Release only on request", "When to cut releases", "Never release unless asked.\n", "agent");
        a.created_at = 1_793_149_200.0;
        a.updated_at = 1_793_149_260.0;
        a.pinned = true;
        a.recall_count = 3;
        a.last_recalled_at = Some(1_793_149_300.0);
        a.sources = vec![("T1".into(), "toolu_1".into())];
        let mut b = Memory::new("project:P", "task", "Move billing: phase 2", "", "The log.", "agent");
        b.created_at = 1_793_149_200.0;
        b.updated_at = 1_793_149_200.0;
        b.task_state = Some("active".into());
        b.steps = vec![Step { text: "Plan".into(), done: true }, Step { text: "Build".into(), done: false }];
        b.links = vec![(a.id.clone(), Some("see also".into()))];
        let mut c = Memory::new("project:P", "reference", "Release only on request", "# dashboards/x", "Old.", "agent");
        c.status = "archived".into();
        c.created_at = 1_793_149_200.0;
        c.updated_at = 1_793_149_200.0;
        vec![a, b, c]
    }

    #[test]
    fn json_round_trip() {
        let memories = sample();
        let data = json(&memories, "app", Some("/tmp/app"), 1_793_149_200.9);
        let text = String::from_utf8(data.clone()).unwrap();
        assert!(text.starts_with("{\n  \"exportedAt\" : \"2026-10-28T01:00:00Z\",\n  \"format\" : \"yourharness-memory\",\n"));
        assert!(text.contains("\"path\" : \"/tmp/app\""));
        assert!(text.contains("\"sources\" : [\n        {\n          \"entryID\" : \"toolu_1\",\n          \"threadID\" : \"T1\"\n        }\n      ]"));
        let back = memories_from_json(&data, "project:Q").unwrap();
        assert_eq!(back.len(), 3);
        for (m, b) in memories.iter().zip(&back) {
            assert_eq!(
                (&b.id, &b.kind, &b.title, &b.description, &b.body, &b.status, b.pinned, &b.task_state, &b.steps, &b.links, &b.sources),
                (&m.id, &m.kind, &m.title, &m.description, &m.body, &m.status, m.pinned, &m.task_state, &m.steps, &m.links, &m.sources)
            );
            assert_eq!(b.scope, "project:Q");
            assert_eq!((b.created_at, b.updated_at, b.recall_count, b.last_recalled_at), (m.created_at, m.updated_at, m.recall_count, m.last_recalled_at));
        }
        assert_eq!(memories_from_json(b"{\"format\": \"x\"}", "s"), Err(ReadError::NotAnExport));
        assert_eq!(memories_from_json(b"not json", "s"), Err(ReadError::NotAnExport));
        let newer = br#"{"format": "yourharness-memory", "version": 2, "exportedAt": "2026-10-28T01:00:00Z", "memories": []}"#;
        assert_eq!(memories_from_json(newer, "s"), Err(ReadError::NewerVersion(2)));
        assert_eq!(swift_pretty(&json!({"a": [], "b": {}})), "{\n  \"a\" : [\n\n  ],\n  \"b\" : {\n\n  }\n}");
    }

    #[test]
    fn markdown_round_trip() {
        let memories = sample();
        let files = markdown_files(&memories, "app");
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["MEMORY.md", "release-only-on-request.md", "move-billing-phase-2.md", "release-only-on-request-2.md"]);
        let index = &files[0].1;
        assert!(index.contains("## Feedback\n\n- [Release only on request](release-only-on-request.md) — When to cut releases\n\n"));
        assert!(index.contains("## Task\n\n- [Move billing: phase 2](move-billing-phase-2.md) — \n\n"));
        assert!(index.ends_with("## Archived\n\n- [Release only on request](release-only-on-request-2.md) — # dashboards/x\n"));
        let task = &files[2].1;
        assert!(task.starts_with("---\nname: move-billing-phase-2\ndescription: \"\"\nmetadata:\n  type: task\n  title: \"Move billing: phase 2\"\n"));
        assert!(task.contains("  created: 2026-10-28T01:00:00Z\n"));
        assert!(task.ends_with("---\n\nThe log.\n\n## Steps\n\n- [x] Plan\n- [ ] Build\n\nRelated: [[release-only-on-request]]\n"));
        let back = memories_from_markdown(&files.into_iter().collect(), "project:Q", 0.0);
        assert_eq!(back.len(), 3);
        for m in &memories {
            let b = back.iter().find(|b| b.id == m.id).unwrap();
            assert_eq!((&b.title, &b.kind, &b.description, b.body.as_str(), &b.status, b.pinned), (&m.title, &m.kind, &m.description, m.body.trim(), &m.status, m.pinned));
            assert_eq!((&b.task_state, &b.steps, b.created_at, b.updated_at), (&m.task_state, &m.steps, m.created_at, m.updated_at));
            assert_eq!(b.links.iter().map(|l| &l.0).collect::<Vec<_>>(), m.links.iter().map(|l| &l.0).collect::<Vec<_>>());
        }
    }

    #[test]
    fn reads_claude_code_memory() {
        let files = BTreeMap::from([
            ("MEMORY.md".to_string(), "- [Prefers terse answers](user_terse.md) — style\n".to_string()),
            (
                "user_terse.md".to_string(),
                "---\nname: user_terse\ndescription: 'How they like answers'\ntype: user\n---\n\nKeep answers short. See [[feedback_tests|tests]].\n".to_string(),
            ),
            ("feedback_tests.md".to_string(), "---\nname: feedback_tests\ndescription: Run tests first\nmetadata:\n  type: feedback\n---\nAlways run tests.\n".to_string()),
            ("odd.md".to_string(), "---\nname: odd\ndescription: x\nmetadata:\n  type: something-new\n  modified: 2026-10-28T01:00:00.5Z\n---\nBody".to_string()),
            ("notes.md".to_string(), "# Just notes\n".to_string()),
            ("CLAUDE.md".to_string(), "---\ntitle: no type\n---\n".to_string()),
        ]);
        let memories = memories_from_markdown(&files, "project:P", 42.0);
        assert_eq!(memories.len(), 3);
        let terse = memories.iter().find(|m| m.kind == "user").unwrap();
        assert_eq!((terse.title.as_str(), terse.description.as_str()), ("Prefers terse answers", "How they like answers"));
        assert_eq!(terse.body, "Keep answers short. See [[feedback_tests|tests]].");
        assert_eq!(terse.origin.as_deref(), Some("import"));
        assert_eq!(terse.updated_at, 42.0);
        let tests = memories.iter().find(|m| m.kind == "feedback").unwrap();
        assert_eq!(tests.title, "Feedback tests");
        assert_eq!(terse.links, vec![(tests.id.clone(), None)]);
        let odd = memories.iter().find(|m| m.title == "Odd").unwrap();
        assert_eq!((odd.kind.as_str(), odd.updated_at), ("project", 1_793_149_200.5));
    }

    #[test]
    fn merge_rules() {
        let s = store("merge");
        let base = sample();
        let report = merge(base.clone(), "project:P", &s, &actor());
        assert_eq!(report, ImportReport { added: 3, updated: 0, unchanged: 0 });
        assert_eq!(s.memory(&base[1].id).unwrap().links, vec![(base[0].id.clone(), Some("see also".into()))]);

        // The same again: nothing changes.
        assert_eq!(merge(base.clone(), "project:P", &s, &actor()), ImportReport { added: 0, updated: 0, unchanged: 3 });

        // Newer: updated. Older: left alone.
        let mut newer = base[0].clone();
        newer.body = "Never release unless the user asks.".into();
        newer.updated_at += 100.0;
        let mut older = base[1].clone();
        older.body = "Stale.".into();
        older.updated_at -= 100.0;
        assert_eq!(merge(vec![newer, older], "project:P", &s, &actor()), ImportReport { added: 0, updated: 1, unchanged: 1 });
        assert_eq!(s.memory(&base[0].id).unwrap().body, "Never release unless the user asks.");
        assert_eq!(s.memory(&base[1].id).unwrap().body, "The log.");

        // Same title, another ID: matched by title (the one in use), updated in place.
        let mut renamed = Memory::new("x", "feedback", "release only on request", "d", "From elsewhere.", "import");
        renamed.updated_at = base[0].updated_at + 1_000.0;
        assert_eq!(merge(vec![renamed], "project:P", &s, &actor()), ImportReport { added: 0, updated: 1, unchanged: 0 });
        let current = s.memory(&base[0].id).unwrap();
        assert_eq!((current.body.as_str(), current.title.as_str()), ("From elsewhere.", "release only on request"));
        assert_eq!(s.memories(Some("project:P"), None).len(), 3);

        // An ID another workspace uses: a new ID.
        let mut other = base[2].clone();
        other.title = "Something else".into();
        let report = merge(vec![other], "project:Other", &s, &actor());
        assert_eq!(report.added, 1);
        let theirs = s.memories(Some("project:Other"), None);
        assert_eq!(theirs.len(), 1);
        assert_ne!(theirs[0].id, base[2].id);
        assert!(s.events("project:P", 50).iter().any(|e| e.0 == "imported"));
    }

    #[test]
    fn claude_code_directory_and_setting() {
        let dir = std::env::temp_dir().join(format!("yh-cc-memory-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (folder, config) = (dir.join("my project.x"), dir.join("config"));
        std::fs::create_dir_all(&folder).unwrap();
        let real = std::fs::canonicalize(&folder).unwrap().to_string_lossy().to_string();
        let name: String = real.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        let memory_dir = claude_code::directory(folder.to_str().unwrap(), &config);
        assert_eq!(memory_dir, config.join("projects").join(name).join("memory"));
        std::fs::create_dir_all(&memory_dir).unwrap();
        for f in ["b.md", "a.md", "MEMORY.md", "c.txt"] {
            std::fs::write(memory_dir.join(f), "x").unwrap();
        }
        let files = claude_code::files(folder.to_str().unwrap(), &config);
        assert_eq!(files, vec![memory_dir.join("a.md"), memory_dir.join("b.md")]);

        let f = folder.to_str().unwrap();
        assert!(!claude_code::setting(f, &config).is_off());
        std::fs::write(config.join("settings.json"), r#"{"autoMemoryEnabled": false}"#).unwrap();
        assert!(claude_code::setting(f, &config).is_off());
        std::fs::create_dir_all(folder.join(".claude")).unwrap();
        std::fs::write(folder.join(".claude/settings.json"), r#"{"autoMemoryEnabled": true}"#).unwrap();
        let setting = claude_code::setting(f, &config);
        assert!(setting.on_here && setting.off_everywhere && !setting.is_off());
        std::fs::write(claude_code::local_settings(f), r#"{"model": "haiku/x"}"#).unwrap();
        claude_code::turn_off(&claude_code::local_settings(f)).unwrap();
        assert_eq!(
            std::fs::read_to_string(claude_code::local_settings(f)).unwrap(),
            "{\n  \"autoMemoryEnabled\" : false,\n  \"model\" : \"haiku/x\"\n}\n"
        );
        assert!(claude_code::setting(f, &config).off_here);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
