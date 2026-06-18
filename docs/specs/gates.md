# B03 — `marsad-gates`

**Tier:** L0. **Mode:** one-shot. **Coverage:** 100% line+region.

## Purpose

The gate domain: model permission/plan/edit gates, build idempotent decision IDs, and produce
`GateDecision` payloads for Claude Code's control protocol. Guarantees a decision routes to **exactly
one** session and that re-delivering the same decision is a no-op (idempotent).

## Public API (authoritative)

```rust
// Re-export the diff line types so downstream crates (and tests) can name them as
// `marsad_gates::DiffLine` / `marsad_gates::Sig` without each depending on marsad-diff directly.
// REQUIRED public surface — `marsad-format` tests import `marsad_gates::{DiffLine, Sig}`.
pub use marsad_diff::{DiffLine, Sig};

pub enum Gate {
    Permission { command: String, cwd: String, risk: String, explanation: String },
    Plan { summary: String, steps: Vec<PlanStep> },
    Edit { file: String, diff: Vec<DiffLine> },   // DiffLine re-exported above
}
pub struct PlanStep { pub title: String, pub detail: String }

pub struct DecisionId(pub String);           // stable, derived from (session_id, gate identity)

pub enum Decision { Approve, Reject { reason: String } }

pub struct GateDecision {
    pub id: DecisionId,
    pub session_id: String,
    pub decision: Decision,
}

pub fn decision_id(session_id: &str, gate: &Gate) -> DecisionId;   // deterministic; same inputs -> same id
pub fn approve(session_id: &str, gate: &Gate) -> GateDecision;
pub fn reject(session_id: &str, gate: &Gate, reason: &str) -> GateDecision;
pub fn route(decision: &GateDecision) -> &str;                    // -> the one session_id

/// Encode for the control protocol (allow {behavior:"allow", updatedInput?} / deny {behavior:"deny", message}).
pub fn to_control_response(decision: &GateDecision) -> marsad_proto::ControlResponse;

pub enum GateError { UnknownGate, ClosedGate, DuplicateDecision(DecisionId) }

/// Tracks which decision ids have been delivered, to enforce idempotency.
pub struct DecisionLedger { /* ... */ }
impl DecisionLedger {
    pub fn new() -> Self;
    pub fn record(&mut self, d: &GateDecision) -> Result<(), GateError>;  // Err(DuplicateDecision) on repeat
    pub fn seen(&self, id: &DecisionId) -> bool;
}
```

Derive `Debug, Clone, PartialEq` (+ serde) on public types.

## Behavior notes (port `gateMeta` + decision flow from both `.dc.html`)

- `decision_id` must be stable across process restarts for the same `(session_id, gate)` — hash the
  gate's identifying content (e.g. permission command+cwd; plan summary; edit file). This is what
  makes re-delivery safe.
- `approve`/`reject` build a `GateDecision`; `to_control_response` maps Approve→allow, Reject→deny
  with the reason as `message`.
- `DecisionLedger.record` returns `DuplicateDecision` the second time the same id is recorded —
  callers treat that as an idempotent no-op (the integration test `i02` asserts this).
- `route` always yields exactly one session id (the decision's own).

## Test buckets (`tests/`)

- `happy.rs` — approve + reject for each gate type; `to_control_response` mapping; `route`.
- `errors.rs` — `UnknownGate`/`ClosedGate` paths; `DuplicateDecision` on second `record`.
- `branches.rs` — `decision_id` stability (same inputs → equal; different session or gate → distinct);
  each `Gate` variant; empty reason; ledger `seen` true/false.

## Edit scope

Code-agent: write `crates/marsad-gates/src/**` + `Cargo.toml`; read-only `tests/`, this spec.
The crate already depends on `marsad-diff`; the code-agent MUST add `pub use marsad_diff::{DiffLine,
Sig};` at the crate root so the re-export above is part of the public API (the `marsad-format` test
suite relies on `marsad_gates::{DiffLine, Sig}` resolving).
