//! X2 — Wasl connector wiring: config loading, `WaslEvent` -> IPC-line mapping,
//! and the connector registry/factory test seams.
//!
//! Authoritative spec: `docs/specs/wasl-connector.md` §4.

use marsad_proto::{encode_ipc, IpcMsg};

use super::Agent;

// ---------------------------------------------------------------------------
// §4.1 — load_wasl_config
// ---------------------------------------------------------------------------

/// Read `<data_root>/wasl.json`. Absent file -> `None` (the inert path).
/// Malformed JSON / empty `operators` -> `None` + one stderr line (config
/// problems must never take the agent down). Duplicate `host_id` -> first
/// entry wins + stderr line.
pub fn load_wasl_config(data_root: &str) -> Option<marsad_wasl::WaslOperatorsConfig> {
    let path = std::path::Path::new(data_root).join("wasl.json");
    let text = std::fs::read_to_string(&path).ok()?;

    let parsed: marsad_wasl::WaslOperatorsConfig = match serde_json::from_str(&text) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!(
                "marsad-agent: malformed wasl config {}: {e}",
                path.display()
            );
            return None;
        }
    };

    if parsed.operators.is_empty() {
        eprintln!(
            "marsad-agent: wasl config {} has no operators",
            path.display()
        );
        return None;
    }

    let mut seen = std::collections::HashSet::new();
    let mut operators = Vec::with_capacity(parsed.operators.len());
    for entry in parsed.operators {
        if seen.insert(entry.host_id.clone()) {
            operators.push(entry);
        } else {
            eprintln!(
                "marsad-agent: duplicate wasl operator host_id {:?} in {} — first entry wins",
                entry.host_id,
                path.display()
            );
        }
    }

    Some(marsad_wasl::WaslOperatorsConfig { operators })
}

// ---------------------------------------------------------------------------
// §4.2 — wasl_event_lines (pure mapping)
// ---------------------------------------------------------------------------

/// Pure mapping (unit-testable without sockets): `WaslEvent` -> zero or more
/// `encode_ipc`'d broadcast lines.
///
/// - `Connected{..}`         -> `[]` (stderr log only)
/// - `GateRequested(g)`      -> `[WaslGateRequested{ host_id, ...g }]`
/// - `GateDecided(d)`        -> `[WaslGateDecided{ host_id, ...d }]`
/// - `Audit(e)`              -> `[WaslObservation{ host_id, entry: to_value(e) }]`
/// - `GateVanished{gate_id}` -> `[WaslGateDecided{ host_id, gate_id,
///   outcome:"unknown", decided_by:"resync", reason:None, decided_at:"" }]`
/// - `Disconnected{..}`      -> `[]` (stderr log only)
pub fn wasl_event_lines(host_id: &str, ev: &marsad_wasl::WaslEvent) -> Vec<String> {
    match ev {
        marsad_wasl::WaslEvent::Connected { hello } => {
            eprintln!(
                "marsad-agent: wasl connector for host {host_id} connected (agent_id={}, last_seq={})",
                hello.agent_id, hello.last_seq
            );
            vec![]
        }
        marsad_wasl::WaslEvent::Disconnected { error } => {
            eprintln!("marsad-agent: wasl connector for host {host_id} disconnected: {error}");
            vec![]
        }
        marsad_wasl::WaslEvent::GateRequested(g) => vec![encode_ipc(&IpcMsg::WaslGateRequested {
            host_id: host_id.to_string(),
            gate_id: g.gate_id.clone(),
            mode: g.mode.clone(),
            decidable: g.decidable,
            request: serde_json::to_value(&g.request).expect("GateRequest always serializes"),
            requested_at: g.requested_at.clone(),
            timeout_ms: g.timeout_ms,
            deadline: g.deadline.clone(),
        })],
        marsad_wasl::WaslEvent::GateDecided(d) => vec![encode_ipc(&IpcMsg::WaslGateDecided {
            host_id: host_id.to_string(),
            gate_id: d.gate_id.clone(),
            outcome: d.outcome.clone(),
            decided_by: d.decided_by.clone(),
            reason: d.reason.clone(),
            decided_at: d.decided_at.clone(),
        })],
        marsad_wasl::WaslEvent::Audit(entry) => vec![encode_ipc(&IpcMsg::WaslObservation {
            host_id: host_id.to_string(),
            entry: serde_json::to_value(entry).expect("AuditEntry always serializes"),
        })],
        marsad_wasl::WaslEvent::GateVanished { gate_id } => {
            vec![encode_ipc(&IpcMsg::WaslGateDecided {
                host_id: host_id.to_string(),
                gate_id: gate_id.clone(),
                outcome: "unknown".to_string(),
                decided_by: "resync".to_string(),
                reason: None,
                decided_at: String::new(),
            })]
        }
    }
}

// ---------------------------------------------------------------------------
// §4.2/§4.3 — connector registry + factory test seams
// ---------------------------------------------------------------------------

/// The `set_wasl_connector_factory` parameter type, pulled out to a named
/// alias so clippy's `type_complexity` lint doesn't fire on the signature
/// (structurally identical for callers — Rust doesn't care about alias names
/// at the call site).
pub type WaslConnectorFactoryFn =
    Box<dyn Fn(&marsad_wasl::WaslOperatorEntry) -> Box<dyn marsad_wasl::WaslConnector> + Send>;

impl Agent {
    /// Additive test seam (also the direct registration seam used by
    /// `serve()`'s real-IO wiring): register a connector for `host_id`,
    /// making it reachable from `handle_ipc`'s `AnswerWaslGate` arm.
    pub fn register_wasl_connector(
        &mut self,
        host_id: &str,
        connector: Box<dyn marsad_wasl::WaslConnector>,
    ) {
        self.wasl_connectors
            .0
            .lock()
            .unwrap()
            .insert(host_id.to_string(), connector);
    }

    /// Additive test seam; default = the real client factory (supplied
    /// lazily by `serve()`, since it is only ever needed from real-IO code).
    /// Setter so no constructor changes.
    pub fn set_wasl_connector_factory(&mut self, f: WaslConnectorFactoryFn) {
        self.wasl_connector_factory.0 = Some(f);
    }
}
