//! Domain output — `WaslEvent` / `DecisionAck` (`docs/specs/wasl-connector.md`
//! §2.4 / §2.5).

use crate::types::{AuditEntry, GateDecided, GateRequested, Hello};

#[derive(Debug, Clone, PartialEq)]
pub enum WaslEvent {
    /// `hello` parsed and `api_version` verified (== 1).
    Connected {
        hello: Hello,
    },
    GateRequested(GateRequested),
    GateDecided(GateDecided),
    Audit(AuditEntry),
    /// A tracked pending gate is absent from a reconcile snapshot and no
    /// `gate_decided` frame was seen: it resolved while this connection
    /// lagged/was down. The outcome is UNKNOWN — the connector never invents
    /// one.
    GateVanished {
        gate_id: String,
    },
    /// Connection lost; the client will retry with backoff. Informational.
    Disconnected {
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum DecisionAck {
    /// 204 — decision DELIVERED to the gate (not proof any send succeeded;
    /// the `gate_decided` frame + audit feed are the observables).
    Delivered,
    /// 409 `already_decided` — benign race.
    AlreadyDecided,
    /// 404 `unknown_gate` — never a decidable pending gate on this node.
    UnknownGate,
}
