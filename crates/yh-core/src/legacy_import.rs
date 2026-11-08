//! The JSON files the app kept before the database (0.0.6 and earlier): `state.json` and one
//! `threads/<id>.json` per transcript. A folder that never
//! ran 0.0.7+ comes over exactly as 0.0.7 would have brought it:
//!
//! - `state.json` goes in when the database opens, once (`legacy_imports` name `state`); a file
//!   that isn't JSON is left for a later launch to try.
//! - Transcripts go in one at a time on a background thread, from 3 s after launch, each in a
//!   transaction of its own, recorded under the thread's ID. A transcript read before its turn
//!   comes in first (`Database::transcript_count` and `read_rows`).
//! - Files are read as 0.0.x read them: ISO 8601 dates, an entry it can't
//!   decode costs that entry (`Lossy`), a file that isn't a JSON array of entries counts as
//!   empty and is recorded all the same, so it never comes back. Rows written since are newer
//!   than the file: a thread that has some keeps them.
//! - Files are only read, never written or deleted, so an older app still finds its data.

use crate::db::{Row, StateRows, ThreadRecord};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Seconds between 1970 and 2001, Foundation's reference date: `JSONEncoder`'s default dates.
const REFERENCE_DATE: f64 = 978_307_200.0;

#[derive(Clone, Debug)]
pub struct LegacyStore {
    pub root: PathBuf,
}

/// One old transcript as read.
#[derive(Debug, Default)]
pub struct LegacyTranscript {
    pub rows: Vec<Row>,
    /// Entries in the file that didn't decode (skipped, as 0.0.x skipped them).
    pub dropped: usize,
    /// Why the whole file didn't read, when it didn't: it counts as empty.
    pub unreadable: Option<String>,
    /// When the file was last written (seconds since 1970).
    pub modified: Option<f64>,
}

impl LegacyStore {
    pub fn new(root: &Path) -> LegacyStore {
        LegacyStore { root: root.to_path_buf() }
    }

    pub fn state_path(&self) -> PathBuf {
        self.root.join("state.json")
    }

    pub fn transcript_path(&self, id: &str) -> PathBuf {
        self.root.join("threads").join(format!("{id}.json"))
    }

    /// IDs of the transcripts on disk: `<UUID>.json` files.
    pub fn transcript_ids(&self) -> Vec<String> {
        let Ok(list) = std::fs::read_dir(self.root.join("threads")) else { return vec![] };
        let mut ids: Vec<String> = list
            .flatten()
            .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".json")).map(str::to_string))
            .filter(|id| is_uuid(id))
            .collect();
        ids.sort();
        ids
    }

    pub fn modified(&self, id: &str) -> Option<f64> {
        let m = std::fs::metadata(self.transcript_path(id)).ok()?.modified().ok()?;
        m.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs_f64())
    }

    /// A transcript file read the way 0.0.x read it.
    pub fn transcript(&self, id: &str) -> LegacyTranscript {
        let modified = self.modified(id);
        let data = match std::fs::read(self.transcript_path(id)) {
            Ok(d) => d,
            Err(e) => return LegacyTranscript { unreadable: Some(e.to_string()), modified, ..Default::default() },
        };
        let mut t = parse_transcript(&data);
        t.modified = modified;
        t
    }

    /// `state.json` split the way the database keeps it: each top-level setting on its own,
    /// projects and threads one row each. `Ok(None)` when there's no file; an error when it
    /// isn't a JSON object (it's kept for a later launch).
    pub fn state(&self) -> Result<Option<StateRows>, String> {
        let Ok(data) = std::fs::read(self.state_path()) else { return Ok(None) };
        let object = match serde_json::from_slice::<Value>(&data) {
            Ok(Value::Object(o)) => o,
            Ok(_) => return Err("not a JSON object".into()),
            Err(e) => return Err(e.to_string()),
        };
        let mut rows = StateRows::default();
        for (key, value) in &object {
            if key != "projects" && key != "threads" {
                rows.values.insert(key.clone(), serialization_json(value).into_bytes());
            }
        }
        // `as? [[String: Any]]`: one item that isn't an object and the whole list is none.
        let objects = |key: &str| -> Vec<&Map<String, Value>> {
            match object.get(key) {
                Some(Value::Array(items)) => items.iter().map(Value::as_object).collect::<Option<Vec<_>>>().unwrap_or_default(),
                _ => vec![],
            }
        };
        for project in objects("projects") {
            let Some(id) = project.get("id").and_then(Value::as_str) else { continue };
            rows.projects.push((id.to_string(), serialization_json(&Value::Object(project.clone())).into_bytes()));
        }
        for thread in objects("threads") {
            let (Some(id), Some(project_id)) = (thread.get("id").and_then(Value::as_str), thread.get("projectID").and_then(Value::as_str)) else {
                continue;
            };
            rows.threads.push(ThreadRecord {
                id: id.to_string(),
                project_id: project_id.to_string(),
                updated_at: thread.get("updatedAt").and_then(Value::as_str).and_then(parse_iso8601),
                session_id: thread.get("sessionID").and_then(Value::as_str).map(str::to_string),
                json: serialization_json(&Value::Object(thread.clone())).into_bytes(),
            });
        }
        Ok(Some(rows))
    }
}

