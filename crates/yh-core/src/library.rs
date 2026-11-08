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
    /// Images sent with your message: names in the data folder's `attachments/`.
    pub attachments: Vec<String>,
    /// When it was written (seconds since 1970), when known.
    pub date: Option<f64>,
    /// On a message of yours that started a turn: when that turn ended (seconds since 1970).
    pub turn_ended_at: Option<f64>,
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
            attachments: vec![],
            date: None,
            turn_ended_at: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProjectMeta {
    pub id: String,
    pub name: String,
    pub path: String,
    /// The app's permission mode for the project's threads: `auto`, `ask`, `acceptEdits`,
    /// `plan`, `bypass` (see `cli_mode`).
    pub mode: String,
}

/// The app's permission mode as the CLI's `--permission-mode` (none for `ask`: the default).
pub fn cli_mode(mode: &str) -> Option<&'static str> {
    match mode {
        "ask" | "default" => None,
        "bypass" | "bypassPermissions" => Some("bypassPermissions"),
        "acceptEdits" => Some("acceptEdits"),
        "plan" => Some("plan"),
        _ => Some("auto"),
    }
}

/// As `set_permission_mode` names it, mid-session.
pub fn control_mode(mode: &str) -> &'static str {
    cli_mode(mode).unwrap_or("default")
}

