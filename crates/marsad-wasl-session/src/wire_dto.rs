//! Private wire-DTO translation layer (task finding #6/#7; mirrors how
//! `marsad-wasl/src/wire.rs` translates between the operator wire and the public
//! types). Marsad's public caller-mirror types ([`crate::WaslTargetStatus`],
//! [`crate::WaslTurnReceipt`], [`crate::WaslSessionEventKind`], …) derive PLAIN,
//! externally-tagged serde — but the real Wasl runtime-control wire is
//! **internally-tagged snake_case** (`{"type":"started",...}`,
//! `{"type":"idle",...}`) per `Wasl/docs/specs/runtime-control-api.md`. This
//! module holds PRIVATE DTOs with `#[serde(tag = "type", rename_all =
//! "snake_case")]` matching the exact real wire shapes, and converts to/from the
//! frozen public types. The public types' own serde derives are NOT changed
//! (they are frozen and used elsewhere).
//!
//! Always compiled; the `real_*.rs` tests drive every translation through the
//! real client's parse/build paths.

use serde::{Deserialize, Serialize};

use crate::types::{
    WaslAgentCard, WaslBusyPolicy, WaslCancelReason, WaslCapabilities, WaslCausation,
    WaslGateDecision, WaslNativeGateDetail, WaslSessionEventKind, WaslShutdownMode,
    WaslTargetStatus, WaslTurnReceipt, WaslTurnRequest,
};

// ---------------------------------------------------------------------------
// Payload structs for the rich informational TurnEvent variants. Field names
// and serde attributes match wasl-runtime's definitions EXACTLY (they mirror
// the `wasit_core` shapes that wasl-runtime replicated verbatim). We replicate
// them here rather than adding a wasl-runtime dev-dep so the consumer crate
// stays independent of the Wasl workspace.
// ---------------------------------------------------------------------------

/// Token and cost snapshot for `UsageUpdate`. Mirrors `wasl_runtime::UsageSnapshot`
/// (snake_case fields, all defaulted).
#[derive(Debug, Clone, Deserialize)]
struct UsageSnapshotDto {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_creation_tokens: u64,
    #[serde(default)]
    pub cost_usd: f64,
}

/// One selectable agent mode for `ModesUpdated`. Mirrors `wasl_runtime::AgentMode`
/// (snake_case, re-uses marsad_proto::SessionMode's {id, label, description} shape).
#[derive(Debug, Clone, Deserialize)]
struct AgentModeDto {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
}

impl AgentModeDto {
    fn into_session_mode(self) -> marsad_proto::SessionMode {
        marsad_proto::SessionMode {
            id: self.id,
            label: self.label,
            description: self.description,
        }
    }
}

/// One plan entry for `PlanUpdated`. Mirrors `wasl_runtime::PlanEntry`.
#[derive(Debug, Clone, Deserialize)]
struct PlanEntryDto {
    pub content: String,
    #[serde(default)]
    pub priority: String,
}

impl PlanEntryDto {
    fn into_plan_entry(self) -> marsad_proto::PlanEntry {
        marsad_proto::PlanEntry {
            content: self.content,
            priority: self.priority,
        }
    }
}

/// One slash command for `SlashCommandsUpdated`. Mirrors `wasl_runtime::SlashCommandInfo`.
#[derive(Debug, Clone, Deserialize)]
struct SlashCommandInfoDto {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub argument_hint: String,
}

impl SlashCommandInfoDto {
    fn into_slash_command(self) -> marsad_proto::SlashCommandInfo {
        marsad_proto::SlashCommandInfo {
            name: self.name,
            description: self.description,
            argument_hint: self.argument_hint,
        }
    }
}

/// Compaction metadata for `Compaction`. Mirrors `wasl_runtime::CompactionInfo`.
#[derive(Debug, Clone, Deserialize)]
struct CompactionInfoDto {
    #[serde(default)]
    pub trigger: String,
    #[serde(default)]
    pub pre_tokens: u64,
    #[serde(default)]
    pub post_tokens: u64,
    #[serde(default)]
    pub cumulative_dropped_tokens: u64,
    #[serde(default)]
    pub duration_ms: u64,
}

