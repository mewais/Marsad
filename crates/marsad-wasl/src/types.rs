//! Typed frame structs mirroring the vendored golden fixtures verbatim
//! (`docs/specs/wasl-connector.md` §2.1) plus the shared agent-config shape
//! (§2.7). No `deny_unknown_fields` anywhere: the contract's unknown-field
//! tolerance is mandatory, and closed-set wire strings (`kind`, `mode`,
//! `outcome`, `decided_by`, `resume`, `error`) stay plain `String` — Marsad
//! relays them, it never branches on an exhaustive enum a v1-additive server
//! could outgrow.

use serde::{Deserialize, Serialize};

/// `GET /v1/info` body — fixture `info.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperatorInfo {
    pub agent_id: String,
    pub host_id: String,
    pub version: String,
}

/// `hello` frame payload — fixture `hello.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub api_version: u64,
    pub agent_id: String,
    pub host_id: String,
    pub version: String,
    pub last_seq: u64,
    /// v1: always `"snapshot"`.
    pub resume: String,
}

/// Embedded `GateRequest` — verbatim Wasl wire form (inside `gate_requested_*`
/// and `gates_snapshot`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateRequest {
    /// The initiating tool-call id. MAY repeat; never correlate by it.
    pub id: String,
    /// "consult" | "command" | "message" | "transfer"
    pub kind: String,
    pub from: String,
    pub summary: String,
    pub preview: String,
}

/// `gate_requested` frame payload — fixtures
/// `gate_requested_{external,elicitation}.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateRequested {
    pub gate_id: String,
    /// "elicitation" | "external" ("prepare_commit" reserved).
    pub mode: String,
    pub decidable: bool,
    pub request: GateRequest,
    pub requested_at: String,
    pub timeout_ms: u64,
    pub deadline: String,
}

/// `gate_decided` frame payload — fixtures `gate_decided_*.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateDecided {
    pub gate_id: String,
    /// "approved" | "denied" | "dismissed"
    pub outcome: String,
    /// "operator" | "elicitation" | "timeout"
    pub decided_by: String,
    /// String iff denied; the fixtures show explicit `null` otherwise.
    pub reason: Option<String>,
    pub decided_at: String,
}

/// One audit-feed entry — embedded in the `audit` frame wrapper (fixtures
/// `audit_{decision,message}.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// The audit log's own sequence, independent of the SSE `id:`.
    pub seq: u64,
    /// "consult"|"command"|"message"|"transfer"|"decision"
    pub kind: String,
    pub actor: String,
    pub targets: Vec<String>,
    pub summary: String,
    /// `null` on auto-approved sends.
    pub decision: Option<AuditDecision>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditDecision {
    /// "approve" | "deny"
    pub decision: String,
    /// Present on deny.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The `audit` frame wrapper — fixtures `audit_{decision,message}.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditFrame {
    pub entry: AuditEntry,
}

/// `GET /v1/gates` body — fixture `gates_snapshot.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GatesSnapshot {
    pub last_seq: u64,
    pub gates: Vec<PendingGate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingGate {
    pub gate_id: String,
    pub request: GateRequest,
    pub requested_at: String,
    pub age_ms: u64,
    pub deadline: String,
}

/// `POST /v1/gates/{id}/decision` body — fixtures
/// `decision_request_{approved,denied}.json`. Golden encode: approve emits NO
/// `reason` key at all (`skip_serializing_if`), deny carries it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRequest {
    /// "approved"|"denied"|"dismissed"
    pub outcome: String,
    /// Only meaningful with "denied".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// 4xx body — fixtures `error_*.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: String,
    pub detail: String,
}

// ---------------------------------------------------------------------------
// §2.7 — shared config shape for the agent's `wasl.json`.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaslOperatorsConfig {
    pub operators: Vec<WaslOperatorEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaslOperatorEntry {
    /// Marsad host-registry id this node belongs to ("local" for the local host).
    pub host_id: String,
    /// Operator API base URL, reachable from the agent.
    pub url: String,
}
