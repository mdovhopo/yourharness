//! `mcp__automations__…`: an agent makes, lists, changes, runs and deletes the project's
//! automations (`automations.rs`) when you ask for something to happen later or again and
//! again. Only the caller's project's. Changes are saved here; `on_change` tells the app (its
//! timer and the project page), and Run Now goes through `run_now`, which only the app can do.

use crate::automations::{self, Automation, Settings};
use crate::mcp::{schema, tool, ToolResult, ToolServer};
use serde_json::{json, Value};
use std::sync::Arc;

/// At most this many automations in a project.
const MAX_PER_PROJECT: usize = 50;
const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

pub struct AutomationTools {
    pub store: Arc<dyn Settings>,
    pub project_id: String,
    pub project_name: String,
    /// The thread whose agent calls.
    pub caller: String,
    /// The automations changed.
    pub on_change: Box<dyn Fn() + Send + Sync>,
    /// Runs one now, by id: the thread it started or sent to, or why not.
    pub run_now: Box<dyn Fn(String) -> Result<String, String> + Send + Sync>,
    pub now: fn() -> f64,
}

/// What the agent is told.
pub fn instructions(project: &str) -> String {
    format!(
        "Automations run a prompt in the project {project} by themselves, once at a time or on a schedule, while YourHarness is open (a run missed while it was closed happens once when it's back). mcp__automations__create_automation, list_automations, update_automation, run_automation and delete_automation manage them. When the user asks for something to happen later, at a time or again and again (\"every morning…\", \"remind me tomorrow…\", \"check the PRs every hour\"), make one instead of saying you can't, and only when they ask. Write its prompt so it stands alone: each run starts a fresh thread that knows nothing of this conversation (unless thread is \"this\"), in the project's permission mode. Times are the Mac's local time ({}). Then tell the user what you made: its schedule and its next run.",
        automations::zone()
    )
}

fn one_line(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max { flat } else { format!("{}…", flat.chars().take(max).collect::<String>()) }
}

