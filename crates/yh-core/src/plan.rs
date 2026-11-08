//! The checklist Claude keeps while it works, rebuilt from its task tool calls in order:
//! `TaskCreate` and `TaskUpdate` (Claude Code's task tools), or `TodoWrite` (the older tool,
//! which replaces the whole list each time).

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pending,
    InProgress,
    Completed,
}

impl Status {
    fn parse(s: &str) -> Option<Status> {
        match s {
            "pending" => Some(Status::Pending),
            "in_progress" => Some(Status::InProgress),
            "completed" => Some(Status::Completed),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanItem {
    pub id: String,
    pub subject: String,
    /// "Running the tests" for "Run the tests": how it reads while it's underway.
    pub active_form: Option<String>,
    pub status: Status,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub items: Vec<PlanItem>,
}

/// The calls that change or read the plan.
pub const TOOLS: &[&str] = &["TaskCreate", "TaskUpdate", "TaskList", "TaskGet", "TodoWrite"];

fn string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

impl Plan {
    pub fn completed(&self) -> usize {
        self.items.iter().filter(|i| i.status == Status::Completed).count()
    }

    pub fn is_finished(&self) -> bool {
        !self.items.is_empty() && self.items.iter().all(|i| i.status == Status::Completed)
    }

    /// What Claude is on now.
    pub fn current(&self) -> Option<&PlanItem> {
        self.items.iter().find(|i| i.status == Status::InProgress)
    }

    /// `result` is the call's output, which is where a new task's ID comes from.
    pub fn apply(&mut self, tool: &str, input: &str, result: Option<&str>, tool_use_id: &str) {
        let Ok(input) = serde_json::from_str::<Value>(input) else { return };
        match tool {
            "TaskCreate" => {
                let Some(subject) = input["subject"].as_str() else { return };
                // Its ID arrives with the result; until then the call stands in for it.
                let id = result.and_then(created_id).unwrap_or_else(|| tool_use_id.to_string());
                if self.items.iter().any(|i| i.id == id) {
                    return;
                }
                self.items.push(PlanItem {
                    id,
                    subject: subject.into(),
                    active_form: input["activeForm"].as_str().map(str::to_string),
                    status: Status::Pending,
                });
            }
            "TaskUpdate" => {
                let Some(id) = string(&input["taskId"]) else { return };
                let index = self.items.iter().position(|i| i.id == id);
                if input["status"] == "deleted" {
                    if let Some(i) = index {
                        self.items.remove(i);
                    }
                    return;
                }
                let status = input["status"].as_str().and_then(Status::parse);
                match index {
                    Some(i) => {
                        let item = &mut self.items[i];
                        if let Some(s) = status {
                            item.status = s;
                        }
                        if let Some(s) = input["subject"].as_str() {
                            item.subject = s.into();
                        }
                        if let Some(a) = input["activeForm"].as_str() {
                            item.active_form = Some(a.into());
                        }
                    }
                    None => {
                        // Made before this conversation began (task lists can outlive a session).
                        if let Some(subject) = input["subject"].as_str() {
                            self.items.push(PlanItem {
                                id,
                                subject: subject.into(),
                                active_form: input["activeForm"].as_str().map(str::to_string),
                                status: status.unwrap_or(Status::Pending),
                            });
                        }
                    }
                }
            }
            "TodoWrite" => {
                let Some(todos) = input["todos"].as_array() else { return };
                self.items = todos
                    .iter()
                    .enumerate()
                    .filter_map(|(n, t)| {
                        Some(PlanItem {
                            id: string(&t["id"]).unwrap_or_else(|| n.to_string()),
                            subject: t["content"].as_str()?.to_string(),
                            active_form: t["activeForm"].as_str().map(str::to_string),
                            status: t["status"].as_str().and_then(Status::parse).unwrap_or(Status::Pending),
                        })
                    })
                    .collect();
            }
            _ => {}
        }
    }
}

/// "Task #12 created successfully: …" → "12".
fn created_id(result: &str) -> Option<String> {
    let at = result.find("Task #")? + "Task #".len();
    let id: String = result[at..].chars().take_while(|c| !c.is_whitespace() && *c != ':').collect();
    (!id.is_empty()).then_some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_tools_build_the_checklist() {
        let mut p = Plan::default();
        p.apply("TaskCreate", r#"{"subject":"Read the code","activeForm":"Reading the code"}"#, Some("Task #1 created successfully: Read the code"), "t1");
        p.apply("TaskCreate", r#"{"subject":"Fix it"}"#, Some("Task #2 created successfully: Fix it"), "t2");
        p.apply("TaskUpdate", r#"{"taskId":"1","status":"completed"}"#, None, "t3");
        p.apply("TaskUpdate", r#"{"taskId":2,"status":"in_progress"}"#, None, "t4");
        assert_eq!(p.items.len(), 2);
        assert_eq!(p.completed(), 1);
        assert_eq!(p.current().unwrap().subject, "Fix it");
        p.apply("TaskUpdate", r#"{"taskId":"2","status":"deleted"}"#, None, "t5");
        assert_eq!(p.items.len(), 1);
        assert!(p.is_finished());
    }

    #[test]
    fn todo_write_replaces_the_list() {
        let mut p = Plan::default();
        p.apply("TodoWrite", r#"{"todos":[{"content":"A","status":"completed"},{"content":"B","status":"in_progress","activeForm":"Doing B"}]}"#, None, "t");
        assert_eq!(p.items.len(), 2);
        assert_eq!(p.current().unwrap().active_form.as_deref(), Some("Doing B"));
    }
}
