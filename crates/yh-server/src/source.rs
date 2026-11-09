//! Reading a data folder's `harness.sqlite`, read-only. The app writes it (WAL), so reads never
//! wait on it; every row it writes takes the next `rev`, which is how changes are found.

use std::path::Path;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::{Map, Value};
use yh_core::library::ThreadMeta;

use yh_proto::{INPUT_LIMIT, LimitWindow, ModelOption, Project, RESULT_LIMIT, Row, SearchHit, SlashCommand, SubagentInfo, Thread, trim};

pub struct Source {
    c: Connection,
}

/// A row that changed, and its thread.
pub struct Change {
    pub thread: String,
    pub row: Row,
}

const COLUMNS: &str = "seq, rev, id, kind, text, tool_name, tool_input, tool_result, tool_result_length, tool_is_error,
                       notice_style, attachments, date, turn_ended_at, changed_files, subagent, subagent_blob";

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
        let rows = s.query_map(params![rev, limit], |r| Ok((r.get::<_, String>(0)?, row(r, 1)?)))?;
        rows.map(|r| r.map(|(thread, raw)| Change { thread, row: self.finish(raw) })).collect()
    }

    /// The models the CLI last listed (the app keeps them as `claudeModels`).
    pub fn models(&self) -> rusqlite::Result<Vec<ModelOption>> {
        let list: Value = self.setting("claudeModels")?.unwrap_or_default();
        Ok(list
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(yh_core::claude::ModelInfo::from_json)
            .map(|m| ModelOption { id: m.value, name: m.display_name, efforts: m.efforts, auto: m.supports_auto_mode })
            .collect())
    }

    /// Each project's slash commands as the CLI last listed them (`slashCommands`), the app's own
    /// first (`/btw`, `/mcp`), terminal-only ones left out.
    pub fn commands(&self) -> rusqlite::Result<std::collections::BTreeMap<String, Vec<SlashCommand>>> {
        let all = self.setting("slashCommands")?.unwrap_or_default();
        let app = [
            SlashCommand { name: "btw".into(), hint: "<question>".into(), description: "Ask a side question. Claude sees this thread; nothing is added to it.".into() },
            SlashCommand { name: "mcp".into(), hint: String::new(), description: "See your MCP servers.".into() },
        ];
        let mut out = std::collections::BTreeMap::new();
        for (project, list) in all.as_object().into_iter().flatten() {
            let mut commands: Vec<SlashCommand> = app.to_vec();
            for c in list.as_array().into_iter().flatten().filter_map(yh_core::claude::SlashCommand::from_json) {
                if ["color", "focus", "heapdump"].contains(&c.name.as_str()) || commands.iter().any(|x| x.name == c.name) {
                    continue;
                }
                commands.push(SlashCommand { name: c.name, hint: c.hint, description: c.description });
            }
            out.insert(project.clone(), commands);
        }
        Ok(out)
    }

    /// Usage limits as last saved (`rateLimit`).
    pub fn limits(&self) -> rusqlite::Result<Vec<LimitWindow>> {
        let saved = self.setting("rateLimit")?.unwrap_or_default();
        Ok(yh_core::limits::from_saved(&saved).map(|info| limit_windows(&info)).unwrap_or_default())
    }

    fn setting(&self, key: &str) -> rusqlite::Result<Option<Value>> {
        let json: Option<Vec<u8>> = self.c.query_row("SELECT value FROM settings WHERE key = ?", [key], |r| r.get(0)).optional()?;
        Ok(json.and_then(|j| serde_json::from_slice(&j).ok()))
    }

    /// Transcript rows with these words (every word, the last as a prefix), best first.
    pub fn search(&self, query: &str, limit: i64) -> rusqlite::Result<Vec<SearchHit>> {
        let Some(expression) = yh_core::db::match_expression(query) else { return Ok(vec![]) };
        let mut s = self.c.prepare_cached(
            "SELECT e.thread_id, e.seq, snippet(entries_fts, 0, '[', ']', '…', 12)
             FROM entries_fts JOIN entries e ON e.rowid = entries_fts.rowid
             WHERE entries_fts MATCH ?1 ORDER BY bm25(entries_fts) LIMIT ?2",
        )?;
        let rows = s.query_map(params![expression, limit], |r| Ok(SearchHit { thread: r.get(0)?, seq: r.get(1)?, snippet: r.get(2)? }))?;
        rows.collect()
    }

    /// Where a thread's `claude` runs: its own folder, else its project's.
    pub fn folder_of(&self, thread: &str) -> rusqlite::Result<Option<String>> {
        let found: Option<(String, Vec<u8>)> = self.c.query_row("SELECT project_id, json FROM threads WHERE id = ?", [thread], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
        let Some((project, json)) = found else { return Ok(None) };
        let exists = |p: &String| std::path::Path::new(p).is_dir();
        if let Some(own) = object(&json).get("workingDirectory").and_then(Value::as_str).map(str::to_string).filter(exists) {
            return Ok(Some(own));
        }
        let project: Option<Vec<u8>> = self.c.query_row("SELECT json FROM projects WHERE id = ?", [project], |r| r.get(0)).optional()?;
        Ok(project.and_then(|j| object(&j).get("path").and_then(Value::as_str).map(str::to_string)).filter(exists))
    }

    /// A project's folder, when it's there.
    pub fn project_folder(&self, project: &str) -> rusqlite::Result<Option<String>> {
        let json: Option<Vec<u8>> = self.c.query_row("SELECT json FROM projects WHERE id = ?", [project], |r| r.get(0)).optional()?;
        Ok(json.and_then(|j| object(&j).get("path").and_then(Value::as_str).map(str::to_string)).filter(|p| std::path::Path::new(p).is_dir()))
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
        rows.map(|r| r.map(|raw| self.finish(raw))).collect()
    }

    /// A row read, its sub-agent's run summarized (read from its blob when it's kept apart).
    fn finish(&self, (mut row, run): (Row, RawRun)) -> Row {
        let bytes = match run {
            RawRun::None => None,
            RawRun::Inline(b) => Some(b),
            RawRun::Blob(hash) => yh_core::db::blob(&self.c, &hash).ok().flatten(),
        };
        row.subagent = bytes.and_then(|b| yh_core::subagent_record::SubagentRecord::summary(&b)).map(|s| SubagentInfo {
            description: s.description,
            status: s.status,
            agent_type: s.agent_type,
            tool_uses: s.tool_uses,
            tokens: s.tokens,
            duration_ms: s.duration_ms,
            activity: None,
            background: s.is_background,
        });
        row
    }

    /// A row's sub-agent run whole, and its call's input, as saved.
    pub fn subagent_run(&self, thread: &str, seq: i64) -> rusqlite::Result<Option<(yh_core::subagent_record::SubagentRecord, Option<String>)>> {
        let found: Option<(Option<Vec<u8>>, Option<String>, Option<String>)> = self
            .c
            .query_row("SELECT subagent, subagent_blob, tool_input FROM entries WHERE thread_id = ? AND seq = ?", params![thread, seq], |r| Ok((bytes(r, 0)?, r.get(1)?, r.get(2)?)))
            .optional()?;
        let Some((inline, hash, input)) = found else { return Ok(None) };
        let bytes = match (inline, hash) {
            (Some(b), _) => Some(b),
            (None, Some(h)) => yh_core::db::blob(&self.c, &h)?,
            _ => None,
        };
        Ok(bytes.and_then(|b| yh_core::subagent_record::SubagentRecord::decode(&b)).map(|r| (r, input)))
    }

    /// A row's kind, tool, whole input and attachments: where its pictures are.
    pub fn media_of(&self, thread: &str, seq: i64) -> rusqlite::Result<Option<(String, Option<String>, Option<String>, Vec<String>)>> {
        let found: Option<(String, Option<String>, Option<String>, Option<String>, Option<String>)> = self
            .c
            .query_row("SELECT kind, tool_name, tool_input, tool_input_blob, attachments FROM entries WHERE thread_id = ? AND seq = ?", params![thread, seq], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .optional()?;
        let Some((kind, tool, input, blob, attachments)) = found else { return Ok(None) };
        let input = match blob {
            Some(h) => yh_core::db::blob(&self.c, &h)?.map(|b| String::from_utf8_lossy(&b).into_owned()).or(input),
            None => input,
        };
        let attachments = attachments.and_then(|a| serde_json::from_str(&a).ok()).unwrap_or_default();
        Ok(Some((kind, tool, input, attachments)))
    }
}

/// A row's sub-agent run as read: in the row, in a blob, or none.
enum RawRun {
    None,
    Inline(Vec<u8>),
    Blob(String),
}

fn row(r: &rusqlite::Row, at: usize) -> rusqlite::Result<(Row, RawRun)> {
    let list = |s: Option<String>| -> Vec<String> { s.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default() };
    let run = match (bytes(r, at + 15)?, r.get::<_, Option<String>>(at + 16)?) {
        (Some(b), _) => RawRun::Inline(b),
        (None, Some(h)) => RawRun::Blob(h),
        _ => RawRun::None,
    };
    let row = Row {
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
        subagent: None,
    };
    Ok((row, run))
}

/// A column's bytes, kept as text or as a blob.
fn bytes(r: &rusqlite::Row, i: usize) -> rusqlite::Result<Option<Vec<u8>>> {
    use rusqlite::types::ValueRef;
    Ok(match r.get_ref(i)? {
        ValueRef::Blob(b) => Some(b.to_vec()),
        ValueRef::Text(t) => Some(t.to_vec()),
        _ => None,
    })
}

fn object(json: &[u8]) -> Map<String, Value> {
    match serde_json::from_slice(json) {
        Ok(Value::Object(o)) => o,
        _ => Map::new(),
    }
}

/// A `rate_limit_info`'s windows as the wire says them.
pub fn limit_windows(info: &Value) -> Vec<LimitWindow> {
    yh_core::limits::windows(info).into_iter().map(|w| LimitWindow { name: w.name, used: w.utilization, resets_at: w.resets_at }).collect()
}