/// `UUID(uuidString:)`: 8-4-4-4-12 hex digits, either case.
fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { *c == b'-' } else { c.is_ascii_hexdigit() })
}

/// A file's bytes as `[EntrySnapshot]`, each entry on its own (`[Lossy<EntrySnapshot>]`).
pub fn parse_transcript(data: &[u8]) -> LegacyTranscript {
    let items = match serde_json::from_slice::<Value>(data) {
        Ok(Value::Array(items)) => items,
        Ok(_) => return LegacyTranscript { unreadable: Some("not a JSON array".into()), ..Default::default() },
        Err(e) => return LegacyTranscript { unreadable: Some(e.to_string()), ..Default::default() },
    };
    let mut t = LegacyTranscript::default();
    for item in &items {
        match Snapshot::decode(item) {
            Some(s) => t.rows.push(s.into_row()),
            None => t.dropped += 1,
        }
    }
    t
}

// MARK: - Decoding, by the rules 0.0.x's decoder followed

/// An entry of the file (`EntrySnapshot`), as 0.0.x read it.
#[derive(Clone, Debug, PartialEq)]
struct Snapshot {
    id: String,
    kind: String,
    text: String,
    tool_name: Option<String>,
    tool_input: Option<String>,
    tool_result: Option<String>,
    tool_is_error: Option<bool>,
    notice_style: Option<String>,
    attachments: Option<Vec<String>>,
    subagent: Option<Subagent>,
    delivery: Option<String>,
    date: Option<f64>,
    turn_ended_at: Option<f64>,
    tool_result_ref: Option<String>,
    tool_result_length: Option<i64>,
    changed_files: Option<Vec<String>>,
}

/// `SubagentSnapshot`.
#[derive(Clone, Debug, PartialEq)]
struct Subagent {
    description: String,
    agent_type: Option<String>,
    prompt: String,
    is_background: Option<bool>,
    status: String,
    tool_uses: Option<i64>,
    tokens: Option<i64>,
    duration_ms: Option<i64>,
    report: Option<String>,
    started_at: f64,
    entries: Vec<Snapshot>,
}

const KINDS: &[&str] = &["user", "assistant", "thinking", "tool", "notice", "commandOutput"];
const NOTICE_STYLES: &[&str] = &["info", "error"];
const DELIVERIES: &[&str] = &["normal", "steered", "interrupted"];
const STATUSES: &[&str] = &["running", "completed", "failed", "stopped"];

/// `decodeIfPresent`: absent or null is `None`, the wrong type fails the whole value.
type Field<T> = Result<Option<T>, ()>;

fn present<'a>(o: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    o.get(key).filter(|v| !v.is_null())
}

fn opt_string(o: &Map<String, Value>, key: &str) -> Field<String> {
    present(o, key).map(|v| v.as_str().map(str::to_string).ok_or(())).transpose()
}

fn opt_enum(o: &Map<String, Value>, key: &str, cases: &[&str]) -> Field<String> {
    opt_string(o, key).and_then(|v| match v {
        Some(s) if !cases.contains(&s.as_str()) => Err(()),
        v => Ok(v),
    })
}

