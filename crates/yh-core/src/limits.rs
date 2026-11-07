//! Claude's usage limits (the 5-hour and weekly windows) as the CLI reports them in a
//! `rate_limit_event`, saved as the Swift app saved them (`rateLimit`, `rateLimitUpdatedAt`),
//! read for the sidebar's meters, and refreshed on demand with the smallest request there is.

use serde_json::{json, Value};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// One window of a reading.
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    /// `five_hour`, `seven_day`, `seven_day_opus`, `seven_day_sonnet`…
    pub name: String,
    /// 0…1.
    pub utilization: f64,
    /// Seconds since 1970.
    pub resets_at: Option<f64>,
}

/// Always shown, even before the first reading, so the footer never jumps.
pub const PRIMARY: [&str; 2] = ["five_hour", "seven_day"];

/// The windows of a `rate_limit_info` (`unifiedWindows`), by name.
pub fn windows(info: &Value) -> Vec<Window> {
    let mut list: Vec<Window> = info["unifiedWindows"]
        .as_object()
        .map(|o| {
            o.iter()
                .map(|(name, w)| Window {
                    name: name.clone(),
                    utilization: w["utilization"].as_f64().unwrap_or(0.),
                    resets_at: w["resetsAt"].as_f64(),
                })
                .collect()
        })
        .unwrap_or_default();
    list.sort_by(|a, b| a.name.cmp(&b.name));
    list
}

/// The two primary windows first (`None` when not reported), then any others.
pub fn meters(info: Option<&Value>) -> Vec<(String, Option<Window>)> {
    let reported = info.map(windows).unwrap_or_default();
    let mut out: Vec<(String, Option<Window>)> = PRIMARY.iter().map(|n| (n.to_string(), reported.iter().find(|w| w.name == *n).cloned())).collect();
    out.extend(reported.into_iter().filter(|w| !PRIMARY.contains(&w.name.as_str())).map(|w| (w.name.clone(), Some(w))));
    out
}

/// As the Swift app's `RateLimitInfo` is saved: `{status, windows: [{name, utilization, resetsAt}]}`, dates ISO.
pub fn to_saved(info: &Value) -> Value {
    let windows: Vec<Value> = windows(info)
        .into_iter()
        .map(|w| {
            let mut o = json!({"name": w.name, "utilization": w.utilization});
            if let Some(at) = w.resets_at {
                o["resetsAt"] = Value::from(crate::library::iso(at as i64));
            }
            o
        })
        .collect();
    json!({"status": info["status"].as_str().unwrap_or("unknown"), "windows": windows})
}

/// A saved reading back in the CLI's shape, which the app keeps.
pub fn from_saved(saved: &Value) -> Option<Value> {
    let list = saved["windows"].as_array()?;
    let mut unified = serde_json::Map::new();
    for w in list {
        let Some(name) = w["name"].as_str() else { continue };
        let mut o = json!({"utilization": w["utilization"].as_f64().unwrap_or(0.)});
        let at = match &w["resetsAt"] {
            Value::String(s) => crate::library::parse_iso(s),
            // Swift's default date encoding: seconds since 2001.
            Value::Number(n) => n.as_f64().map(|v| if v < 1_500_000_000. { v + 978_307_200. } else { v }),
            _ => None,
        };
        if let Some(at) = at {
            o["resetsAt"] = Value::from(at);
        }
        unified.insert(name.to_string(), o);
    }
    Some(json!({"status": saved["status"].as_str().unwrap_or("unknown"), "unifiedWindows": unified}))
}

/// "5h", "7d", "Opus", "Sonnet": a meter's label.
pub fn short_label(name: &str) -> String {
    match name {
        "five_hour" => "5h".into(),
        "seven_day" => "7d".into(),
        "seven_day_opus" => "Opus".into(),
        "seven_day_sonnet" => "Sonnet".into(),
        other => {
            let s: String = other.chars().take(4).collect();
            capitalize(&s)
        }
    }
}

