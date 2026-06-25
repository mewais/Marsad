//! marsad-proto — wire types + serde for Claude stream-json and the GUI↔agent IPC protocol.
//!
//! See `docs/specs/proto.md` for the authoritative contract.

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
// ProtoError
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Error)]
pub enum ProtoError {
    #[error("malformed JSON: {0}")]
    MalformedJson(String),
    #[error("missing type discriminant")]
    MissingDiscriminant,
    #[error("unknown discriminant: {0}")]
    UnknownDiscriminant(String),
    #[error("schema mismatch at field `{field}`: {detail}")]
    SchemaMismatch { field: &'static str, detail: String },
}

// ---------------------------------------------------------------------------
// Usage
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
}

// ---------------------------------------------------------------------------
// ContentBlock
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: serde_json::Value,
        #[serde(default)]
        is_error: bool,
    },
}

// ---------------------------------------------------------------------------
// SystemInit
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemInit {
    pub session_id: String,
    pub model: String,
    pub cwd: String,
    pub tools: Vec<String>,
    #[serde(default)]
    pub mcp_servers: Vec<serde_json::Value>,
    #[serde(rename = "permissionMode")]
    pub permission_mode: String,
}

// ---------------------------------------------------------------------------
// AssistantMsg / AssistantMessage
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub id: String,
    pub content: Vec<ContentBlock>,
    pub usage: Usage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantMsg {
    pub message: AssistantMessage,
}

// ---------------------------------------------------------------------------
// UserMsg / UserMessage
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserMessage {
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserMsg {
    pub message: UserMessage,
}

// ---------------------------------------------------------------------------
// UserInput (for encode_user_input)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserInput {
    pub content: Vec<ContentBlock>,
}

// ---------------------------------------------------------------------------
// ResultSubtype
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultSubtype {
    Success,
    ErrorMaxTurns,
    ErrorDuringExecution,
}