fn opt_bool(o: &Map<String, Value>, key: &str) -> Field<bool> {
    present(o, key).map(|v| v.as_bool().ok_or(())).transpose()
}

/// `Int` from a JSON number: whole, and in range.
fn opt_int(o: &Map<String, Value>, key: &str) -> Field<i64> {
    present(o, key)
        .map(|v| match v {
            Value::Number(n) => n.as_i64().or_else(|| n.as_f64().filter(|f| f.fract() == 0.0 && f.abs() < 9.2e18).map(|f| f as i64)).ok_or(()),
            _ => Err(()),
        })
        .transpose()
}

fn opt_strings(o: &Map<String, Value>, key: &str) -> Field<Vec<String>> {
    present(o, key)
        .map(|v| match v {
            Value::Array(items) => items.iter().map(|i| i.as_str().map(str::to_string)).collect::<Option<Vec<_>>>().ok_or(()),
            _ => Err(()),
        })
        .transpose()
}

/// `.iso8601`: ISO8601DateFormatter's internet date time, no fractions.
fn opt_date(o: &Map<String, Value>, key: &str) -> Field<f64> {
    present(o, key).map(|v| v.as_str().and_then(parse_iso8601).ok_or(())).transpose()
}

fn required_string(o: &Map<String, Value>, key: &str) -> Result<String, ()> {
    opt_string(o, key)?.ok_or(())
}

impl Snapshot {
    fn decode(v: &Value) -> Option<Snapshot> {
        Self::decode_object(v.as_object()?).ok()
    }

    fn decode_object(o: &Map<String, Value>) -> Result<Snapshot, ()> {
        Ok(Snapshot {
            id: required_string(o, "id")?,
            kind: opt_enum(o, "kind", KINDS)?.ok_or(())?,
            text: required_string(o, "text")?,
            tool_name: opt_string(o, "toolName")?,
            tool_input: opt_string(o, "toolInput")?,
            tool_result: opt_string(o, "toolResult")?,
            tool_is_error: opt_bool(o, "toolIsError")?,
            notice_style: opt_enum(o, "noticeStyle", NOTICE_STYLES)?,
            attachments: opt_strings(o, "attachments")?,
            subagent: present(o, "subagent").map(|v| v.as_object().ok_or(()).and_then(Subagent::decode)).transpose()?,
            delivery: opt_enum(o, "delivery", DELIVERIES)?,
            date: opt_date(o, "date")?,
            turn_ended_at: opt_date(o, "turnEndedAt")?,
            tool_result_ref: opt_string(o, "toolResultRef")?,
            tool_result_length: opt_int(o, "toolResultLength")?,
            changed_files: opt_strings(o, "changedFiles")?,
        })
    }

    /// As `Writer.upsert` binds it; big text goes to blobs when the row is written.
    fn into_row(self) -> Row {
        Row {
            subagent: self.subagent.as_ref().map(|s| s.encoded().into_bytes()),
            attachments: self.attachments.as_ref().map(|a| encoder_strings(a)),
            changed_files: self.changed_files.as_ref().map(|a| encoder_strings(a)),
            id: self.id,
            kind: self.kind,
            text: self.text,
            tool_name: self.tool_name,
            tool_input: self.tool_input,
            tool_result: self.tool_result,
            tool_result_ref: self.tool_result_ref,
            tool_result_length: self.tool_result_length,
            tool_is_error: self.tool_is_error,
            notice_style: self.notice_style,
            delivery: self.delivery,
            date: self.date,
            turn_ended_at: self.turn_ended_at,
        }
    }

