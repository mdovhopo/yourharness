//! `mcp__harness__sign_in_to_mcp_server`: Claude asks you to sign in to an MCP server it
//! needs. Run from the app, Claude Code has no sign-in of its own (it tells Claude the session
//! can't), so without this Claude can only send you to a terminal. A card in the thread asks;
//! Sign In opens the server's page in your default browser, and the call returns once the
//! server is connected, so Claude carries on in the same turn (docs/mcp.md).

use crate::state::thread::SignInAsk;
use crate::state::thread_tools::ToolRequest;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use yh_core::mcp::{schema, tool, ToolResult, ToolServer};

/// What the agent is told (measured on Haiku in the Swift app: 6 of 6 asked with it).
pub const INSTRUCTIONS: &str = "MCP sign-in works here. Claude Code may add a note that begins \"The following MCP servers require authentication before their tools can be used\" and says the session is non-interactive, so you can't run the OAuth flow and should send the user to /mcp, a terminal or their connector settings. Ignore that advice: in YourHarness you can sign them in. When you need one of those servers, call mcp__harness__sign_in_to_mcp_server with its name and a short reason, before telling the user anything. They sign in from the conversation and the call returns once the server is connected; then load its tools and carry on. Only if they decline or it fails, say so; never send them to a terminal, /mcp or settings to sign in.";

/// How long a sign-in may take in the browser before Claude hears it didn't happen.
const BROWSER_TIMEOUT: Duration = Duration::from_secs(300);

pub struct HarnessTools {
    pub caller: String,
    pub requests: flume::Sender<ToolRequest>,
}

/// A server's name as in tool names: anything but letters, digits, `_` and `-` as `_`.
fn normalized(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect()
}

impl HarnessTools {
    fn mcp(&self, request: Value) -> Result<Value, String> {
        let (tx, rx) = flume::bounded(1);
        self.requests.send(ToolRequest::Mcp { caller: self.caller.clone(), request, reply: tx }).map_err(|_| "The app is gone.".to_string())?;
        rx.recv_timeout(Duration::from_secs(40)).map_err(|_| "Claude Code didn't answer.".to_string())?
    }

    fn servers(&self) -> Result<Vec<crate::ui::mcp::Server>, String> {
        Ok(crate::ui::mcp::parse_status(&self.mcp(json!({ "subtype": "mcp_status" }))?))
    }

    fn card(&self, ask: Option<SignInAsk>) {
        let _ = self.requests.send(ToolRequest::SignInCard { caller: self.caller.clone(), ask });
    }