// ---------------------------------------------------------------------------
// ResultMsg
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultMsg {
    pub subtype: ResultSubtype,
    pub total_cost_usd: f64,
    pub usage: Usage,
    pub num_turns: u64,
    pub is_error: bool,
    #[serde(default)]
    pub permission_denials: Vec<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// ControlRequest / ControlResponse
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlRequest {
    pub tool_name: String,
    pub input: serde_json::Value,
    pub request_id: String,
}

/// Outbound permission answer the GUI/agent sends back to the `claude` CLI in
/// response to a `can_use_tool` control request.
///
/// Wire shape (see `encode_control_response`) is the real Agent-SDK control
/// protocol envelope, NOT a flat object — this struct is the in-memory payload
/// the agent fills in before encoding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlResponse {
    pub request_id: String,
    pub allow: bool,
    #[serde(default)]
    pub deny_reason: Option<String>,
    /// For `allow`, the (possibly edited) tool input echoed back as
    /// `updatedInput`. The CLI USES this value as the tool's input, so for a
    /// plain approval it must equal the original `can_use_tool` input — an
    /// empty object would run e.g. Bash with no command. `None` encodes `{}`.
    #[serde(default)]
    pub updated_input: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Posture / LaunchSpec (protocol data)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Posture {
    Ask,
    AutoApproveSafe,
    FullAutonomy,
}

/// How a session is started:
/// - `Fresh`: a brand-new conversation in the project dir.
/// - `Continue`: resume the most recent conversation in the project dir
///   (`claude --continue`).
/// - `Resume(id)`: resume a specific prior conversation by its claude
///   session id (`claude --resume <id>`).
///
/// `Continue`/`Resume` let the user pick up existing Claude Code sessions —
/// including ones started outside Marsad — by running in the right `cwd`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum LaunchMode {
    #[default]
    Fresh,
    Continue,
    Resume(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LaunchSpec {
    pub project: String,
    pub host_id: String,
    pub model: String,
    pub posture: Posture,
    pub first_prompt: Option<String>,
    /// Start mode (fresh / continue / resume-by-id). Defaults to `Fresh` so
    /// older callers and serialized specs without this field keep working.
    #[serde(default)]
    pub mode: LaunchMode,
    /// Resolved remote execution target for the session's host, or `None`/`local`
    /// for the local machine. `user@host` (or a bare ssh alias) selects SSH;
    /// `wsl:<distro>` selects WSL. Drives which container/transport the agent uses.
    /// `#[serde(default)]` keeps older serialized specs (no field) working.
    #[serde(default)]
    pub host_target: Option<String>,
    /// The `claude` executable to run on the host (a wrapper like `genie` or an
    /// absolute path). `None` → the default `"claude"`. Resolved per-host from the
    /// registry; `#[serde(default)]` keeps older serialized specs working.
    #[serde(default)]
    pub claude_bin: Option<String>,
    /// A shell command run BEFORE claude on the host (e.g. `module load cuda`).
    /// `None` → no setup. `#[serde(default)]` keeps older serialized specs working.
    #[serde(default)]
    pub setup: Option<String>,
    /// Environment variables exported before claude on the host, as `(key, value)`
    /// pairs. `#[serde(default)]` keeps older serialized specs working.
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

// ---------------------------------------------------------------------------
// StreamMsg
// ---------------------------------------------------------------------------

/// Typed representation of one NDJSON line from the Claude CLI stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StreamMsg {
    SystemInit(SystemInit),
    Assistant(AssistantMsg),
    User(UserMsg),
    Result(ResultMsg),
    ControlRequest(ControlRequest),
    ControlResponse(ControlResponse),
    Unknown(serde_json::Value),
}

// ---------------------------------------------------------------------------
// IpcMsg
// ---------------------------------------------------------------------------

/// GUI ↔ agent IPC messages (closed protocol — no Unknown variant).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IpcMsg {
    LaunchSession {
        cwd: String,
        model: String,
        permission_mode: String,
    },
    ResumeSession {
        session_id: String,
    },
    AttachSession {
        session_id: String,
    },
    StopSession {
        session_id: String,
    },
    SendPrompt {
        session_id: String,
        text: String,
    },
    AnswerGate {
        /// The session whose pending gate this answers (the agent keys pending
        /// gates by session id).
        session_id: String,
        request_id: String,
        allow: bool,
        #[serde(default)]
        deny_reason: Option<String>,
    },
    Subscribe {
        session_id: String,
    },
    VmUpdate {
        session_id: String,
        update: serde_json::Value,
    },
    AgentEvent {
        session_id: String,
        event: serde_json::Value,
    },
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Convert a serde deserialization error into a `SchemaMismatch`.
/// Centralised so the closure body is in one place and always covered by
/// whichever arm fires a schema error first (currently the "result" arm).
fn schema_err(field: &'static str, e: serde_json::Error) -> ProtoError {
    ProtoError::SchemaMismatch {
        field,
        detail: e.to_string(),
    }
}

/// Deserialise `value` into `T`, mapping any error to `SchemaMismatch`.
fn from_val<T: serde::de::DeserializeOwned>(
    field: &'static str,
    value: serde_json::Value,
) -> Result<T, ProtoError> {
    serde_json::from_value(value).map_err(|e| schema_err(field, e))
}

// ---------------------------------------------------------------------------
// parse_stream_line
// ---------------------------------------------------------------------------

/// Parse one NDJSON line from the Claude CLI stream into a typed `StreamMsg`.
///
/// - Non-JSON or JSON arrays → `MalformedJson`
/// - JSON object without `type` field → `MissingDiscriminant`
/// - JSON object with unrecognized `type` → `StreamMsg::Unknown` (forward-compat)
/// - JSON object with recognized `type` but wrong field types → `SchemaMismatch`
pub fn parse_stream_line(line: &str) -> Result<StreamMsg, ProtoError> {
    // Parse the raw JSON value first.
    let value: serde_json::Value =
        serde_json::from_str(line).map_err(|e| ProtoError::MalformedJson(e.to_string()))?;

    // Must be an object (not array, string, number, etc.)
    let obj = value
        .as_object()
        .ok_or_else(|| ProtoError::MalformedJson("expected a JSON object".to_string()))?;

    // Extract the `type` discriminant.
    let type_str = obj
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or(ProtoError::MissingDiscriminant)?;

    match type_str {
        "system" => from_val("system_init", value).map(StreamMsg::SystemInit),
        "assistant" => from_val("assistant", value).map(StreamMsg::Assistant),
        "user" => from_val("user", value).map(StreamMsg::User),
        "result" => from_val("result", value).map(StreamMsg::Result),
        "control_request" => Ok(parse_control_request(value)),
        // Inbound `control_response` from the CLI is the `initialize` handshake
        // ack (carries `commands`/`output_style`, not a permission decision).
        // The agent sends `initialize` fire-and-forget, so this is a no-op —
        // preserved verbatim for forward-compat, same as `Unknown`.
        "control_response" => Ok(StreamMsg::Unknown(value)),
        _ => Ok(StreamMsg::Unknown(value)),
    }
}

/// Parse a real `control_request` line.
///
/// The Agent-SDK control protocol nests the payload under `request`:
/// `{"type":"control_request","request_id":"X","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{…}}}`.
/// Only the `can_use_tool` subtype maps to a [`ControlRequest`] (a permission
/// gate); any other subtype (or a malformed/missing `request`) is preserved as
/// `Unknown` for forward-compatibility rather than rejected.
fn parse_control_request(value: serde_json::Value) -> StreamMsg {
    let request_id = value.get("request_id").and_then(|v| v.as_str());
    let request = value.get("request").and_then(|v| v.as_object());

    match (request_id, request) {
        (Some(rid), Some(req))
            if req.get("subtype").and_then(|v| v.as_str()) == Some("can_use_tool") =>
        {
            let tool_name = req
                .get("tool_name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let input = req.get("input").cloned().unwrap_or(serde_json::Value::Null);
            StreamMsg::ControlRequest(ControlRequest {
                tool_name,
                input,
                request_id: rid.to_string(),
            })
        }
        _ => StreamMsg::Unknown(value),
    }
}

// ---------------------------------------------------------------------------
// encode_user_input
// ---------------------------------------------------------------------------

/// Encode a `UserInput` as a stream-json `user` message NDJSON line.
pub fn encode_user_input(input: &UserInput) -> String {
    // Build the wire object: {"type":"user","message":{"role":"user","content":[...]}}
    let wire = serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": input.content,
        }
    });
    serde_json::to_string(&wire).expect("UserInput is always serializable")
}

