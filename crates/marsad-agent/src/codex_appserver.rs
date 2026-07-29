//! Codex `app-server` — pure JSON-RPC 2.0 param constructors + a tolerant line
//! parser for the persistent codex session (replaces the one-shot `codex exec`
//! eval-loop). Transport-agnostic and unit-testable.
//!
//! Verified live against the real codex-cli 0.142.5 binary (2026-07-21):
//! - Wire is **newline-delimited JSON**, and codex **omits the `jsonrpc` field**
//!   (responses are `{"id":N,"result":{…}}`, notifications `{"method":…,"params":…}`).
//!   So we reuse the generic `acp::AcpMessage` enum + `acp::encode_*` for sending
//!   (codex accepts a `jsonrpc` field on input), but need our OWN parser here that
//!   does NOT require `jsonrpc:"2.0"` (`acp::parse_acp_line` rejects codex lines).
//! - Handshake: `initialize` → `{result:{userAgent,codexHome,modelProvider,…}}`;
//!   `initialized` (notification); `thread/start {cwd,…}` → `{result:{thread:{id,
//!   sessionId}}}` + a `thread/started` notification; `turn/start {threadId,
//!   input:[{type:"text",text}]}` drives the turn.
//!
//! Method/shape names come from `codex app-server generate-json-schema` for the
//! installed binary (authoritative per version).

use crate::acp::{AcpError, AcpMessage};
use serde_json::{json, Value};

// ── Method constants (client→server requests / notifications) ─────────────────

pub const INITIALIZE: &str = "initialize";
pub const INITIALIZED: &str = "initialized"; // notification
pub const THREAD_START: &str = "thread/start";
pub const THREAD_RESUME: &str = "thread/resume";
pub const TURN_START: &str = "turn/start";
pub const TURN_INTERRUPT: &str = "turn/interrupt";
pub const THREAD_SETTINGS_UPDATE: &str = "thread/settings/update";
pub const MODEL_LIST: &str = "model/list";
pub const COLLABORATION_MODE_LIST: &str = "collaborationMode/list";

// ── Server→client request methods (approvals / questions = the gates) ─────────

pub const REQ_COMMAND_APPROVAL: &str = "item/commandExecution/requestApproval";
pub const REQ_FILE_APPROVAL: &str = "item/fileChange/requestApproval";
pub const REQ_PERMISSION_APPROVAL: &str = "item/permissions/requestApproval";
pub const REQ_USER_INPUT: &str = "item/tool/requestUserInput";
pub const REQ_ELICITATION: &str = "mcpServer/elicitation/request";
// Legacy (turns started via the exec path) — kept for completeness.
pub const REQ_EXEC_APPROVAL: &str = "execCommandApproval";
pub const REQ_APPLY_PATCH_APPROVAL: &str = "applyPatchApproval";

/// True if `method` is a server→client APPROVAL request that should surface as a
/// HITL gate. The `item/…/requestApproval` family answers with a
/// `CommandExecutionApprovalDecision` (`accept`/`decline`/…); the legacy
/// `execCommandApproval`/`applyPatchApproval` answer with a `ReviewDecision`
/// (`approved`/`denied`).
pub fn is_command_approval(method: &str) -> bool {
    matches!(method, REQ_COMMAND_APPROVAL | REQ_FILE_APPROVAL | REQ_PERMISSION_APPROVAL)
}

/// True if `method` is a legacy exec-path approval (answered with a `ReviewDecision`).
pub fn is_legacy_approval(method: &str) -> bool {
    matches!(method, REQ_EXEC_APPROVAL | REQ_APPLY_PATCH_APPROVAL)
}

// ── Line parsing (codex omits `jsonrpc`) ──────────────────────────────────────

/// Parse one newline-delimited codex app-server line into an `AcpMessage`.
///
/// Identical discrimination to `acp::parse_acp_line` EXCEPT it does not require a
/// `jsonrpc` field (codex never sends one). A tolerant `jsonrpc` is accepted if
/// present.
pub fn parse_line(line: &str) -> Result<AcpMessage, AcpError> {
    let v: Value =
        serde_json::from_str(line).map_err(|e| AcpError::InvalidJson(e.to_string()))?;
    let obj = match v.as_object() {
        Some(o) => o,
        None => return Err(AcpError::UnrecognizedShape(format!("not a JSON object: {line}"))),
    };
    let has_id = obj.contains_key("id");
    let has_method = obj.contains_key("method");
    let has_result = obj.contains_key("result");
    let has_error = obj.contains_key("error");

    if has_id && has_method && !has_result && !has_error {
        Ok(AcpMessage::Request {
            id: obj["id"].clone(),
            method: obj["method"]
                .as_str()
                .ok_or_else(|| AcpError::UnrecognizedShape("method not a string".into()))?
                .to_string(),
            params: obj.get("params").cloned().unwrap_or(Value::Null),
        })
    } else if has_id && has_result && !has_method {
        Ok(AcpMessage::Response { id: obj["id"].clone(), result: Ok(obj["result"].clone()) })
    } else if has_id && has_error && !has_method {
        Ok(AcpMessage::Response { id: obj["id"].clone(), result: Err(obj["error"].clone()) })
    } else if !has_id && has_method {
        Ok(AcpMessage::Notification {
            method: obj["method"]
                .as_str()
                .ok_or_else(|| AcpError::UnrecognizedShape("method not a string".into()))?
                .to_string(),
            params: obj.get("params").cloned().unwrap_or(Value::Null),
        })
    } else {
        Err(AcpError::UnrecognizedShape(format!("cannot classify codex line: {line}")))
    }
}

