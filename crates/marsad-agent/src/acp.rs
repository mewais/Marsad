//! ACP (Agent Client Protocol) — pure JSON-RPC 2.0 types and codec.
//!
//! Transport-agnostic, unit-testable. The spec is docs/specs/opencode-acp.md §2/§8.
//!
//! All messages are newline-free JSON strings; the caller appends `\n`.

use serde_json::{json, Value};

// ── Error ────────────────────────────────────────────────────────────────────

/// Parse or encode failure.
#[derive(Debug, Clone, PartialEq)]
pub enum AcpError {
    /// The input is not valid JSON.
    InvalidJson(String),
    /// The JSON is valid but does not match any ACP message shape.
    UnrecognizedShape(String),
}

impl std::fmt::Display for AcpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AcpError::InvalidJson(s) => write!(f, "invalid ACP JSON: {s}"),
            AcpError::UnrecognizedShape(s) => write!(f, "unrecognized ACP shape: {s}"),
        }
    }
}

impl std::error::Error for AcpError {}

// ── AcpMessage ───────────────────────────────────────────────────────────────

/// A discriminated JSON-RPC 2.0 message as used by the ACP protocol.
///
/// - `Request`      — has `id` + `method` + `params`; caller expects a `Response`.
/// - `Response`     — has `id` + either `result` or `error`; answers a `Request`.
/// - `Notification` — has `method` + `params`, no `id`; fire-and-forget.
#[derive(Debug, Clone, PartialEq)]
pub enum AcpMessage {
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Response {
        id: Value,
        /// `Ok(result)` on success, `Err(error_obj)` on JSON-RPC error.
        result: Result<Value, Value>,
    },
    Notification {
        method: String,
        params: Value,
    },
}

// ── Parsing and encoding ─────────────────────────────────────────────────────

/// Parse one newline-delimited JSON-RPC line into an `AcpMessage`.
///
/// Returns `Err(AcpError::InvalidJson)` when the line is not valid JSON,
/// or `Err(AcpError::UnrecognizedShape)` when the JSON does not match any
/// of the three ACP message shapes (Request / Response / Notification).
pub fn parse_acp_line(line: &str) -> Result<AcpMessage, AcpError> {
    // Parse into a JSON value first.
    let v: Value = serde_json::from_str(line)
        .map_err(|e| AcpError::InvalidJson(e.to_string()))?;

    // Must be an object.
    let obj = match v.as_object() {
        Some(o) => o,
        None => return Err(AcpError::UnrecognizedShape(format!("not a JSON object: {line}"))),
    };

    // Strict JSON-RPC 2.0: must have "jsonrpc":"2.0".
    match obj.get("jsonrpc").and_then(|v| v.as_str()) {
        Some("2.0") => {}
        _ => {
            return Err(AcpError::UnrecognizedShape(format!(
                "missing or wrong jsonrpc version: {line}"
            )));
        }
    }

    // Discriminate by which fields are present:
    //   Request:      has id + method (+ optional params)
    //   Response:     has id + result OR id + error
    //   Notification: has method, no id
    let has_id = obj.contains_key("id");
    let has_method = obj.contains_key("method");
    let has_result = obj.contains_key("result");
    let has_error = obj.contains_key("error");

    if has_id && has_method && !has_result && !has_error {
        // Request
        let id = obj["id"].clone();
        let method = obj["method"]
            .as_str()
            .ok_or_else(|| AcpError::UnrecognizedShape("method is not a string".to_string()))?
            .to_string();
        let params = obj.get("params").cloned().unwrap_or(Value::Null);
        Ok(AcpMessage::Request { id, method, params })
    } else if has_id && has_result && !has_method {
        // Success response
        let id = obj["id"].clone();
        let result = obj["result"].clone();
        Ok(AcpMessage::Response { id, result: Ok(result) })
    } else if has_id && has_error && !has_method {
        // Error response
        let id = obj["id"].clone();
        let error = obj["error"].clone();
        Ok(AcpMessage::Response { id, result: Err(error) })
    } else if !has_id && has_method {
        // Notification
        let method = obj["method"]
            .as_str()
            .ok_or_else(|| AcpError::UnrecognizedShape("method is not a string".to_string()))?
            .to_string();
        let params = obj.get("params").cloned().unwrap_or(Value::Null);
        Ok(AcpMessage::Notification { method, params })
    } else {
        Err(AcpError::UnrecognizedShape(format!(
            "cannot classify message (has_id={has_id}, has_method={has_method}, has_result={has_result}, has_error={has_error}): {line}"
        )))
    }
}

/// Encode a JSON-RPC 2.0 **request** as a newline-free JSON string.
///
/// `id` is an opaque correlation id (typically a monotonic counter or UUID).
/// The caller appends `\n`.
pub fn encode_request(id: Value, method: &str, params: Value) -> String {
    let obj = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    serde_json::to_string(&obj).expect("serde_json::to_string never fails on a json! value")
}

