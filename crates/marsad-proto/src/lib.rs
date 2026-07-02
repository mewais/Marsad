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
    /// Extended-thinking block. Real accounts with `alwaysThinkingEnabled: true`
    /// prepend one of these to assistant messages that reasoned before replying
    /// (`{"type":"thinking","thinking":"...","signature":"..."}`). `signature` is
    /// an opaque provider-side token (not human-readable reasoning); both fields
    /// default so absent/empty variants still parse. This is NOT the assistant's
    /// reply — reducers must not surface `thinking` into transcript/say events.
    Thinking {
        #[serde(default)]
        thinking: String,
        #[serde(default)]
        signature: String,
    },
}

// ---------------------------------------------------------------------------
// McpServerInfo / SlashCommandInfo
// ---------------------------------------------------------------------------

/// One MCP server entry from the `system/init` event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct McpServerInfo {
    pub name: String,
    #[serde(default)]
    pub status: String,
}

/// One slash command entry from the `system/init` event.
///
/// `slash_commands` may be a string-array OR an object-array — the custom
/// deserializer handles both: a bare string `"clear"` maps to
/// `SlashCommandInfo { name: "/clear", description: "", argument_hint: "" }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SlashCommandInfo {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "argumentHint")]
    pub argument_hint: String,
}

/// Deserialize a `Vec<SlashCommandInfo>` that may contain bare strings or objects.
pub fn deserialize_slash_commands<'de, D>(
    deserializer: D,
) -> Result<Vec<SlashCommandInfo>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Deserialize};
    // Deserialize the field to a Vec of raw JSON values first, then normalise each
    // element (claude sends slash_commands as EITHER a string array or an object
    // array). Going through `Vec<Value>` keeps every branch below reachable for
    // tests — a hand-rolled SeqAccess visitor has a `next_element()?` error edge
    // that can't fire when deserializing from an already-parsed value.
    let raw = Vec::<serde_json::Value>::deserialize(deserializer)?;
    let mut commands = Vec::with_capacity(raw.len());
    for elem in raw {
        match elem {
            serde_json::Value::String(s) => {
                // Bare string: normalise to "/name" if it lacks the slash.
                let name = if s.starts_with('/') {
                    s
                } else {
                    format!("/{s}")
                };
                commands.push(SlashCommandInfo {
                    name,
                    ..Default::default()
                });
            }
            obj @ serde_json::Value::Object(_) => {
                commands.push(serde_json::from_value(obj).map_err(de::Error::custom)?);
            }
            other => {
                return Err(de::Error::custom(format!(
                    "expected string or object in slash_commands, got: {other}"
                )));
            }
        }
    }
    Ok(commands)
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
    pub mcp_servers: Vec<McpServerInfo>,
    #[serde(rename = "permissionMode")]
    pub permission_mode: String,
    #[serde(default, deserialize_with = "deserialize_slash_commands")]
    pub slash_commands: Vec<SlashCommandInfo>,
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
    #[serde(deserialize_with = "deserialize_user_content")]
    pub content: Vec<ContentBlock>,
}

/// Deserialize a `user` message's `content` field, which real claude streams
/// send as EITHER a `Vec<ContentBlock>` (the normal shape) OR a bare string
/// (post-`/compact` synthesized messages: the compaction summary, and the
/// `<local-command-stdout>...</local-command-stdout>` marker). A bare string is
/// wrapped as a single-element `[ContentBlock::Text]` so callers (including
/// `UserMessage.content.len()`/indexing) keep working unchanged.
fn deserialize_user_content<'de, D>(deserializer: D) -> Result<Vec<ContentBlock>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ContentShape {
        Blocks(Vec<ContentBlock>),
        Bare(String),
    }

    match ContentShape::deserialize(deserializer)? {
        ContentShape::Blocks(blocks) => Ok(blocks),
        ContentShape::Bare(text) => Ok(vec![ContentBlock::Text { text }]),
    }
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
// ModelUsage
// ---------------------------------------------------------------------------

/// Per-model token/cost breakdown from claude's `result` event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ModelUsage {
    #[serde(default, rename = "inputTokens")]
    pub input_tokens: u64,
    #[serde(default, rename = "outputTokens")]
    pub output_tokens: u64,
    #[serde(default, rename = "costUSD")]
    pub cost_usd: f64,
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
    /// Cumulative per-model token + cost breakdown (populated by newer claude
    /// CLI versions). Empty when absent (e.g. older claude or no model breakdown).
    #[serde(default, rename = "modelUsage")]
    pub model_usage: std::collections::HashMap<String, ModelUsage>,
}

