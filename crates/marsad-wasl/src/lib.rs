//! `marsad-wasl` — Wasl operator-API v1 connector (X1, crate bucket L1).
//!
//! Authoritative contract: `docs/specs/wasl-connector.md` §2. This crate sits
//! beside `marsad-transport` in the DAG (`wasl ← agent, core`; no
//! `agent → core` edge introduced) and depends on **no other Marsad crate** —
//! only `serde`, `serde_json`, `thiserror`. It never depends on a Wasl crate:
//! the wire contract is spec + the vendored golden fixtures at
//! `fixtures/operator-v1/` (§1), not shared code.
//!
//! See `./AGENTS.md` for the coverage regime (the `cfg(coverage)` gating
//! around the real `WaslConnector` impl on `WaslOperatorClient`, and the
//! documented "trusted v1 node" simplifications).

mod client;
mod error;
mod event;
mod fake;
mod sse;
mod types;
mod wire;

pub use client::{
    wasl_backoff_next, WaslClientConfig, WaslConnector, WaslOperatorClient, WASL_BACKOFF_MAX_MS,
};
pub use error::WaslError;
pub use event::{DecisionAck, WaslEvent};
pub use fake::FakeWaslConnector;
pub use sse::{SseFrame, SseParser};
pub use types::{
    AuditDecision, AuditEntry, AuditFrame, DecisionRequest, ErrorBody, GateDecided, GateRequest,
    GateRequested, GatesSnapshot, Hello, OperatorInfo, PendingGate, WaslOperatorEntry,
    WaslOperatorsConfig,
};
