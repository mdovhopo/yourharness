//! What Antigravity spent, from its own records: each conversation is a SQLite file of its own
//! (`~/.gemini/antigravity-cli/conversations/<id>.db`, the IDE's under `antigravity/`), whose
//! `gen_metadata` rows are its model calls, protobuf with no published schema. Worked out
//! against turns whose result said what they used (1.5.14):
//!
//! - a call's field 1.19 is its model (`gemini-4-neon`), 1.4 its usage: 2 input (not from the
//!   cache), 3 output, 5 read from the cache. Added up, these are the ACP result's
//!   `inputTokens`, `outputTokens` and `cachedReadTokens`.
//! - calls have no time; the n-th call is the n-th step of type 15 (the agent's response),
//!   whose metadata's field 1 is when it started (seconds, nanos).
//!
//! A call still under way has no usage yet and is skipped. Anything that doesn't read as above
//! is skipped too: a format that changed counts nothing rather than something wrong.

use crate::usage::Tokens;
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

/// One model call of a conversation.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    /// Its place among the conversation's calls.
    pub index: i64,
    /// Seconds since 1970.
    pub at: f64,
    pub model: String,
    pub tokens: Tokens,
}

/// Where Antigravity keeps conversations: its command line's, then the IDE's.
/// `YH_AGY_CONVERSATIONS=<folder>` reads that one instead (test runs: synthetic files).
pub fn conversation_folders() -> Vec<PathBuf> {
    if let Ok(dir) = std::env::var("YH_AGY_CONVERSATIONS") {
        return vec![PathBuf::from(dir)];
    }
    let home = std::env::var("HOME").unwrap_or_default();
    ["antigravity-cli", "antigravity"].iter().map(|d| Path::new(&home).join(".gemini").join(d).join("conversations")).collect()
}

/// Every conversation file: (path, its session ID, size with its log, last change).
pub fn conversation_files(folders: &[PathBuf]) -> Vec<(PathBuf, String, i64, f64)> {
    let mut files = vec![];
    for folder in folders {
        let Ok(entries) = std::fs::read_dir(folder) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().is_none_or(|x| x != "db") {
                continue;
            }
            let Some(session) = path.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
            let stat = |p: &Path| std::fs::metadata(p).ok().map(|m| (m.len() as i64, m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0., |d| d.as_secs_f64())));
            let Some((size, modified)) = stat(&path) else { continue };
            let (wal_size, wal_modified) = stat(&PathBuf::from(format!("{}-wal", path.display()))).unwrap_or((0, 0.));
            files.push((path, session, size + wal_size, modified.max(wal_modified)));
        }
    }
    files
}

/// A conversation's finished calls, oldest first; none if it can't be read.
pub fn read_conversation(path: &Path) -> Vec<Call> {
    let Ok(c) = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX) else { return vec![] };
    let _ = c.busy_timeout(std::time::Duration::from_secs(2));
    let times: Vec<Option<f64>> = c
        .prepare("SELECT metadata FROM steps WHERE step_type = 15 ORDER BY idx")
        .and_then(|mut s| s.query_map([], |r| r.get::<_, Option<Vec<u8>>>(0)).map(|rows| rows.flatten().map(|m| m.as_deref().and_then(step_time)).collect()))
        .unwrap_or_default();
    let fallback = std::fs::metadata(path).ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0., |d| d.as_secs_f64());
    let Ok(mut s) = c.prepare("SELECT idx, data FROM gen_metadata ORDER BY idx") else { return vec![] };
    let Ok(rows) = s.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<Vec<u8>>>(1)?))) else { return vec![] };
    let mut calls = vec![];
    let mut last_at = None;
    for (n, (index, data)) in rows.flatten().enumerate() {
        // A call's time is its step's; one without, the call before's (or the file's).
        let at = times.get(n).copied().flatten().or(last_at).unwrap_or(fallback);
        last_at = Some(at);
        if let Some((model, tokens)) = data.as_deref().and_then(call_usage) {
            calls.push(Call { index, at, model, tokens });
        }
    }
    calls
}

