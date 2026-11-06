//! Projects and threads: the app's own `harness.sqlite` (the user's data folder, or
//! `YH_DATA_DIR`), through `db::Database`, in the Swift app's format. Records are kept as the
//! JSON they were read as and changed field by field, so whatever the Swift app wrote stays.
//! With `YH_CLAUDE_HISTORY=1`, Claude Code's own sessions are listed too, read-only.

use crate::history;
use crate::db::{Database, Row, StateRows, ThreadRecord};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Long tool text is cut to a head: a folded step shows a head anyway, and the whole text
/// would only cost memory.
pub const TOOL_TEXT_LIMIT: usize = 4000;
/// Streamed tool output, cut shorter: hundreds of results a turn at full length would be
/// most of the memory.
pub const LIVE_RESULT_LIMIT: usize = 1500;

pub fn head(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    User,
    Assistant,
    Thinking,
    Tool,
    Notice,
    Command,
}

/// One transcript row as read from storage or built from Claude's events.
#[derive(Clone, Debug)]
pub struct EntryData {
    pub id: String,
    pub kind: Kind,
    pub text: String,
    pub tool_name: Option<String>,
    pub tool_input: Option<String>,
    pub tool_result: Option<String>,
    pub result_len: usize,
    pub is_error: bool,
}

