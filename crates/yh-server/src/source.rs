//! Reading a data folder's `harness.sqlite`, read-only. The app writes it (WAL), so reads never
//! wait on it; every row it writes takes the next `rev`, which is how changes are found.

use std::path::Path;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::{Map, Value};
use yh_core::library::ThreadMeta;

use yh_proto::{INPUT_LIMIT, ModelOption, Project, RESULT_LIMIT, Row, Thread, trim};

pub struct Source {
    c: Connection,
}

/// A row that changed, and its thread.
pub struct Change {
    pub thread: String,
    pub row: Row,
}

const COLUMNS: &str = "seq, rev, id, kind, text, tool_name, tool_input, tool_result, tool_result_length, tool_is_error,
                       notice_style, attachments, date, turn_ended_at, changed_files";

impl Source {
    pub fn open(directory: &Path) -> Result<Source, String> {
        let path = directory.join("harness.sqlite");
        if !path.exists() {
            return Err(format!("no harness.sqlite in {}", directory.display()));
        }
        let c = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
            .map_err(|e| format!("opening {}: {e}", path.display()))?;
        c.busy_timeout(std::time::Duration::from_secs(2)).map_err(|e| e.to_string())?;
        Ok(Source { c })
    }

    pub fn projects(&self) -> rusqlite::Result<Vec<Project>> {
        let mut s = self.c.prepare_cached("SELECT id, json FROM projects ORDER BY position")?;
        let rows = s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?)))?;
        rows.map(|r| {
            r.map(|(id, json)| {
                let o = object(&json);
                let name = o.get("name").and_then(Value::as_str).unwrap_or("Project").to_string();
                let mode = Some(o.get("permissionMode").and_then(Value::as_str).unwrap_or("auto").to_string());
                Project { id, name, mode }
            })
        })
        .collect()
    }

    /// Every thread in sidebar order, `active`, `running` and `waiting` left false.
    pub fn threads(&self) -> rusqlite::Result<Vec<Thread>> {
        let mut s = self.c.prepare_cached("SELECT id, project_id, updated_at, session_id, json FROM threads ORDER BY position")?;
        let rows = s.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<f64>>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, Vec<u8>>(4)?))
        })?;
        rows.map(|r| {
            r.map(|(id, project, updated, session, json)| {
                let m = ThreadMeta::from_record(id, project, updated, session, &object(&json));
                Thread {
                    id: m.id,
                    project_id: m.project_id,
                    title: m.title,
                    created_at: m.created_at,
                    updated_at: m.updated_at,
                    pinned: m.pinned,
                    settled: m.settled_at.is_some(),
                    model: m.model,
                    effort: m.effort,
                    active: false,
                    running: false,
                    waiting: false,
                }
            })
        })
        .collect()
    }

    pub fn max_rev(&self) -> rusqlite::Result<i64> {
        self.c.query_row("SELECT coalesce(max(rev), 0) FROM entries", [], |r| r.get(0))
    }

    /// Rows written after `rev`, oldest first, at most `limit`.
    pub fn changes_since(&self, rev: i64, limit: i64) -> rusqlite::Result<Vec<Change>> {
        let mut s = self.c.prepare_cached(&format!("SELECT thread_id, {COLUMNS} FROM entries WHERE rev > ? ORDER BY rev LIMIT ?"))?;
        let rows = s.query_map(params![rev, limit], |r| Ok(Change { thread: r.get(0)?, row: row(r, 1)? }))?;
        rows.collect()
    }

    /// The models the CLI last listed (the app keeps them as `claudeModels`).
    pub fn models(&self) -> rusqlite::Result<Vec<ModelOption>> {
        let json: Option<Vec<u8>> = self.c.query_row("SELECT value FROM settings WHERE key = 'claudeModels'", [], |r| r.get(0)).optional()?;
        let list: Value = json.and_then(|j| serde_json::from_slice(&j).ok()).unwrap_or_default();
        Ok(list
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(yh_core::claude::ModelInfo::from_json)
            .map(|m| ModelOption { id: m.value, name: m.display_name, efforts: m.efforts, auto: m.supports_auto_mode })
            .collect())
    }

    /// A row's whole tool input and result: the columns, or their blobs when they're long.
    pub fn whole(&self, thread: &str, seq: i64) -> rusqlite::Result<(Option<String>, Option<String>)> {
        let found: Option<(Option<String>, Option<String>, Option<String>, Option<String>)> = self
            .c
            .query_row("SELECT tool_input, tool_input_blob, tool_result, tool_result_blob FROM entries WHERE thread_id = ? AND seq = ?", params![thread, seq], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .optional()?;
        let Some((input, input_blob, result, result_blob)) = found else { return Ok((None, None)) };
        let whole = |head: Option<String>, blob: Option<String>| -> rusqlite::Result<Option<String>> {
            match blob {
                Some(hash) => Ok(yh_core::db::blob(&self.c, &hash)?.map(|b| String::from_utf8_lossy(&b).into_owned()).or(head)),
                None => Ok(head),
            }
        };
        Ok((whole(input, input_blob)?, whole(result, result_blob)?))
    }

    /// How many rows a transcript has (they're numbered from 0 without gaps).
    pub fn count(&self, thread: &str) -> rusqlite::Result<i64> {
        self.c
            .query_row("SELECT max(seq) FROM entries WHERE thread_id = ?", [thread], |r| r.get::<_, Option<i64>>(0))
            .map(|n| n.map_or(0, |n| n + 1))
    }

    /// Rows `from..to`.
    pub fn rows(&self, thread: &str, from: i64, to: i64) -> rusqlite::Result<Vec<Row>> {
        let mut s = self.c.prepare_cached(&format!("SELECT {COLUMNS} FROM entries WHERE thread_id = ? AND seq >= ? AND seq < ? ORDER BY seq"))?;
        let rows = s.query_map(params![thread, from, to], |r| row(r, 0))?;
        rows.collect()
    }
}

fn row(r: &rusqlite::Row, at: usize) -> rusqlite::Result<Row> {
    let list = |s: Option<String>| -> Vec<String> { s.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default() };
    Ok(Row {
        seq: r.get(at)?,
        rev: r.get(at + 1)?,
        id: r.get(at + 2)?,
        kind: r.get(at + 3)?,
        text: r.get::<_, Option<String>>(at + 4)?.unwrap_or_default(),
        tool_name: r.get(at + 5)?,
        tool_input: r.get::<_, Option<String>>(at + 6)?.map(|s| trim(s, INPUT_LIMIT)),
        tool_result: r.get::<_, Option<String>>(at + 7)?.map(|s| trim(s, RESULT_LIMIT)),
        tool_result_length: r.get(at + 8)?,
        tool_is_error: r.get::<_, Option<i64>>(at + 9)?.map(|v| v != 0),
        notice_style: r.get(at + 10)?,
        attachments: list(r.get(at + 11)?),
        date: r.get(at + 12)?,
        turn_ended_at: r.get(at + 13)?,
        changed_files: list(r.get(at + 14)?),
    })
}

fn object(json: &[u8]) -> Map<String, Value> {
    match serde_json::from_slice(json) {
        Ok(Value::Object(o)) => o,
        _ => Map::new(),
    }
}