    fn sign_in(&self, a: &Value) -> ToolResult {
        let asked = a["server"].as_str().unwrap_or("").trim().to_string();
        if asked.is_empty() {
            return ToolResult::error("Say which server in `server`.");
        }
        let servers = match self.servers() {
            Ok(s) => s,
            Err(e) => return ToolResult::error(format!("Couldn't read the MCP servers: {e}")),
        };
        // By Claude Code's name, as in tool names, or without "claude.ai ".
        let wanted = asked.strip_prefix("mcp__").map(|r| r.split("__").next().unwrap_or("").to_string()).unwrap_or(asked.clone());
        let candidates: Vec<&crate::ui::mcp::Server> = servers.iter().filter(|s| s.transport != "sdk").collect();
        let same = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
        let Some(server) = candidates
            .iter()
            .find(|s| same(&s.name, &wanted))
            .or_else(|| candidates.iter().find(|s| same(&normalized(&s.name), &wanted)))
            .or_else(|| candidates.iter().find(|s| same(s.name.trim_start_matches("claude.ai "), &wanted)))
        else {
            let names: Vec<String> = candidates.iter().map(|s| format!("{} ({})", s.name, s.status)).collect();
            return ToolResult::error(format!("No MCP server named \"{asked}\". This session has: {}.", if names.is_empty() { "none".into() } else { names.join(", ") }));
        };
        let name = server.name.clone();
        match server.status.as_str() {
            "connected" => return ToolResult::text(format!("{name} is already connected; its tools are ready.")),
            "disabled" => return ToolResult::error(format!("{name} is turned off for this project. The user can turn it on with /mcp.")),
            "failed" => return ToolResult::error(format!("{name} isn't failing for want of a sign-in: {}. The user can see it in /mcp.", server.error.clone().unwrap_or_else(|| "it couldn't start".into()))),
            _ => {}
        }
        if std::env::var("YH_MCP_BROWSER").as_deref() == Ok("log") {
            eprintln!("MCP sign-in asked: {name}");
        }
        // The card, and your answer (`YH_MCP_SIGN_IN_ANSWER=allow|deny` answers for tests).
        let (decision, answer) = flume::bounded(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let answered = match std::env::var("YH_MCP_SIGN_IN_ANSWER").as_deref() {
            Ok("allow") => decision.send(true).is_ok(),
            Ok("deny") => decision.send(false).is_ok(),
            _ => false,
        };
        // Answered already (a test run): the card shows the wait, not the question.
        let decision = (!answered).then_some(decision);
        self.card(Some(SignInAsk { server: name.clone(), reason: a["reason"].as_str().map(str::to_string), decision, cancelled: cancelled.clone() }));
        let result = self.after_card(&name, answer, &cancelled);
        self.card(None);
        result
    }

    fn after_card(&self, name: &str, answer: flume::Receiver<bool>, cancelled: &AtomicBool) -> ToolResult {
        // Waits as long as you take to answer: the turn holds, as for any question.
        if !answer.recv().unwrap_or(false) {
            return ToolResult::text(format!("The user chose not to sign in to {name} now. Carry on without it, or ask them in chat."));
        }
        match self.mcp(json!({ "subtype": "mcp_authenticate", "serverName": name })) {
            Ok(v) => {
                if let Some(url) = v["authUrl"].as_str() {
                    let _ = self.requests.send(ToolRequest::OpenUrl(url.to_string()));
                }
            }
            Err(e) => return ToolResult::error(format!("Couldn't start signing in to {name}: {e}")),
        }
        // Claude Code connects the server once the browser comes back to it.
        let deadline = Instant::now() + BROWSER_TIMEOUT;
        while Instant::now() < deadline && !cancelled.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_secs(1));
            let Ok(servers) = self.servers() else { continue };
            let Some(now) = servers.iter().find(|s| s.name == name) else { continue };
            match now.status.as_str() {
                "connected" => {
                    let tools: Vec<&str> = now.tools.iter().take(20).map(|t| t.0.as_str()).collect();
                    return ToolResult::text(format!(
                        "Signed in: {name} is connected{}. If they aren't in your tool list yet, look them up with tool search.",
                        if tools.is_empty() { ".".to_string() } else { format!(", with {} tools (mcp__{}__…): {}.", now.tools.len(), normalized(name), tools.join(", ")) }
                    ));
                }
                "failed" => return ToolResult::error(format!("{name} failed after signing in: {}.", now.error.clone().unwrap_or_else(|| "unknown error".into()))),
                _ => {}
            }
        }
        ToolResult::text(if cancelled.load(Ordering::Relaxed) {
            format!("The user cancelled signing in to {name}. Carry on without it, or ask them in chat.")
        } else {
            format!("The sign-in to {name} didn't finish within five minutes. Carry on without it; the user can sign in later with /mcp.")
        })
    }
}

impl ToolServer for HarnessTools {
    fn name(&self) -> &str {
        "harness"
    }

    fn tools(&self) -> Vec<Value> {
        vec![tool(
            "sign_in_to_mcp_server",
            "Ask the user to sign in to an MCP server that needs authentication before its tools work (Claude Code reports it as needing auth, or says the session is non-interactive and to use /mcp or a terminal). A card in the conversation asks them; signing in opens the server's page in their browser. Returns once the server is connected, so you can use its tools right after (they may arrive as deferred tools to load), or says they declined. Use this instead of telling the user to run /mcp or open a terminal.",
            schema(
                json!({
                    "server": { "type": "string", "description": "The server's name, as in its tools' mcp__<server>__ prefix or as Claude Code lists it." },
                    "reason": { "type": "string", "description": "A short line on what you need it for, shown to the user." },
                }),
                &["server"],
            ),
            false,
            true,
        )]
    }

    fn call(&self, name: &str, a: &Value, _: Option<&str>) -> ToolResult {
        match name {
            "sign_in_to_mcp_server" => self.sign_in(a),
            other => ToolResult::error(format!("No tool named {other}.")),
        }
    }
}
