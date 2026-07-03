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
// Capabilities (U1 — dynamic capability discovery, docs/specs/dynamic-capabilities.md §1)
// ---------------------------------------------------------------------------

/// One selectable model, exactly as the driver reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    /// The exact string passed to the CLI's model flag (`--model` / `-m`). Verbatim,
    /// never normalized: agy model names contain spaces/parens ("Gemini 3.5 Flash (Low)").
    pub id: String,
    /// Driver-reported qualifier, verbatim (e.g. codex catalog priority/flagship info).
    /// NEVER synthesized by Marsad — no hand-written "Most capable" sublabels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotation: Option<String>,
}

/// Provenance of a model catalog — drives the UI's "unverified list" treatment.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ModelListSource {
    /// The CLI enumerated it (codex `debug models`, `agy models`).
    Listed,
    /// Inferred from observation (claude: models seen in `system:init` + persisted
    /// last-known). UI MUST label this "unverified" and MUST offer free-text entry.
    Observed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelCatalog {
    #[serde(default)]
    pub models: Vec<ModelInfo>,
    pub source: ModelListSource,
    /// UI must additionally offer a free-text model field. true for claude (always)
    /// and codex (account catalog may lag); false for agy (the CLI accepts exactly
    /// the `agy models` strings — an unknown model is a designated error case,
    /// antigravity-research §6#4).
    #[serde(default)]
    pub free_text: bool,
}

impl Default for ModelCatalog {
    fn default() -> Self {
        // The honest empty state: nothing observed yet, free text allowed.
        ModelCatalog {
            models: vec![],
            source: ModelListSource::Observed,
            free_text: true,
        }
    }
}

/// One resumable past session discovered on the host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveredSession {
    /// Driver-native resume id (claude UUID stem / codex thread uuid / agy conversation uuid).
    pub id: String,
    /// Project/workspace directory. None where the store doesn't record it
    /// retrievably (agy: only the most-recent conversation per workspace is mapped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// First user prompt, trimmed + truncated (~100 chars), driver-appropriate parse.
    #[serde(default)]
    pub snippet: String,
    /// Last-activity mtime, Unix epoch milliseconds.
    #[serde(default)]
    pub modified_ms: u64,
}

/// What the session scan covered — claude's on-disk project encoding is lossy
/// (`encode_project_dir`, crates/marsad-core/src/discovery.rs:205 — `/` and `_` both
/// map to `-`), so claude sessions can only be enumerated per REQUESTED project;
/// codex/agy stores are globally enumerable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SessionScope {
    /// Everything the driver's store contains (codex, agy).
    AllKnown,
    /// Only these requested project paths (claude). Grows as projects are requested;
    /// persisted so the cache replays them.
    Projects(Vec<String>),
}

/// Discovered capabilities for one (driver, host) pair. The unit of caching,
/// persistence, wire transfer, and VmUpdate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capabilities {
    pub driver: String,
    pub host_id: String,
    /// Whether the driver's binary was found/runnable on the host. Discovered
    /// (`command -v`/probe), never assumed. M7d's driver picker gains its
    /// ground truth here later.
    #[serde(default)]
    pub cli_available: bool,
    #[serde(default)]
    pub models: ModelCatalog,
    #[serde(default)]
    pub session_scope: Option<SessionScope>,
    #[serde(default)]
    pub sessions: Vec<DiscoveredSession>,
    /// When the last successful probe finished; 0 = never (persisted-cache only /
    /// first run). UI derives staleness from this — no boolean baked in.
    #[serde(default)]
    pub fetched_at_ms: u64,
    /// Per-probe failures, verbatim, surfaced not swallowed ("agy models: exit 1: …").
    #[serde(default)]
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// IpcMsg
// ---------------------------------------------------------------------------

