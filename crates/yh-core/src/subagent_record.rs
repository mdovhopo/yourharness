//! A sub-agent's run as the `subagent` column keeps it, the record 0.0.x wrote
//! (`SubagentSnapshot`: keys in its order, nils left out, dates as seconds since 2001,
//! slashes escaped), so either version reads what the other saved. Read back leniently: a run
//! still `running` when saved ended with the process it ran in.

use serde_json::Value;

/// Seconds between 1970 and 2001, Foundation's reference date.
const REFERENCE_DATE: f64 = 978_307_200.0;

#[derive(Clone, Debug, PartialEq, Default)]
pub struct SubagentRecord {
    pub description: String,
    pub agent_type: Option<String>,
    pub prompt: String,
    pub is_background: bool,
    /// `running`, `completed`, `failed`, `stopped`.
    pub status: String,
    pub tool_uses: Option<u64>,
    pub tokens: Option<u64>,
    pub duration_ms: Option<u64>,
    pub report: Option<String>,
    /// Seconds since 1970.
    pub started_at: f64,
    pub entries: Vec<SubagentEntry>,
}

/// One thing it did: a message (`kind` assistant) or a tool call with its result.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SubagentEntry {
    pub id: String,
    pub kind: String,
    pub text: String,
    pub tool_name: Option<String>,
    pub tool_input: Option<String>,
    pub tool_result: Option<String>,
    pub tool_is_error: Option<bool>,
}

fn string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default().replace('/', "\\/")
}

fn number(f: f64) -> String {
    if f.fract() == 0.0 && f.abs() < 1e15 { format!("{}", f as i64) } else { format!("{f}") }
}

/// `startedAt` as saved: seconds since 2001 (or ISO 8601, from older builds), to since 1970.
fn started_at(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Number(n)) => n.as_f64().map(|f| f + REFERENCE_DATE),
        Some(Value::String(iso)) => crate::legacy_import::parse_iso8601(iso),
        _ => None,
    }
    .unwrap_or(0.0)
}

/// A run as the Sub-agents panel lists it: what it was and how it went, without its steps.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SubagentSummary {
    pub description: String,
    pub agent_type: Option<String>,
    pub is_background: bool,
    /// As `SubagentRecord::decode` reads it: a run saved `running` is `stopped`.
    pub status: String,
    pub tool_uses: Option<u64>,
    pub tokens: Option<u64>,
    pub duration_ms: Option<u64>,
    pub has_report: bool,
    /// Seconds since 1970.
    pub started_at: f64,
}

/// The summary's fields; the rest (its steps, prompt and report) skipped, never kept.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SummaryFields {
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    agent_type: Option<String>,
    #[serde(default)]
    is_background: Option<bool>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    tool_uses: Option<u64>,
    #[serde(default)]
    tokens: Option<u64>,
    #[serde(default)]
    duration_ms: Option<u64>,
    #[serde(default)]
    report: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    started_at: Option<Value>,
}

struct Writer(String);

impl Writer {
    fn new() -> Writer {
        Writer(String::from("{"))
    }
    fn raw(&mut self, key: &str, value: &str) {
        if self.0.len() > 1 {
            self.0.push(',');
        }
        self.0.push_str(&string(key));
        self.0.push(':');
        self.0.push_str(value);
    }
    fn str(&mut self, key: &str, v: &str) {
        self.raw(key, &string(v));
    }
    fn opt(&mut self, key: &str, v: &Option<String>) {
        if let Some(v) = v {
            self.str(key, v);
        }
    }
    fn int(&mut self, key: &str, v: Option<u64>) {
        if let Some(v) = v {
            self.raw(key, &v.to_string());
        }
    }
    fn end(mut self) -> String {
        self.0.push('}');
        self.0
    }
}

/// The statuses 0.0.x knows; anything else reads as stopped.
const STATUSES: &[&str] = &["running", "completed", "failed", "stopped"];