impl CompactionInfoDto {
    fn into_compaction_info(self) -> marsad_proto::CompactionInfo {
        marsad_proto::CompactionInfo {
            trigger: self.trigger,
            pre_tokens: self.pre_tokens,
            post_tokens: self.post_tokens,
            cumulative_dropped_tokens: self.cumulative_dropped_tokens,
            duration_ms: self.duration_ms,
        }
    }
}

// ===========================================================================
// TargetSnapshot — GET /v1/targets/{id} and each entry of GET /v1/targets.
// ===========================================================================

/// The wire `TargetSnapshot` (`target.json`, and each element of
/// `targets_snapshot.json`). The provider parses the FULL shape so it can build
/// the caller-side [`crate::WaslTargetSnapshot`] the §6.1 reconnect comparison
/// runs against — `last_seq` is the crux of that decision, so it is parsed
/// HONESTLY here (never synthesized/hardcoded). `status` is an internally-tagged
/// wire status; the remaining fields default (no `deny_unknown_fields`,
/// forward-compat) so a `native_session_id`-less snapshot still parses.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct TargetSnapshotDto {
    pub target_id: String,
    pub capabilities: WaslCapabilities,
    #[serde(default)]
    pub status: Option<StatusDto>,
    #[serde(default)]
    pub last_seq: u64,
    #[serde(default)]
    pub queued: usize,
    #[serde(default)]
    pub native_session_id: Option<String>,
}

impl TargetSnapshotDto {
    /// The advisory status the wire snapshot carried, or a conservative `Offline`
    /// when the snapshot omitted `status` entirely.
    pub(crate) fn status_public(&self) -> WaslTargetStatus {
        self.status
            .clone()
            .map(StatusDto::into_public)
            .unwrap_or(WaslTargetStatus::Offline)
    }
}

/// The wire `GET /v1/targets` list body (`targets_snapshot.json`).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct TargetsSnapshotDto {
    pub targets: Vec<TargetSnapshotDto>,
}

// ===========================================================================
// TargetStatus — internally-tagged snake_case (`{"type":"idle",...}`).
// ===========================================================================

/// Wire `TargetStatus`. `source` on `busy` is a plain string here (the
/// caller-mirror keeps it a `String` Marsad only relays/renders).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum StatusDto {
    Offline,
    Starting,
    Idle {
        #[serde(default)]
        since: String,
    },
    Busy {
        #[serde(default)]
        active_turn: String,
        #[serde(default)]
        source: String,
        #[serde(default)]
        phase: String,
        #[serde(default)]
        since: String,
        #[serde(default)]
        queued: usize,
    },
    AwaitingGate {
        #[serde(default)]
        active_turn: String,
        detail: WaslNativeGateDetail,
        #[serde(default)]
        since: String,
        #[serde(default)]
        queued: usize,
    },
    Interrupting {
        #[serde(default)]
        active_turn: String,
        #[serde(default)]
        since: String,
    },
    Draining {
        #[serde(default)]
        active_turn: String,
        #[serde(default)]
        since: String,
    },
    Faulted {
        #[serde(default)]
        error: String,
        #[serde(default)]
        recoverable: bool,
    },
    Stopped,
}

impl StatusDto {
    /// Translate the wire status into Marsad's public [`WaslTargetStatus`]. An
    /// unrecognized wire `type` deserializes as an error at the serde layer; the
    /// caller maps that (and any absent status) to a conservative status.
    pub(crate) fn into_public(self) -> WaslTargetStatus {
        match self {
            StatusDto::Offline => WaslTargetStatus::Offline,
            StatusDto::Starting => WaslTargetStatus::Starting,
            StatusDto::Idle { since } => WaslTargetStatus::Idle { since },
            StatusDto::Busy {
                active_turn,
                source,
                phase,
                since,
                queued,
            } => WaslTargetStatus::Busy {
                active_turn,
                source,
                phase,
                since,
                queued,
            },
            StatusDto::AwaitingGate {
                active_turn,
                detail,
                since,
                queued,
            } => WaslTargetStatus::AwaitingGate {
                active_turn,
                detail,
                since,
                queued,
            },
            StatusDto::Interrupting { active_turn, since } => {
                WaslTargetStatus::Interrupting { active_turn, since }
            }
            StatusDto::Draining { active_turn, since } => {
                WaslTargetStatus::Draining { active_turn, since }
            }
            StatusDto::Faulted { error, recoverable } => {
                WaslTargetStatus::Faulted { error, recoverable }
            }
            StatusDto::Stopped => WaslTargetStatus::Stopped,
        }
    }
}