impl ProjectMeta {
    fn from_record(id: String, o: &Map<String, Value>) -> ProjectMeta {
        ProjectMeta {
            id,
            name: o.get("name").and_then(Value::as_str).unwrap_or("Project").to_string(),
            path: o.get("path").and_then(Value::as_str).unwrap_or("").to_string(),
            mode: o.get("permissionMode").and_then(Value::as_str).unwrap_or("auto").to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ThreadMeta {
    pub id: String,
    pub project_id: String,
    pub title: String,
    /// Milliseconds since 1970: the later of last activity and when it was brought back.
    pub updated_at: f64,
    /// When it was made (ms): the sidebar's order, so rows don't move as threads work.
    pub created_at: f64,
    pub session_id: Option<String>,
    pub pinned: bool,
    /// When it was put on its project's Settled shelf (ms).
    pub settled_at: Option<f64>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub draft: Option<String>,
    /// Renamed by hand: generated titles leave it alone.
    pub title_chosen: bool,
    /// A turn was running when the app last saved: a quit or crash cut it off.
    pub turn_in_progress: bool,
    /// Where its `claude` runs, when not the project's folder (a worktree).
    pub working_directory: Option<String>,
    /// The folders its tool calls touched, and the ones they changed something in
    /// (`footprint`, `writtenFootprint`): which checkouts it works in.
    pub footprint: Vec<String>,
    pub written_footprint: Vec<String>,
    /// How full its context was when last heard from (tokens), and its model's window.
    pub context_tokens: Option<u64>,
    pub context_window: Option<u64>,
}

impl ThreadMeta {
    pub fn new(id: String, project_id: String, title: String) -> ThreadMeta {
        ThreadMeta {
            id,
            project_id,
            title,
            updated_at: now_ms(),
            created_at: now_ms(),
            session_id: None,
            pinned: false,
            settled_at: None,
            model: None,
            effort: None,
            draft: None,
            title_chosen: false,
            turn_in_progress: false,
            working_directory: None,
            footprint: vec![],
            written_footprint: vec![],
            context_tokens: None,
            context_window: None,
        }
    }

    /// From a thread's record as the Swift app writes it.
    pub fn from_record(id: String, project_id: String, updated_at: Option<f64>, session_id: Option<String>, o: &Map<String, Value>) -> ThreadMeta {
        let s = |k: &str| o.get(k).and_then(Value::as_str).map(str::to_string);
        let ms = |k: &str| o.get(k).and_then(Value::as_str).and_then(parse_iso).map(|t| t * 1000.0);
        let updated = updated_at.map(|t| t * 1000.0).or_else(|| ms("updatedAt")).unwrap_or(0.0);
        ThreadMeta {
            id,
            project_id,
            title: s("title").unwrap_or_else(|| "Thread".into()),
            updated_at: updated.max(ms("reactivatedAt").unwrap_or(0.0)),
            created_at: ms("createdAt").unwrap_or(updated),
            session_id: session_id.or_else(|| s("sessionID")),
            pinned: o.get("isPinned").and_then(Value::as_bool).unwrap_or(false),
            settled_at: ms("settledAt"),
            model: s("model").filter(|m| !m.is_empty()),
            effort: s("effort").filter(|m| !m.is_empty()),
            draft: s("draft").filter(|m| !m.is_empty()),
            title_chosen: o.get("titleChosen").and_then(Value::as_bool).unwrap_or(false),
            turn_in_progress: o.get("turnInProgress").and_then(Value::as_bool).unwrap_or(false),
            working_directory: s("workingDirectory"),
            footprint: list(o, "footprint"),
            written_footprint: list(o, "writtenFootprint"),
            context_tokens: o.get("contextTokens").and_then(Value::as_u64),
            context_window: o.get("contextWindow").and_then(Value::as_u64),
        }
    }
}

fn list(o: &Map<String, Value>, key: &str) -> Vec<String> {
    o.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default()
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

    /// After an import: someone else's Home folder isn't here. A development run's Home is its
    /// own `home` folder, whatever the file says; otherwise a Home whose folder is missing
    /// goes back to your home folder (the Swift app's `importedState`). Returns where it runs.
    pub fn fix_imported_home(&mut self, data_folder: &Path, development: bool) -> Option<String> {
        let id = self.setting("homeProjectID").and_then(|v| v.as_str().map(str::to_string)).or_else(|| self.projects.first().map(|p| p.id.clone()))?;
        let current = self.projects.iter().find(|p| p.id == id)?.path.clone();
        let (path, name) = if development {
            let home = data_folder.join("home");
            let _ = std::fs::create_dir_all(&home);
            (home.to_string_lossy().into_owned(), None)
        } else if !Path::new(&current).is_dir() {
            (env("HOME").unwrap_or_default(), Some("Home".to_string()))
        } else {
            return Some(current);
        };
        self.update_project(&id, |p| {
            p.insert("path".into(), path.clone().into());
            if let Some(n) = &name {
                p.insert("name".into(), n.clone().into());
            }
        });
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == id) {
            p.path = path.clone();
            if let Some(n) = name {
                p.name = n;
            }
        }
        Some(path)
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
                self.projects.push(ProjectMeta::from_record(id, &object));
                records.projects.push(object);
            }
            for t in state.threads {
                let Ok(Value::Object(object)) = serde_json::from_slice::<Value>(&t.json) else { continue };
                self.threads.push(ThreadMeta::from_record(t.id, t.project_id, t.updated_at, t.session_id, &object));
                records.threads.push(object);
            }
            *self.records.lock().unwrap() = records;
        }
        // `YH_PROJECT=<folder>`: a project of its own, for trying live turns somewhere throwaway;
        // saved like any project when there's a database, so its threads are real ones.
        if let Some(path) = env("YH_PROJECT") {
            // `YH_MODE` sets its permission mode (`ask` to see permission cards).
            let mode = env("YH_MODE").unwrap_or_else(|| "auto".into());
            if self.db.is_some() {
                let mut project = self.add_project(&path);
                self.update_project(&project.id, |p| {
                    p.insert("permissionMode".into(), mode.clone().into());
                });
                project.mode = mode;
                self.projects.retain(|p| p.id != project.id);
                self.projects.insert(0, project);
            } else {
                self.projects.insert(0, ProjectMeta { id: format!("dir:{path}"), name: last_component(&path), path, mode });
            }
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
                    self.projects.push(ProjectMeta { id: project_id.clone(), name: last_component(&path), path, mode: "auto".into() });
                }
                let id = format!("cc:{}", file.session_id);
                self.session_files.insert(id.clone(), file.path);
                let mut meta = ThreadMeta::new(id, project_id, title);
                meta.updated_at = file.modified;
                meta.session_id = Some(file.session_id);
                self.threads.push(meta);
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
        ThreadMeta::new(id, project_id.into(), title.into())
    }

    /// Removes a thread and its transcript.
    pub fn remove_thread(&self, id: &str) {
        self.records.lock().unwrap().threads.retain(|t| t.get("id").and_then(Value::as_str) != Some(id));
        if let Some(db) = &self.db {
            db.delete_transcript(id);
        }
        self.save();
    }

    /// A thread's record as saved.
    pub fn thread_record(&self, id: &str) -> Option<Map<String, Value>> {
        self.records.lock().unwrap().threads.iter().find(|t| t.get("id").and_then(Value::as_str) == Some(id)).cloned()
    }

