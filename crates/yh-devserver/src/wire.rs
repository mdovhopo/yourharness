//! The messages the phone app and the dev server trade: JSON text frames tagged with `type`
//! (docs/mobile.md, "The wire"). Names follow what docs/remote.md plans for `yh-proto`, so the
//! app moves to `yh-server` without changing. Unknown messages and fields are ignored.

use serde::{Deserialize, Serialize};

pub const PROTOCOL: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub project_id: String,
    pub title: String,
    /// Milliseconds since 1970.
    pub created_at: f64,
    pub updated_at: f64,
    pub pinned: bool,
    pub settled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Its rows changed in the last few seconds: the dev server's stand-in for "running".
    pub active: bool,
}

/// One transcript row, as much of it as the phone draws.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub seq: i64,
    /// The database's change counter when it was written: of two copies, the higher is newer,
    /// whichever message brought it.
    pub rev: i64,
    pub id: String,
    /// `user`, `assistant`, `thinking`, `tool`, `notice`, `commandOutput`, or newer kinds.
    pub kind: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Trimmed to `INPUT_LIMIT` characters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<String>,
    /// Trimmed to `RESULT_LIMIT` characters; `tool_result_length` is the whole one's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_result_length: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_is_error: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice_style: Option<String>,
    /// Seconds since 1970.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_ended_at: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub attachments: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub changed_files: Vec<String>,
}

pub const INPUT_LIMIT: usize = 4000;
pub const RESULT_LIMIT: usize = 2000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToApp {
    Hello { protocol: u32, server: String, data: String },
    /// Every project and thread, in sidebar order.
    Threads { projects: Vec<Project>, threads: Vec<Thread> },
    ThreadUpserted { thread: Thread },
    ThreadRemoved { id: String },
    /// The newest rows of the thread subscribed to: `rows` are `first_seq..count`.
    Snapshot { thread: String, first_seq: i64, count: i64, rows: Vec<Row> },
    /// Rows added or changed, oldest first; `count` is how many the transcript has now.
    RowsChanged { thread: String, count: i64, rows: Vec<Row> },
    /// Earlier rows asked for with `page`.
    PageRows { thread: String, rows: Vec<Row> },
    Error { message: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FromApp {
    SubscribeApp,
    /// Watch this thread (one at a time): answered with a `snapshot`, then `rows_changed`.
    SubscribeThread { thread: String, #[serde(default)] limit: Option<i64> },
    UnsubscribeThread,
    /// Rows before `before_seq`, at most `limit`.
    Page { thread: String, before_seq: i64, #[serde(default)] limit: Option<i64> },
    #[serde(other)]
    Unknown,
}

/// `s` cut to at most `limit` characters, on a character boundary.
pub fn trim(s: String, limit: usize) -> String {
    match s.char_indices().nth(limit) {
        Some((at, _)) => s[..at].to_string(),
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_messages_and_fields_are_ignored() {
        let m: FromApp = serde_json::from_str(r#"{"type":"send","text":"hi"}"#).unwrap();
        assert_eq!(m, FromApp::Unknown);
        let m: FromApp = serde_json::from_str(r#"{"type":"subscribe_thread","thread":"t","extra":1}"#).unwrap();
        assert_eq!(m, FromApp::SubscribeThread { thread: "t".into(), limit: None });
    }

    #[test]
    fn tagged_as_the_app_reads_them() {
        let s = serde_json::to_string(&ToApp::ThreadRemoved { id: "a".into() }).unwrap();
        assert_eq!(s, r#"{"type":"thread_removed","id":"a"}"#);
    }

    #[test]
    fn trims_on_character_boundaries() {
        assert_eq!(trim("héllo".into(), 2), "hé");
        assert_eq!(trim("hi".into(), 5), "hi");
    }
}