/// Parse a wire `TargetStatus` JSON value into the public type, tolerating an
/// unrecognized `type` by projecting it onto the forward-compat `Unknown` arm
/// (its raw tag preserved for logging), per the caller-mirror's forward-compat
/// convention (§4.4).
pub(crate) fn status_from_value(value: &serde_json::Value) -> WaslTargetStatus {
    match serde_json::from_value::<StatusDto>(value.clone()) {
        Ok(dto) => dto.into_public(),
        Err(_) => {
            let tag = value
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            WaslTargetStatus::Unknown(tag)
        }
    }
}

// ===========================================================================
// TurnReceipt — internally-tagged snake_case (`receipt_*.json`).
// ===========================================================================

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ReceiptDto {
    Started { turn_id: String, first_seq: u64 },
    Queued { turn_id: String, position: usize },
    RejectedBusy { active_turn: String },
    RejectedUnavailable { status: StatusDto },
    Duplicate { original: Box<ReceiptDto> },
}

impl ReceiptDto {
    pub(crate) fn into_public(self) -> WaslTurnReceipt {
        match self {
            ReceiptDto::Started { turn_id, first_seq } => {
                WaslTurnReceipt::Started { turn_id, first_seq }
            }
            ReceiptDto::Queued { turn_id, position } => {
                WaslTurnReceipt::Queued { turn_id, position }
            }
            ReceiptDto::RejectedBusy { active_turn } => {
                WaslTurnReceipt::RejectedBusy { active_turn }
            }
            ReceiptDto::RejectedUnavailable { status } => WaslTurnReceipt::RejectedUnavailable {
                status: status.into_public(),
            },
            ReceiptDto::Duplicate { original } => WaslTurnReceipt::Duplicate {
                original: Box::new(original.into_public()),
            },
        }
    }
}

// ===========================================================================
// TurnEvent — internally-tagged snake_case (`turn_event_*.json`).
// ===========================================================================

/// The wire `TurnSource` (`{"type":"local_human"}` | `{"type":"agent",...}`).
/// Marsad relays it as the tag string only.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SourceDto {
    LocalHuman,
    // The wire `agent` source also carries `from`; Marsad relays only the tag
    // string, and (no `deny_unknown_fields`) tolerantly ignores `from`.
    Agent,
}

impl SourceDto {
    fn tag(&self) -> String {
        match self {
            SourceDto::LocalHuman => "local_human".to_string(),
            SourceDto::Agent => "agent".to_string(),
        }
    }
}