impl EntryData {
    pub fn new(id: impl Into<String>, kind: Kind, text: impl Into<String>) -> Self {
        EntryData {
            id: id.into(),
            kind,
            text: text.into(),
            tool_name: None,
            tool_input: None,
            tool_result: None,
            result_len: 0,
            is_error: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProjectMeta {
    pub id: String,
    pub name: String,
    pub path: String,
}

#[derive(Clone, Debug)]
pub struct ThreadMeta {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub updated_at: f64,
    pub session_id: Option<String>,
}

pub struct Page {
    pub total: usize,
    pub from: usize,
    pub entries: Vec<EntryData>,
}

/// The records as read, changed in place, saved whole (the database writes what differs).
#[derive(Default)]
struct Records {
    values: std::collections::BTreeMap<String, Vec<u8>>,
    projects: Vec<Map<String, Value>>,
    threads: Vec<Map<String, Value>>,
}

pub struct Library {
    pub db: Option<Arc<Database>>,
    records: Mutex<Records>,
    claude_config: Option<PathBuf>,
    pub projects: Vec<ProjectMeta>,
    pub threads: Vec<ThreadMeta>,
    session_files: HashMap<String, PathBuf>,
    /// Claude Code sessions are one file each, read whole; the last few are kept.
    archives: Mutex<Vec<(String, Arc<Vec<EntryData>>)>>,
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

impl Library {
    pub fn open() -> Library {
        let db = match Database::open(&data_directory()) {
            Ok(db) => Some(Arc::new(db)),
            Err(e) => {
                eprintln!("{e}");
                None
            }
        };
        let claude_config = if env("YH_CLAUDE_HISTORY").as_deref() != Some("1") {
            None
        } else if let Some(dir) = env("YH_CLAUDE_CONFIG").or_else(|| env("CLAUDE_CONFIG_DIR")) {
            Some(PathBuf::from(expand_tilde(&dir)))
        } else {
            env("HOME").map(|h| Path::new(&h).join(".claude"))
        };
        let mut library = Library {
            db,
            records: Mutex::new(Records::default()),
            claude_config,
            projects: vec![],
            threads: vec![],
            session_files: HashMap::new(),
            archives: Mutex::new(vec![]),
        };
        library.scan();
        library
    }

    pub fn project_path(&self, id: &str) -> Option<String> {
        self.projects.iter().find(|p| p.id == id).map(|p| p.path.clone())
    }

    fn scan(&mut self) {
        let started = Instant::now();
        if let Some(db) = &self.db {
            let state = db.load_state();
            let mut records = Records { values: state.values, ..Default::default() };
            for (id, json) in state.projects {
                let Ok(Value::Object(object)) = serde_json::from_slice::<Value>(&json) else { continue };
                self.projects.push(ProjectMeta {
                    id,
                    name: object.get("name").and_then(Value::as_str).unwrap_or("Project").to_string(),
                    path: object.get("path").and_then(Value::as_str).unwrap_or("").to_string(),
                });
                records.projects.push(object);
            }
            for t in state.threads {
                let Ok(Value::Object(object)) = serde_json::from_slice::<Value>(&t.json) else { continue };
                self.threads.push(ThreadMeta {
                    id: t.id,
                    project_id: t.project_id,
                    title: object.get("title").and_then(Value::as_str).unwrap_or("Thread").to_string(),
                    updated_at: t.updated_at.map(|s| s * 1000.0).unwrap_or(0.0),
                    session_id: t.session_id,
                });
                records.threads.push(object);
            }
            *self.records.lock().unwrap() = records;
        }
        // `YH_PROJECT=<folder>`: a project of its own, for trying live turns somewhere throwaway.
        if let Some(path) = env("YH_PROJECT") {
            self.projects.insert(
                0,
                ProjectMeta { id: format!("dir:{path}"), name: last_component(&path), path },
            );
        }
        if let Some(config) = self.claude_config.clone() {
            let limit = env("YH_CLAUDE_LIMIT").and_then(|v| v.parse().ok()).unwrap_or(400);
            let mut by_path: HashMap<String, ()> = HashMap::new();
            for file in session_files(&config).into_iter().take(limit) {
                let (cwd, title) = history::peek(&file.path);
                let Some(title) = title else { continue };
                let path = cwd.unwrap_or_else(|| file.folder.clone());
                let project_id = format!("cc:{path}");
                if by_path.insert(path.clone(), ()).is_none() {
                    self.projects.push(ProjectMeta { id: project_id.clone(), name: last_component(&path), path });
                }
                let id = format!("cc:{}", file.session_id);
                self.session_files.insert(id.clone(), file.path);
                self.threads.push(ThreadMeta {
                    id,
                    project_id,
                    title,
                    updated_at: file.modified,
                    session_id: Some(file.session_id),
                });
            }
        }
        eprintln!(
            "library: {} projects, {} threads in {:.2} s",
            self.projects.len(),
            self.threads.len(),
            started.elapsed().as_secs_f64()
        );
    }

    /// Rows `from..before` of a thread: the newest `limit` before `before` (or the end).
    /// Runs on a background thread.
    pub fn page(&self, thread: &str, before: Option<usize>, limit: usize) -> Page {
        if thread.starts_with("cc:") {
            let entries = self.archive(thread);
            let end = before.unwrap_or(entries.len()).min(entries.len());
            let from = end.saturating_sub(limit);
            return Page { total: entries.len(), from, entries: entries[from..end].to_vec() };
        }
        let Some(db) = &self.db else { return Page { total: 0, from: 0, entries: vec![] } };
        let t0 = Instant::now();
        let total = db.transcript_count(thread);
        let end = before.unwrap_or(total).min(total);
        let from = end.saturating_sub(limit);
        let rows = db.read_rows(thread, from, end - from);
        let entries: Vec<EntryData> = rows.into_iter().filter_map(EntryData::from_row).collect();
        if env("YH_TIMING").as_deref() == Some("1") {
            eprintln!("page {thread}: {} rows in {:.0} ms", entries.len(), t0.elapsed().as_secs_f64() * 1000.0);
        }
        Page { total, from, entries }
    }

    // MARK: Changing

    /// A new thread at the top of its project, saved. Its ID is a UUID, as the Swift app's are.
    pub fn add_thread(&self, project_id: &str, title: &str) -> ThreadMeta {
        let id = new_uuid();
        let now = iso_now();
        let mut object = Map::new();
        object.insert("id".into(), id.clone().into());
        object.insert("projectID".into(), project_id.into());
        object.insert("title".into(), title.into());
        object.insert("createdAt".into(), now.clone().into());
        object.insert("updatedAt".into(), now.into());
        self.records.lock().unwrap().threads.insert(0, object);
        self.save();
        ThreadMeta { id, project_id: project_id.into(), title: title.into(), updated_at: now_ms(), session_id: None }
    }

    /// Changes a thread's record (`title`, `sessionID`, `updatedAt`…) and saves.
    pub fn update_thread(&self, id: &str, change: impl FnOnce(&mut Map<String, Value>)) {
        let mut records = self.records.lock().unwrap();
        let Some(t) = records.threads.iter_mut().find(|t| t.get("id").and_then(Value::as_str) == Some(id)) else { return };
        change(t);
        drop(records);
        self.save();
    }

    /// The records into the database, which writes what differs, in the background.
    pub fn save(&self) {
        let Some(db) = &self.db else { return };
        let records = self.records.lock().unwrap();
        let json = |o: &Map<String, Value>| serde_json::to_vec(o).unwrap_or_else(|_| b"{}".to_vec());
        let state = StateRows {
            values: records.values.clone(),
            projects: records
                .projects
                .iter()
                .map(|p| (p.get("id").and_then(Value::as_str).unwrap_or("").to_string(), json(p)))
                .collect(),
            threads: records
                .threads
                .iter()
                .map(|t| ThreadRecord {
                    id: t.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                    project_id: t.get("projectID").and_then(Value::as_str).unwrap_or("").to_string(),
                    updated_at: t.get("updatedAt").and_then(Value::as_str).and_then(parse_iso),
                    session_id: t.get("sessionID").and_then(Value::as_str).map(str::to_string),
                    json: json(t),
                })
                .collect(),
        };
        db.save_state(state);
    }

    fn archive(&self, thread: &str) -> Arc<Vec<EntryData>> {
        if let Some((_, cached)) = self.archives.lock().unwrap().iter().find(|(id, _)| id == thread) {
            return cached.clone();
        }
        let entries = Arc::new(self.session_files.get(thread).map(|p| history::load(p)).unwrap_or_default());
        let mut archives = self.archives.lock().unwrap();
        archives.retain(|(id, _)| id != thread);
        archives.push((thread.to_string(), entries.clone()));
        if archives.len() > 6 {
            archives.remove(0);
        }
        entries
    }
}

struct SessionFile {
    path: PathBuf,
    folder: String,
    session_id: String,
    modified: f64,
}

/// Every session file under `<config>/projects`, newest first.
fn session_files(config: &Path) -> Vec<SessionFile> {
    let mut files = vec![];
    let Ok(folders) = std::fs::read_dir(config.join("projects")) else { return files };
    for folder in folders.flatten() {
        let Ok(list) = std::fs::read_dir(folder.path()) else { continue };
        for file in list.flatten() {
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let modified = file
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64() * 1000.0)
                .unwrap_or(0.0);
            files.push(SessionFile {
                session_id: path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string(),
                folder: folder.file_name().to_string_lossy().to_string(),
                path,
                modified,
            });
        }
    }
    files.sort_by(|a, b| b.modified.total_cmp(&a.modified));
    files
}

pub fn last_component(path: &str) -> String {
    Path::new(path).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string())
}

fn expand_tilde(path: &str) -> String {
    match (path.strip_prefix("~/"), env("HOME")) {
        (Some(rest), Some(home)) => format!("{home}/{rest}"),
        _ => path.to_string(),
    }
}

pub fn now_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
}

/// The user's data folder, or `YH_DATA_DIR` (development runs: never the real one).
pub fn data_directory() -> PathBuf {
    if let Some(dir) = env("YH_DATA_DIR") {
        return PathBuf::from(expand_tilde(&dir));
    }
    Path::new(&env("HOME").unwrap_or_default()).join("Library/Application Support/YourHarness")
}

impl EntryData {
    /// A stored row as the transcript shows it; `None` for a kind this app doesn't know.
    pub fn from_row(row: Row) -> Option<EntryData> {
        let kind = match row.kind.as_str() {
            "user" => Kind::User,
            "assistant" => Kind::Assistant,
            "thinking" => Kind::Thinking,
            "tool" => Kind::Tool,
            "notice" => Kind::Notice,
            "commandOutput" => Kind::Command,
            _ => return None,
        };
        let mut entry = EntryData::new(row.id, kind, row.text);
        entry.tool_name = row.tool_name;
        entry.result_len = row
            .tool_result_length
            .map(|n| n as usize)
            .unwrap_or_else(|| row.tool_result.as_ref().map_or(0, |r| r.len()));
        entry.tool_input = row.tool_input.map(|s| head(&s, TOOL_TEXT_LIMIT));
        entry.tool_result = row.tool_result.map(|s| head(&s, TOOL_TEXT_LIMIT));
        entry.is_error = row.tool_is_error == Some(true);
        Some(entry)
    }
}

/// A random (v4) UUID, upper case as Foundation writes them.
pub fn new_uuid() -> String {
    let mut b = [0u8; 16];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut b);
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02X}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// Now as the Swift app writes dates in its JSON (ISO 8601, seconds, `Z`).
pub fn iso_now() -> String {
    iso(SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0))
}

