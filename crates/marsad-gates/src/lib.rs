//! marsad-gates — gate domain: permission/plan/edit gates, idempotent decisions,
//! single-session routing.
//!
//! See `docs/specs/gates.md` for the authoritative contract.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

// Re-export diff types so downstream crates (marsad-format) can use
// `marsad_gates::{DiffLine, Sig}` without a direct dep on marsad-diff.
pub use marsad_diff::{DiffLine, Sig};

// ---------------------------------------------------------------------------
// Gate
// ---------------------------------------------------------------------------

/// A pending gate that Claude Code is asking the user to approve or reject.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Gate {
    Permission {
        /// The control-request id this gate answers. Unique per gate occurrence —
        /// the UI uses it as the decision id so distinct gates within one session
        /// are not collapsed by idempotency dedup.
        request_id: String,
        command: String,
        cwd: String,
        risk: String,
        explanation: String,
    },
    Plan {
        summary: String,
        steps: Vec<PlanStep>,
    },
    Edit {
        file: String,
        diff: Vec<DiffLine>,
    },
    Question {
        /// The control-request id this gate answers.
        request_id: String,
        questions: Vec<QuestionPrompt>,
    },
}

// ---------------------------------------------------------------------------
// QuestionPrompt / QuestionOption
// ---------------------------------------------------------------------------

/// One question in an `AskUserQuestion` gate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuestionPrompt {
    pub question: String,
    pub header: String,
    pub multi_select: bool,
    pub options: Vec<QuestionOption>,
}

/// One selectable option inside a `QuestionPrompt`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
}

// ---------------------------------------------------------------------------
// PlanStep
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanStep {
    pub title: String,
    pub detail: String,
}

// ---------------------------------------------------------------------------
// DecisionId
// ---------------------------------------------------------------------------

/// A stable, deterministic identifier derived from `(session_id, gate identity)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DecisionId(pub String);

// ---------------------------------------------------------------------------
// Decision
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Decision {
    Approve,
    Reject { reason: String },
}

// ---------------------------------------------------------------------------
// GateDecision
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateDecision {
    pub id: DecisionId,
    pub session_id: String,
    pub decision: Decision,
}

// ---------------------------------------------------------------------------
// GateError
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Error)]
pub enum GateError {
    #[error("unknown gate")]
    UnknownGate,
    #[error("closed gate")]
    ClosedGate,
    #[error("duplicate decision: {0:?}")]
    DuplicateDecision(DecisionId),
}

// ---------------------------------------------------------------------------
// DecisionLedger
// ---------------------------------------------------------------------------

/// Tracks which decision ids have been delivered, enforcing idempotency.
#[derive(Debug, Clone)]
pub struct DecisionLedger {
    seen: HashSet<String>,
}

#[allow(clippy::new_without_default)]
impl DecisionLedger {
    pub fn new() -> Self {
        Self {
            seen: HashSet::new(),
        }
    }

    /// Record a decision. Returns `Err(DuplicateDecision)` if the same id has
    /// already been recorded.
    pub fn record(&mut self, d: &GateDecision) -> Result<(), GateError> {
        if self.seen.contains(&d.id.0) {
            return Err(GateError::DuplicateDecision(d.id.clone()));
        }
        self.seen.insert(d.id.0.clone());
        Ok(())
    }

    /// Returns `true` if this id has already been recorded.
    pub fn seen(&self, id: &DecisionId) -> bool {
        self.seen.contains(&id.0)
    }
}

// ---------------------------------------------------------------------------
// decision_id — stable, deterministic
// ---------------------------------------------------------------------------

/// Compute a stable `DecisionId` from `(session_id, gate)`.
///
/// Stability guarantee: identical inputs always produce identical ids, across
/// process restarts, because the hash function used is purely content-driven
/// (no random seed). We use `DefaultHasher` seeded to a fixed value by feeding
/// a discriminant tag and the gate's identifying fields.
///
/// To ensure cross-restart stability we use a simple, deterministic format:
/// concatenate the variant tag, session_id, and identifying gate fields into a
/// canonical string, then take a hex-encoded SHA-256-style digest. Because we
/// cannot use external crates, we use a hand-rolled FNV-1a 64-bit hash which
/// is deterministic across runs (unlike `DefaultHasher`).
pub fn decision_id(session_id: &str, gate: &Gate) -> DecisionId {
    // Build a canonical, unambiguous string for the (session_id, gate) pair.
    let canonical = gate_canonical(session_id, gate);
    let hash = fnv1a_64(canonical.as_bytes());
    DecisionId(format!("{hash:016x}"))
}

/// Build an unambiguous canonical string for hashing.
fn gate_canonical(session_id: &str, gate: &Gate) -> String {
    match gate {
        Gate::Permission { command, cwd, .. } => {
            // Use variant tag + session + command + cwd as the identity.
            format!("permission\x00{session_id}\x00{command}\x00{cwd}")
        }
        Gate::Plan { summary, .. } => {
            format!("plan\x00{session_id}\x00{summary}")
        }
        Gate::Edit { file, .. } => {
            format!("edit\x00{session_id}\x00{file}")
        }
        Gate::Question { request_id, .. } => {
            format!("question\x00{session_id}\x00{request_id}")
        }
    }
}

/// FNV-1a 64-bit hash — deterministic, no random seed, stable across restarts.
fn fnv1a_64(data: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 14695981039346656037;
    const PRIME: u64 = 1099511628211;
    let mut hash = OFFSET_BASIS;
    for &byte in data {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

// ---------------------------------------------------------------------------
// approve / reject / route
// ---------------------------------------------------------------------------

/// Build an approval `GateDecision`.
pub fn approve(session_id: &str, gate: &Gate) -> GateDecision {
    GateDecision {
        id: decision_id(session_id, gate),
        session_id: session_id.to_string(),
        decision: Decision::Approve,
    }
}

/// Build a rejection `GateDecision`.
pub fn reject(session_id: &str, gate: &Gate, reason: &str) -> GateDecision {
    GateDecision {
        id: decision_id(session_id, gate),
        session_id: session_id.to_string(),
        decision: Decision::Reject {
            reason: reason.to_string(),
        },
    }
}

/// Route a decision to its single owning session.
pub fn route(decision: &GateDecision) -> &str {
    &decision.session_id
}

// ---------------------------------------------------------------------------
// to_control_response
// ---------------------------------------------------------------------------

/// Encode a `GateDecision` as a `marsad_proto::ControlResponse`.
///
/// - `Approve` → `allow: true`, `deny_reason: None`
/// - `Reject`  → `allow: false`, `deny_reason: Some(reason)`
pub fn to_control_response(decision: &GateDecision) -> marsad_proto::ControlResponse {
    match &decision.decision {
        Decision::Approve => marsad_proto::ControlResponse {
            request_id: decision.id.0.clone(),
            allow: true,
            deny_reason: None,
            // The original tool input isn't known at the gate-domain layer; the
            // agent fills `updated_input` from the pending `can_use_tool`
            // request before encoding the wire response.
            updated_input: None,
        },
        Decision::Reject { reason } => marsad_proto::ControlResponse {
            request_id: decision.id.0.clone(),
            allow: false,
            deny_reason: Some(reason.clone()),
            updated_input: None,
        },
    }
}