/// The wire `TurnEvent`. Marsad maps each to a public [`WaslSessionEventKind`];
/// the wire `process_exited` maps to Marsad's `SessionExited` (§4.4).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum TurnEventDto {
    SessionReady {
        #[serde(default)]
        native: Option<String>,
    },
    TurnAccepted {
        turn_id: String,
        source: SourceDto,
    },
    TurnQueued {
        turn_id: String,
        position: usize,
    },
    AssistantDelta {
        turn_id: String,
        text: String,
    },
    ToolStarted {
        turn_id: String,
        name: String,
    },
    ToolFinished {
        turn_id: String,
        name: String,
        ok: bool,
    },
    NativeGateRequested {
        turn_id: String,
        detail: WaslNativeGateDetail,
    },
    NativeGateResolved {
        turn_id: String,
        decision: String,
    },
    TurnCompleted {
        turn_id: String,
        reply: String,
        certainty: String,
    },
    TurnFailed {
        turn_id: String,
        error: String,
        certainty: String,
    },
    TurnCancelled {
        turn_id: String,
        reason: String,
    },
    TurnExpired {
        turn_id: String,
    },
    StatusChanged {
        status: StatusDto,
    },
    ProcessExited {
        #[serde(default)]
        code: Option<i32>,
    },
    // ── Rich informational variants (added by wasit-adapter work) ─────────────
    // These carry no `turn_id` — they are session-global, not per-turn.
    /// Incremental token/cost snapshot. Wire tag `"usage_update"`.
    UsageUpdate {
        usage: UsageSnapshotDto,
        #[serde(default)]
        context_pct: Option<u8>,
    },
    /// Full replacement of the opencode plan/todo list. Wire tag `"plan_updated"`.
    PlanUpdated {
        entries: Vec<PlanEntryDto>,
    },
    /// Available mode list or current mode changed. Wire tag `"modes_updated"`.
    ModesUpdated {
        #[serde(default)]
        available_modes: Vec<AgentModeDto>,
        #[serde(default)]
        current_mode: Option<String>,
    },
    /// Active model changed mid-session. Wire tag `"model_changed"`.
    ModelChanged {
        model: String,
    },
    /// Slash-command catalog replaced. Wire tag `"slash_commands_updated"`.
    SlashCommandsUpdated {
        commands: Vec<SlashCommandInfoDto>,
    },
    /// Context compaction occurred. Wire tag `"compaction"`.
    Compaction {
        info: CompactionInfoDto,
    },
    // `thinking_delta` is deliberately NOT mapped — Marsad drops thinking deltas
    // (like the solo-session path does). An unknown `type` is still tolerantly
    // skipped by `turn_event_from_json` returning `None`.
}

impl TurnEventDto {
    fn into_public(self) -> WaslSessionEventKind {
        match self {
            TurnEventDto::SessionReady { native } => WaslSessionEventKind::SessionReady { native },
            TurnEventDto::TurnAccepted { turn_id, source } => WaslSessionEventKind::TurnAccepted {
                turn_id,
                source: source.tag(),
            },
            TurnEventDto::TurnQueued { turn_id, position } => {
                WaslSessionEventKind::TurnQueued { turn_id, position }
            }
            TurnEventDto::AssistantDelta { turn_id, text } => {
                WaslSessionEventKind::AssistantDelta { turn_id, text }
            }
            TurnEventDto::ToolStarted { turn_id, name } => {
                WaslSessionEventKind::ToolStarted { turn_id, name }
            }
            TurnEventDto::ToolFinished { turn_id, name, ok } => {
                WaslSessionEventKind::ToolFinished { turn_id, name, ok }
            }
            TurnEventDto::NativeGateRequested { turn_id, detail } => {
                WaslSessionEventKind::NativeGateRequested { turn_id, detail }
            }
            TurnEventDto::NativeGateResolved { turn_id, decision } => {
                WaslSessionEventKind::NativeGateResolved { turn_id, decision }
            }
            TurnEventDto::TurnCompleted {
                turn_id,
                reply,
                certainty,
            } => WaslSessionEventKind::TurnCompleted {
                turn_id,
                reply,
                certainty,
            },
            TurnEventDto::TurnFailed {
                turn_id,
                error,
                certainty,
            } => WaslSessionEventKind::TurnFailed {
                turn_id,
                error,
                certainty,
            },
            TurnEventDto::TurnCancelled { turn_id, reason } => {
                WaslSessionEventKind::TurnCancelled { turn_id, reason }
            }
            TurnEventDto::TurnExpired { turn_id } => WaslSessionEventKind::TurnExpired { turn_id },
            TurnEventDto::StatusChanged { status } => WaslSessionEventKind::StatusChanged {
                status: status.into_public(),
            },
            TurnEventDto::ProcessExited { code } => WaslSessionEventKind::SessionExited { code },
            TurnEventDto::UsageUpdate { usage, context_pct } => {
                WaslSessionEventKind::UsageUpdate {
                    cost_usd: usage.cost_usd,
                    tokens: usage
                        .input_tokens
                        .saturating_add(usage.output_tokens)
                        .saturating_add(usage.cache_read_tokens)
                        .saturating_add(usage.cache_creation_tokens),
                    context_pct,
                }
            }
            TurnEventDto::PlanUpdated { entries } => WaslSessionEventKind::PlanUpdated {
                entries: entries.into_iter().map(PlanEntryDto::into_plan_entry).collect(),
            },
            TurnEventDto::ModesUpdated {
                available_modes,
                current_mode,
            } => WaslSessionEventKind::ModesUpdated {
                available_modes: available_modes
                    .into_iter()
                    .map(AgentModeDto::into_session_mode)
                    .collect(),
                current_mode,
            },
            TurnEventDto::ModelChanged { model } => WaslSessionEventKind::ModelChanged { model },
            TurnEventDto::SlashCommandsUpdated { commands } => {
                WaslSessionEventKind::SlashCommandsUpdated {
                    commands: commands
                        .into_iter()
                        .map(SlashCommandInfoDto::into_slash_command)
                        .collect(),
                }
            }
            TurnEventDto::Compaction { info } => WaslSessionEventKind::Compaction {
                info: info.into_compaction_info(),
            },
        }
    }
}