// ── Typed param constructors (client→server) ──────────────────────────────────

/// `initialize` params. Only `clientInfo` is required.
pub fn initialize_params(client_name: &str, client_version: &str) -> Value {
    json!({ "clientInfo": { "name": client_name, "version": client_version } })
}

/// `thread/start` params. All fields optional; we pass the project `cwd`, and —
/// when set — the model, the approval policy, and the sandbox mode (mapped from
/// Marsad's posture by the caller). An empty `model`/`approval`/`sandbox` is omitted.
pub fn thread_start_params(
    cwd: &str,
    model: Option<&str>,
    approval_policy: Option<&str>,
    sandbox: Option<&str>,
) -> Value {
    let mut o = serde_json::Map::new();
    o.insert("cwd".into(), json!(cwd));
    if let Some(m) = model.filter(|s| !s.is_empty()) {
        o.insert("model".into(), json!(m));
    }
    if let Some(a) = approval_policy.filter(|s| !s.is_empty()) {
        o.insert("approvalPolicy".into(), json!(a));
    }
    if let Some(s) = sandbox.filter(|s| !s.is_empty()) {
        o.insert("sandbox".into(), json!(s));
    }
    Value::Object(o)
}

/// `turn/start` params: send the user's prompt as a single text input item.
/// Verified live: `input:[{type:"text",text}]` is accepted and echoed back as a
/// `userMessage` item.
pub fn turn_start_params(thread_id: &str, text: &str) -> Value {
    json!({
        "threadId": thread_id,
        "input": [ { "type": "text", "text": text } ],
    })
}

/// `turn/interrupt` params — the REAL cancel (replaces codex's eval-loop no-op).
pub fn turn_interrupt_params(thread_id: &str, turn_id: &str) -> Value {
    json!({ "threadId": thread_id, "turnId": turn_id })
}

/// `thread/settings/update` params — live model/mode switch, no restart. `config_id`
/// is "model" | "approvalPolicy" | "collaborationMode" | "sandboxPolicy"; the value
/// is the option id.
pub fn settings_update_params(thread_id: &str, config_id: &str, value: &str) -> Value {
    let mut o = serde_json::Map::new();
    o.insert("threadId".into(), json!(thread_id));
    o.insert(config_id.into(), json!(value));
    Value::Object(o)
}

/// Build the **result** for an `item/…/requestApproval` server request:
/// `{decision: "accept"|"decline"}` (optionally `"acceptForSession"`).
pub fn approval_decision(accept: bool, for_session: bool) -> Value {
    let d = if !accept {
        "decline"
    } else if for_session {
        "acceptForSession"
    } else {
        "accept"
    };
    json!({ "decision": d })
}

/// Build the **result** for a legacy `execCommandApproval`/`applyPatchApproval`
/// request: `{decision: "approved"|"denied"}` (uses the `ReviewDecision` vocab).
pub fn legacy_review_decision(approve: bool) -> Value {
    json!({ "decision": if approve { "approved" } else { "denied" } })
}

// ── Result field helpers ──────────────────────────────────────────────────────

/// Extract the thread id from a `thread/start`/`thread/resume` result
/// (`result.thread.id`).
pub fn extract_thread_id(result: &Value) -> Option<String> {
    result.get("thread")?.get("id")?.as_str().map(|s| s.to_string())
}

/// Extract the turn id from a `turn/started`/`turn/completed` notification params
/// (`params.turn.id`), for a later `turn/interrupt`.
pub fn extract_turn_id(params: &Value) -> Option<String> {
    params.get("turn")?.get("id")?.as_str().map(|s| s.to_string())
}

/// Extract a turn's terminal status from a `turn/completed` params
/// (`params.turn.status`), e.g. "completed" | "failed" | "interrupted".
pub fn extract_turn_status(params: &Value) -> Option<String> {
    params.get("turn")?.get("status")?.as_str().map(|s| s.to_string())
}