    /// `JSONEncoder()` (no options): keys in declaration order, nils left out, dates as
    /// seconds since 2001, slashes escaped.
    fn encode(&self, out: &mut String) {
        let mut w = ObjectWriter::new(out);
        w.string("id", &self.id);
        w.string("kind", &self.kind);
        w.string("text", &self.text);
        w.opt_string("toolName", &self.tool_name);
        w.opt_string("toolInput", &self.tool_input);
        w.opt_string("toolResult", &self.tool_result);
        w.opt_bool("toolIsError", self.tool_is_error);
        w.opt_string("noticeStyle", &self.notice_style);
        w.opt_strings("attachments", &self.attachments);
        if let Some(s) = &self.subagent {
            let encoded = s.encoded();
            w.raw("subagent", &encoded);
        }
        w.opt_string("delivery", &self.delivery);
        w.opt_date("date", self.date);
        w.opt_date("turnEndedAt", self.turn_ended_at);
        w.opt_string("toolResultRef", &self.tool_result_ref);
        w.opt_int("toolResultLength", self.tool_result_length);
        w.opt_strings("changedFiles", &self.changed_files);
        w.end();
    }
}

impl Subagent {
    fn decode(o: &Map<String, Value>) -> Result<Subagent, ()> {
        Ok(Subagent {
            description: required_string(o, "description")?,
            agent_type: opt_string(o, "agentType")?,
            prompt: required_string(o, "prompt")?,
            is_background: opt_bool(o, "isBackground")?,
            status: opt_enum(o, "status", STATUSES)?.ok_or(())?,
            tool_uses: opt_int(o, "toolUses")?,
            tokens: opt_int(o, "tokens")?,
            duration_ms: opt_int(o, "durationMs")?,
            report: opt_string(o, "report")?,
            started_at: opt_date(o, "startedAt")?.ok_or(())?,
            // Not lossy: one entry that doesn't decode fails the run, and so its row.
            entries: match present(o, "entries") {
                Some(Value::Array(items)) => {
                    items.iter().map(|i| i.as_object().ok_or(()).and_then(Snapshot::decode_object)).collect::<Result<_, ()>>()?
                }
                _ => return Err(()),
            },
        })
    }

    /// The `subagent` column: the run as `JSONEncoder()` writes it.
    fn encoded(&self) -> String {
        let mut out = String::new();
        let mut w = ObjectWriter::new(&mut out);
        w.string("description", &self.description);
        w.opt_string("agentType", &self.agent_type);
        w.string("prompt", &self.prompt);
        w.opt_bool("isBackground", self.is_background);
        w.string("status", &self.status);
        w.opt_int("toolUses", self.tool_uses);
        w.opt_int("tokens", self.tokens);
        w.opt_int("durationMs", self.duration_ms);
        w.opt_string("report", &self.report);
        w.opt_date("startedAt", Some(self.started_at));
        let mut entries = String::from("[");
        for (i, e) in self.entries.iter().enumerate() {
            if i > 0 {
                entries.push(',');
            }
            e.encode(&mut entries);
        }
        entries.push(']');
        w.raw("entries", &entries);
        w.end();
        out
    }
}

// MARK: - Encoding, as Foundation writes JSON

/// A JSON string as `JSONEncoder` writes it: slashes escaped.
fn encoder_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default().replace('/', "\\/")
}

/// `[String]` through `JSONEncoder()`: the `attachments` and `changed_files` columns.
fn encoder_strings(items: &[String]) -> String {
    format!("[{}]", items.iter().map(|s| encoder_string(s)).collect::<Vec<_>>().join(","))
}

/// A number as Foundation writes it: whole ones without a fraction.
fn number(f: f64) -> String {
    if f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{}", f as i64)
    } else {
        format!("{f}")
    }
}

struct ObjectWriter<'a> {
    out: &'a mut String,
    first: bool,
}

impl<'a> ObjectWriter<'a> {
    fn new(out: &'a mut String) -> Self {
        out.push('{');
        ObjectWriter { out, first: true }
    }

    fn raw(&mut self, key: &str, value: &str) {
        if !self.first {
            self.out.push(',');
        }
        self.first = false;
        self.out.push_str(&encoder_string(key));
        self.out.push(':');
        self.out.push_str(value);
    }

    fn string(&mut self, key: &str, v: &str) {
        self.raw(key, &encoder_string(v));
    }

    fn opt_string(&mut self, key: &str, v: &Option<String>) {
        if let Some(v) = v {
            self.string(key, v);
        }
    }

    fn opt_bool(&mut self, key: &str, v: Option<bool>) {
        if let Some(v) = v {
            self.raw(key, if v { "true" } else { "false" });
        }
    }