/// "5-hour limit", "Weekly Opus limit": a meter's name in its tooltip.
pub fn title(name: &str) -> String {
    match name {
        "five_hour" => "5-hour limit".into(),
        "seven_day" => "Weekly limit".into(),
        "seven_day_opus" => "Weekly Opus limit".into(),
        "seven_day_sonnet" => "Weekly Sonnet limit".into(),
        other => other.split('_').map(capitalize).collect::<Vec<_>>().join(" "),
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// What a window says now: a reading from before it reset says 0.
pub fn utilization(w: &Window, now: f64) -> f64 {
    if w.resets_at.is_some_and(|at| at <= now) { 0. } else { w.utilization.clamp(0., 1.) }
}

/// The CLI only reports limits alongside a real request, so this sends the smallest one it
/// can: Haiku, one word, no tools, no MCP, none of your settings or hooks, nothing saved to
/// session history. About 600 tokens, a couple of seconds. Blocks (45 s at most).
pub fn probe(executable: &Path, path_env: Option<&str>) -> Result<Value, String> {
    let folder = std::env::temp_dir().join("yourharness-limits");
    let _ = std::fs::create_dir_all(&folder);
    let mut command = Command::new(executable);
    command
        .args(["-p", "ping", "--output-format", "stream-json", "--verbose", "--model", "haiku", "--tools", ""])
        .args(["--strict-mcp-config", "--setting-sources", "", "--no-session-persistence", "--system-prompt", "Reply with one word."])
        .current_dir(&folder)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = path_env {
        command.env("PATH", path);
    }
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let (mut stdout, mut stderr) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let out = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let err = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let started = Instant::now();
    let mut timed_out = false;
    loop {
        if child.try_wait().ok().flatten().is_some() {
            break;
        }
        if started.elapsed() > Duration::from_secs(45) {
            let _ = child.kill();
            let _ = child.wait();
            timed_out = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let (stdout, stderr) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    if let Some(info) = parse(&stdout) {
        return Ok(info);
    }
    if timed_out {
        return Err("Claude didn't answer in time.".into());
    }
    Err(stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
        .or_else(|| error_result(&stdout))
        .unwrap_or_else(|| "Claude didn't report usage.".into()))
}

/// The last reading with windows in a stream-json transcript.
pub fn parse(stdout: &str) -> Option<Value> {
    stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|o| o["type"] == "rate_limit_event")
        .map(|o| o["rate_limit_info"].clone())
        .filter(|i| i["unifiedWindows"].as_object().is_some_and(|w| !w.is_empty()))
        .last()
}

/// An error result ("Not logged in", "Credit balance too low"): the most useful thing to show.
fn error_result(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|o| o["type"] == "result" && o["is_error"] == true)
        .and_then(|o| o["result"].as_str().map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> Value {
        json!({"status": "allowed", "unifiedWindows": {
            "seven_day": {"utilization": 0.37, "resetsAt": 1_793_991_600.0},
            "five_hour": {"utilization": 0.23, "resetsAt": 1_793_900_000.0},
            "seven_day_opus": {"utilization": 0.9}
        }})
    }

    #[test]
    fn saved_round_trip() {
        let saved = to_saved(&info());
        assert_eq!(saved["windows"][0]["name"], "five_hour");
        assert!(saved["windows"][0]["resetsAt"].as_str().unwrap().ends_with('Z'));
        assert!(saved["windows"][2].get("resetsAt").is_none());
        let back = from_saved(&saved).unwrap();
        assert_eq!(windows(&back), windows(&info()));
        // Swift's reference-date numbers too.
        let swift = json!({"status": "allowed", "windows": [{"name": "five_hour", "utilization": 0.5, "resetsAt": 815_000_000.0}]});
        assert_eq!(windows(&from_saved(&swift).unwrap())[0].resets_at, Some(815_000_000.0 + 978_307_200.));
    }

    #[test]
    fn meters_keep_the_primary_two_first() {
        let m = meters(Some(&info()));
        assert_eq!(m.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["five_hour", "seven_day", "seven_day_opus"]);
        let empty = meters(None);
        assert_eq!(empty.len(), 2);
        assert!(empty.iter().all(|(_, w)| w.is_none()));
        assert_eq!(short_label("seven_day_opus"), "Opus");
        assert_eq!(short_label("weekly_extra"), "Week");
        assert_eq!(title("seven_day_sonnet"), "Weekly Sonnet limit");
        assert_eq!(title("odd_window"), "Odd Window");
    }

    #[test]
    fn a_reading_from_before_the_reset_says_zero() {
        let w = Window { name: "five_hour".into(), utilization: 0.8, resets_at: Some(100.) };
        assert_eq!(utilization(&w, 50.), 0.8);
        assert_eq!(utilization(&w, 150.), 0.);
    }

    #[test]
    fn parses_the_last_reading() {
        let out = format!(
            "{}\n{{\"type\":\"rate_limit_event\",\"rate_limit_info\":{{\"unifiedWindows\":{{}}}}}}\nnot json\n{}\n",
            json!({"type": "rate_limit_event", "rate_limit_info": info()}),
            json!({"type": "result", "is_error": false})
        );
        assert_eq!(windows(&parse(&out).unwrap()).len(), 3);
        assert!(parse("{\"type\":\"result\",\"is_error\":true,\"result\":\"Not logged in\"}").is_none());
        assert_eq!(error_result("{\"type\":\"result\",\"is_error\":true,\"result\":\"Not logged in\"}").as_deref(), Some("Not logged in"));
    }
}