    /// Takes a thread's record out (its rows stay, for Undo); `purge` deletes them later.
    pub fn take_thread(&self, id: &str) -> Option<Map<String, Value>> {
        let mut records = self.records.lock().unwrap();
        let i = records.threads.iter().position(|t| t.get("id").and_then(Value::as_str) == Some(id))?;
        let record = records.threads.remove(i);
        drop(records);
        self.save();
        Some(record)
    }

    /// Puts a thread's record back (Undo of a delete).
    pub fn restore_thread(&self, record: Map<String, Value>) {
        self.records.lock().unwrap().threads.push(record);
        self.save();
    }

    /// Deletes a thread's rows for good.
    pub fn purge(&self, id: &str) {
        if let Some(db) = &self.db {
            db.delete_transcript(id);
        }
    }

    /// Takes a project out of the list (its threads are removed by the caller). Its ID is kept
    /// by folder (`removedProjectIDs`, as the Swift app keeps it): adding the folder again gets
    /// it back, and with it its memory.
    pub fn remove_project(&self, id: &str) {
        let mut records = self.records.lock().unwrap();
        let path = records.projects.iter().find(|p| p.get("id").and_then(Value::as_str) == Some(id)).and_then(|p| p.get("path").and_then(Value::as_str).map(str::to_string));
        records.projects.retain(|p| p.get("id").and_then(Value::as_str) != Some(id));
        if let Some(path) = path {
            let mut removed = records.values.get("removedProjectIDs").and_then(|v| serde_json::from_slice::<Map<String, Value>>(v).ok()).unwrap_or_default();
            removed.insert(path, id.into());
            records.values.insert("removedProjectIDs".into(), serde_json::to_vec(&removed).unwrap_or_default());
        }
        drop(records);
        self.save();
    }

    /// Changes a project's record (`permissionMode`…) and saves.
    pub fn update_project(&self, id: &str, change: impl FnOnce(&mut Map<String, Value>)) {
        let mut records = self.records.lock().unwrap();
        let Some(p) = records.projects.iter_mut().find(|p| p.get("id").and_then(Value::as_str) == Some(id)) else { return };
        change(p);
        drop(records);
        self.save();
    }

    /// A folder as a project; the one already there if it's known.
    pub fn add_project(&self, path: &str) -> ProjectMeta {
        let mut records = self.records.lock().unwrap();
        if let Some(p) = records.projects.iter().find(|p| p.get("path").and_then(Value::as_str) == Some(path)) {
            let id = p.get("id").and_then(Value::as_str).unwrap_or("").to_string();
            return ProjectMeta::from_record(id, p);
        }
        // A folder you had before gets its old ID back, unless something else has it now.
        let mut removed = records.values.get("removedProjectIDs").and_then(|v| serde_json::from_slice::<Map<String, Value>>(v).ok()).unwrap_or_default();
        let old = removed.remove(path).and_then(|v| v.as_str().map(str::to_string));
        let id = match old {
            Some(old) if !records.projects.iter().any(|p| p.get("id").and_then(Value::as_str) == Some(old.as_str())) => {
                records.values.insert("removedProjectIDs".into(), serde_json::to_vec(&removed).unwrap_or_default());
                old
            }
            _ => new_uuid(),
        };
        let mut object = Map::new();
        object.insert("id".into(), id.clone().into());
        object.insert("name".into(), last_component(path).into());
        object.insert("path".into(), path.into());
        // Settings → General's permissions for new projects.
        let mode = records.values.get("defaults").and_then(|v| serde_json::from_slice::<Value>(v).ok()).and_then(|d| d["permissionMode"].as_str().map(str::to_string));
        object.insert("permissionMode".into(), mode.unwrap_or_else(|| "auto".into()).into());
        let meta = ProjectMeta::from_record(id, &object);
        records.projects.push(object);
        drop(records);
        self.save();
        meta
    }

    /// A setting as JSON (`defaults`, `claudeModels`, `homeProjectID`…).
    pub fn setting(&self, key: &str) -> Option<Value> {
        let records = self.records.lock().unwrap();
        let value = records.values.get(key).and_then(|v| serde_json::from_slice(v).ok())?;
        Some(if DATE_MAPS.contains(&key) { date_map(value, |d| d.as_str().and_then(parse_iso).or_else(|| d.as_f64()).map(Value::from)) } else { value })
    }

