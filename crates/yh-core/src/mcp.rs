//! Tools the app serves to its agents itself, as MCP servers inside the app. Claude Code reaches
//! them over the stream-json control channel it already has with us (the SDK's in-process
//! servers: named in `initialize`'s `sdkMcpServers`, called with `mcp_message` control requests),
//! so there's no port, no token, and nothing else can call them. ACP agents (`agy acp`) reach the
//! same `ToolServer`s over a loopback HTTP bridge (`HttpBridge`), bound to `127.0.0.1:0` with a
//! per-session bearer token.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct ToolResult {
    pub text: String,
    pub is_error: bool,
    /// Pictures before the text: (media type, base64).
    pub images: Vec<(String, String)>,
}

impl ToolResult {
    pub fn text(text: impl Into<String>) -> ToolResult {
        ToolResult { text: text.into(), is_error: false, images: vec![] }
    }

    pub fn error(text: impl Into<String>) -> ToolResult {
        ToolResult { text: text.into(), is_error: true, images: vec![] }
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

/// Called before `ToolServer::call` when a request doesn't carry `claudecode/toolUseId` (ACP
/// calls): `(server, tool, arguments) -> Option<tool_use_id>`.
pub type CallHook = Arc<dyn Fn(&str, &str, &Value) -> Option<String> + Send + Sync>;
/// Called right after `ToolServer::call` with its `tool_use_id` and result.
pub type ResultHook = Arc<dyn Fn(&str, &ToolResult) + Send + Sync>;

/// The reply to one JSON-RPC message; `None` for a notification.
pub fn handle(server: &dyn ToolServer, message: &Value) -> Option<Value> {
    handle_with_hooks(server, message, None, None)
}

/// Like `handle`, with hooks to resolve a `tool_use_id` and observe the `ToolResult`.
pub fn handle_with_hooks(server: &dyn ToolServer, message: &Value, on_call: Option<&CallHook>, on_result: Option<&ResultHook>) -> Option<Value> {
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
            let hook_id = if params["_meta"]["claudecode/toolUseId"].as_str().is_none() {
                on_call.and_then(|f| f(server.name(), name, &args))
            } else {
                None
            };
            let tool_use_id = params["_meta"]["claudecode/toolUseId"].as_str().or(hook_id.as_deref());
            let result = server.call(name, &args, tool_use_id);
            if let (Some(id), Some(cb)) = (tool_use_id, on_result) {
                cb(id, &result);
            }
            let mut content: Vec<Value> = result.images.iter().map(|(media, data)| json!({ "type": "image", "data": data, "mimeType": media })).collect();
            content.push(json!({ "type": "text", "text": result.text }));
            reply(json!({ "content": content, "isError": result.is_error }))
        }
        method => failure(-32601, format!("Method not found: {method}")),
    })
}

/// Cap on one HTTP request's headers or body, so a bad client can't eat memory.
const MAX_HEADERS: usize = 64 * 1024;
const MAX_BODY: usize = 64 * 1024 * 1024;

/// A loopback Streamable HTTP bridge serving in-process `ToolServer`s to one ACP process.
/// Bound to `127.0.0.1:0` with a random 256-bit bearer token; closed on `shutdown` or `Drop`.
pub struct HttpBridge {
    port: u16,
    token: String,
    names: Vec<String>,
    shutdown: Arc<AtomicBool>,
}

impl HttpBridge {
    pub fn start(servers: Vec<Arc<dyn ToolServer>>, on_call: Option<CallHook>, on_result: Option<ResultHook>) -> std::io::Result<HttpBridge> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let token = format!("{}{}", crate::library::new_uuid(), crate::library::new_uuid());
        let map: Arc<HashMap<String, Arc<dyn ToolServer>>> = Arc::new(servers.into_iter().map(|s| (s.name().to_string(), s)).collect());
        let mut names: Vec<String> = map.keys().cloned().collect();
        names.sort();
        let shutdown = Arc::new(AtomicBool::new(false));
        let expected = Arc::new(format!("Bearer {token}"));
        {
            let shutdown = shutdown.clone();
            std::thread::spawn(move || {
                for incoming in listener.incoming() {
                    if shutdown.load(Ordering::Acquire) {
                        break;
                    }
                    let Ok(stream) = incoming else { continue };
                    let (map, expected, on_call, on_result, shutdown) = (map.clone(), expected.clone(), on_call.clone(), on_result.clone(), shutdown.clone());
                    std::thread::spawn(move || serve_connection(stream, &map, &expected, on_call.as_ref(), on_result.as_ref(), &shutdown));
                }
            });
        }
        Ok(HttpBridge { port, token, names, shutdown })
    }

    /// ACP `mcpServers` entries (`type: "http"`) for `session/new`, `session/resume`, `session/load`.
    pub fn acp_configs(&self) -> Vec<Value> {
        self.names
            .iter()
            .map(|name| {
                json!({
                    "type": "http",
                    "name": name,
                    "url": format!("http://127.0.0.1:{}/mcp/{name}", self.port),
                    "headers": [{ "name": "Authorization", "value": format!("Bearer {}", self.token) }],
                })
            })
            .collect()
    }

    pub fn shutdown(&self) {
        if !self.shutdown.swap(true, Ordering::AcqRel) {
            let _ = TcpStream::connect(("127.0.0.1", self.port));
        }
    }
}

