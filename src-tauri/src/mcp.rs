//! An MCP server that exposes OpenCode delegation as a tool.
//!
//! Run as `aiterm mcp`, this is a stdio [Model Context
//! Protocol](https://modelcontextprotocol.io) server speaking newline-delimited
//! JSON-RPC 2.0. A Claude Code session that adds it gets one tool,
//! `opencode_delegate`, and can hand a self-contained task to an OpenCode agent
//! running on the user's configured model — the same engine `opencode_dispatch`
//! drives (see [`crate::opencode_agent`]). This is "Claude drives OpenCode as a
//! subagent": Claude packages context into the task, OpenCode runs it
//! autonomously with tools, and its report comes back as the tool result.
//!
//! Why MCP and not ACP: a Claude Code session acquires tools over MCP (it is the
//! client, this is the server). ACP points the other way — it is for an editor
//! to host OpenCode's own interactive loop — so it would hand Claude nothing to
//! call. The actual OpenCode run happens through `opencode run` in the engine,
//! so none of ACP's session machinery is needed here.
//!
//! The transport is deliberately tiny: one request at a time, read a line, write
//! a line. Claude Code calls tools sequentially within a session, and a
//! delegated run is a foreground job by nature, so there is nothing to gain from
//! concurrency here and a simple loop is easier to trust.

use std::io::{BufRead, Write};

use serde_json::{json, Value};

/// The protocol version answered when a client does not name one.
const DEFAULT_PROTOCOL: &str = "2025-06-18";

/// Run the stdio server until stdin closes. Returns a process exit code.
pub fn run() -> i32 {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            // A malformed line gets a parse-error reply with a null id, per
            // JSON-RPC, rather than killing the server.
            Err(_) => {
                let _ = write_message(&mut out, &error(Value::Null, -32700, "parse error"));
                continue;
            }
        };
        if let Some(response) = handle(&request) {
            if write_message(&mut out, &response).is_err() {
                return 1;
            }
        }
    }
    0
}

/// Write one JSON-RPC message as a single line, then flush.
fn write_message(out: &mut impl Write, msg: &Value) -> std::io::Result<()> {
    writeln!(out, "{msg}")?;
    out.flush()
}

/// Handle one JSON-RPC message. `None` for notifications (no `id`), which are
/// acknowledged by silence.
///
/// Pure over its input except for `tools/call`, which runs the delegated agent
/// — so every routing and shape decision here is unit-testable without spawning
/// anything.
fn handle(request: &Value) -> Option<Value> {
    let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");

    // A message with no id is a notification: act on it if it matters, but never
    // answer. `notifications/initialized` is the client saying the handshake is
    // done; there is nothing to do but stay quiet — `?` returns `None` for it.
    let id = request.get("id").cloned()?;

    match method {
        "initialize" => Some(result(id, initialize_result(request))),
        "tools/list" => Some(result(id, json!({ "tools": [tool_schema()] }))),
        "tools/call" => Some(tools_call(id, request)),
        "ping" => Some(result(id, json!({}))),
        _ => Some(error(id, -32601, &format!("method not found: {method}"))),
    }
}

/// The `initialize` reply: echo the client's protocol version when it names one
/// (the compatibility-safe move), declare the one capability we have, and name
/// the server.
fn initialize_result(request: &Value) -> Value {
    let protocol = request
        .get("params")
        .and_then(|p| p.get("protocolVersion"))
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_PROTOCOL);
    json!({
        "protocolVersion": protocol,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "aiterm-opencode", "version": env!("CARGO_PKG_VERSION") }
    })
}