// ---------------------------------------------------------------------------
// ControlRequest / ControlResponse
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlRequest {
    pub tool_name: String,
    pub input: serde_json::Value,
    pub request_id: String,
    /// Set (to the human-readable approval prompt) when this is an MCP
    /// `elicitation` request rather than a `can_use_tool` request. An MCP server
    /// (e.g. a sandboxed-bash server) raises an elicitation to ask the user to
    /// approve an action; claude bridges it over the control protocol. Presence
    /// of this field marks the request as an elicitation, which MUST be answered
    /// with `{action: accept|decline}` — NOT the `can_use_tool` `{behavior,
    /// updatedInput}` shape. `None` for a normal tool-permission request.
    #[serde(default)]
    pub elicitation_message: Option<String>,
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
    /// Optional human-readable display name for the session. `None` → name is
    /// derived from the project directory basename. `#[serde(default)]` keeps
    /// older serialized specs (no field) working.
    #[serde(default)]
    pub name: Option<String>,
    /// Tools that should be disabled for this session. When non-empty the agent
    /// passes `--disallowedTools <comma-joined-list>` to the `claude` CLI.
    /// `#[serde(default)]` keeps older serialized specs (no field) working.
    #[serde(default)]
    pub disallowed_tools: Vec<String>,
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
    AnswerQuestion {
        /// The session whose pending AskUserQuestion gate this answers.
        session_id: String,
        request_id: String,
        /// Answers map: question text → selected label(s).  An empty object
        /// counts as "skip" (the tool returns "did not answer").
        answers: serde_json::Value,
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
        // Only `system/init` carries SystemInit's required fields
        // (model/cwd/tools/permissionMode). Real live streams emit many other
        // `system` subtypes (thinking_tokens, status, task_started/progress/
        // updated/notification, compact_boundary, hook_started/hook_response,
        // …) that lack them entirely — those are forward-compat `Unknown`
        // (the full JSON, including `subtype`, is preserved in the `Value` for
        // observability), not a schema error.
        "system" if obj.get("subtype").and_then(|v| v.as_str()) == Some("init") => {
            from_val("system_init", value).map(StreamMsg::SystemInit)
        }
        "system" => Ok(StreamMsg::Unknown(value)),
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
                elicitation_message: None,
            })
        }
        // MCP elicitation: an MCP server asking the user to approve an action.
        // `tool_name` is set to the server name so the gate identifies the source;
        // `elicitation_message` carries the prompt and marks the kind.
        (Some(rid), Some(req))
            if req.get("subtype").and_then(|v| v.as_str()) == Some("elicitation") =>
        {
            let server = req
                .get("mcp_server_name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let message = req
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            StreamMsg::ControlRequest(ControlRequest {
                tool_name: server,
                input: serde_json::Value::Null,
                request_id: rid.to_string(),
                elicitation_message: Some(message),
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

/// Encode the control_response that answers an MCP `elicitation` request.
///
/// Unlike `can_use_tool` (which uses `{behavior, updatedInput}`), an elicitation
/// is answered with an MCP elicitation result `{action, content}`. Verified
/// against the CLI bundle (`handleElicitation` returns this object verbatim from
/// `sendRequest`; the host then reads `{action, content}`):
/// - accept: `…"response":{"action":"accept","content":{}}`
/// - decline: `…"response":{"action":"decline"}`
///
/// A declined/denied gate maps to `decline` (the user said no). `content` is an
/// empty object on accept — the matched action carries no form fields here.
pub fn encode_control_response_elicitation(request_id: &str, accept: bool) -> String {
    let inner = if accept {
        serde_json::json!({ "action": "accept", "content": {} })
    } else {
        serde_json::json!({ "action": "decline" })
    };
    let wire = serde_json::json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": inner,
        }
    });
    serde_json::to_string(&wire).expect("elicitation response is always serializable")
}

/// Encode the control_response that answers an `AskUserQuestion` `can_use_tool`
/// request.
///
/// Wire shape (verified from the spec):
/// `{"type":"control_response","response":{"subtype":"success","request_id":"X","response":{"behavior":"allow","updatedInput":{"questions":[…],"answers":{"<question>":"<label>"}}}}}`
///
/// `original_input` is the full `input` object captured when the gate arrived
/// (so we can echo the original `questions` array back as `updatedInput`).
/// `answers` is an object keyed by question text → selected label(s) string; an
/// empty object (`{}`) signals "skip" to the tool (it returns "did not answer").
pub fn encode_control_response_question(
    request_id: &str,
    original_input: &serde_json::Value,
    answers: &serde_json::Value,
) -> String {
    let questions = original_input
        .get("questions")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));
    let wire = serde_json::json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": {
                "behavior": "allow",
                "updatedInput": {
                    "questions": questions,
                    "answers": answers,
                }
            }
        }
    });
    serde_json::to_string(&wire).expect("question response is always serializable")
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

/// Encode the control-protocol `interrupt` request that stops claude's current
/// turn (verified against the CLI: it acks with a `control_response` success and
/// ends the turn with `result: error_during_execution`). Written to claude's
/// stdin like any other control message.
///
/// Shape: `{"type":"control_request","request_id":"X","request":{"subtype":"interrupt"}}`.
pub fn encode_control_request_interrupt(request_id: &str) -> String {
    let wire = serde_json::json!({
        "type": "control_request",
        "request_id": request_id,
        "request": {
            "subtype": "interrupt",
        }
    });
    serde_json::to_string(&wire).expect("interrupt request is always serializable")
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
        "AnswerQuestion",
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