impl Drop for HttpBridge {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn write_response(stream: &mut TcpStream, status: &str, headers: &[(&str, &str)], body: &[u8], close: bool) -> bool {
    let mut head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n", body.len());
    for (k, v) in headers {
        head.push_str(k);
        head.push_str(": ");
        head.push_str(v);
        head.push_str("\r\n");
    }
    if close {
        head.push_str("Connection: close\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).is_ok() && stream.write_all(body).is_ok() && stream.flush().is_ok()
}

fn read_chunked(reader: &mut BufReader<TcpStream>) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let hex = line.trim().split(';').next().unwrap_or("");
        let size = usize::from_str_radix(hex, 16).ok()?;
        if size == 0 {
            // Trailing headers, if any, end with an empty line.
            loop {
                line.clear();
                if reader.read_line(&mut line).ok()? == 0 || line == "\r\n" || line == "\n" {
                    break;
                }
            }
            return Some(out);
        }
        if out.len().saturating_add(size) > MAX_BODY {
            return None;
        }
        let start = out.len();
        out.resize(start + size, 0);
        reader.read_exact(&mut out[start..]).ok()?;
        line.clear();
        let _ = reader.read_line(&mut line);
    }
}

fn serve_connection(
    stream: TcpStream,
    servers: &HashMap<String, Arc<dyn ToolServer>>,
    expected_auth: &str,
    on_call: Option<&CallHook>,
    on_result: Option<&ResultHook>,
    shutdown: &AtomicBool,
) {
    let Ok(mut writer) = stream.try_clone() else { return };
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let mut parts = line.trim_end_matches(['\r', '\n']).split_whitespace();
        let (Some(method), Some(raw_path), Some(version)) = (parts.next(), parts.next(), parts.next()) else {
            write_response(&mut writer, "400 Bad Request", &[], b"Bad request", true);
            break;
        };
        let method = method.to_string();
        let path = raw_path.split('?').next().unwrap_or(raw_path).to_string();
        let mut close = version == "HTTP/1.0";
        let mut auth: Option<String> = None;
        let mut content_length: usize = 0;
        let mut chunked = false;
        let mut header_bytes = line.len();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(n) => header_bytes += n,
            }
            if header_bytes > MAX_HEADERS {
                write_response(&mut writer, "431 Request Header Fields Too Large", &[], b"", true);
                return;
            }
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if trimmed.is_empty() {
                break;
            }
            if let Some((k, v)) = trimmed.split_once(':') {
                let v = v.trim();
                if k.eq_ignore_ascii_case("authorization") {
                    auth = Some(v.to_string());
                } else if k.eq_ignore_ascii_case("content-length") {
                    content_length = v.parse().unwrap_or(0);
                } else if k.eq_ignore_ascii_case("transfer-encoding") {
                    chunked = v.to_ascii_lowercase().contains("chunked");
                } else if k.eq_ignore_ascii_case("connection") && v.to_ascii_lowercase().contains("close") {
                    close = true;
                }
            }
        }
        if auth.as_deref() != Some(expected_auth) {
            write_response(&mut writer, "401 Unauthorized", &[], b"Unauthorized", true);
            break;
        }
        let body = if chunked {
            match read_chunked(&mut reader) {
                Some(b) => b,
                None => {
                    write_response(&mut writer, "400 Bad Request", &[], b"Bad chunked body", true);
                    break;
                }
            }
        } else if content_length > MAX_BODY {
            write_response(&mut writer, "413 Payload Too Large", &[], b"", true);
            break;
        } else if content_length > 0 {
            let mut buf = vec![0u8; content_length];
            if reader.read_exact(&mut buf).is_err() {
                break;
            }
            buf
        } else {
            Vec::new()
        };
        let Some(server) = path.strip_prefix("/mcp/").and_then(|name| servers.get(name)) else {
            if !write_response(&mut writer, "404 Not Found", &[], b"Unknown server", close) || close {
                break;
            }
            continue;
        };
        let ok = match method.as_str() {
            "GET" => write_response(&mut writer, "405 Method Not Allowed", &[("Allow", "POST, DELETE")], b"", close),
            "DELETE" => write_response(&mut writer, "200 OK", &[], b"", close),
            "POST" => match serde_json::from_slice::<Value>(&body) {
                Ok(message) => match handle_with_hooks(server.as_ref(), &message, on_call, on_result) {
                    Some(reply) => {
                        let bytes = reply.to_string();
                        write_response(&mut writer, "200 OK", &[("Content-Type", "application/json")], bytes.as_bytes(), close)
                    }
                    None => write_response(&mut writer, "202 Accepted", &[], b"", close),
                },
                Err(_) => write_response(&mut writer, "400 Bad Request", &[], b"Invalid JSON", close),
            },
            _ => write_response(&mut writer, "405 Method Not Allowed", &[("Allow", "POST, DELETE")], b"", close),
        };
        if !ok || close {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct EchoServer;
    impl ToolServer for EchoServer {
        fn name(&self) -> &str {
            "memory"
        }
        fn tools(&self) -> Vec<Value> {
            vec![tool("echo", "Echoes note", schema(json!({ "note": { "type": "string" } }), &["note"]), true, false)]
        }
        fn call(&self, _tool: &str, args: &Value, tool_use_id: Option<&str>) -> ToolResult {
            ToolResult::text(format!("{}|{}", args["note"].as_str().unwrap_or(""), tool_use_id.unwrap_or("none")))
        }
    }

    fn http_roundtrip(port: u16, raw: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(raw.as_bytes()).unwrap();
        s.flush().unwrap();
        let mut resp = String::new();
        s.read_to_string(&mut resp).unwrap();
        resp
    }

    #[test]
    fn http_bridge_authenticates_routes_and_runs_hooks() {
        let seen: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
        let seen_res = seen.clone();
        let on_call: CallHook = Arc::new(|server, tool, _| Some(format!("call-{server}-{tool}")));
        let on_result: ResultHook = Arc::new(move |id, res| {
            seen_res.lock().unwrap().push((id.to_string(), res.text.clone()));
        });
        let bridge = HttpBridge::start(vec![Arc::new(EchoServer)], Some(on_call), Some(on_result)).unwrap();
        let configs = bridge.acp_configs();
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0]["type"], "http");
        assert_eq!(configs[0]["name"], "memory");
        let auth = configs[0]["headers"][0]["value"].as_str().unwrap().to_string();

        // Wrong token is refused.
        let unauth = http_roundtrip(bridge.port, "POST /mcp/memory HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer nope\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
        assert!(unauth.starts_with("HTTP/1.1 401 "), "{unauth}");

        // Notification gets 202.
        let notif_body = r#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#;
        let notif = http_roundtrip(
            bridge.port,
            &format!("POST /mcp/memory HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: {auth}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{notif_body}", notif_body.len()),
        );
        assert!(notif.starts_with("HTTP/1.1 202 "), "{notif}");

        // tools/call runs with hook id and reports result to on_result.
        let call_body = r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"echo","arguments":{"note":"hi"}}}"#;
        let resp = http_roundtrip(
            bridge.port,
            &format!("POST /mcp/memory HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: {auth}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{call_body}", call_body.len()),
        );
        assert!(resp.starts_with("HTTP/1.1 200 "), "{resp}");
        let body: Value = serde_json::from_str(resp.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["id"], 7);
        assert_eq!(body["result"]["content"][0]["text"], "hi|call-memory-echo");
        assert_eq!(*seen.lock().unwrap(), vec![("call-memory-echo".to_string(), "hi|call-memory-echo".to_string())]);

        // Chunked request also works.
        let chunk_body = r#"{"jsonrpc":"2.0","id":8,"method":"ping"}"#;
        let chunked = http_roundtrip(
            bridge.port,
            &format!("POST /mcp/memory HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: {auth}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{chunk_body}\r\n0\r\n\r\n", chunk_body.len()),
        );
        assert!(chunked.starts_with("HTTP/1.1 200 "), "{chunked}");
        assert!(chunked.contains(r#""id":8"#));
    }
}