/// The one tool this server exposes.
fn tool_schema() -> Value {
    json!({
        "name": "opencode_delegate",
        "description": "Delegate a self-contained task to an OpenCode agent running on the \
            user's own configured model (e.g. GLM 5.2 via OpenRouter). OpenCode runs \
            autonomously with tools enabled in the given directory and returns its final \
            report. Fold any context the agent needs directly into `task`. Good for handing \
            off a chunk of work to a different model, a long autonomous tool-loop, or work to \
            run in parallel while you continue.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "The task for the agent, with any needed context included."
                },
                "cwd": {
                    "type": "string",
                    "description": "Absolute path of the directory the agent runs in."
                },
                "model": {
                    "type": "string",
                    "description": "Optional OpenRouter model id (e.g. 'z-ai/glm-5.2'). \
                        Defaults to the user's first configured startup model."
                },
                "provider": {
                    "type": "string",
                    "description": "Optional provider id. Defaults to 'openrouter'."
                }
            },
            "required": ["task", "cwd"]
        }
    })
}

/// Run `opencode_delegate`. Missing arguments and a failed run both come back as
/// tool errors (`isError: true`) rather than protocol errors: they are results
/// the model should see and can react to, not malformed calls.
fn tools_call(id: Value, request: &Value) -> Value {
    let args = request.get("params").and_then(|p| p.get("arguments"));
    let task = args.and_then(|a| a.get("task")).and_then(|t| t.as_str());
    let cwd = args.and_then(|a| a.get("cwd")).and_then(|c| c.as_str());
    let (Some(task), Some(cwd)) = (task, cwd) else {
        return result(
            id,
            tool_error("opencode_delegate needs both `task` and `cwd`"),
        );
    };

    let (default_provider, default_model) = crate::opencode_agent::default_target();
    let provider = args
        .and_then(|a| a.get("provider"))
        .and_then(|p| p.as_str())
        .map(String::from)
        .or(default_provider);
    let model = args
        .and_then(|a| a.get("model"))
        .and_then(|m| m.as_str())
        .map(String::from)
        .or(default_model);

    match crate::opencode_agent::dispatch(task, cwd, provider.as_deref(), model.as_deref()) {
        Ok(report) => result(
            id,
            json!({
                "content": [{ "type": "text", "text": report.text }],
                // The session id rides in structured content so a caller can
                // find the run in the sidebar or continue it later.
                "structuredContent": { "sessionId": report.session_id }
            }),
        ),
        Err(e) => result(id, tool_error(&format!("delegation failed: {e}"))),
    }
}

/// A tool-result payload flagged as an error.
fn tool_error(message: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": message }], "isError": true })
}

/// A JSON-RPC success envelope.
fn result(id: Value, value: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": value })
}

/// A JSON-RPC error envelope.
fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_echoes_the_clients_protocol_and_names_the_server() {
        let req = json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2024-11-05", "capabilities": {} }
        });
        let resp = handle(&req).unwrap();
        assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(resp["result"]["serverInfo"]["name"], "aiterm-opencode");
        assert_eq!(resp["result"]["capabilities"]["tools"], json!({}));
    }

    #[test]
    fn initialize_without_a_version_answers_the_default() {
        let req = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} });
        let resp = handle(&req).unwrap();
        assert_eq!(resp["result"]["protocolVersion"], DEFAULT_PROTOCOL);
    }

    #[test]
    fn tools_list_offers_exactly_the_delegate_tool() {
        let req = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });
        let resp = handle(&req).unwrap();
        let tools = resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "opencode_delegate");
        assert!(tools[0]["inputSchema"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r == "task"));
    }

    #[test]
    fn a_notification_is_answered_by_silence() {
        let req = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert!(handle(&req).is_none());
    }

    #[test]
    fn an_unknown_method_is_a_method_not_found_error() {
        let req = json!({ "jsonrpc": "2.0", "id": 9, "method": "resources/list" });
        let resp = handle(&req).unwrap();
        assert_eq!(resp["error"]["code"], -32601);
    }

    #[test]
    fn a_call_missing_arguments_is_a_tool_error_not_a_protocol_error() {
        let req = json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "opencode_delegate", "arguments": { "task": "hi" } }
        });
        let resp = handle(&req).unwrap();
        // A result envelope carrying isError, not a top-level JSON-RPC error.
        assert!(resp.get("error").is_none());
        assert_eq!(resp["result"]["isError"], true);
    }
}
