//! Notifications about threads you aren't looking at, as the Swift app's `Notifier` posts them:
//! the thread's title, its project as the subtitle, what happened as the body, with a sound.
//! A finished turn only while the app is in the background (the sidebar marks it otherwise);
//! Claude waiting for you whenever it's another thread. Clicking one opens its thread; opening a
//! thread clears its notifications.

use crate::state::thread::{ItemType, ThreadState};
use crate::state::workspace::{Workspace, SIDE};
use gpui_kit::{Context, Entity};
use serde_json::Value;

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Finished,
    NeedsYou,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Finished => "finished",
            Kind::NeedsYou => "needsYou",
        }
    }
}

impl Workspace {
    /// Clicks on the app's notifications (and `YH_NOTIFICATION_CLICK`'s pretend ones) open
    /// their thread.
    pub fn listen_for_notification_clicks(&mut self, cx: &mut Context<Self>) {
        let clicks = crate::platform::notification_clicks();
        cx.spawn(async move |this, cx| {
            while let Ok(id) = clicks.recv_async().await {
                if this.update(cx, |ws, cx| ws.open_from_notification(&id, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn open_from_notification(&mut self, id: &str, cx: &mut Context<Self>) {
        let id = id.strip_prefix(SIDE).unwrap_or(id).to_string();
        if std::env::var("YH_NOTIFICATIONS").as_deref() == Ok("log") {
            eprintln!("NOTIFY clicked: {id}");
        }
        let Some(thread) = self.thread(&id) else { return };
        if thread.read(cx).meta.settled_at.is_some() {
            self.set_settled(&id, false, cx);
        }
        self.select(&id, cx);
        crate::platform::bring_to_front();
    }

    /// Tells you about `thread` unless you're looking at it.
    pub fn notify_about(&self, kind: Kind, thread: &Entity<ThreadState>, body: String, cx: &Context<Self>) {
        let t = thread.read(cx);
        if !self.library.setting("notifications").and_then(|v| v.as_bool()).unwrap_or(true) {
            return;
        }
        let id = t.id().strip_prefix(SIDE).unwrap_or(t.id()).to_string();
        let active = crate::platform::app_is_active();
        let looking = active && self.selected.as_deref() == Some(id.as_str());
        if looking || (kind == Kind::Finished && active) {
            return;
        }
        let title = self.thread(&id).map(|p| p.read(cx).title.clone()).unwrap_or_else(|| t.title.clone());
        let subtitle = self.project(&t.meta.project_id).map(|p| p.name.clone()).unwrap_or_default();
        crate::platform::notify_thread(kind.name(), &id, &title, &subtitle, &body);
    }

    /// What a request to you is, as a notification's body.
    pub fn describe_request(thread: &ThreadState) -> String {
        if thread.elicitation.is_some() && !thread.waiting_for_you() {
            return "An MCP server is asking you something.".into();
        }
        // The newest request: the one that just came.
        let Some(p) = thread.permissions.last() else { return "Claude is waiting for you.".into() };
        let input: Value = serde_json::from_str(&p.input).unwrap_or(Value::Null);
        match p.tool.as_str() {
            "AskUserQuestion" => input["questions"][0]["question"].as_str().map(|q| format!("Claude asks: {q}")).unwrap_or_else(|| "Claude has a question for you.".into()),
            "ExitPlanMode" => "Claude has a plan ready for you to review.".into(),
            tool => {
                let detail = ["command", "file_path", "path", "url", "pattern", "description"].iter().find_map(|k| input[*k].as_str().map(str::to_string));
                let verb = match tool {
                    "Bash" => "run command",
                    "Edit" | "MultiEdit" => "edit",
                    "Write" => "write",
                    "Read" => "read",
                    "WebFetch" => "fetch",
                    "WebSearch" => "search the web",
                    other => other,
                };
                let detail = detail.map(|d| format!(": {}", truncate(&d, 120))).unwrap_or_default();
                format!("Allow {verb}{detail}?")
            }
        }
    }

    /// The turn's last reply, as a notification's line or two.
    pub fn last_reply(thread: &ThreadState) -> String {
        let reply = thread
            .items
            .iter()
            .rev()
            .take_while(|i| i.ty != ItemType::User)
            .filter(|i| i.ty == ItemType::Text)
            .find_map(|i| i.entries.first().map(|e| e.borrow().data.text.clone()).filter(|t| !t.trim().is_empty()));
        let Some(reply) = reply else { return "Claude finished.".into() };
        let plain: String = reply.chars().filter(|c| !"*_`#>".contains(*c)).collect();
        let plain = plain.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
        truncate(&plain, 180)
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() > n { s.chars().take(n).collect::<String>() + "…" } else { s.to_string() }
}
