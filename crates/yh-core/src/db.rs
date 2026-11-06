//! `harness.sqlite`, read and written in the Swift app's format (docs/storage.md), so the user's
//! data carries over both ways until the switch. The port of `HarnessDatabase`:
//!
//! - **Writes go through one background thread**, in one transaction per batch. Saves that arrive
//!   while one is being written collapse: the newest state wins, and a transcript's changed rows
//!   merge.
//! - **Reads use connections of their own** (WAL), so they never wait for a write. A read of a
//!   transcript with writes still waiting flushes them first, so what's read has them.
//! - **Rows keep every column the Swift app writes**, even the ones this app doesn't use yet, so
//!   a row read and saved again comes back unchanged.

use crate::schema::STEPS;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Tool input, result or sub-agent run over this many bytes goes in `blobs`.
pub const BLOB_THRESHOLD: usize = 32 * 1024;
/// A result stored as a blob keeps this many characters in the row: what's loaded.
pub const RESULT_HEAD: usize = 20_000;
/// Tools whose result is read whole: a sub-agent's report is shown in full.
pub const WHOLE_RESULT_TOOLS: &[&str] = &["Agent", "Task"];

/// One transcript row, every column of `entries` (but `thread_id`, `seq`, `rev`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    pub id: String,
    /// `user`, `assistant`, `thinking`, `tool`, `notice`, `commandOutput`; kept as written, so a
    /// newer app's kind survives a save.
    pub kind: String,
    pub text: String,
    pub tool_name: Option<String>,
    pub tool_input: Option<String>,
    pub tool_result: Option<String>,
    /// The whole result's blob, when the row holds only its head.
    pub tool_result_ref: Option<String>,
    pub tool_result_length: Option<i64>,
    pub tool_is_error: Option<bool>,
    pub notice_style: Option<String>,
    /// JSON array of attachment names, as written.
    pub attachments: Option<String>,
    pub delivery: Option<String>,
    /// Seconds since 1970.
    pub date: Option<f64>,
    pub turn_ended_at: Option<f64>,
    /// The sub-agent run as the Swift app's JSON (`SubagentSnapshot`).
    pub subagent: Option<Vec<u8>>,
    /// JSON array of paths.
    pub changed_files: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ThreadRecord {
    pub id: String,
    pub project_id: String,
    pub updated_at: Option<f64>,
    pub session_id: Option<String>,
    pub json: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StateRows {
    pub values: BTreeMap<String, Vec<u8>>,
    /// In sidebar order: (id, json).
    pub projects: Vec<(String, Vec<u8>)>,
    pub threads: Vec<ThreadRecord>,
}

/// A write of its own (memories), run on the writer thread in a transaction.
pub type WriteFn = Box<dyn FnOnce(&Connection, &mut i64) -> rusqlite::Result<()> + Send>;

enum Job {
    State(StateRows),
    Exec(WriteFn, flume::Sender<rusqlite::Result<()>>),
    Rows { thread: String, count: usize, rows: Vec<(usize, Row)> },
    DeleteTranscript(String),
    Flush(flume::Sender<()>),
}

pub struct Database {
    path: PathBuf,
    jobs: flume::Sender<Job>,
    readers: Mutex<Vec<Connection>>,
    /// Threads with rows waiting to be written.
    waiting: Arc<Mutex<HashSet<String>>>,
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn open_connection(path: &Path, read_only: bool) -> rusqlite::Result<Connection> {
    let flags = if read_only {
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_NO_MUTEX
    };
    let c = Connection::open_with_flags(path, flags)?;
    c.busy_timeout(std::time::Duration::from_secs(10))?;
    if read_only {
        c.execute_batch("PRAGMA query_only = 1; PRAGMA foreign_keys = ON;")?;
    }
    Ok(c)
}

fn migrate(c: &Connection) -> rusqlite::Result<()> {
    let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let target = STEPS.len() as i64;
    if version >= target {
        if version > target {
            // A newer app's database: what it added is left alone, and the rest still works.
            eprintln!("Database schema {version} is newer than this app's {target}");
        }
        return Ok(());
    }
    c.execute_batch("BEGIN")?;
    for step in &STEPS[version as usize..] {
        if let Err(e) = c.execute_batch(step) {
            let _ = c.execute_batch("ROLLBACK");
            return Err(e);
        }
    }
    c.execute_batch(&format!("PRAGMA user_version = {target}; COMMIT"))?;
    Ok(())
}

impl Database {
    /// Opens (or creates) `harness.sqlite` in `directory`.
    pub fn open(directory: &Path) -> Result<Database, String> {
        std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
        let path = directory.join("harness.sqlite");
        let writer = open_connection(&path, false).map_err(|e| format!("can't open {}: {e}", path.display()))?;
        writer
            .execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;")
            .map_err(|e| e.to_string())?;
        migrate(&writer).map_err(|e| format!("can't migrate {}: {e}", path.display()))?;
        let mut w = Writer::new(writer).map_err(|e| e.to_string())?;
        let (jobs, inbox) = flume::unbounded::<Job>();
        let waiting: Arc<Mutex<HashSet<String>>> = Arc::default();
        let waiting2 = waiting.clone();
        std::thread::Builder::new()
            .name("harness.sqlite writer".into())
            .spawn(move || {
                while let Ok(first) = inbox.recv() {
                    // Everything that's arrived meanwhile goes in the same transaction.
                    let mut jobs = vec![first];
                    jobs.extend(inbox.drain());
                    let threads: Vec<String> = jobs
                        .iter()
                        .filter_map(|j| if let Job::Rows { thread, .. } = j { Some(thread.clone()) } else { None })
                        .collect();
                    let mut flushes = vec![];
                    if let Err(e) = w.write(jobs, &mut flushes) {
                        eprintln!("harness.sqlite: write failed: {e}");
                    }
                    let mut waiting = waiting2.lock().unwrap();
                    for t in threads {
                        waiting.remove(&t);
                    }
                    drop(waiting);
                    for f in flushes {
                        let _ = f.send(());
                    }
                    // Idle: bring the full-text index up to date, a batch at a time, so a write
                    // that arrives waits for one batch at most.
                    while inbox.is_empty() {
                        match w.index_batch(500) {
                            Ok(true) => continue,
                            Ok(false) => break,
                            Err(e) => {
                                eprintln!("harness.sqlite: indexing failed: {e}");
                                break;
                            }
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Database { path, jobs, readers: Mutex::new(vec![]), waiting })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read<T>(&self, body: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> rusqlite::Result<T> {
        let connection = match self.readers.lock().unwrap().pop() {
            Some(c) => c,
            None => open_connection(&self.path, true)?,
        };
        let result = body(&connection);
        self.readers.lock().unwrap().push(connection);
        result
    }

    /// Runs `f` on the writer thread in a transaction and waits for it. `rev` is the change
    /// counter rows and memories share: take the next with `*rev += 1`.
    pub fn write(&self, f: impl FnOnce(&Connection, &mut i64) -> rusqlite::Result<()> + Send + 'static) -> rusqlite::Result<()> {
        let (tx, rx) = flume::bounded(1);
        self.jobs.send(Job::Exec(Box::new(f), tx)).map_err(|_| rusqlite::Error::InvalidQuery)?;
        rx.recv().map_err(|_| rusqlite::Error::InvalidQuery)?
    }

    /// A read on a connection of the pool.
    pub fn read_with<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> rusqlite::Result<T> {
        self.read(f)
    }

    /// Waits until everything sent so far is on disk.
    pub fn flush(&self) {
        let (tx, rx) = flume::bounded(1);
        if self.jobs.send(Job::Flush(tx)).is_ok() {
            let _ = rx.recv();
        }
    }

    pub fn load_state(&self) -> StateRows {
        self.read(read_state).unwrap_or_default()
    }

    /// Writes what changed since the last save, in the background.
    pub fn save_state(&self, state: StateRows) {
        let _ = self.jobs.send(Job::State(state));
    }

    /// Rows `seq → row` changed, and how many rows the transcript has now (rows from `count` on
    /// are deleted).
    pub fn save_rows(&self, thread: &str, count: usize, rows: Vec<(usize, Row)>) {
        self.waiting.lock().unwrap().insert(thread.to_string());
        let _ = self.jobs.send(Job::Rows { thread: thread.to_string(), count, rows });
    }

    pub fn delete_transcript(&self, thread: &str) {
        let _ = self.jobs.send(Job::DeleteTranscript(thread.to_string()));
    }

    fn before_reading(&self, thread: &str) {
        if self.waiting.lock().unwrap().contains(thread) {
            self.flush();
        }
    }

    /// How many rows a transcript has, without reading them.
    pub fn transcript_count(&self, thread: &str) -> usize {
        self.before_reading(thread);
        // Rows are numbered from 0 without gaps: the last number is a lookup, a count a scan.
        self.read(|c| {
            c.query_row("SELECT max(seq) FROM entries WHERE thread_id = ?", [thread], |r| r.get::<_, Option<i64>>(0))
                .map(|n| n.map_or(0, |n| n as usize + 1))
        })
        .unwrap_or(0)
    }

    /// Rows `from..from + limit`.
    pub fn read_rows(&self, thread: &str, from: usize, limit: usize) -> Vec<Row> {
        self.before_reading(thread);
        self.read(|c| read_rows(c, thread, from, limit as i64)).unwrap_or_else(|e| {
            eprintln!("harness.sqlite: reading {thread}: {e}");
            vec![]
        })
    }

    /// Transcript rows matching `query` (every word, the last as a prefix), best first.
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        let Some(expression) = match_expression(query) else { return vec![] };
        self.read(|c| {
            let mut s = c.prepare(
                "SELECT e.thread_id, e.seq, snippet(entries_fts, 0, '[', ']', '…', 12)
                 FROM entries_fts JOIN entries e ON e.rowid = entries_fts.rowid
                 WHERE entries_fts MATCH ?1 ORDER BY bm25(entries_fts) LIMIT ?2",
            )?;
            let rows = s.query_map(params![expression, limit as i64], |r| {
                Ok(SearchHit { thread: r.get(0)?, seq: r.get::<_, i64>(1)? as usize, snippet: r.get(2)? })
            })?;
            rows.collect()
        })
        .unwrap_or_else(|e| {
            eprintln!("harness.sqlite: search failed: {e}");
            vec![]
        })
    }

    /// A whole tool result kept as a blob.
    pub fn blob_string(&self, hash: &str) -> Option<String> {
        self.read(|c| blob(c, hash)).ok().flatten().map(|b| String::from_utf8_lossy(&b).into_owned())
    }
}

#[derive(Clone, Debug)]
pub struct SearchHit {
    pub thread: String,
    pub seq: usize,
    /// The match in context, the words in `[brackets]`.
    pub snippet: String,
}

/// FTS5 syntax for what was typed: each word quoted, all required, the last as a prefix.
pub fn match_expression(query: &str) -> Option<String> {
    let words: Vec<&str> = query.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        return None;
    }
    let last = words.len() - 1;
    Some(words.iter().enumerate().map(|(i, w)| format!("\"{w}\"{}", if i == last { "*" } else { "" })).collect::<Vec<_>>().join(" "))
}

fn read_state(c: &Connection) -> rusqlite::Result<StateRows> {
    let mut state = StateRows::default();
    let mut s = c.prepare("SELECT key, value FROM settings")?;
    for row in s.query_map([], |r| Ok((r.get::<_, String>(0)?, value_bytes(r, 1)?)))? {
        let (k, v) = row?;
        state.values.insert(k, v);
    }
    let mut s = c.prepare("SELECT id, json FROM projects ORDER BY position")?;
    for row in s.query_map([], |r| Ok((r.get::<_, String>(0)?, value_bytes(r, 1)?)))? {
        state.projects.push(row?);
    }
    let mut s = c.prepare("SELECT id, project_id, updated_at, session_id, json FROM threads ORDER BY position")?;
    for row in s.query_map([], |r| {
        Ok(ThreadRecord {
            id: r.get(0)?,
            project_id: r.get(1)?,
            updated_at: r.get(2)?,
            session_id: r.get(3)?,
            json: value_bytes(r, 4)?,
        })
    })? {
        state.threads.push(row?);
    }
    Ok(state)
}

/// A BLOB or TEXT column as bytes (JSON was written as BLOB, but be lenient).
fn value_bytes(r: &rusqlite::Row, i: usize) -> rusqlite::Result<Vec<u8>> {
    use rusqlite::types::ValueRef;
    Ok(match r.get_ref(i)? {
        ValueRef::Blob(b) => b.to_vec(),
        ValueRef::Text(t) => t.to_vec(),
        _ => vec![],
    })
}

fn opt_bytes(r: &rusqlite::Row, i: usize) -> rusqlite::Result<Option<Vec<u8>>> {
    use rusqlite::types::ValueRef;
    Ok(match r.get_ref(i)? {
        ValueRef::Blob(b) => Some(b.to_vec()),
        ValueRef::Text(t) => Some(t.to_vec()),
        _ => None,
    })
}

fn read_rows(c: &Connection, thread: &str, from: usize, limit: i64) -> rusqlite::Result<Vec<Row>> {
    let mut s = c.prepare_cached(
        "SELECT id, kind, text, tool_name, tool_input, tool_input_blob, tool_result, tool_result_blob, tool_result_length,
                tool_is_error, notice_style, attachments, delivery, date, turn_ended_at, subagent, subagent_blob, changed_files
         FROM entries WHERE thread_id = ? AND seq >= ? ORDER BY seq LIMIT ?",
    )?;
    let raw: Vec<(Row, Option<String>, Option<String>)> = s
        .query_map(params![thread, from as i64, limit], |r| {
            Ok((
                Row {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    text: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    tool_name: r.get(3)?,
                    tool_input: r.get(4)?,
                    tool_result: r.get(6)?,
                    tool_result_ref: r.get(7)?,
                    tool_result_length: r.get(8)?,
                    tool_is_error: r.get::<_, Option<i64>>(9)?.map(|v| v != 0),
                    notice_style: r.get(10)?,
                    attachments: r.get(11)?,
                    delivery: r.get(12)?,
                    date: r.get(13)?,
                    turn_ended_at: r.get(14)?,
                    subagent: opt_bytes(r, 15)?,
                    changed_files: r.get(17)?,
                },
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(16)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut rows = Vec::with_capacity(raw.len());
    for (mut row, input_blob, subagent_blob) in raw {
        if let Some(hash) = input_blob {
            row.tool_input = Some(blob(c, &hash)?.map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default());
        }
        if let (Some(hash), Some(name)) = (&row.tool_result_ref, &row.tool_name) {
            if WHOLE_RESULT_TOOLS.contains(&name.as_str()) {
                if let Some(whole) = blob(c, hash)? {
                    row.tool_result = Some(String::from_utf8_lossy(&whole).into_owned());
                    row.tool_result_ref = None;
                }
            }
        }
        if row.subagent.is_none() {
            if let Some(hash) = subagent_blob {
                row.subagent = blob(c, &hash)?;
            }
        }
        rows.push(row);
    }
    Ok(rows)
}

fn blob(c: &Connection, hash: &str) -> rusqlite::Result<Option<Vec<u8>>> {
    let found: Option<(i64, i64, Vec<u8>)> = c
        .query_row("SELECT codec, size, data FROM blobs WHERE hash = ?", [hash], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?;
    Ok(found.and_then(|(codec, size, data)| if codec == 1 { lzfse::decode(&data, size as usize) } else { Some(data) }))
}

/// The write connection and what it remembers between saves.
struct Writer {
    c: Connection,
    rev: i64,
    state: StateRows,
}

impl Writer {
    fn new(c: Connection) -> rusqlite::Result<Writer> {
        let rev = c.query_row(
            "SELECT max(coalesce((SELECT max(rev) FROM entries), 0), coalesce((SELECT max(rev) FROM memories), 0))",
            [],
            |r| r.get::<_, i64>(0),
        )?;
        let state = read_state(&c)?;
        Ok(Writer { c, rev, state })
    }

    fn write(&mut self, jobs: Vec<Job>, flushes: &mut Vec<flume::Sender<()>>) -> rusqlite::Result<()> {
        // Collapse: the newest state, each thread's rows merged in order.
        let mut state = None;
        let mut transcripts: Vec<(String, usize, BTreeMap<usize, Row>)> = vec![];
        let mut deletes = vec![];
        for job in jobs {
            match job {
                Job::State(s) => state = Some(s),
                Job::Rows { thread, count, rows } => {
                    deletes.retain(|t| t != &thread);
                    match transcripts.iter_mut().find(|(t, ..)| t == &thread) {
                        Some((_, c, map)) => {
                            *c = count;
                            map.extend(rows);
                        }
                        None => transcripts.push((thread, count, rows.into_iter().collect())),
                    }
                }
                Job::DeleteTranscript(thread) => {
                    transcripts.retain(|(t, ..)| t != &thread);
                    deletes.push(thread);
                }
                Job::Flush(tx) => flushes.push(tx),
                Job::Exec(f, reply) => {
                    // Its own transaction, in the order it came.
                    let result = (|| {
                        self.c.execute_batch("BEGIN IMMEDIATE")?;
                        match f(&self.c, &mut self.rev) {
                            Ok(()) => self.c.execute_batch("COMMIT"),
                            Err(e) => {
                                let _ = self.c.execute_batch("ROLLBACK");
                                Err(e)
                            }
                        }
                    })();
                    let _ = reply.send(result);
                }
            }
        }
        if state.is_none() && transcripts.is_empty() && deletes.is_empty() {
            return Ok(());
        }
        self.c.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| {
            if let Some(s) = state {
                self.write_state(s)?;
            }
            for (thread, count, rows) in transcripts {
                self.write_rows(&thread, count, rows)?;
            }
            for thread in deletes {
                self.truncate(&thread, 0)?;
                self.c.execute("DELETE FROM transcripts WHERE thread_id = ?", [&thread])?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => self.c.execute_batch("COMMIT"),
            Err(e) => {
                let _ = self.c.execute_batch("ROLLBACK");
                // What we remember may be ahead of the disk now: read it again.
                if let Ok(s) = read_state(&self.c) {
                    self.state = s;
                }
                Err(e)
            }
        }
    }

    fn write_state(&mut self, state: StateRows) -> rusqlite::Result<()> {
        let old = &self.state;
        for (key, value) in &state.values {
            if old.values.get(key) != Some(value) {
                self.c.execute(
                    "INSERT INTO settings(key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![key, value],
                )?;
            }
        }
        for key in old.values.keys() {
            if !state.values.contains_key(key) {
                self.c.execute("DELETE FROM settings WHERE key = ?", [key])?;
            }
        }
        let old_projects: HashMap<&str, (usize, &Vec<u8>)> =
            old.projects.iter().enumerate().map(|(i, (id, json))| (id.as_str(), (i, json))).collect();
        for (position, (id, json)) in state.projects.iter().enumerate() {
            if old_projects.get(id.as_str()) == Some(&(position, json)) {
                continue;
            }
            self.c.execute(
                "INSERT INTO projects(id, position, json) VALUES (?, ?, ?)
                 ON CONFLICT(id) DO UPDATE SET position = excluded.position, json = excluded.json",
                params![id, position as i64, json],
            )?;
        }
        let ids: HashSet<&str> = state.projects.iter().map(|(id, _)| id.as_str()).collect();
        for (id, _) in &old.projects {
            if !ids.contains(id.as_str()) {
                self.c.execute("DELETE FROM projects WHERE id = ?", [id])?;
            }
        }
        let old_threads: HashMap<&str, (usize, &ThreadRecord)> =
            old.threads.iter().enumerate().map(|(i, t)| (t.id.as_str(), (i, t))).collect();
        for (position, t) in state.threads.iter().enumerate() {
            if old_threads.get(t.id.as_str()) == Some(&(position, t)) {
                continue;
            }
            self.c.execute(
                "INSERT INTO threads(id, project_id, position, updated_at, session_id, json) VALUES (?, ?, ?, ?, ?, ?)
                 ON CONFLICT(id) DO UPDATE SET project_id = excluded.project_id, position = excluded.position,
                     updated_at = excluded.updated_at, session_id = excluded.session_id, json = excluded.json",
                params![t.id, t.project_id, position as i64, t.updated_at, t.session_id, t.json],
            )?;
        }
        let ids: HashSet<&str> = state.threads.iter().map(|t| t.id.as_str()).collect();
        for t in &old.threads {
            if !ids.contains(t.id.as_str()) {
                // The transcript stays until it's deleted: an undone delete brings the thread back.
                self.c.execute("DELETE FROM threads WHERE id = ?", [&t.id])?;
            }
        }
        self.state = state;
        Ok(())
    }

    fn write_rows(&mut self, thread: &str, count: usize, rows: BTreeMap<usize, Row>) -> rusqlite::Result<()> {
        let saved: usize = self
            .c
            .query_row("SELECT entry_count FROM transcripts WHERE thread_id = ?", [thread], |r| r.get::<_, i64>(0))
            .optional()?
            .map_or(0, |n| n as usize);
        for (seq, row) in rows {
            if seq < count {
                self.upsert(thread, seq, &row)?;
            }
        }
        if saved > count {
            self.truncate(thread, count)?;
        }
        self.c.execute(
            "INSERT INTO transcripts(thread_id, saved_at, entry_count) VALUES (?, ?, ?)
             ON CONFLICT(thread_id) DO UPDATE SET saved_at = excluded.saved_at, entry_count = excluded.entry_count",
            params![thread, now(), count as i64],
        )?;
        Ok(())
    }

    fn truncate(&mut self, thread: &str, from: usize) -> rusqlite::Result<()> {
        self.c.execute(
            "DELETE FROM entries_fts WHERE rowid IN (SELECT rowid FROM entries WHERE thread_id = ? AND seq >= ?)",
            params![thread, from as i64],
        )?;
        self.c.execute("DELETE FROM entries WHERE thread_id = ? AND seq >= ?", params![thread, from as i64])?;
        Ok(())
    }

    fn store_blob(&mut self, data: &[u8]) -> rusqlite::Result<String> {
        let hash = sha256_hex(data);
        let (codec, stored) = match lzfse::encode(data) {
            Some(compressed) if compressed.len() < data.len() => (1, compressed),
            _ => (0, data.to_vec()),
        };
        self.c.execute(
            "INSERT OR IGNORE INTO blobs(hash, size, codec, data) VALUES (?, ?, ?, ?)",
            params![hash, data.len() as i64, codec, stored],
        )?;
        Ok(hash)
    }

    fn upsert(&mut self, thread: &str, seq: usize, row: &Row) -> rusqlite::Result<()> {
        let (mut input, mut input_blob) = (row.tool_input.clone(), None::<String>);
        if let Some(text) = &row.tool_input {
            if text.len() > BLOB_THRESHOLD {
                input_blob = Some(self.store_blob(text.as_bytes())?);
                input = None;
            }
        }
        let (mut result, mut result_blob, mut result_length) =
            (row.tool_result.clone(), row.tool_result_ref.clone(), row.tool_result_length);
        if result_blob.is_none() {
            if let Some(text) = &row.tool_result {
                if text.len() > BLOB_THRESHOLD {
                    result_blob = Some(self.store_blob(text.as_bytes())?);
                    result_length = Some(text.chars().count() as i64);
                    result = Some(text.chars().take(RESULT_HEAD).collect());
                }
            }
        }
        let (mut subagent, mut subagent_blob) = (row.subagent.clone(), None::<String>);
        if let Some(json) = &row.subagent {
            if json.len() > BLOB_THRESHOLD {
                subagent_blob = Some(self.store_blob(json)?);
                subagent = None;
            }
        }
        self.rev += 1;
        self.c.execute(
            "INSERT INTO entries(thread_id, seq, id, kind, text, tool_name, tool_input, tool_input_blob, tool_result,
                                 tool_result_blob, tool_result_length, tool_is_error, notice_style, attachments, delivery,
                                 date, turn_ended_at, subagent, subagent_blob, rev, changed_files)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(thread_id, seq) DO UPDATE SET id = excluded.id, kind = excluded.kind, text = excluded.text,
                 tool_name = excluded.tool_name, tool_input = excluded.tool_input, tool_input_blob = excluded.tool_input_blob,
                 tool_result = excluded.tool_result, tool_result_blob = excluded.tool_result_blob,
                 tool_result_length = excluded.tool_result_length, tool_is_error = excluded.tool_is_error,
                 notice_style = excluded.notice_style, attachments = excluded.attachments, delivery = excluded.delivery,
                 date = excluded.date, turn_ended_at = excluded.turn_ended_at, subagent = excluded.subagent,
                 subagent_blob = excluded.subagent_blob, rev = excluded.rev, changed_files = excluded.changed_files",
            params![
                thread,
                seq as i64,
                row.id,
                row.kind,
                row.text,
                row.tool_name,
                input,
                input_blob,
                result,
                result_blob,
                result_length,
                row.tool_is_error,
                row.notice_style,
                row.attachments,
                row.delivery,
                row.date,
                row.turn_ended_at,
                subagent,
                subagent_blob,
                self.rev,
                row.changed_files
            ],
        )?;
        Ok(())
    }
}

impl Writer {
    /// One batch of the change feed into the full-text index (what was said; for a tool call
    /// its name, input and the start of its result). True if there's more.
    fn index_batch(&mut self, limit: usize) -> rusqlite::Result<bool> {
        let from: i64 = self
            .c
            .query_row("SELECT rev FROM index_state WHERE indexer = 'fts'", [], |r| r.get(0))
            .optional()?
            .unwrap_or(0);
        let changes: Vec<(i64, String, String, Option<String>, Option<String>, Option<String>, i64)> = {
            let mut s = self.c.prepare_cached(
                "SELECT rowid, kind, text, tool_name, substr(tool_input, 1, 4096), substr(tool_result, 1, 2048), rev
                 FROM entries WHERE rev > ? ORDER BY rev LIMIT ?",
            )?;
            let rows = s.query_map(params![from, limit as i64], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?.unwrap_or_default(), r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let Some(last) = changes.last().map(|c| c.6) else { return Ok(false) };
        self.c.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| {
            for (rowid, kind, text, name, input, result, _) in &changes {
                self.c.execute("DELETE FROM entries_fts WHERE rowid = ?", [rowid])?;
                let body = match kind.as_str() {
                    "user" | "assistant" | "notice" | "commandOutput" => text.clone(),
                    "tool" => [name, input, result].iter().filter_map(|x| x.as_deref()).collect::<Vec<_>>().join("\n"),
                    _ => String::new(),
                };
                if !body.is_empty() {
                    self.c.execute("INSERT INTO entries_fts(rowid, body) VALUES (?, ?)", params![rowid, body])?;
                }
            }
            self.c.execute(
                "INSERT INTO index_state(indexer, rev, updated_at) VALUES ('fts', ?, ?)
                 ON CONFLICT(indexer) DO UPDATE SET rev = excluded.rev, updated_at = excluded.updated_at",
                params![last, now()],
            )?;
            Ok(())
        })();
        match result {
            Ok(()) => self.c.execute_batch("COMMIT")?,
            Err(e) => {
                let _ = self.c.execute_batch("ROLLBACK");
                return Err(e);
            }
        }
        Ok(changes.len() == limit)
    }
}

fn sha256_hex(data: &[u8]) -> String {
    let digest = sha256::digest(data);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Apple's libcompression, which `NSData.compressed(using: .lzfse)` is: the same bytes as the
/// Swift app's blobs.
mod lzfse {
    const LZFSE: i32 = 0x801;
    #[link(name = "compression")]
    unsafe extern "C" {
        fn compression_encode_buffer(dst: *mut u8, dst_size: usize, src: *const u8, src_size: usize, scratch: *mut u8, algorithm: i32) -> usize;
        fn compression_decode_buffer(dst: *mut u8, dst_size: usize, src: *const u8, src_size: usize, scratch: *mut u8, algorithm: i32) -> usize;
    }

    pub fn encode(data: &[u8]) -> Option<Vec<u8>> {
        let mut out = vec![0u8; data.len() + 1024];
        let n = unsafe { compression_encode_buffer(out.as_mut_ptr(), out.len(), data.as_ptr(), data.len(), std::ptr::null_mut(), LZFSE) };
        (n > 0).then(|| {
            out.truncate(n);
            out
        })
    }

    pub fn decode(data: &[u8], size: usize) -> Option<Vec<u8>> {
        let mut out = vec![0u8; size.max(1)];
        let n = unsafe { compression_decode_buffer(out.as_mut_ptr(), out.len(), data.as_ptr(), data.len(), std::ptr::null_mut(), LZFSE) };
        (n == size).then(|| {
            out.truncate(n);
            out
        })
    }
}

/// SHA-256 from CommonCrypto (the system's), as the Swift app's CryptoKit hashes blobs.
mod sha256 {
    #[link(name = "System")]
    unsafe extern "C" {
        fn CC_SHA256(data: *const u8, len: u32, md: *mut u8) -> *mut u8;
    }

    pub fn digest(data: &[u8]) -> [u8; 32] {
        let mut out = [0u8; 32];
        unsafe { CC_SHA256(data.as_ptr(), data.len() as u32, out.as_mut_ptr()) };
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("yh-db-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn row(id: &str, kind: &str, text: &str) -> Row {
        Row { id: id.into(), kind: kind.into(), text: text.into(), ..Default::default() }
    }

    #[test]
    fn creates_the_swift_schema() {
        let dir = scratch("schema");
        let db = Database::open(&dir).unwrap();
        db.flush();
        let c = Connection::open(db.path()).unwrap();
        let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, STEPS.len() as i64);
        let has: i64 = c.query_row("SELECT count(*) FROM pragma_table_info('entries') WHERE name = 'changed_files'", [], |r| r.get(0)).unwrap();
        assert_eq!(has, 1);
    }

    #[test]
    fn rows_round_trip_with_blobs() {
        let dir = scratch("rows");
        let db = Database::open(&dir).unwrap();
        let big = "result line\n".repeat(10_000);
        let mut tool = row("toolu_1", "tool", "");
        tool.tool_name = Some("Bash".into());
        tool.tool_input = Some("{\"command\":\"ls\"}".into());
        tool.tool_result = Some(big.clone());
        tool.tool_is_error = Some(false);
        tool.date = Some(1_700_000_000.5);
        db.save_rows("T1", 2, vec![(0, row("u1", "user", "hi")), (1, tool)]);
        assert_eq!(db.transcript_count("T1"), 2);
        let rows = db.read_rows("T1", 0, 10);
        assert_eq!(rows[0], row("u1", "user", "hi"));
        let t = &rows[1];
        assert_eq!(t.tool_result.as_ref().unwrap().chars().count(), RESULT_HEAD);
        assert_eq!(t.tool_result_length, Some(big.chars().count() as i64));
        assert_eq!(db.blob_string(t.tool_result_ref.as_ref().unwrap()).unwrap(), big);
        assert_eq!(t.date, Some(1_700_000_000.5));
        // Fewer rows: the rest go.
        db.save_rows("T1", 1, vec![]);
        assert_eq!(db.transcript_count("T1"), 1);
    }

    #[test]
    fn state_saves_only_changes_and_keeps_order() {
        let dir = scratch("state");
        let db = Database::open(&dir).unwrap();
        let mut s = StateRows::default();
        s.values.insert("defaults".into(), b"{}".to_vec());
        s.projects.push(("P1".into(), br#"{"id":"P1","name":"A"}"#.to_vec()));
        s.threads.push(ThreadRecord { id: "T1".into(), project_id: "P1".into(), updated_at: Some(5.0), session_id: None, json: b"{}".to_vec() });
        s.threads.push(ThreadRecord { id: "T2".into(), project_id: "P1".into(), updated_at: None, session_id: Some("s".into()), json: b"{}".to_vec() });
        db.save_state(s.clone());
        db.flush();
        assert_eq!(db.load_state(), s);
        s.threads.remove(0);
        db.save_state(s.clone());
        db.flush();
        assert_eq!(db.load_state(), s);
    }

    #[test]
    fn rows_are_indexed_and_found() {
        let dir = scratch("search");
        let db = Database::open(&dir).unwrap();
        let mut tool = row("t1", "tool", "");
        tool.tool_name = Some("Bash".into());
        tool.tool_input = Some(r#"{"command":"cargo build"}"#.into());
        db.save_rows("T", 2, vec![(0, row("u1", "user", "Why is the sourdough flat?")), (1, tool)]);
        db.flush();
        // The writer indexes when idle: wait for it.
        for _ in 0..50 {
            if !db.search("sourdo", 10).is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let hits = db.search("sourdo", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].seq, 0);
        assert!(hits[0].snippet.contains("[sourdough]"));
        assert_eq!(db.search("cargo build", 10)[0].seq, 1);
        assert!(db.search("??", 10).is_empty());
    }

    #[test]
    fn lzfse_round_trips() {
        let data = b"abcabcabcabc".repeat(1000);
        let packed = lzfse::encode(&data).unwrap();
        assert!(packed.len() < data.len());
        assert_eq!(lzfse::decode(&packed, data.len()).unwrap(), data);
    }

    /// The user's kind of data: the extreme preset (a 2 GB copy of real-sized threads), read.
    #[test]
    fn reads_the_extreme_preset_when_present() {
        let preset = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-harness-root/extreme");
        if !preset.join("harness.sqlite").exists() {
            return;
        }
        let dir = scratch("extreme");
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["harness.sqlite", "harness.sqlite-wal", "harness.sqlite-shm"] {
            if preset.join(f).exists() {
                // APFS clone: instant, and the preset is never written.
                let status = std::process::Command::new("cp").arg("-c").arg(preset.join(f)).arg(dir.join(f)).status().unwrap();
                assert!(status.success());
            }
        }
        let db = Database::open(&dir).unwrap();
        let state = db.load_state();
        assert!(state.threads.len() >= 20);
        let t = &state.threads[0].id;
        let n = db.transcript_count(t);
        assert!(n > 1000);
        let rows = db.read_rows(t, n - 160, 160);
        assert_eq!(rows.len(), 160);
        // A blob the Swift app wrote (LZFSE) decodes to its full length.
        let c = Connection::open(db.path()).unwrap();
        let (hash, size): (String, i64) = c
            .query_row("SELECT hash, size FROM blobs WHERE codec = 1 LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(db.blob_string(&hash).unwrap().len() as i64, size);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