pub fn iso(seconds: i64) -> String {
    let (days, secs) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", secs / 3600, secs % 3600 / 60, secs % 60)
}

/// Seconds since 1970 from `2026-10-28T01:00:00Z` (fractions and offsets tolerated).
pub fn parse_iso(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d, hh, mm, ss) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    let (y2, m2) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let mut t = (days * 86_400 + hh * 3600 + mm * 60 + ss) as f64;
    let rest = &s[19..];
    let rest = if let Some(frac) = rest.strip_prefix('.') {
        let digits: String = frac.chars().take_while(|c| c.is_ascii_digit()).collect();
        t += format!("0.{digits}").parse::<f64>().unwrap_or(0.0);
        &frac[digits.len()..]
    } else {
        rest
    };
    if let Some(sign) = rest.chars().next().filter(|c| *c == '+' || *c == '-') {
        let (oh, om) = (rest.get(1..3)?.parse::<i64>().ok()?, rest.get(rest.len() - 2..)?.parse::<i64>().ok()?);
        let offset = (oh * 3600 + om * 60) as f64;
        t += if sign == '+' { -offset } else { offset };
    }
    Some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_dates_round_trip() {
        assert_eq!(iso(1_793_149_200), "2026-10-28T01:00:00Z");
        assert_eq!(parse_iso("2026-10-28T01:00:00Z"), Some(1_793_149_200.0));
        assert_eq!(parse_iso("2026-10-28T03:00:00+02:00"), Some(1_793_149_200.0));
        assert_eq!(parse_iso("2026-10-28T01:00:00.5Z"), Some(1_793_149_200.5));
    }

    #[test]
    fn uuids_look_like_foundations() {
        let u = new_uuid();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
        assert_eq!(u, u.to_uppercase());
    }
}
