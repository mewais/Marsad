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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlResponse {
    pub request_id: String,
    pub allow: bool,
    #[serde(default)]
    pub deny_reason: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LaunchSpec {
    pub project: String,
    pub host_id: String,
    pub model: String,
    pub posture: Posture,
    pub first_prompt: Option<String>,
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
        "control" => from_val("control_request", value).map(StreamMsg::ControlRequest),
        "control_response" => from_val("control_response", value).map(StreamMsg::ControlResponse),
        _ => Ok(StreamMsg::Unknown(value)),
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

/// Encode a `ControlResponse` as a stream-json NDJSON line.
pub fn encode_control_response(resp: &ControlResponse) -> String {
    let wire = serde_json::json!({
        "type": "control_response",
        "request_id": resp.request_id,
        "allow": resp.allow,
        "deny_reason": resp.deny_reason,
    });
    serde_json::to_string(&wire).expect("ControlResponse is always serializable")
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