/// Parse a `turn_event` frame's `data:` payload into a public event kind.
/// Returns `None` for an unrecognized wire `type` (tolerant-skip, §4.4: a future
/// additive variant is logged-and-ignored by the pump, never surfaced as an
/// event) and for a malformed payload.
pub(crate) fn turn_event_from_json(data: &str) -> Option<WaslSessionEventKind> {
    serde_json::from_str::<TurnEventDto>(data)
        .ok()
        .map(TurnEventDto::into_public)
}

/// The wire `truncated` frame (`truncated.json`).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct TruncatedDto {
    pub earliest: u64,
    pub latest: u64,
}

/// Parse a `truncated` frame's `data:` payload.
pub(crate) fn truncated_from_json(data: &str) -> Option<TruncatedDto> {
    serde_json::from_str::<TruncatedDto>(data).ok()
}

// ===========================================================================
// Outbound request bodies (Marsad → wire).
// ===========================================================================

/// The wire `POST /v1/targets` body (`create_request.json`). The node's
/// create-target reads TOP-LEVEL `protocol` and `context` (the `SpawnContext`:
/// cwd/model/session_mode/mcp_config/…); Marsad's caller nests those inside the
/// opaque target key as `{ protocol, context, … }`. Unpack them to the top
/// level so the worker actually receives its protocol + spawn context. The wire
/// `key` is a bare STRING (`TargetKey(String)` — the create-or-reuse/turn-routing
/// context id), lifted via [`target_context_id`], NOT the whole object (sending
/// the object 422s the create).
pub(crate) fn create_request_body(key: &serde_json::Value) -> String {
    let mut body = serde_json::Map::new();
    // The node's `CreateTargetRequest.key` is a bare STRING (the context_id) — NOT
    // the whole opaque object. Lift the string identity; the rich `protocol` /
    // `context` ride as their own top-level fields the node reads directly. Sending
    // the object as `key` makes the node reject the create with a 422
    // ("key: invalid type: map, expected a string"), so no target is ever created.
    body.insert(
        "key".to_string(),
        serde_json::Value::String(target_context_id(key)),
    );
    if let Some(protocol) = key.get("protocol") {
        body.insert("protocol".to_string(), protocol.clone());
    }
    if let Some(context) = key.get("context") {
        body.insert("context".to_string(), context.clone());
    }
    serde_json::to_string(&serde_json::Value::Object(body))
        .expect("create request always serializes")
}

/// Extract the node-side context_id STRING from Marsad's opaque `WaslTargetKey`.
///
/// The node's runtime-control wire uses a plain STRING for BOTH the target `key`
/// (`CreateTargetRequest.key: TargetKey(String)`) and a submit's
/// `delivery.context_id` — it identifies the live target for create-or-reuse and
/// for turn routing. Marsad's [`crate::WaslTargetKey`] is a rich JSON object
/// `{ "key": "<id>", "protocol": "…", "context": { … } }` (the desktop builds it
/// that way). Lift its string `key` field as the identity; a key that is ALREADY
/// a bare string is used verbatim; an object that carries no string `key` falls
/// back to a canonical JSON encoding of the whole value (stable per distinct key,
/// so the create-or-reuse identity is the same every time). Both the create body
/// and the submit body derive the id THROUGH THIS ONE fn so a turn always routes
/// to the target the create made.
pub(crate) fn target_context_id(key: &serde_json::Value) -> String {
    if let Some(s) = key.as_str() {
        return s.to_string();
    }
    if let Some(s) = key.get("key").and_then(|v| v.as_str()) {
        return s.to_string();
    }
    serde_json::to_string(key).unwrap_or_default()
}