/// Encode a JSON-RPC 2.0 **response** (success) as a newline-free JSON string.
///
/// The caller appends `\n`.
pub fn encode_response(id: Value, result: Value) -> String {
    let obj = json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    });
    serde_json::to_string(&obj).expect("serde_json::to_string never fails on a json! value")
}

/// Encode a JSON-RPC 2.0 **notification** as a newline-free JSON string.
///
/// Notifications have no `id` and expect no reply. The caller appends `\n`.
pub fn encode_notification(method: &str, params: Value) -> String {
    let obj = json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    });
    serde_json::to_string(&obj).expect("serde_json::to_string never fails on a json! value")
}

// ── Typed param constructors (client→agent) ──────────────────────────────────

/// Build the `params` object for an `initialize` request.
///
/// `client_name`/`client_version` identify Marsad; we DECLINE fs + terminal
/// so opencode never sends fs/* or terminal/* to us.
pub fn initialize_params(client_name: &str, client_version: &str) -> Value {
    json!({
        // ACP protocolVersion is an INTEGER, not a string. opencode validates it
        // strictly: a string "1" is rejected with -32602 "Invalid params: expected
        // number, received string", breaking the whole handshake. Verified against
        // opencode 1.18: the integer 1 makes initialize succeed.
        "protocolVersion": 1,
        "clientInfo": {
            "name": client_name,
            "version": client_version,
        },
        "clientCapabilities": {
            "fs": {
                "readTextFile": false,
                "writeTextFile": false,
            },
            "terminal": false,
        },
    })
}

/// Build the `params` object for a `session/new` request.
///
/// `cwd` is the project directory; `mcp_servers` is the list forwarded from
/// `LaunchSpec`; `additional_dirs` is optional.
pub fn session_new_params(cwd: &str, mcp_servers: &Value, additional_dirs: Option<&Value>) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("cwd".to_string(), json!(cwd));
    obj.insert("mcpServers".to_string(), mcp_servers.clone());
    if let Some(dirs) = additional_dirs {
        obj.insert("additionalDirectories".to_string(), dirs.clone());
    }
    Value::Object(obj)
}

/// Build the `params` object for a `session/load` request (resume path).
///
/// Only sent when `agentCapabilities.loadSession` is true.
pub fn session_load_params(session_id: &str, cwd: &str, mcp_servers: &Value) -> Value {
    json!({
        "sessionId": session_id,
        "cwd": cwd,
        "mcpServers": mcp_servers,
    })
}

/// Build the `params` object for a `session/prompt` request.
///
/// `text` is the user's prompt; `session_id` identifies the ACP session.
pub fn session_prompt_params(session_id: &str, text: &str) -> Value {
    json!({
        "sessionId": session_id,
        "prompt": [
            {
                "type": "text",
                "text": text,
            }
        ],
    })
}

/// Build the `params` object for a `session/cancel` notification.
///
/// This is a notification (no reply expected), so only the params are needed.
pub fn session_cancel_params(session_id: &str) -> Value {
    json!({
        "sessionId": session_id,
    })
}

/// Build the **result** object for a `session/request_permission` response.
///
/// - `option_id = Some(id)` → `{ outcome: "selected", optionId: id }`.
/// - `option_id = None`     → `{ outcome: "cancelled" }`.
pub fn request_permission_response(option_id: Option<&str>) -> Value {
    match option_id {
        Some(id) => json!({
            "outcome": "selected",
            "optionId": id,
        }),
        None => json!({
            "outcome": "cancelled",
        }),
    }
}

// ── Predicates and field helpers ──────────────────────────────────────────────

/// Returns `true` if `method` is the agent→client permission request method
/// (`"session/request_permission"`).
pub fn is_permission_request(method: &str) -> bool {
    method == "session/request_permission"
}

/// Extract the `sessionId` string from a `session/new` or `session/load` result.
///
/// Returns `None` when the field is absent or not a string.
pub fn extract_session_id(result: &Value) -> Option<String> {
    result.get("sessionId")?.as_str().map(|s| s.to_string())
}

/// Extract the `stopReason` string from a `session/prompt` result.
///
/// Returns `None` when the field is absent or not a string.
pub fn extract_stop_reason(result: &Value) -> Option<String> {
    result.get("stopReason")?.as_str().map(|s| s.to_string())
}

/// Extract the `update` object from a `session/update` notification params.
///
/// Returns `None` when the field is absent.
pub fn extract_update(params: &Value) -> Option<&Value> {
    params.get("update")
}

/// Extract the `type` field from a `session/update` update object.
///
/// Returns `None` when the field is absent or not a string.
pub fn extract_update_type(update: &Value) -> Option<&str> {
    update.get("type")?.as_str()
}