/// A `gen_metadata` row's model and usage, if it has them.
pub fn call_usage(data: &[u8]) -> Option<(String, Tokens)> {
    let call = field(data, 1)?.bytes()?;
    let model = std::str::from_utf8(field(call, 19)?.bytes()?).ok()?.trim().to_string();
    let usage = field(call, 4)?.bytes()?;
    let n = |f: u64| field(usage, f).and_then(|v| v.number()).unwrap_or(0) as i64;
    let tokens = Tokens { input: n(2), output: n(3), cache_read: n(5), cache_5m: 0, cache_1h: 0 };
    (!model.is_empty() && tokens.total() > 0).then_some((model, tokens))
}

/// A step's start, from its metadata's field 1 (seconds, nanos).
fn step_time(metadata: &[u8]) -> Option<f64> {
    let t = field(metadata, 1)?.bytes()?;
    let seconds = field(t, 1)?.number()? as f64;
    let nanos = field(t, 2).and_then(|v| v.number()).unwrap_or(0) as f64;
    (seconds > 1e9).then_some(seconds + nanos / 1e9)
}

// MARK: Protobuf, as little as it takes

enum Value<'a> {
    Number(u64),
    Bytes(&'a [u8]),
    Other,
}

impl<'a> Value<'a> {
    fn number(&self) -> Option<u64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }
    fn bytes(&self) -> Option<&'a [u8]> {
        match self {
            Value::Bytes(b) => Some(b),
            _ => None,
        }
    }
}

fn varint(b: &[u8], i: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let c = *b.get(*i)?;
        *i += 1;
        v |= ((c & 0x7f) as u64) << shift;
        if c < 0x80 {
            return Some(v);
        }
    }
    None
}

