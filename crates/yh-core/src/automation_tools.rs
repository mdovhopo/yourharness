//! `mcp__automations__…`: an agent lists, creates, updates, runs and deletes automations
//! (`automations.rs`) when you ask for something to happen later, on a schedule, or in reaction
//! to an event on your Mac. From a project thread, manages that project's automations (and can
//! see global ones); from Home, manages global automations and any project's.

use crate::automations::{self, Automation, Scope, Settings, ThreadPolicy};
use crate::mcp::{schema, tool, ToolResult, ToolServer};
use serde_json::{json, Value};
use std::sync::Arc;

/// At most this many automations per scope.
const MAX_PER_SCOPE: usize = 50;
const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
const MODES: [&str; 4] = ["ask", "acceptEdits", "plan", "bypass"];

pub struct AutomationTools {
    pub store: Arc<dyn Settings>,
    pub project_id: String,
    pub project_name: String,
    /// True when called from Home: can create/manage global automations and target other projects.
    pub is_home: bool,
    /// Known projects `(id, name)` for resolving `project` from Home.
    pub projects: Vec<(String, String)>,
    /// The thread whose agent calls.
    pub caller: String,
    /// The automations changed.
    pub on_change: Box<dyn Fn() + Send + Sync>,
    /// Runs one now, by id: the thread it started or sent to, or why not.
    pub run_now: Box<dyn Fn(String) -> Result<String, String> + Send + Sync>,
    pub now: fn() -> f64,
}