/// Serialize a [`WaslBusyPolicy`] to its wire tagged form
/// (`{"type":"queue","max_wait_ms":N}` | `{"type":"reject"}` |
/// `{"type":"queue_related","max_wait_ms":N}`).
fn busy_policy_value(policy: &WaslBusyPolicy) -> serde_json::Value {
    match policy {
        WaslBusyPolicy::Queue { max_wait_ms } => {
            serde_json::json!({ "type": "queue", "max_wait_ms": max_wait_ms })
        }
        WaslBusyPolicy::Reject => serde_json::json!({ "type": "reject" }),
        WaslBusyPolicy::QueueRelated { max_wait_ms } => {
            serde_json::json!({ "type": "queue_related", "max_wait_ms": max_wait_ms })
        }
    }
}

/// Serialize a [`WaslCausation`] to its wire form (`root_request`,
/// `parent_request` (null for Marsad), `path` (`[]`), `hop` (`0`)) — plain-string
/// RequestIds per the wire spec.
fn causation_value(c: &WaslCausation) -> serde_json::Value {
    serde_json::json!({
        "root_request": c.root_request,
        "parent_request": c.parent_request,
        "path": c.path,
        "hop": c.hop,
    })
}

/// Build the wire `POST /v1/targets/{id}/turns` body (`submit_request.json`
/// shape) from a caller-facing [`WaslTurnRequest`] plus the chosen busy policy
/// and the target's key (used to fill `delivery.context_id`). `source` is ALWAYS
/// `local_human` (Marsad never originates an Agent turn, §11).
pub(crate) fn submit_request_body(
    req: &WaslTurnRequest,
    busy_policy: &WaslBusyPolicy,
    key: &serde_json::Value,
) -> String {
    // `delivery.context_id` is the node-side STRING context id (== the target's
    // `key`), NOT Marsad's opaque object. Derive it the SAME way the create body
    // derives the target `key` (`target_context_id`) so the turn routes to the
    // target the create made; sending the object here would 422 the submit.
    let context_id = target_context_id(key);
    let body = serde_json::json!({
        "delivery": {
            "context_id": context_id,
            "from": "human",
            "kind": "message",
            "text": req.text,
            "data": [],
        },
        "source": { "type": "local_human" },
        "idempotency_key": req.idempotency_key,
        "busy_policy": busy_policy_value(busy_policy),
        "causation": causation_value(&req.causation),
    });
    serde_json::to_string(&body).expect("submit request always serializes")
}

/// Parse a `POST /v1/targets/{id}/turns` response body into a public receipt.
pub(crate) fn receipt_from_slice(bytes: &[u8]) -> Option<WaslTurnReceipt> {
    serde_json::from_slice::<ReceiptDto>(bytes)
        .ok()
        .map(ReceiptDto::into_public)
}

/// Serialize a [`WaslGateDecision`] to its wire form
/// (`{"decision":"approve"}` | `{"decision":"deny","reason":"…"}`).
fn gate_decision_value(decision: &WaslGateDecision) -> serde_json::Value {
    match decision {
        WaslGateDecision::Approve => serde_json::json!({ "decision": "approve" }),
        WaslGateDecision::Deny { reason } => {
            serde_json::json!({ "decision": "deny", "reason": reason })
        }
    }
}

/// Build the wire `POST /v1/targets/{id}/gate` body (`gate_request_*.json`):
/// keyed by `turn_id` (there is no gate id), carrying the tagged decision.
pub(crate) fn gate_request_body(turn_id: &str, decision: &WaslGateDecision) -> String {
    let body = serde_json::json!({
        "turn_id": turn_id,
        "decision": gate_decision_value(decision),
    });
    serde_json::to_string(&body).expect("gate request always serializes")
}