fn opt_str(a: &Value, key: &str) -> Option<String> {
    a[key].as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

impl AutomationTools {
    fn target(a: &Automation) -> String {
        match (a.same_thread, &a.thread_id) {
            (false, _) => "a new thread each run".into(),
            (true, Some(t)) => format!("the same thread each run ({})", &t[..t.len().min(8)]),
            (true, None) => "the same thread each run (made by the first)".into(),
        }
    }

    fn describe(&self, a: &Automation, detail: bool) -> String {
        let now = (self.now)();
        let state = if a.enabled { "on" } else if a.next_run.is_none() && !a.schedule.repeats() && !a.runs.is_empty() { "done" } else { "paused" };
        let mut out = format!("• {} ({}) · {} · {} · {state}", a.name, a.short_id(), a.schedule.describe(now), Self::target(a));
        if let Some(next) = a.next_run.filter(|_| a.enabled) {
            out += &format!("\n  next: {}", automations::when(next, now));
        }
        if let Some(run) = a.last_run() {
            out += &format!("\n  last: {}", Self::run_line(run, now));
        }
        if let Some(m) = &a.model {
            out += &format!("\n  model: {m}");
        }
        if let Some(e) = &a.effort {
            out += &format!(" · effort: {e}");
        }
        if detail {
            out += &format!("\n  prompt:\n{}", a.prompt.lines().map(|l| format!("    {l}")).collect::<Vec<_>>().join("\n"));
            if a.runs.len() > 1 {
                out += "\n  runs, newest first:";
                for run in a.runs.iter().rev() {
                    out += &format!("\n    {}", Self::run_line(run, now));
                }
            }
        } else {
            out += &format!("\n  prompt: {}", one_line(&a.prompt, 100));
        }
        out
    }

    fn run_line(run: &automations::Run, now: f64) -> String {
        let mut line = automations::when(run.at, now);
        match (&run.error, &run.thread_id) {
            (Some(e), _) => line += &format!(", couldn't run: {e}"),
            (None, Some(t)) => line += &format!(", in thread {}", &t[..t.len().min(8)]),
            _ => {}
        }
        if run.manual {
            line += " (run by hand)";
        }
        if run.caught_up {
            line += " (late: the app was closed or asleep)";
        }
        line
    }

    fn mine(&self) -> Vec<Automation> {
        automations::all(self.store.as_ref()).into_iter().filter(|a| a.project_id == self.project_id).collect()
    }

    fn list(&self, a: &Value) -> ToolResult {
        let list = self.mine();
        if let Some(key) = opt_str(a, "id") {
            return match automations::find(&list, &self.project_id, &key) {
                Ok(found) => ToolResult::text(self.describe(found, true)),
                Err(e) => ToolResult::error(e),
            };
        }
        if list.is_empty() {
            return ToolResult::text(format!("{} has no automations.", self.project_name));
        }
        ToolResult::text(format!("{} has {} automation{}:\n{}", self.project_name, list.len(), if list.len() == 1 { "" } else { "s" }, list.iter().map(|x| self.describe(x, false)).collect::<Vec<_>>().join("\n")))
    }

    /// Where runs go, from `thread`: "new", "same" or "this".
    fn apply_thread(&self, auto: &mut Automation, thread: &str) -> Result<(), String> {
        match thread {
            "new" => {
                auto.same_thread = false;
                auto.thread_id = None;
            }
            "same" => {
                if !auto.same_thread {
                    auto.thread_id = None;
                }
                auto.same_thread = true;
            }
            "this" => {
                auto.same_thread = true;
                auto.thread_id = Some(self.caller.clone());
            }
            other => return Err(format!("thread is \"new\", \"same\" or \"this\", not \"{other}\".")),
        }
        Ok(())
    }

    fn apply_common(&self, auto: &mut Automation, a: &Value) -> Result<(), String> {
        if let Some(name) = opt_str(a, "name") {
            if name.chars().count() > 80 {
                return Err("Keep name under 80 characters.".into());
            }
            auto.name = name;
        }
        if let Some(prompt) = opt_str(a, "prompt") {
            auto.prompt = prompt;
        }
        if let Some(thread) = opt_str(a, "thread") {
            self.apply_thread(auto, &thread)?;
        }
        if let Some(model) = a.get("model").filter(|v| !v.is_null()) {
            auto.model = model.as_str().map(str::trim).filter(|m| !m.is_empty() && *m != "default").map(str::to_string);
        }
        if let Some(effort) = a.get("effort").filter(|v| !v.is_null()) {
            let e = effort.as_str().map(str::trim).filter(|e| !e.is_empty() && *e != "default").map(str::to_string);
            if let Some(e) = &e {
                if !EFFORTS.contains(&e.as_str()) {
                    return Err(format!("effort is one of {}, or \"default\".", EFFORTS.join(", ")));
                }
            }
            auto.effort = e;
        }
        Ok(())
    }

    fn create(&self, a: &Value) -> ToolResult {
        let now = (self.now)();
        let (Some(name), Some(prompt), Some(when)) = (opt_str(a, "name"), opt_str(a, "prompt"), opt_str(a, "schedule")) else {
            return ToolResult::error("Give name, prompt and schedule.");
        };
        let schedule = match automations::parse(&when, now) {
            Ok(s) => s,
            Err(e) => return ToolResult::error(e),
        };
        let mut auto = match Automation::new(&self.project_id, &name, &prompt, schedule, now) {
            Ok(x) => x,
            Err(e) => return ToolResult::error(format!("{e} ({when})")),
        };
        auto.created_by = Some(self.caller.clone());
        if let Err(e) = self.apply_common(&mut auto, a) {
            return ToolResult::error(e);
        }
        let made = automations::edit(self.store.as_ref(), |list| {
            if list.iter().filter(|x| x.project_id == self.project_id).count() >= MAX_PER_PROJECT {
                return Err(format!("{} has {MAX_PER_PROJECT} automations, the most a project may; delete some first.", self.project_name));
            }
            list.push(auto.clone());
            Ok(())
        });
        if let Err(e) = made {
            return ToolResult::error(e);
        }
        (self.on_change)();
        ToolResult::text(format!("Made:\n{}", self.describe(&auto, false)))
    }

    fn update(&self, a: &Value) -> ToolResult {
        let Some(key) = opt_str(a, "id") else { return ToolResult::error("Give id: the automation's id or name.") };
        let now = (self.now)();
        let schedule = match opt_str(a, "schedule").map(|s| automations::parse(&s, now)) {
            Some(Err(e)) => return ToolResult::error(e),
            Some(Ok(s)) => Some(s),
            None => None,
        };
        let result = automations::edit(self.store.as_ref(), |list| {
            let id = automations::find(list, &self.project_id, &key)?.id.clone();
            let auto = list.iter_mut().find(|x| x.id == id).expect("found above");
            let mut changed = auto.clone();
            self.apply_common(&mut changed, a)?;
            if let Some(s) = schedule.clone() {
                changed.set_schedule(s, now)?;
            }
            if let Some(on) = a["enabled"].as_bool() {
                changed.set_enabled(on, now);
                if on && !changed.enabled {
                    return Err("It has no runs left to make: give it a new schedule.".into());
                }
            }
            *auto = changed.clone();
            Ok::<Automation, String>(changed)
        });
        match result {
            Ok(auto) => {
                (self.on_change)();
                ToolResult::text(format!("Changed:\n{}", self.describe(&auto, false)))
            }
            Err(e) => ToolResult::error(e),
        }
    }

    fn delete(&self, a: &Value) -> ToolResult {
        let Some(key) = opt_str(a, "id") else { return ToolResult::error("Give id: the automation's id or name.") };
        let result = automations::edit(self.store.as_ref(), |list| {
            let id = automations::find(list, &self.project_id, &key)?.id.clone();
            let name = list.iter().find(|x| x.id == id).map(|x| x.name.clone()).unwrap_or_default();
            list.retain(|x| x.id != id);
            Ok::<String, String>(name)
        });
        match result {
            Ok(name) => {
                (self.on_change)();
                ToolResult::text(format!("Deleted “{name}”. The threads it started are still there."))
            }
            Err(e) => ToolResult::error(e),
        }
    }

    fn run(&self, a: &Value) -> ToolResult {
        let Some(key) = opt_str(a, "id") else { return ToolResult::error("Give id: the automation's id or name.") };
        let list = self.mine();
        let auto = match automations::find(&list, &self.project_id, &key) {
            Ok(x) => x.clone(),
            Err(e) => return ToolResult::error(e),
        };
        if auto.same_thread && auto.thread_id.as_deref() == Some(self.caller.as_str()) {
            return ToolResult::error("This automation sends to this thread: its prompt would only queue behind this turn. Do the work here instead.");
        }
        match (self.run_now)(auto.id.clone()) {
            Ok(thread) => ToolResult::text(format!("Running “{}” now, in thread {}. Its schedule is unchanged.", auto.name, &thread[..thread.len().min(8)])),
            Err(e) => ToolResult::error(e),
        }
    }
}

impl ToolServer for AutomationTools {
    fn name(&self) -> &str {
        "automations"
    }

    fn tools(&self) -> Vec<Value> {
        let thread = json!({ "type": "string", "enum": ["new", "same", "this"], "description": "Where each run goes: \"new\" a new thread every run (the default), \"same\" one thread every run (made by the first run), \"this\" this thread every run." });
        let model = json!({ "type": "string", "description": "The model for its runs (e.g. \"haiku\", \"sonnet\", \"opus\"); \"default\" or left out: the app's default." });
        let effort = json!({ "type": "string", "enum": ["low", "medium", "high", "xhigh", "max", "default"], "description": "Reasoning effort for its runs." });
        let when = format!("When it runs, in the Mac's local time: {}.", automations::FORMS);
        vec![
            tool("list_automations", "List this project's automations: schedule, where runs go, next and last run. Give id for one in full: its prompt and its runs.", schema(json!({ "id": { "type": "string", "description": "An automation's id (or its first 6+ characters) or name." } }), &[]), true, false),
            tool(
                "create_automation",
                "Make an automation: a prompt this project runs by itself, once or on a schedule. Only when the user asks for something later or repeated.",
                schema(
                    json!({
                        "name": { "type": "string", "description": "A short name, shown in the list and as its threads' title." },
                        "prompt": { "type": "string", "description": "What each run is told to do, written to stand alone." },
                        "schedule": { "type": "string", "description": when },
                        "thread": thread,
                        "model": model,
                        "effort": effort,
                    }),
                    &["name", "prompt", "schedule"],
                ),
                false,
                false,
            ),
            tool(
                "update_automation",
                "Change an automation: any of its name, prompt, schedule, where runs go, model, effort; enabled false pauses it, true resumes it.",
                schema(
                    json!({
                        "id": { "type": "string", "description": "Its id (or first 6+ characters) or name." },
                        "name": { "type": "string" },
                        "prompt": { "type": "string" },
                        "schedule": { "type": "string", "description": when },
                        "thread": thread,
                        "model": model,
                        "effort": effort,
                        "enabled": { "type": "boolean" },
                    }),
                    &["id"],
                ),
                false,
                false,
            ),
            tool("run_automation", "Run an automation now, once, as its schedule would; the schedule stays as it is.", schema(json!({ "id": { "type": "string" } }), &["id"]), false, false),
            tool("delete_automation", "Delete an automation. The threads it started stay. Only when the user asks.", schema(json!({ "id": { "type": "string" } }), &["id"]), false, false),
        ]
    }

    fn call(&self, tool: &str, a: &Value, _: Option<&str>) -> ToolResult {
        match tool {
            "list_automations" => self.list(a),
            "create_automation" => self.create(a),
            "update_automation" => self.update(a),
            "run_automation" => self.run(a),
            "delete_automation" => self.delete(a),
            other => ToolResult::error(format!("No tool {other}.")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Fake(Mutex<Option<Value>>);
    impl Settings for Fake {
        fn get(&self) -> Option<Value> {
            self.0.lock().unwrap().clone()
        }
        fn set(&self, value: &Value) {
            *self.0.lock().unwrap() = Some(value.clone());
        }
    }

    fn now() -> f64 {
        1_795_000_000.0
    }

    fn tools(store: Arc<Fake>, project: &str) -> AutomationTools {
        AutomationTools { store, project_id: project.into(), project_name: project.into(), caller: "CALLER-THREAD".into(), on_change: Box::new(|| {}), run_now: Box::new(|id| Ok(format!("thread-for-{id}"))), now }
    }

    #[test]
    fn make_change_run_delete() {
        let store = Arc::new(Fake(Mutex::new(None)));
        let t = tools(store.clone(), "P");
        let made = t.call("create_automation", &json!({ "name": "PRs", "prompt": "Review open PRs", "schedule": "weekdays 9:00", "effort": "low" }), None);
        assert!(!made.is_error, "{}", made.text);
        assert!(made.text.contains("Weekdays at 9:00") && made.text.contains("next:"), "{}", made.text);
        let bad = t.call("create_automation", &json!({ "name": "x", "prompt": "y", "schedule": "whenever" }), None);
        assert!(bad.is_error && bad.text.contains("Couldn't read"));
        let changed = t.call("update_automation", &json!({ "id": "PRs", "schedule": "every 2h", "thread": "this" }), None);
        assert!(changed.text.contains("Every 2 hours") && changed.text.contains("same thread"), "{}", changed.text);
        // Run Now into the caller's own thread would only queue behind this turn.
        assert!(t.call("run_automation", &json!({ "id": "PRs" }), None).is_error);
        t.call("update_automation", &json!({ "id": "PRs", "thread": "new" }), None);
        assert!(t.call("run_automation", &json!({ "id": "prs" }), None).text.contains("Running"));
        let paused = t.call("update_automation", &json!({ "id": "PRs", "enabled": false }), None);
        assert!(paused.text.contains("paused"), "{}", paused.text);
        // Another project doesn't see it.
        assert!(tools(store.clone(), "Q").call("list_automations", &json!({}), None).text.contains("no automations"));
        assert!(tools(store.clone(), "Q").call("delete_automation", &json!({ "id": "PRs" }), None).is_error);
        assert!(t.call("list_automations", &json!({ "id": "PRs" }), None).text.contains("Review open PRs"));
        assert!(t.call("delete_automation", &json!({ "id": "PRs" }), None).text.contains("Deleted"));
        assert!(automations::all(store.as_ref()).is_empty());
    }
}
