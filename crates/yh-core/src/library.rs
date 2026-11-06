//! What there is to show: the harness's own database (a copy: the extreme preset for load
//! tests) and Claude Code's history in `~/.claude/projects`, read-only. The twin of
//! web/host/Library.swift, without HarnessCore: the same tables and files, read directly.

use crate::history;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
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

pub struct Library {
    db: Option<Mutex<Connection>>,
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
        let db = env("YH_DATA_DIR").and_then(|dir| {
            let path = Path::new(&dir).join("harness.sqlite");
            match Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX) {
                Ok(c) => Some(Mutex::new(c)),
                Err(e) => {
                    eprintln!("can't open {}: {e}", path.display());
                    None
                }
            }
        });
        let claude_config = if env("YH_NO_CLAUDE").as_deref() == Some("1") {
            None
        } else if let Some(dir) = env("YH_CLAUDE_CONFIG").or_else(|| env("CLAUDE_CONFIG_DIR")) {
            Some(PathBuf::from(expand_tilde(&dir)))
        } else {
            env("HOME").map(|h| Path::new(&h).join(".claude"))
        };
        let mut library = Library {
            db,
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
            let db = db.lock().unwrap();
            let mut statement = db.prepare("SELECT id, json FROM projects ORDER BY position").unwrap();
            let rows = statement
                .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)))
                .unwrap();
            for (id, json) in rows.flatten() {
                let Ok(object) = serde_json::from_slice::<serde_json::Value>(&json) else { continue };
                self.projects.push(ProjectMeta {
                    id,
                    name: object["name"].as_str().unwrap_or("Project").to_string(),
                    path: object["path"].as_str().unwrap_or("").to_string(),
                });
            }
            let mut statement = db
                .prepare("SELECT id, project_id, updated_at, session_id, json FROM threads ORDER BY position")
                .unwrap();
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<f64>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                    ))
                })
                .unwrap();
            for (id, project_id, updated_at, session_id, json) in rows.flatten() {
                let Ok(object) = serde_json::from_slice::<serde_json::Value>(&json) else { continue };
                self.threads.push(ThreadMeta {
                    id,
                    project_id,
                    title: object["title"].as_str().unwrap_or("Thread").to_string(),
                    updated_at: updated_at.map(|s| s * 1000.0).unwrap_or(0.0),
                    session_id,
                });
            }
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
        let db = db.lock().unwrap();
        let t0 = Instant::now();
        // Rows are numbered from 0 without gaps: the last number is a lookup, a count a scan.
        let total: usize = db
            .query_row("SELECT max(seq) FROM entries WHERE thread_id = ?", [thread], |r| r.get::<_, Option<i64>>(0))
            .optional()
            .ok()
            .flatten()
            .flatten()
            .map(|n| n as usize + 1)
            .unwrap_or(0);
        let t1 = Instant::now();
        let end = before.unwrap_or(total).min(total);
        let from = end.saturating_sub(limit);
        let mut statement = db
            .prepare_cached(
                "SELECT id, kind, text, tool_name, tool_input, tool_result, tool_result_length, tool_is_error \
                 FROM entries WHERE thread_id = ? AND seq >= ? ORDER BY seq LIMIT ?",
            )
            .unwrap();
        let rows = statement
            .query_map(rusqlite::params![thread, from as i64, (end - from) as i64], |row| {
                let kind = match row.get::<_, String>(1)?.as_str() {
                    "user" => Some(Kind::User),
                    "assistant" => Some(Kind::Assistant),
                    "thinking" => Some(Kind::Thinking),
                    "tool" => Some(Kind::Tool),
                    "notice" => Some(Kind::Notice),
                    "commandOutput" => Some(Kind::Command),
                    _ => None,
                };
                let Some(kind) = kind else { return Ok(None) };
                let result: Option<String> = row.get(5)?;
                let mut entry = EntryData::new(row.get::<_, String>(0)?, kind, row.get::<_, String>(2)?);
                entry.tool_name = row.get(3)?;
                entry.tool_input = row.get::<_, Option<String>>(4)?.map(|s| head(&s, TOOL_TEXT_LIMIT));
                entry.result_len = row
                    .get::<_, Option<i64>>(6)?
                    .map(|n| n as usize)
                    .unwrap_or_else(|| result.as_ref().map_or(0, |r| r.len()));
                entry.tool_result = result.map(|s| head(&s, TOOL_TEXT_LIMIT));
                entry.is_error = row.get::<_, Option<i64>>(7)? == Some(1);
                Ok(Some(entry))
            })
            .unwrap();
        let entries: Vec<EntryData> = rows.flatten().flatten().collect();
        if env("YH_TIMING").as_deref() == Some("1") {
            eprintln!(
                "page {thread}: count {:.0} ms, rows {:.0} ms",
                (t1 - t0).as_secs_f64() * 1000.0,
                t1.elapsed().as_secs_f64() * 1000.0
            );
        }
        Page { total, from, entries }
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