    fn opt_int(&mut self, key: &str, v: Option<i64>) {
        if let Some(v) = v {
            self.raw(key, &v.to_string());
        }
    }

    fn opt_date(&mut self, key: &str, v: Option<f64>) {
        if let Some(v) = v {
            self.raw(key, &number(v - REFERENCE_DATE));
        }
    }

    fn opt_strings(&mut self, key: &str, v: &Option<Vec<String>>) {
        if let Some(v) = v {
            self.raw(key, &encoder_strings(v));
        }
    }

    fn end(self) {
        self.out.push('}');
    }
}

/// `JSONSerialization.data(withJSONObject:options: [.fragmentsAllowed, .sortedKeys,
/// .withoutEscapingSlashes])`: how settings, projects and threads were moved in.
pub fn serialization_json(v: &Value) -> String {
    let mut out = String::new();
    write_sorted(v, &mut out);
    out
}

fn write_sorted(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => match (n.as_i64(), n.as_u64(), n.as_f64()) {
            (Some(i), ..) => out.push_str(&i.to_string()),
            (_, Some(u), _) => out.push_str(&u.to_string()),
            (.., Some(f)) => out.push_str(&number(f)),
            _ => out.push_str(&n.to_string()),
        },
        Value::String(s) => out.push_str(&serde_json::to_string(s).unwrap_or_default()),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_sorted(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).unwrap_or_default());
                out.push(':');
                write_sorted(&map[k.as_str()], out);
            }
            out.push('}');
        }
    }
}

