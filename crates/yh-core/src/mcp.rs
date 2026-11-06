//! Tools the app serves to its agents itself, as MCP servers inside the app. Claude Code reaches
//! them over the stream-json control channel it already has with us (the SDK's in-process
//! servers: named in `initialize`'s `sdkMcpServers`, called with `mcp_message` control requests),
//! so there's no port, no token, and nothing else can call them.

use serde_json::{json, Value};

pub struct ToolResult {
    pub text: String,
    pub is_error: bool,
}

impl ToolResult {
    pub fn text(text: impl Into<String>) -> ToolResult {
        ToolResult { text: text.into(), is_error: false }
    }

    pub fn error(text: impl Into<String>) -> ToolResult {
        ToolResult { text: text.into(), is_error: true }
    }
}

pub trait ToolServer: Send + Sync {
    /// What the agent sees: tools are `mcp__<name>__<tool>`.
    fn name(&self) -> &str;
    /// Each tool's definition (see `tool`).
    fn tools(&self) -> Vec<Value>;
    /// Runs on a thread of its own; `tool_use_id` ties the call to its transcript row.
    fn call(&self, tool: &str, arguments: &Value, tool_use_id: Option<&str>) -> ToolResult;
}

/// A tool's definition. `read_only` is shown as `readOnlyHint`; `always_load` keeps it in the
/// agent's tool list rather than deferred behind Claude Code's tool search.
pub fn tool(name: &str, description: &str, schema: Value, read_only: bool, always_load: bool) -> Value {
    let mut t = json!({ "name": name, "description": description, "inputSchema": schema, "annotations": { "readOnlyHint": read_only } });
    if always_load {
        t["_meta"] = json!({ "anthropic/alwaysLoad": true });
    }
    t
}

/// An object schema from its properties.
pub fn schema(properties: Value, required: &[&str]) -> Value {
    let mut s = json!({ "type": "object", "properties": properties, "additionalProperties": false });
    if !required.is_empty() {
        s["required"] = json!(required);
    }
    s
}

/// The reply to one JSON-RPC message; `None` for a notification.
pub fn handle(server: &dyn ToolServer, message: &Value) -> Option<Value> {
    let id = message.get("id").filter(|v| !v.is_null())?.clone();
    let reply = |result: Value| json!({ "jsonrpc": "2.0", "id": id, "result": result });
    let failure = |code: i64, text: String| json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": text } });
    Some(match message["method"].as_str().unwrap_or("") {
        "initialize" => reply(json!({
            "protocolVersion": message["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": server.name(), "version": "1.0" },
        })),
        "ping" => reply(json!({})),
        "tools/list" => reply(json!({ "tools": server.tools() })),
        "tools/call" => {
            let params = &message["params"];
            let Some(name) = params["name"].as_str() else { return Some(failure(-32602, "Missing tool name".into())) };
            if !server.tools().iter().any(|t| t["name"] == name) {
                return Some(failure(-32602, format!("Unknown tool {name}")));
            }
            let args = if params["arguments"].is_object() { params["arguments"].clone() } else { json!({}) };
            let result = server.call(name, &args, params["_meta"]["claudecode/toolUseId"].as_str());
            reply(json!({ "content": [{ "type": "text", "text": result.text }], "isError": result.is_error }))
        }
        method => failure(-32601, format!("Method not found: {method}")),
    })
}