    pub fn set_setting(&self, key: &str, value: &Value) {
        let value = if DATE_MAPS.contains(&key) {
            &date_map(value.clone(), |d| d.as_f64().map(|s| Value::from(iso(s as i64))).or_else(|| d.is_string().then(|| d.clone())))
        } else {
            value
        };
        let bytes = serde_json::to_vec(value).unwrap_or_default();
        self.records.lock().unwrap().values.insert(key.to_string(), bytes);
        self.save();
    }

    /// One field of a thread's record, as saved (`pullRequests`, `workedPullRequests`…).
    pub fn thread_field(&self, id: &str, key: &str) -> Option<Value> {
        let records = self.records.lock().unwrap();
        records.threads.iter().find(|t| t.get("id").and_then(Value::as_str) == Some(id)).and_then(|t| t.get(key).cloned())
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

pub struct SessionFile {
    pub path: PathBuf,
    pub folder: String,
    pub session_id: String,
    pub modified: f64,
}

/// Every session file under `<config>/projects`, newest first.
pub fn session_files(config: &Path) -> Vec<SessionFile> {
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

/// Settings that are `[String: Date]` in the Swift app (`claudeMemorySyncedAt`): ISO 8601
/// strings in the database, as 0.0.16 reads them, and seconds since 1970 to the app.
const DATE_MAPS: &[&str] = &["claudeMemorySyncedAt"];

/// Each value of an object changed by `f`; one it can't change is left out.
fn date_map(value: Value, f: impl Fn(&Value) -> Option<Value>) -> Value {
    match value {
        Value::Object(m) => Value::Object(m.into_iter().filter_map(|(k, v)| f(&v).map(|v| (k, v))).collect()),
        other => other,
    }
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
        entry.date = row.date;
        entry.turn_ended_at = row.turn_ended_at;
        entry.attachments = row.attachments.as_deref().and_then(|a| serde_json::from_str(a).ok()).unwrap_or_default();
        // A notice in the error style reads as one.
        entry.is_error = row.tool_is_error == Some(true) || row.notice_style.as_deref() == Some("error");
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
    fn date_settings_are_iso_on_disk_and_seconds_to_the_app() {
        let saved = date_map(serde_json::json!({"P1": 1_793_149_200.0, "P2": "2026-10-28T01:00:00Z"}), |d| {
            d.as_f64().map(|s| Value::from(iso(s as i64))).or_else(|| d.is_string().then(|| d.clone()))
        });
        assert_eq!(saved, serde_json::json!({"P1": "2026-10-28T01:00:00Z", "P2": "2026-10-28T01:00:00Z"}));
        let read = date_map(serde_json::json!({"P1": "2026-10-28T01:00:00Z", "P2": 5.0, "P3": true}), |d| {
            d.as_str().and_then(parse_iso).or_else(|| d.as_f64()).map(Value::from)
        });
        assert_eq!(read, serde_json::json!({"P1": 1_793_149_200.0, "P2": 5.0}));
    }

    #[test]
    fn uuids_look_like_foundations() {
        let u = new_uuid();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
        assert_eq!(u, u.to_uppercase());
    }

    fn bare() -> Library {
        Library { db: None, records: Mutex::new(Records::default()), claude_config: None, projects: vec![], threads: vec![], session_files: HashMap::new(), archives: Mutex::new(vec![]) }
    }

    #[test]
    fn a_removed_project_gets_its_id_back() {
        let lib = bare();
        let first = lib.add_project("/tmp/yh-a");
        lib.remove_project(&first.id);
        let again = lib.add_project("/tmp/yh-a");
        assert_eq!(again.id, first.id);
        // Only once: removing and adding a different folder makes a new one.
        let other = lib.add_project("/tmp/yh-b");
        assert_ne!(other.id, first.id);
    }

    #[test]
    fn threads_keep_when_they_were_made() {
        let mut o = Map::new();
        o.insert("createdAt".into(), "2026-10-28T01:00:00Z".into());
        o.insert("updatedAt".into(), "2026-10-29T01:00:00Z".into());
        let m = ThreadMeta::from_record("a".into(), "p".into(), None, None, &o);
        assert_eq!(m.created_at, 1_793_149_200_000.0);
        assert!(m.updated_at > m.created_at);
    }
}