/// What the agent is told.
pub fn instructions(project: &str, is_home: bool) -> String {
    let scope_note = if is_home {
        "From Home you can manage Global automations (scope \"global\", the default here) as well as any project's automations."
    } else {
        "Automations made here run in this workspace (or pass scope \"global\" for a Mac-wide automation)."
    };
    format!(
        "Automations run a prompt in {project} (or globally) by themselves: on a schedule (once, repeating, daily, cron), or on Mac events (wake, unlock, app launch/quit, power, network, file/folder watch, git branch/commit, PR events), or manually via `yh trigger`. {scope_note} mcp__automations__create_automation, list_automations, update_automation, run_automation and delete_automation manage them. When the user asks for something to happen later, repeatedly, or when an event happens on the Mac, make an automation instead of saying you can't, and only when they ask. Write its prompt so it stands alone; choose thread \"new\", \"new_settle_previous\" (fresh thread + settles the previous run), \"same\", or \"this\". Times are local time ({}). Then tell the user what you made: its scope, trigger, and next run (if scheduled).",
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
    fn scope_label(&self, a: &Automation) -> String {
        match a.scope() {
            Scope::Global => "Global".into(),
            Scope::Project { project_id } => {
                if project_id == self.project_id {
                    self.project_name.clone()
                } else {
                    self.projects.iter().find(|(id, _)| id == &project_id).map(|(_, n)| n.clone()).unwrap_or(project_id)
                }
            }
        }
    }

    fn target(a: &Automation) -> String {
        match (a.thread_policy(), &a.thread_id) {
            (ThreadPolicy::New, _) => "a new thread each run".into(),
            (ThreadPolicy::NewSettlePrevious, _) => "a new thread each run (settling previous)".into(),
            (ThreadPolicy::Same, Some(t)) => format!("the same thread each run ({})", &t[..t.len().min(8)]),
            (ThreadPolicy::Same, None) => "the same thread each run (made by the first)".into(),
        }
    }

    fn describe(&self, a: &Automation, detail: bool) -> String {
        let now = (self.now)();
        let state = if a.enabled { "on" } else if a.is_done() { "done" } else { "paused" };
        let trig = a.effective_trigger().describe(now);
        let mut out = format!("• {} ({}) [{}] · {} · {} · {state}", a.name, a.short_id(), self.scope_label(a), trig, Self::target(a));
        if let Some(next) = a.next_run.filter(|_| a.enabled) {
            out += &format!("\n  next: {}", automations::when(next, now));
        }
        if let Some(run) = a.last_run() {
            out += &format!("\n  last: {}", Self::run_line(run, now));
        }
        let mut cfg = vec![];
        if let Some(ag) = &a.agent {
            cfg.push(format!("agent: {ag}"));
        }
        if let Some(m) = &a.model {
            cfg.push(format!("model: {m}"));
        }
        if let Some(e) = &a.effort {
            cfg.push(format!("effort: {e}"));
        }
        if let Some(mode) = &a.mode {
            cfg.push(format!("mode: {mode}"));
        }
        if a.guardrails.cooldown_secs > 0 {
            cfg.push(format!("cooldown: {}", automations::format_duration(a.guardrails.cooldown_secs)));
        }
        if !cfg.is_empty() {
            out += &format!("\n  {}", cfg.join(" · "));
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
        if let Some(reason) = &run.trigger_reason {
            line += &format!(" [{reason}]");
        }
        match (&run.error, &run.thread_id) {
            (Some(e), _) => line += &format!(", {}: {e}", if run.effective_status() == automations::RunStatus::Skipped { "skipped" } else { "couldn't run" }),
            (None, Some(t)) => line += &format!(", in thread {}", &t[..t.len().min(8)]),
            _ => {}
        }
        if let Some(d) = run.duration_secs {
            line += &format!(" ({:.0}s)", d);
        }
        if run.manual {
            line += " (run by hand)";
        }
        if run.caught_up {
            line += " (late: the app was closed or asleep)";
        }
        line
    }

    fn visible(&self, list: &[Automation]) -> Vec<Automation> {
        list.iter()
            .filter(|a| self.is_home || a.is_global || a.project_id == self.project_id)
            .cloned()
            .collect()
    }

    fn find_editable<'a>(&self, list: &'a [Automation], key: &str) -> Result<&'a Automation, String> {
        let candidates: Vec<&Automation> = list
            .iter()
            .filter(|a| self.is_home || a.is_global || a.project_id == self.project_id)
            .collect();
        automations::find_in(&candidates, key, if self.is_home { "" } else { "in this project (or global)" })
    }

    fn resolve_scope(&self, a: &Value) -> Result<Scope, String> {
        if let Some(proj) = opt_str(a, "project") {
            if !self.is_home && !proj.eq_ignore_ascii_case(&self.project_name) && proj != self.project_id {
                return Err("Only threads in Home can target another project's automations.".into());
            }
            if let Some((id, _)) = self.projects.iter().find(|(id, name)| id == &proj || name.eq_ignore_ascii_case(&proj)) {
                return Ok(Scope::Project { project_id: id.clone() });
            }
            return Err(format!("No project \"{proj}\"."));
        }
        match opt_str(a, "scope").as_deref() {
            Some("global") => Ok(Scope::Global),
            Some("project") | Some("workspace") => Ok(Scope::Project { project_id: self.project_id.clone() }),
            Some(other) => Err(format!("scope is \"project\" or \"global\", not \"{other}\".")),
            None if self.is_home => Ok(Scope::Global),
            None => Ok(Scope::Project { project_id: self.project_id.clone() }),
        }
    }

    fn list(&self, a: &Value) -> ToolResult {
        let all = automations::all(self.store.as_ref());
        let list = self.visible(&all);
        if let Some(key) = opt_str(a, "id") {
            return match self.find_editable(&all, &key) {
                Ok(found) => ToolResult::text(self.describe(found, true)),
                Err(e) => ToolResult::error(e),
            };
        }
        if list.is_empty() {
            return ToolResult::text(format!("{} has no automations.", self.project_name));
        }
        ToolResult::text(format!(
            "{} automation{}:\n{}",
            list.len(),
            if list.len() == 1 { "" } else { "s" },
            list.iter().map(|x| self.describe(x, false)).collect::<Vec<_>>().join("\n")
        ))
    }

    /// Where runs go, from `thread`: "new", "new_settle_previous", "same", or "this".
    fn apply_thread(&self, auto: &mut Automation, thread: &str) -> Result<(), String> {
        match thread {
            "new" => auto.set_thread_policy(ThreadPolicy::New),
            "new_settle_previous" | "settle_previous" => auto.set_thread_policy(ThreadPolicy::NewSettlePrevious),
            "same" => auto.set_thread_policy(ThreadPolicy::Same),
            "this" => {
                auto.set_thread_policy(ThreadPolicy::Same);
                auto.thread_id = Some(self.caller.clone());
            }
            other => return Err(format!("thread is \"new\", \"new_settle_previous\", \"same\" or \"this\", not \"{other}\".")),
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
        if let Some(agent) = a.get("agent").filter(|v| !v.is_null()) {
            let ag = agent.as_str().map(str::trim).filter(|m| !m.is_empty() && *m != "default").map(str::to_string);
            if let Some(ag) = &ag {
                if !matches!(ag.as_str(), "claude" | "antigravity") {
                    return Err("agent is \"claude\", \"antigravity\", or \"default\".".into());
                }
            }
            auto.agent = ag;
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
        if let Some(mode) = a.get("mode").filter(|v| !v.is_null()) {
            let m = mode.as_str().map(str::trim).filter(|m| !m.is_empty() && *m != "default").map(str::to_string);
            if let Some(m) = &m {
                if !MODES.contains(&m.as_str()) {
                    return Err(format!("mode is one of {}, or \"default\".", MODES.join(", ")));
                }
            }
            auto.mode = m;
        }
        if let Some(cd) = opt_str(a, "cooldown") {
            let secs = automations::parse_duration_secs(&cd).ok_or_else(|| format!("Couldn't read cooldown \"{cd}\" (try \"15m\", \"1h\", \"1d\", or \"0\")."))?;
            auto.guardrails.cooldown_secs = secs;
        }
        if let Some(cmd) = a.get("condition_cmd").filter(|v| !v.is_null()) {
            auto.guardrails.condition_cmd = cmd.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        }
        Ok(())
    }

    fn create(&self, a: &Value) -> ToolResult {
        let now = (self.now)();
        let (Some(name), Some(prompt)) = (opt_str(a, "name"), opt_str(a, "prompt")) else {
            return ToolResult::error("Give name and prompt.");
        };
        let when_str = opt_str(a, "trigger").or_else(|| opt_str(a, "schedule"));
        let Some(when_str) = when_str else {
            return ToolResult::error("Give schedule or trigger.");
        };
        let trigger = match automations::parse_trigger(&when_str, now) {
            Ok(t) => t,
            Err(e) => return ToolResult::error(e),
        };
        let scope = match self.resolve_scope(a) {
            Ok(s) => s,
            Err(e) => return ToolResult::error(e),
        };
        let mut auto = match Automation::new_with_trigger(scope.clone(), &name, &prompt, trigger, now) {
            Ok(x) => x,
            Err(e) => return ToolResult::error(format!("{e} ({when_str})")),
        };
        auto.created_by = Some(self.caller.clone());
        if let Err(e) = self.apply_common(&mut auto, a) {
            return ToolResult::error(e);
        }
        let made = automations::edit(self.store.as_ref(), |list| {
            let count = list.iter().filter(|x| x.scope() == scope).count();
            if count >= MAX_PER_SCOPE {
                return Err(format!("That scope already has {MAX_PER_SCOPE} automations; delete some first."));
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
        let trigger = match opt_str(a, "trigger").or_else(|| opt_str(a, "schedule")).map(|s| automations::parse_trigger(&s, now)) {
            Some(Err(e)) => return ToolResult::error(e),
            Some(Ok(t)) => Some(t),
            None => None,
        };
        let result = automations::edit(self.store.as_ref(), |list| {
            let id = self.find_editable(list, &key)?.id.clone();
            let auto = list.iter_mut().find(|x| x.id == id).expect("found above");
            let mut changed = auto.clone();
            self.apply_common(&mut changed, a)?;
            if a.get("scope").is_some() || a.get("project").is_some() {
                changed.set_scope(self.resolve_scope(a)?);
            }
            if let Some(t) = trigger.clone() {
                changed.set_trigger(t, now)?;
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
            let id = self.find_editable(list, &key)?.id.clone();
            let name = list.iter().find(|x| x.id == id).map(|x| x.name.clone()).unwrap_or_default();
            list.retain(|x| x.id != id);
            Ok::<String, String>(name)
        });
        match result {
            Ok(name) => {
                (self.on_change)();
                ToolResult::text(format!("Deleted “{name}”. The threads it started stay."))
            }
            Err(e) => ToolResult::error(e),
        }
    }

    fn run(&self, a: &Value) -> ToolResult {
        let Some(key) = opt_str(a, "id") else { return ToolResult::error("Give id: the automation's id or name.") };
        let all = automations::all(self.store.as_ref());
        let auto = match self.find_editable(&all, &key) {
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
        let thread = json!({
            "type": "string",
            "enum": ["new", "new_settle_previous", "same", "this"],
            "description": "Where each run goes: \"new\" a new thread every run (default), \"new_settle_previous\" a new thread every run while settling the previous run's thread, \"same\" one thread every run, \"this\" this thread every run."
        });
        let scope = json!({
            "type": "string",
            "enum": ["project", "global"],
            "description": "\"project\" runs in this workspace; \"global\" runs Mac-wide (in Home)."
        });
        let agent = json!({
            "type": "string",
            "enum": ["claude", "antigravity", "default"],
            "description": "Which agent runs it (\"claude\", \"antigravity\", or \"default\")."
        });
        let model = json!({ "type": "string", "description": "The model for its runs (e.g. \"haiku\", \"sonnet\", \"opus\"); \"default\" or left out: the app's default." });
        let effort = json!({ "type": "string", "enum": ["low", "medium", "high", "xhigh", "max", "default"], "description": "Reasoning effort for its runs." });
        let mode = json!({ "type": "string", "enum": ["ask", "acceptEdits", "plan", "bypass", "default"], "description": "Permission mode override (\"default\" inherits the project's mode)." });
        let when = format!(
            "Schedule ({}) OR Mac/repo event: \"wake\", \"unlock\", \"app_launched:<App>\", \"app_quit:<App>\", \"power:ac|battery\", \"network\", \"startup\", \"watch:<path|glob>\", \"branch[:name]\", \"commit[:name]\", \"pr_review\", \"pr_failed\", \"pr_merged\", or \"manual\".",
            automations::FORMS
        );
        let cooldown = json!({ "type": "string", "description": "Minimum time between automatic runs, e.g. \"15m\", \"1h\", \"1d\", or \"0\"." });
        vec![
            tool("list_automations", "List automations (this workspace's and global, or all from Home): scope, trigger, thread policy, next and last run. Give id for one in full.", schema(json!({ "id": { "type": "string", "description": "An automation's id (or its first 6+ characters) or name." } }), &[]), true, false),
            tool(
                "create_automation",
                "Make an automation: a prompt run on a schedule, on a Mac/file/git/PR event, or manually. Only when the user asks.",
                schema(
                    json!({
                        "name": { "type": "string", "description": "A short name, shown in the list and as its threads' title." },
                        "prompt": { "type": "string", "description": "What each run is told to do, written to stand alone." },
                        "schedule": { "type": "string", "description": when },
                        "trigger": { "type": "string", "description": "Alias for schedule, accepting schedules or Mac/file/repo event shorthands." },
                        "scope": scope,
                        "project": { "type": "string", "description": "From Home only: target project name or id." },
                        "thread": thread,
                        "agent": agent,
                        "model": model,
                        "effort": effort,
                        "mode": mode,
                        "cooldown": cooldown,
                        "condition_cmd": { "type": "string", "description": "Optional shell command that must exit 0 before invoking the agent." },
                    }),
                    &["name", "prompt"],
                ),
                false,
                false,
            ),
            tool(
                "update_automation",
                "Change an automation: name, prompt, schedule/trigger, scope, thread policy, agent, model, effort, mode, cooldown, or enabled (false pauses, true resumes).",
                schema(
                    json!({
                        "id": { "type": "string", "description": "Its id (or first 6+ characters) or name." },
                        "name": { "type": "string" },
                        "prompt": { "type": "string" },
                        "schedule": { "type": "string", "description": when },
                        "trigger": { "type": "string" },
                        "scope": scope,
                        "project": { "type": "string" },
                        "thread": thread,
                        "agent": agent,
                        "model": model,
                        "effort": effort,
                        "mode": mode,
                        "cooldown": cooldown,
                        "condition_cmd": { "type": "string" },
                        "enabled": { "type": "boolean" },
                    }),
                    &["id"],
                ),
                false,
                false,
            ),
            tool("run_automation", "Run an automation now, once, as its trigger would; its schedule stays as it is.", schema(json!({ "id": { "type": "string" } }), &["id"]), false, false),
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

    fn tools(store: Arc<Fake>, project: &str, is_home: bool) -> AutomationTools {
        AutomationTools {
            store,
            project_id: project.into(),
            project_name: project.into(),
            is_home,
            projects: vec![("P".into(), "P".into()), ("Q".into(), "Q".into())],
            caller: "CALLER-THREAD".into(),
            on_change: Box::new(|| {}),
            run_now: Box::new(|id| Ok(format!("thread-for-{id}"))),
            now,
        }
    }

    #[test]
    fn make_change_run_delete() {
        let store = Arc::new(Fake(Mutex::new(None)));
        let t = tools(store.clone(), "P", false);
        let made = t.call("create_automation", &json!({ "name": "PRs", "prompt": "Review open PRs", "schedule": "weekdays 9:00", "effort": "low", "thread": "new_settle_previous" }), None);
        assert!(!made.is_error, "{}", made.text);
        assert!(made.text.contains("Weekdays at 9:00") && made.text.contains("settling previous"), "{}", made.text);
        let bad = t.call("create_automation", &json!({ "name": "x", "prompt": "y", "schedule": "whenever" }), None);
        assert!(bad.is_error && bad.text.contains("Couldn't read"));
        let changed = t.call("update_automation", &json!({ "id": "PRs", "schedule": "every 2h", "thread": "this" }), None);
        assert!(changed.text.contains("Every 2 hours") && changed.text.contains("same thread"), "{}", changed.text);
        assert!(t.call("run_automation", &json!({ "id": "PRs" }), None).is_error);
        t.call("update_automation", &json!({ "id": "PRs", "thread": "new" }), None);
        assert!(t.call("run_automation", &json!({ "id": "prs" }), None).text.contains("Running"));
        let paused = t.call("update_automation", &json!({ "id": "PRs", "enabled": false }), None);
        assert!(paused.text.contains("paused"), "{}", paused.text);
        // Another project doesn't see project P's automation.
        assert!(tools(store.clone(), "Q", false).call("list_automations", &json!({}), None).text.contains("no automations"));
        assert!(tools(store.clone(), "Q", false).call("delete_automation", &json!({ "id": "PRs" }), None).is_error);
        // Global automation created from Home is visible and uses event trigger.
        let home = tools(store.clone(), "HOME", true);
        let g = home.call("create_automation", &json!({ "name": "Wake Briefing", "prompt": "Prep my day", "trigger": "wake", "cooldown": "12h" }), None);
        assert!(!g.is_error, "{}", g.text);
        assert!(g.text.contains("[Global]") && g.text.contains("Mac wakes"), "{}", g.text);
        assert!(t.call("list_automations", &json!({ "id": "PRs" }), None).text.contains("Review open PRs"));
        assert!(t.call("delete_automation", &json!({ "id": "PRs" }), None).text.contains("Deleted"));
        assert_eq!(automations::all(store.as_ref()).len(), 1);
    }
}
