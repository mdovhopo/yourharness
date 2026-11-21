//! A thread's agent process, whichever agent it runs: `claude` (stream-json, `claude.rs`) or
//! `agy acp` (ACP, `acp.rs`). Both report the same `claude::Event`s, so the engine and the
//! transcript don't care which; what one of them can't do is a no-op (or an error for
//! `request`).

use crate::acp::AcpProcess;
use crate::agents::Agent;
use crate::claude::{ClaudeProcess, Decision, Event, LaunchOptions};
use serde_json::Value;

pub enum AgentProcess {
    Claude(ClaudeProcess),
    Acp(AcpProcess),
}

impl AgentProcess {
    pub fn spawn(options: LaunchOptions, events: flume::Sender<Event>) -> Result<AgentProcess, String> {
        match options.agent {
            Agent::Claude => ClaudeProcess::spawn(options, events).map(AgentProcess::Claude),
            Agent::Antigravity => AcpProcess::spawn(options, events).map(AgentProcess::Acp),
        }
    }

    pub fn agent(&self) -> Agent {
        match self {
            AgentProcess::Claude(_) => Agent::Claude,
            AgentProcess::Acp(_) => Agent::Antigravity,
        }
    }

    pub fn pid(&self) -> u32 {
        match self {
            AgentProcess::Claude(p) => p.pid,
            AgentProcess::Acp(p) => p.pid,
        }
    }

    pub fn send(&self, text: &str, images: &[(String, String)], id: &str, priority: Option<&str>) {
        match self {
            AgentProcess::Claude(p) => p.send(text, images, id, priority),
            AgentProcess::Acp(p) => p.send(text, images, id, priority),
        }
    }

    pub fn initialize(&mut self) {
        match self {
            AgentProcess::Claude(p) => p.initialize(),
            AgentProcess::Acp(p) => p.initialize(),
        }
    }

    pub fn apply_settings(&mut self, settings: Value) {
        if let AgentProcess::Claude(p) = self {
            p.apply_settings(settings)
        }
    }

    pub fn query_settings(&mut self) {
        if let AgentProcess::Claude(p) = self {
            p.query_settings()
        }
    }

    /// A control request only `claude` answers (MCP status, `/context`…).
    pub fn request(&mut self, request: Value) -> flume::Receiver<Result<Value, String>> {
        match self {
            AgentProcess::Claude(p) => p.request(request),
            AgentProcess::Acp(_) => {
                let (tx, rx) = flume::bounded(1);
                let _ = tx.send(Err("Antigravity can't do that here.".into()));
                rx
            }
        }
    }

    pub fn respond_elicitation(&self, request_id: &str, action: &str, content: Option<Value>) {
        if let AgentProcess::Claude(p) = self {
            p.respond_elicitation(request_id, action, content)
        }
    }

    pub fn interrupt(&mut self) {
        match self {
            AgentProcess::Claude(p) => p.interrupt(),
            AgentProcess::Acp(p) => p.interrupt(),
        }
    }

    /// A waiting message sent now: the turn stops for it (`claude` takes it next by itself).
    pub fn send_now(&mut self, id: &str) {
        match self {
            AgentProcess::Claude(p) => p.interrupt(),
            AgentProcess::Acp(p) => p.send_now(id),
        }
    }

    pub fn stop_task(&mut self, task_id: &str) {
        if let AgentProcess::Claude(p) = self {
            p.stop_task(task_id)
        }
    }

    pub fn cancel_queued(&mut self, id: &str) {
        match self {
            AgentProcess::Claude(p) => p.cancel_queued(id),
            AgentProcess::Acp(p) => p.cancel_queued(id),
        }
    }

    pub fn set_mode(&mut self, mode: &str) {
        match self {
            AgentProcess::Claude(p) => p.set_mode(mode),
            AgentProcess::Acp(p) => p.set_mode(mode),
        }
    }

    pub fn set_model(&mut self, model: &str) {
        match self {
            AgentProcess::Claude(p) => p.set_model(model),
            AgentProcess::Acp(p) => p.set_model(model),
        }
    }

    pub fn respond(&self, request_id: &str, input: &str, decision: &Decision) {
        match self {
            AgentProcess::Claude(p) => p.respond(request_id, input, decision),
            AgentProcess::Acp(p) => p.respond(request_id, decision),
        }
    }

    pub fn terminate(&self) {
        match self {
            AgentProcess::Claude(p) => p.terminate(),
            AgentProcess::Acp(p) => p.terminate(),
        }
    }
}