/// A message's first field `number`; none if it isn't there or the message doesn't read.
fn field(b: &[u8], number: u64) -> Option<Value<'_>> {
    let mut i = 0;
    while i < b.len() {
        let key = varint(b, &mut i)?;
        let (f, wire) = (key >> 3, key & 7);
        let value = match wire {
            0 => Value::Number(varint(b, &mut i)?),
            1 => {
                i = i.checked_add(8).filter(|e| *e <= b.len())?;
                Value::Other
            }
            2 => {
                let len = varint(b, &mut i)? as usize;
                let end = i.checked_add(len).filter(|e| *e <= b.len())?;
                let v = Value::Bytes(&b[i..end]);
                i = end;
                v
            }
            5 => {
                i = i.checked_add(4).filter(|e| *e <= b.len())?;
                Value::Other
            }
            _ => return None,
        };
        if f == number {
            return Some(value);
        }
    }
    None
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn key(f: u64, wire: u64, out: &mut Vec<u8>) {
        number((f << 3) | wire, out);
    }
    fn number(mut v: u64, out: &mut Vec<u8>) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }
    fn varint_field(f: u64, v: u64, out: &mut Vec<u8>) {
        key(f, 0, out);
        number(v, out);
    }
    fn bytes_field(f: u64, b: &[u8], out: &mut Vec<u8>) {
        key(f, 2, out);
        number(b.len() as u64, out);
        out.extend_from_slice(b);
    }

    /// A `gen_metadata` row as Antigravity 1.5.14 writes one (the fields read, plus some it has).
    pub fn call_row(model: &str, input: u64, output: u64, cached: u64) -> Vec<u8> {
        let mut usage = vec![];
        varint_field(1, 1532, &mut usage);
        varint_field(2, input, &mut usage);
        varint_field(3, output, &mut usage);
        if cached > 0 {
            varint_field(5, cached, &mut usage);
        }
        varint_field(6, 24, &mut usage);
        varint_field(9, 17, &mut usage);
        let mut call = vec![];
        bytes_field(4, &usage, &mut call);
        key(6, 1, &mut call);
        call.extend_from_slice(&[0; 8]);
        bytes_field(17, &[18, 2, 16, 3], &mut call);
        bytes_field(19, model.as_bytes(), &mut call);
        bytes_field(21, b"Gemini 4 Neon (High)", &mut call);
        let mut row = vec![];
        bytes_field(1, &call, &mut row);
        row
    }

    pub fn step_metadata(seconds: u64) -> Vec<u8> {
        let mut t = vec![];
        varint_field(1, seconds, &mut t);
        varint_field(2, 500_000_000, &mut t);
        let mut m = vec![];
        bytes_field(1, &t, &mut m);
        varint_field(3, 2, &mut m);
        m
    }

    /// A conversation file with these calls, each after a response step, one of them unfinished.
    pub fn conversation(path: &Path, calls: &[(&str, u64, u64, u64)], start: u64) {
        let c = Connection::open(path).unwrap();
        c.execute_batch("CREATE TABLE steps (idx integer PRIMARY KEY, step_type integer NOT NULL DEFAULT 0, metadata blob); CREATE TABLE gen_metadata (idx integer PRIMARY KEY, data blob);").unwrap();
        let mut step = 0;
        c.execute("INSERT INTO steps VALUES (?, 14, ?)", rusqlite::params![step, step_metadata(start)]).unwrap();
        for (i, (model, input, output, cached)) in calls.iter().enumerate() {
            step += 1;
            c.execute("INSERT INTO steps VALUES (?, 15, ?)", rusqlite::params![step, step_metadata(start + 10 * i as u64)]).unwrap();
            step += 1;
            c.execute("INSERT INTO steps VALUES (?, 132, NULL)", rusqlite::params![step]).unwrap();
            c.execute("INSERT INTO gen_metadata VALUES (?, ?)", rusqlite::params![i as i64, call_row(model, *input, *output, *cached)]).unwrap();
        }
        // The call under way when it was read: no usage yet.
        let mut unfinished = vec![];
        bytes_field(1, &[8, 0xfc, 0x0b], &mut unfinished);
        c.execute("INSERT INTO gen_metadata VALUES (?, ?)", rusqlite::params![calls.len() as i64, unfinished]).unwrap();
    }

    #[test]
    fn reads_calls_as_the_turns_result_counted_them() {
        let dir = std::env::temp_dir().join(format!("yh-agy-usage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("5445fc20-0e90-46ad-a39b-29c7959a2c17.db");
        let _ = std::fs::remove_file(&path);
        // The five calls of a real turn (1.5.14) whose result said 33697 in, 757 out, 90544 cached.
        let turn = [("gemini-4-neon", 24041, 327, 0), ("gemini-4-neon", 2477, 174, 21984), ("gemini-4-neon", 2491, 181, 22412), ("gemini-4-neon", 2489, 56, 22854), ("gemini-4-neon", 2199, 19, 23294)];
        conversation(&path, &turn, 1_795_288_968);
        let calls = read_conversation(&path);
        assert_eq!(calls.len(), 5);
        let sum = |f: fn(&Tokens) -> i64| calls.iter().map(|c| f(&c.tokens)).sum::<i64>();
        assert_eq!((sum(|t| t.input), sum(|t| t.output), sum(|t| t.cache_read)), (33697, 757, 90544));
        assert_eq!(calls[1].at, 1_795_288_978.5);
        assert_eq!(calls[0].model, "gemini-4-neon");
        let files = conversation_files(&[dir.clone()]);
        assert_eq!(files.iter().map(|f| f.1.as_str()).collect::<Vec<_>>(), ["5445fc20-0e90-46ad-a39b-29c7959a2c17"]);
        // What doesn't read as a call counts nothing.
        assert!(call_usage(b"\x0a\x05garbage").is_none());
        assert!(call_usage(&[0xff, 0xff]).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