// ---------------------------------------------------------------------------
// encode_control_response
// ---------------------------------------------------------------------------

/// Encode a [`ControlResponse`] as a stream-json NDJSON line answering a
/// `can_use_tool` control request.
///
/// Real Agent-SDK control protocol envelope:
/// - allow: `{"type":"control_response","response":{"subtype":"success","request_id":"X","response":{"behavior":"allow","updatedInput":{…}}}}`
/// - deny:  `{"type":"control_response","response":{"subtype":"success","request_id":"X","response":{"behavior":"deny","message":"…"}}}`
pub fn encode_control_response(resp: &ControlResponse) -> String {
    let inner = if resp.allow {
        serde_json::json!({
            "behavior": "allow",
            "updatedInput": resp
                .updated_input
                .clone()
                .unwrap_or_else(|| serde_json::json!({})),
        })
    } else {
        serde_json::json!({
            "behavior": "deny",
            "message": resp.deny_reason.clone().unwrap_or_default(),
        })
    };
    let wire = serde_json::json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": resp.request_id,
            "response": inner,
        }
    });
    serde_json::to_string(&wire).expect("ControlResponse is always serializable")
}

/// Encode the `initialize` control request the agent sends to the `claude` CLI
/// once on connect. This handshake is required for the control protocol; the
/// CLI replies with a `control_response` ack (which the agent ignores).
///
/// Shape: `{"type":"control_request","request_id":"X","request":{"subtype":"initialize","hooks":null}}`.
pub fn encode_control_request_initialize(request_id: &str) -> String {
    let wire = serde_json::json!({
        "type": "control_request",
        "request_id": request_id,
        "request": {
            "subtype": "initialize",
            "hooks": serde_json::Value::Null,
        }
    });
    serde_json::to_string(&wire).expect("initialize request is always serializable")
}

// ---------------------------------------------------------------------------
// parse_ipc
// ---------------------------------------------------------------------------

/// Parse one NDJSON line from the GUI↔agent IPC channel.
///
/// Unlike the stream protocol, an unrecognized `type` is `UnknownDiscriminant` (a bug).
pub fn parse_ipc(line: &str) -> Result<IpcMsg, ProtoError> {
    // Parse raw JSON.
    let value: serde_json::Value =
        serde_json::from_str(line).map_err(|e| ProtoError::MalformedJson(e.to_string()))?;

    // Extract the `type` discriminant — must be a string field in the object.
    let type_str = value
        .as_object()
        .and_then(|obj| obj.get("type"))
        .and_then(|v| v.as_str())
        .ok_or(ProtoError::MissingDiscriminant)?;

    // Known IPC variants — any other type is a bug (UnknownDiscriminant).
    const KNOWN: &[&str] = &[
        "LaunchSession",
        "ResumeSession",
        "AttachSession",
        "StopSession",
        "SendPrompt",
        "AnswerGate",
        "Subscribe",
        "VmUpdate",
        "AgentEvent",
    ];

    if !KNOWN.contains(&type_str) {
        return Err(ProtoError::UnknownDiscriminant(type_str.to_string()));
    }

    // Now deserialize — schema errors will propagate as SchemaMismatch.
    from_val("ipc", value)
}

// ---------------------------------------------------------------------------
// encode_ipc
// ---------------------------------------------------------------------------

/// Encode an `IpcMsg` as an NDJSON line.
pub fn encode_ipc(msg: &IpcMsg) -> String {
    serde_json::to_string(msg).expect("IpcMsg is always serializable")
}