impl SubagentRecord {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.str("description", &self.description);
        w.opt("agentType", &self.agent_type);
        w.str("prompt", &self.prompt);
        if self.is_background {
            w.raw("isBackground", "true");
        }
        let status = match self.status.as_str() {
            s if STATUSES.contains(&s) => s,
            "error" => "failed",
            _ => "stopped",
        };
        w.str("status", status);
        w.int("toolUses", self.tool_uses);
        w.int("tokens", self.tokens);
        w.int("durationMs", self.duration_ms);
        w.opt("report", &self.report);
        w.raw("startedAt", &number(self.started_at - REFERENCE_DATE));
        let entries: Vec<String> = self
            .entries
            .iter()
            .map(|e| {
                let mut w = Writer::new();
                w.str("id", &e.id);
                w.str("kind", &e.kind);
                w.str("text", &e.text);
                w.opt("toolName", &e.tool_name);
                w.opt("toolInput", &e.tool_input);
                w.opt("toolResult", &e.tool_result);
                if let Some(b) = e.tool_is_error {
                    w.raw("toolIsError", if b { "true" } else { "false" });
                }
                w.end()
            })
            .collect();
        w.raw("entries", &format!("[{}]", entries.join(",")));
        w.end().into_bytes()
    }

    /// What the panel lists of a saved run, read without keeping its steps; None when it
    /// isn't a run.
    pub fn summary(bytes: &[u8]) -> Option<SubagentSummary> {
        let f: SummaryFields = serde_json::from_slice(bytes).ok()?;
        let status = f.status.unwrap_or_else(|| "stopped".into());
        Some(SubagentSummary {
            description: f.description.unwrap_or_default(),
            agent_type: f.agent_type,
            is_background: f.is_background.unwrap_or(false),
            status: if status == "running" { "stopped".into() } else { status },
            tool_uses: f.tool_uses,
            tokens: f.tokens,
            duration_ms: f.duration_ms,
            has_report: f.report.is_some(),
            started_at: started_at(f.started_at.as_ref()),
        })
    }

    /// What was saved; None when it isn't a run.
    pub fn decode(bytes: &[u8]) -> Option<SubagentRecord> {
        let v: Value = serde_json::from_slice(bytes).ok()?;
        let o = v.as_object()?;
        let s = |k: &str| o.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let status = s("status").unwrap_or_else(|| "stopped".into());
        let started_at = started_at(o.get("startedAt"));
        let entries = o
            .get("entries")
            .and_then(|e| e.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|i| {
                        let e = i.as_object()?;
                        let s = |k: &str| e.get(k).and_then(|v| v.as_str()).map(str::to_string);
                        Some(SubagentEntry {
                            id: s("id").unwrap_or_default(),
                            kind: s("kind")?,
                            text: s("text").unwrap_or_default(),
                            tool_name: s("toolName"),
                            tool_input: s("toolInput"),
                            tool_result: s("toolResult"),
                            tool_is_error: e.get("toolIsError").and_then(|v| v.as_bool()),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(SubagentRecord {
            description: s("description").unwrap_or_default(),
            agent_type: s("agentType"),
            prompt: s("prompt").unwrap_or_default(),
            is_background: o.get("isBackground").and_then(|v| v.as_bool()).unwrap_or(false),
            // Still running when saved: it ended with the process it ran in.
            status: if status == "running" { "stopped".into() } else { status },
            tool_uses: o.get("toolUses").and_then(|v| v.as_u64()),
            tokens: o.get("tokens").and_then(|v| v.as_u64()),
            duration_ms: o.get("durationMs").and_then(|v| v.as_u64()),
            report: s("report"),
            started_at,
            entries,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_as_json_encoder_and_reads_back() {
        let run = SubagentRecord {
            description: "Look".into(),
            prompt: "Find it".into(),
            status: "completed".into(),
            tool_uses: Some(2),
            report: Some("Found in a/b".into()),
            started_at: REFERENCE_DATE + 814183200.,
            entries: vec![
                SubagentEntry { id: "s1".into(), kind: "assistant".into(), text: "ok".into(), ..Default::default() },
                SubagentEntry { id: "t1".into(), kind: "tool".into(), tool_name: Some("Read".into()), tool_input: Some("{}".into()), tool_result: Some("x".into()), tool_is_error: Some(false), ..Default::default() },
            ],
            ..Default::default()
        };
        let json = String::from_utf8(run.encode()).unwrap();
        assert!(json.starts_with(r#"{"description":"Look","prompt":"Find it","status":"completed","toolUses":2,"report":"Found in a\/b","startedAt":814183200,"entries":[{"id":"s1","kind":"assistant","text":"ok"}"#), "{json}");
        assert_eq!(SubagentRecord::decode(json.as_bytes()), Some(run));
    }

    #[test]
    fn a_summary_is_what_decode_reads_without_the_steps() {
        let run = SubagentRecord {
            description: "Look".into(),
            agent_type: Some("Explore".into()),
            prompt: "Find it".into(),
            is_background: true,
            status: "running".into(),
            tool_uses: Some(7),
            tokens: Some(1200),
            duration_ms: Some(5000),
            report: Some("Found".into()),
            started_at: REFERENCE_DATE + 814183200.,
            entries: vec![SubagentEntry { id: "t1".into(), kind: "tool".into(), tool_name: Some("Read".into()), ..Default::default() }],
        };
        let s = SubagentRecord::summary(&run.encode()).unwrap();
        let full = SubagentRecord::decode(&run.encode()).unwrap();
        assert_eq!(s.description, full.description);
        assert_eq!(s.agent_type.as_deref(), Some("Explore"));
        assert!(s.is_background && s.has_report);
        assert_eq!(s.status, "stopped");
        assert_eq!((s.tool_uses, s.tokens, s.duration_ms), (Some(7), Some(1200), Some(5000)));
        assert_eq!(s.started_at, full.started_at);
        assert_eq!(SubagentRecord::summary(b"[1]"), None);
        assert_eq!(SubagentRecord::summary(b"{}").map(|s| s.status), Some("stopped".into()));
    }

    #[test]
    fn a_running_one_reads_as_stopped_and_bad_ones_as_none() {
        let r = SubagentRecord::decode(br#"{"description":"x","prompt":"","status":"running","startedAt":"2026-10-20T10:00:00Z","entries":[{"text":"no kind"}]}"#).unwrap();
        assert_eq!(r.status, "stopped");
        assert!(r.entries.is_empty());
        assert!(r.started_at > 1.7e9);
        assert_eq!(SubagentRecord::decode(b"[1]"), None);
        assert_eq!(SubagentRecord::decode(b"not json"), None);
    }
}