/// GUI ↔ agent IPC messages (closed protocol — no Unknown variant; see
/// `docs/specs/typed-ipc.md` §4 for core's tolerant-skip disposition of
/// unknown inbound types).
///
/// `LaunchSession` is much larger than the other variants (it carries the
/// full launch wire shape, `docs/specs/typed-ipc.md` §2) — these messages are
/// constructed and immediately encoded/matched, never stored in bulk, so
/// boxing fields to shrink the enum would add indirection for no real benefit
/// (and would change the field types the pinned `ipc_typed.rs` golden tests
/// construct directly).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
#[serde(tag = "type")]
pub enum IpcMsg {
    // ── GUI/core → agent ────────────────────────────────────────────────────
    /// Full launch message. This variant IS the wire shape core's
    /// `launch_session_with_id` builds directly (no post-encode mutation).
    /// Required on parse: `project`, `model`, `posture` (what the agent has
    /// always required). Everything else is parse-optional so the existing
    /// minimal test corpus (typed-ipc.md §7) keeps parsing.
    LaunchSession {
        /// Marsad session id, generated by core. Optional so raw clients that
        /// omit it still parse (the agent then generates one).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        /// Project directory (the agent's cwd for claude). Required.
        project: String,
        /// Legacy mirror of `project`; kept for wire compat with old readers.
        /// Core always emits it (== project); defaulted on parse.
        #[serde(default)]
        cwd: String,
        /// Host registry id ("local" when absent). Core always emits it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host_id: Option<String>,
        /// Remote execution target (`user@host` / ssh alias / `wsl:<distro>`);
        /// absent for local. Emitted only when resolved.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host_target: Option<String>,
        /// Per-host claude executable; emitted only when configured.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        claude_bin: Option<String>,
        /// Per-host setup shell command; emitted only when configured.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        setup: Option<String>,
        /// Per-host env as [k,v] pairs; emitted only when non-empty.
        /// `Vec<(String,String)>` serializes to `[["K","V"],…]` — today's exact shape.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        env: Vec<(String, String)>,
        model: String,
        /// Authoritative permission posture ("Ask"|"AutoApproveSafe"|"FullAutonomy"
        /// — Posture's unit variants serialize to exactly these strings). Required;
        /// an unknown value is a SchemaMismatch (preserves agent's error on
        /// `posture:99` / `"Nonsense"`).
        posture: Posture,
        /// Legacy permission-mode mirror ("default"|"autoApprove"|"bypassPermissions");
        /// kept for back-compat. Core always emits it; defaulted on parse.
        #[serde(default)]
        permission_mode: String,
        /// ALWAYS serialized, `null` when absent — today core unconditionally
        /// inserts it. Plain Option (no skip attribute).
        #[serde(default)]
        first_prompt: Option<String>,
        /// LaunchMode::Continue on the wire. Absent otherwise.
        #[serde(default, rename = "continue", skip_serializing_if = "Option::is_none")]
        continue_: Option<bool>,
        /// LaunchMode::Resume(id) on the wire. Absent otherwise.
        /// Empty string == Fresh — preserved by `launch_mode()`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resume_session_id: Option<String>,
        /// Custom display name; emitted only when trimmed non-empty.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// --disallowedTools list; emitted only when non-empty.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        disallowed_tools: Vec<String>,
    },
    ResumeSession {
        session_id: String,
        /// ADDITIVE: the agent already reads this from the raw value; core
        /// never sends it today. Absent → None; absent on the wire when None.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        first_prompt: Option<String>,
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
    /// Typed form of the raw `Interrupt` line. Stops the current turn via
    /// claude's `interrupt` control_request; never kills the session.
    Interrupt {
        session_id: String,
    },
    AnswerGate {
        /// The session whose pending gate this answers (the agent keys pending
        /// gates by session id).
        session_id: String,
        /// PARSE-TOLERANCE (additive): `#[serde(default)]` — the agent's
        /// handler never actually reads the WIRE `request_id` (it answers the
        /// PENDING gate captured when the `can_use_tool` control_request
        /// arrived, keyed by `session_id`, using THAT gate's own
        /// `request_id`); a caller that omits it is exactly as functional as
        /// one that includes it. Emission unchanged (core always sets it).
        #[serde(default)]
        request_id: String,
        /// PARSE-TOLERANCE (additive): `#[serde(default)]` preserves the
        /// agent's current lenient read
        /// `value["allow"].as_bool().unwrap_or(false)` — a missing `allow`
        /// means deny, not a schema error. Emission unchanged (core always
        /// sets it).
        #[serde(default)]
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
        /// PARSE-TOLERANCE (additive): `#[serde(default)]` — the agent has
        /// never required it. Emission unchanged.
        #[serde(default)]
        session_id: String,
    },
    /// Typed form of the stringly arm that used to ignore `Unsubscribe`.
    /// Benign no-op today.
    Unsubscribe {
        /// Tolerated-absent for the same reason: today's arm ignores the payload.
        #[serde(default)]
        subscriber_id: String,
    },
    /// Typed form of the raw `SlashCmd` line. NOTE: the agent has no handler
    /// today — it falls into the unknown-type arm and replies `Error`. This
    /// becomes an EXPLICIT arm with the same observable reply
    /// (behavior-preserving; real slash forwarding is U-series work).
    SlashCmd {
        session_id: String,
        cmd: String,
    },
    /// Typed form of the raw `SetWorkspace` line. Advisory broadcast; agent
    /// has no handler today (same explicit-arm treatment as SlashCmd).
    SetWorkspace {
        workspace_id: String,
    },
    /// Ask the agent to (re)probe capabilities. Fire-and-forget: the reply is
    /// the `CapabilitiesRefreshQueued` ack; results arrive later as
    /// `CapabilitiesUpdate` broadcasts. `None` = all.
    RefreshCapabilities {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        driver: Option<String>,
        /// claude sessions need a project scope (dynamic-capabilities.md §1.2);
        /// adds it to the host's `SessionScope::Projects` set. Ignored by
        /// `AllKnown` drivers.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project: Option<String>,
    },

    // ── agent → GUI/core ────────────────────────────────────────────────────
    VmUpdate {
        session_id: String,
        update: serde_json::Value,
    },
    AgentEvent {
        session_id: String,
        /// PARSE-TOLERANCE (additive): `#[serde(default)]` mirrors the old
        /// `IpcEnvelope`'s `#[serde(default)] event: Value` — an absent event
        /// parses as Null and flows to StreamMsg::Unknown, exactly as today.
        #[serde(default)]
        event: serde_json::Value,
    },
    /// Typed form of the raw `TranscriptBackfill` line. Remote session
    /// history: the raw `.jsonl` text read on the remote host, shipped so
    /// core can backfill the transcript and aggregate model usage.
    TranscriptBackfill {
        session_id: String,
        /// Absent/empty → core no-ops. Mirrors IpcEnvelope.
        #[serde(default)]
        jsonl: String,
    },
    /// Typed form of the raw `ChangesUpdate` line. Remote git-probe result.
    ChangesUpdate {
        session_id: String,
        /// Kept as Value ON PURPOSE: core parses it leniently
        /// (`from_value::<Vec<FileChange>>(…).unwrap_or_default()` — malformed
        /// → empty, never an error) and `FileChange` lives in
        /// marsad-viewmodel, which proto must not depend on (DAG). Element
        /// shape (informative): `{"path":"a.rs","status":"M","added":3,"removed":1}`.
        #[serde(default)]
        changes: serde_json::Value,
    },
    /// Typed form of the raw `UsageUpdate` line. Project-wide per-model usage
    /// from the remote python3 aggregation script.
    UsageUpdate {
        session_id: String,
        /// Kept as Value ON PURPOSE: core's lenient handling (missing →
        /// no-op, non-object → no-op) must survive. Shape (informative):
        /// `{"<model>":{"input":N,"output":N,"cache_read":N,"cache_creation":N}}`.
        #[serde(default)]
        usage: serde_json::Value,
    },
    // Command acks (agent serve loop replies).
    SessionLaunched {
        session_id: String,
    },
    SessionResumed {
        session_id: String,
    },
    SessionAttached {
        session_id: String,
    },
    SessionStopped {
        session_id: String,
    },
    PromptSent {
        session_id: String,
    },
    Interrupted {
        session_id: String,
    },
    GateAnswered {
        session_id: String,
    },
    QuestionAnswered {
        session_id: String,
    },
    Subscribed {
        subscriber_id: String,
    },
    /// The serve loop's error reply.
    Error {
        message: String,
    },
    /// Serve-loop ack (house convention: every command line gets a typed
    /// reply) for `RefreshCapabilities`.
    CapabilitiesRefreshQueued {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host_id: Option<String>,
    },
    /// One (driver, host) snapshot — broadcast to all subscribers on probe
    /// completion AND replayed from cache to each new subscriber. Always a
    /// COMPLETE snapshot (replace, not merge, on the receiving side).
    CapabilitiesUpdate {
        host_id: String,
        driver: String,
        /// Typed — `Capabilities` lives in proto (§1), unlike `ChangesUpdate`'s
        /// `Value` (whose `FileChange` lives in viewmodel; no such DAG
        /// constraint here).
        capabilities: Capabilities,
    },

    // ── X1 Wasl connector (docs/specs/wasl-connector.md §3) ─────────────────
    // ── GUI/core → agent ────────────────────────────────────────────────────
    /// NEW: relay ONE explicit human decision on a Wasl gate to the node's
    /// operator endpoint. Marsad-originated only — never synthesized (hard rule 2).
    AnswerWaslGate {
        host_id: String,
        gate_id: String,
        /// Wasl outcome string, relayed verbatim: "approved" | "denied"
        /// ("dismissed" is wire-valid for forward compat; core never emits it in X1).
        #[serde(default)]
        outcome: String,
        /// Denial reason. ALWAYS serialized (null when absent), like LaunchSession.first_prompt.
        #[serde(default)]
        reason: Option<String>,
    },

    // ── agent → GUI/core ────────────────────────────────────────────────────
    /// NEW broadcast: a Wasl gate surfaced (or re-surfaced via resnapshot reconcile).
    WaslGateRequested {
        host_id: String,
        gate_id: String,
        #[serde(default)]
        mode: String,
        #[serde(default)]
        decidable: bool,
        /// Verbatim Wasl GateRequest object; Value ON PURPOSE (typed form is
        /// marsad_wasl::GateRequest; core parses leniently — §5.2).
        #[serde(default)]
        request: serde_json::Value,
        #[serde(default)]
        requested_at: String,
        #[serde(default)]
        timeout_ms: u64,
        #[serde(default)]
        deadline: String,
    },
    /// NEW broadcast: a Wasl gate resolved.
    WaslGateDecided {
        host_id: String,
        gate_id: String,
        /// Wasl outcome verbatim, PLUS "unknown" for connector-synthesized
        /// GateVanished (Marsad-wire-only value; never sent to Wasl).
        #[serde(default)]
        outcome: String,
        /// Wasl decided_by verbatim, PLUS "resync" for GateVanished.
        #[serde(default)]
        decided_by: String,
        #[serde(default)]
        reason: Option<String>, // always serialized, null when absent
        /// RFC 3339 from the frame; "" for GateVanished (no node timestamp exists).
        #[serde(default)]
        decided_at: String,
    },
    /// NEW broadcast: one audit-feed entry (the observation feed — EVERY durable
    /// audit append, including auto-approved sends).
    WaslObservation {
        host_id: String,
        /// Verbatim Wasl AuditEntry; Value ON PURPOSE (same rationale as `request`).
        #[serde(default)]
        entry: serde_json::Value,
    },
    /// NEW ack (serve-loop reply to AnswerWaslGate): the decision was RELAYED to the
    /// connector — not that the POST succeeded (the WaslGateDecided broadcast is the
    /// authoritative resolution signal, mirroring the contract's own 204 semantics).
    WaslGateAnswered {
        host_id: String,
        gate_id: String,
    },
}

impl IpcMsg {
    /// `LaunchSession` only: derive the `LaunchMode` from the wire fields.
    /// Precedence: non-empty `resume_session_id` wins, then `continue == true`,
    /// else `Fresh`. Empty-string `resume_session_id` is treated as absent
    /// (preserves the agent's historical `parse_launch_mode_value` behavior).
    pub fn launch_mode(&self) -> Option<LaunchMode> {
        match self {
            IpcMsg::LaunchSession {
                continue_,
                resume_session_id,
                ..
            } => {
                if let Some(id) = resume_session_id {
                    if !id.is_empty() {
                        return Some(LaunchMode::Resume(id.clone()));
                    }
                }
                if *continue_ == Some(true) {
                    Some(LaunchMode::Continue)
                } else {
                    Some(LaunchMode::Fresh)
                }
            }
            _ => None,
        }
    }
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
        "Interrupt",
        "AnswerGate",
        "AnswerQuestion",
        "Subscribe",
        "Unsubscribe",
        "SlashCmd",
        "SetWorkspace",
        "VmUpdate",
        "AgentEvent",
        "TranscriptBackfill",
        "ChangesUpdate",
        "UsageUpdate",
        "SessionLaunched",
        "SessionResumed",
        "SessionAttached",
        "SessionStopped",
        "PromptSent",
        "Interrupted",
        "GateAnswered",
        "QuestionAnswered",
        "Subscribed",
        "Error",
        "RefreshCapabilities",
        "CapabilitiesRefreshQueued",
        "CapabilitiesUpdate",
        "AnswerWaslGate",
        "WaslGateRequested",
        "WaslGateDecided",
        "WaslObservation",
        "WaslGateAnswered",
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