/// The wire `reason` string for a [`WaslCancelReason`] (`cancel_request.json`
/// uses `operator_request` for the human/operator-driven cancel).
fn cancel_reason_str(reason: &WaslCancelReason) -> &'static str {
    match reason {
        WaslCancelReason::Human => "operator_request",
        WaslCancelReason::Superseded => "superseded",
    }
}

/// Build the wire `POST /v1/targets/{id}/cancel` body (`cancel_request.json`).
/// Carries a `turn_id` and a `reason`; Marsad targets the active turn (the node
/// no-ops a cancel of a finished turn, per the wire spec).
pub(crate) fn cancel_request_body(active_turn: &str, reason: &WaslCancelReason) -> String {
    let body = serde_json::json!({
        "turn_id": active_turn,
        "reason": cancel_reason_str(reason),
    });
    serde_json::to_string(&body).expect("cancel request always serializes")
}

/// Build the wire `POST /v1/targets/{id}/shutdown` body
/// (`shutdown_request_drain.json`): `{"mode":"drain"}` | `{"mode":"immediate"}`.
pub(crate) fn shutdown_request_body(mode: &WaslShutdownMode) -> String {
    let mode_str = match mode {
        WaslShutdownMode::Drain => "drain",
        WaslShutdownMode::Immediate => "immediate",
    };
    let body = serde_json::json!({ "mode": mode_str });
    serde_json::to_string(&body).expect("shutdown request always serializes")
}

// ===========================================================================
// Peer CRUD — GET /v1/peers + POST /v1/peers + DELETE /v1/peers/:id.
// ===========================================================================

/// Wire shape for one peer entry (element of `GET /v1/peers` response and
/// body of `POST /v1/peers`). `endpoint` is optional on both directions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PeerDto {
    pub id: String,
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

impl PeerDto {
    pub(crate) fn into_public(self) -> WaslAgentCard {
        WaslAgentCard {
            id: self.id,
            host: self.host,
            endpoint: self.endpoint,
        }
    }

    pub(crate) fn from_public(card: &WaslAgentCard) -> Self {
        Self {
            id: card.id.clone(),
            host: card.host.clone(),
            endpoint: card.endpoint.clone(),
        }
    }
}

/// Wire `GET /v1/peers` response body: `{"peers":[…]}`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PeersListDto {
    pub peers: Vec<PeerDto>,
}

/// Serialize a [`WaslAgentCard`] to the wire `POST /v1/peers` body.
pub(crate) fn register_peer_body(card: &WaslAgentCard) -> String {
    let dto = PeerDto::from_public(card);
    serde_json::to_string(&dto).expect("peer card always serializes")
}

/// Parse a `GET /v1/peers` response body into a sorted `Vec<WaslAgentCard>`.
/// Returns `None` on a malformed body.
pub(crate) fn peers_from_slice(bytes: &[u8]) -> Option<Vec<WaslAgentCard>> {
    let dto: PeersListDto = serde_json::from_slice(bytes).ok()?;
    let mut cards: Vec<WaslAgentCard> = dto.peers.into_iter().map(PeerDto::into_public).collect();
    cards.sort_by(|a, b| a.id.cmp(&b.id));
    Some(cards)
}

// ===========================================================================
// Error bodies (4xx `{error, detail}`).
// ===========================================================================

/// A wire error body (`{"error":"…","detail":"…"}`).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ErrorBodyDto {
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub detail: String,
}

/// Parse a 4xx error body, returning `(error, detail)`; falls back to the raw
/// text on an unparseable body so a detail is always surfaced.
pub(crate) fn parse_error_body(bytes: &[u8]) -> (String, String) {
    match serde_json::from_slice::<ErrorBodyDto>(bytes) {
        Ok(e) => (e.error, e.detail),
        Err(_) => (
            String::new(),
            String::from_utf8_lossy(bytes).trim().to_string(),
        ),
    }
}