/// Seconds since 1970 from an internet date time (`2026-10-20T10:00:00Z`, or `+02:00`), as
/// `.iso8601` reads it: no fractions, a zone required.
pub fn parse_iso8601(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    if b.len() < 20 {
        return None;
    }
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    let shape = digits(0..4) && b[4] == b'-' && digits(5..7) && b[7] == b'-' && digits(8..10) && b[10] == b'T'
        && digits(11..13) && b[13] == b':' && digits(14..16) && b[16] == b':' && digits(17..19);
    if !shape {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s[r].parse::<u32>().unwrap_or(99);
    let (month, day, hour, minute, second) = (n(5..7), n(8..10), n(11..13), n(14..16), n(17..19));
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let zone = &s[19..];
    let zone_ok = zone == "Z"
        || (zone.len() == 6 && matches!(zone.as_bytes()[0], b'+' | b'-') && zone.as_bytes()[3] == b':'
            && zone.as_bytes()[1..3].iter().chain(&zone.as_bytes()[4..6]).all(u8::is_ascii_digit))
        || (zone.len() == 5 && matches!(zone.as_bytes()[0], b'+' | b'-') && zone.as_bytes()[1..].iter().all(u8::is_ascii_digit));
    if !zone_ok {
        return None;
    }
    let normalized = if zone.len() == 5 { format!("{}{}:{}", &s[..19], &zone[..3], &zone[3..]) } else { s.to_string() };
    crate::library::parse_iso(&normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_read_as_the_swift_decoder_reads_them() {
        assert_eq!(parse_iso8601("2026-10-20T10:00:00Z"), Some(1_792_490_400.0));
        assert_eq!(parse_iso8601("2026-10-20T12:00:00+02:00"), Some(1_792_490_400.0));
        assert_eq!(parse_iso8601("2026-10-20T12:00:00+0200"), Some(1_792_490_400.0));
        // `.iso8601` takes no fractions and needs a zone.
        assert_eq!(parse_iso8601("2026-10-20T10:00:00.5Z"), None);
        assert_eq!(parse_iso8601("2026-10-20T10:00:00"), None);
        assert_eq!(parse_iso8601("2026-13-20T10:00:00Z"), None);
        assert_eq!(parse_iso8601("yesterday"), None);
    }

    #[test]
    fn entries_decode_strictly_and_one_bad_entry_costs_only_itself() {
        let file = br#"[
            {"id": "u1", "kind": "user", "text": "hi", "date": "2026-10-20T10:00:00Z", "attachments": ["a/b.png"], "unknownField": 3},
            {"id": "x", "kind": "fromTheFuture", "text": ""},
            {"id": "y", "kind": "user"},
            {"id": "z", "kind": "tool", "text": "", "toolIsError": "yes"},
            {"id": "w", "kind": "notice", "text": "Oops", "noticeStyle": "error", "toolName": null},
            "not an entry",
            {"id": "d", "kind": "user", "text": "", "date": "2026-10-20T10:00:00.250Z"}
        ]"#;
        let t = parse_transcript(file);
        assert_eq!(t.rows.len(), 2);
        assert_eq!(t.dropped, 5);
        assert!(t.unreadable.is_none());
        assert_eq!(t.rows[0].attachments.as_deref(), Some(r#"["a\/b.png"]"#));
        assert_eq!(t.rows[0].date, Some(1_792_490_400.0));
        assert_eq!(t.rows[1].notice_style.as_deref(), Some("error"));
        assert_eq!(t.rows[1].tool_name, None);
    }

    #[test]
    fn a_file_that_isnt_an_array_reads_as_empty() {
        for bad in [&b"{\"id\": 1}"[..], b"[{\"id\": \"u1\", \"kind\": \"user\", \"te", b"", b"\xff\xfe"] {
            let t = parse_transcript(bad);
            assert!(t.rows.is_empty());
            assert!(t.unreadable.is_some());
        }
    }

    #[test]
    fn a_subagent_run_is_written_as_json_encoder_writes_it() {
        let file = br#"[{"id": "toolu_1", "kind": "tool", "text": "", "toolName": "Task",
            "subagent": {"description": "Look", "prompt": "Find it", "status": "completed", "toolUses": 2,
                "startedAt": "2026-10-20T10:00:00Z", "report": "Found in a/b",
                "entries": [{"id": "s1", "kind": "assistant", "text": "ok", "date": "2026-10-20T10:00:05Z"}]}}]"#;
        let t = parse_transcript(file);
        let json = String::from_utf8(t.rows[0].subagent.clone().unwrap()).unwrap();
        assert_eq!(
            json,
            r#"{"description":"Look","prompt":"Find it","status":"completed","toolUses":2,"report":"Found in a\/b","startedAt":814183200,"entries":[{"id":"s1","kind":"assistant","text":"ok","date":814183205}]}"#
        );
        // One bad entry inside the run and the row goes (the run isn't lossy).
        let bad = br#"[{"id": "t", "kind": "tool", "text": "", "subagent": {"description": "", "prompt": "", "status": "completed",
            "startedAt": "2026-10-20T10:00:00Z", "entries": [{"id": "s1", "kind": "nope", "text": ""}]}}]"#;
        assert_eq!(parse_transcript(bad).rows.len(), 0);
    }

    use crate::db::Database;
    use rusqlite::Connection;

    const T1: &str = "BBBBBBBB-0000-4000-8000-000000000001";
    const T2: &str = "BBBBBBBB-0000-4000-8000-000000000002";
    const T3: &str = "BBBBBBBB-0000-4000-8000-000000000003";
    const T4: &str = "BBBBBBBB-0000-4000-8000-000000000004";

    /// A data folder as 0.0.6 left it: the `legacy-0.0.6` preset's files, `{{ROOT}}` filled in,
    /// with thread 3's transcript cut off mid-write, a transcript of a thread that's gone and a
    /// file that isn't a transcript.
    fn legacy_folder(name: &str) -> PathBuf {
        let preset = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../presets/legacy-0.0.6");
        let dir = std::env::temp_dir().join(format!("yh-legacy-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("threads")).unwrap();
        let root = dir.to_string_lossy().to_string();
        let state = std::fs::read_to_string(preset.join("state.json")).unwrap().replace("{{ROOT}}", &root);
        std::fs::write(dir.join("state.json"), state).unwrap();
        for id in [T1, T2, T4] {
            let file = format!("{id}.json");
            std::fs::copy(preset.join("threads").join(&file), dir.join("threads").join(&file)).unwrap();
        }
        let t3 = std::fs::read(preset.join("threads").join(format!("{T3}.json"))).unwrap();
        std::fs::write(dir.join("threads").join(format!("{T3}.json")), &t3[..t3.len() / 2]).unwrap();
        std::fs::write(dir.join("threads").join("CCCCCCCC-0000-4000-8000-000000000009.json"), "[]").unwrap();
        std::fs::write(dir.join("threads").join("notes.json"), "[]").unwrap();
        dir
    }

    fn table(db: &Database, sql: &str) -> Vec<Vec<String>> {
        let c = Connection::open(db.path()).unwrap();
        let mut s = c.prepare(sql).unwrap();
        let n = s.column_count();
        s.query_map([], |r| {
            Ok((0..n).map(|i| format!("{:?}", r.get_ref(i).unwrap())).collect::<Vec<_>>())
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
    }

    fn imported(db: &Database) -> Vec<String> {
        let c = Connection::open(db.path()).unwrap();
        let mut s = c.prepare("SELECT name FROM legacy_imports ORDER BY name").unwrap();
        s.query_map([], |r| r.get::<_, String>(0)).unwrap().map(Result::unwrap).collect()
    }

    #[test]
    fn a_0_0_6_folder_moves_in_once_and_a_cut_off_file_is_recorded() {
        let dir = legacy_folder("whole");
        let db = Database::open_with(&dir, None).unwrap();
        // The state at once: projects, threads, settings.
        let state = db.load_state();
        assert_eq!(state.projects.len(), 2);
        assert_eq!(state.threads.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), vec![T1, T2, T3, T4]);
        assert_eq!(state.threads[0].updated_at, parse_iso8601("2026-10-22T09:00:00Z"));
        assert_eq!(state.values["defaults"], br#"{"effort":"low","model":"haiku"}"#.to_vec());
        // Only threads that exist, by UUID file name.
        assert_eq!(db.legacy_pending(), 4);

        // Read before its turn: it comes in first.
        assert_eq!(db.transcript_count(T1), 7);
        assert_eq!(db.legacy_pending(), 3);
        let rows = db.read_rows(T1, 0, 100);
        assert_eq!(rows[0].kind, "user");
        assert_eq!(rows[0].date, parse_iso8601("2026-10-20T10:00:00Z"));
        assert_eq!(rows[0].turn_ended_at, parse_iso8601("2026-10-22T08:05:00Z"));
        // The big result went to a blob, its head in the row, as 0.0.x keeps it.
        let big = rows.iter().find(|r| r.tool_result_ref.is_some()).expect("a big result");
        assert_eq!(big.tool_result.as_ref().unwrap().chars().count(), crate::db::RESULT_HEAD);
        assert!(db.blob_string(big.tool_result_ref.as_ref().unwrap()).unwrap().len() > crate::db::BLOB_THRESHOLD);

        db.import_all_legacy();
        assert_eq!(db.legacy_pending(), 0);
        assert_eq!(db.transcript_count(T2), 5);
        let t4 = db.read_rows(T4, 0, 10);
        assert_eq!(t4.len(), 3);
        assert_eq!(t4[0].attachments.as_deref(), Some(r#"["BBBBBBBB-0000-4000-8000-000000000004-11111111-0000-4000-8000-000000000001.png"]"#));
        assert_eq!(t4[1].notice_style.as_deref(), Some("error"));
        // Cut off: empty, recorded, never tried again.
        assert_eq!(db.transcript_count(T3), 0);
        assert_eq!(imported(&db), vec![T1, T2, T3, T4, "state"]);
        assert_eq!(table(&db, &format!("SELECT entry_count FROM transcripts WHERE thread_id = '{T3}'")), vec![vec!["Integer(0)".to_string()]]);
        // saved_at is when the file was written.
        let saved: f64 = Connection::open(db.path())
            .unwrap()
            .query_row("SELECT saved_at FROM transcripts WHERE thread_id = ?", [T1], |r| r.get(0))
            .unwrap();
        assert!((saved - LegacyStore::new(&dir).modified(T1).unwrap()).abs() < 1.0);
        db.flush();
        let entries = table(&db, "SELECT * FROM entries ORDER BY thread_id, seq");
        drop(db);

        // Again: nothing moves in twice, even with the files changed meanwhile.
        std::fs::write(dir.join("state.json"), r#"{"projects": [], "threads": []}"#).unwrap();
        std::fs::write(dir.join("threads").join(format!("{T3}.json")), "[]").unwrap();
        let db = Database::open_with(&dir, Some(std::time::Duration::ZERO)).unwrap();
        assert_eq!(db.legacy_pending(), 0);
        assert_eq!(db.load_state().threads.len(), 4);
        assert_eq!(table(&db, "SELECT * FROM entries ORDER BY thread_id, seq"), entries);
        // The files are left as they were.
        assert!(dir.join("threads").join(format!("{T1}.json")).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn transcripts_move_in_on_their_own_in_the_background() {
        let dir = legacy_folder("background");
        let db = Database::open_with(&dir, Some(std::time::Duration::from_millis(50))).unwrap();
        assert_eq!(db.legacy_pending(), 4);
        for _ in 0..200 {
            if db.legacy_pending() == 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(db.legacy_pending(), 0);
        assert_eq!(imported(&db).len(), 5);
        let c = Connection::open(db.path()).unwrap();
        let n: i64 = c.query_row("SELECT count(*) FROM entries", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 7 + 5 + 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_rows_and_deletes_are_never_undone_by_an_old_file() {
        let dir = legacy_folder("writes");
        let db = Database::open_with(&dir, None).unwrap();
        // A message sent in an old thread before its file moved in: after the old rows.
        let n = db.transcript_count(T2);
        assert_eq!(n, 5);
        let new = Row { id: "u-new".into(), kind: "user".into(), text: "again".into(), ..Default::default() };
        db.save_rows(T2, n + 1, vec![(n, new.clone())]);
        assert_eq!(db.read_rows(T2, 0, 10).len(), 6);
        assert_eq!(db.read_rows(T2, 5, 1)[0], new);
        // Deleted before its turn: recorded, so an undone delete doesn't bring the file's rows back.
        db.delete_transcript(T4);
        db.flush();
        assert!(imported(&db).contains(&T4.to_string()));
        db.import_all_legacy();
        assert_eq!(db.transcript_count(T4), 0);
        // Rows written straight to a pending thread (no read first) still come after the file's.
        drop(db);
        let dir2 = legacy_folder("writes2");
        let db = Database::open_with(&dir2, None).unwrap();
        db.save_rows(T1, 8, vec![(7, new.clone())]);
        assert_eq!(db.transcript_count(T1), 8);
        assert_eq!(db.read_rows(T1, 0, 1)[0].id, "u1");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[test]
    fn state_splits_as_the_swift_app_split_it() {
        let dir = std::env::temp_dir().join(format!("yh-legacy-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("state.json"),
            r#"{"projects": [{"path": "/a/b", "id": "P1", "name": "B"}, {"name": "no id"}],
                "threads": [{"id": "T1", "projectID": "P1", "updatedAt": "2026-10-20T10:00:00Z", "sessionID": "s", "title": "x"},
                            {"id": "T2", "title": "no project"}],
                "defaults": {"model": "haiku", "effort": "low"}, "autoSettleDays": 0, "ratio": 1.5, "selectedThreadID": "T1"}"#,
        )
        .unwrap();
        let s = LegacyStore::new(&dir).state().unwrap().unwrap();
        assert_eq!(s.projects, vec![("P1".to_string(), br#"{"id":"P1","name":"B","path":"/a/b"}"#.to_vec())]);
        assert_eq!(s.threads.len(), 1);
        assert_eq!(s.threads[0].updated_at, Some(1_792_490_400.0));
        assert_eq!(s.threads[0].session_id.as_deref(), Some("s"));
        assert_eq!(s.values["defaults"], br#"{"effort":"low","model":"haiku"}"#.to_vec());
        assert_eq!(s.values["autoSettleDays"], b"0".to_vec());
        assert_eq!(s.values["ratio"], b"1.5".to_vec());
        assert_eq!(s.values["selectedThreadID"], br#""T1""#.to_vec());
        std::fs::write(dir.join("state.json"), "not json").unwrap();
        assert!(LegacyStore::new(&dir).state().is_err());
        std::fs::remove_file(dir.join("state.json")).unwrap();
        assert_eq!(LegacyStore::new(&dir).state().unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
