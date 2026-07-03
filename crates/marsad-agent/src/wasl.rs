//! X2 — Wasl connector wiring: config loading, `WaslEvent` -> IPC-line mapping,
//! and the connector registry/factory test seams.
//!
//! Authoritative spec: `docs/specs/wasl-connector.md` §4.

use marsad_proto::{encode_ipc, IpcMsg};

use super::Agent;

// ---------------------------------------------------------------------------
// §4.1 — load_wasl_config
// ---------------------------------------------------------------------------

/// Validate (and normalize) a `WaslOperatorEntry.url` against the exact shape
/// `marsad_wasl::WaslClientConfig::base_url` requires: `http://ip:port`, no
/// path, no trailing slash (`marsad-wasl`'s `wire::parse_base_url` `.expect()`s
/// that shape — a value that fails to parse as `SocketAddr` PANICS the caller,
/// not returns an `Err`).
///
/// Found live (2026-07-03, x6_wasl_live): `docs/specs/node-config.md` pins the
/// trailing-slash form (`http://127.0.0.1:4242/`) as "the existing convention"
/// for Wasl peer URLs — exactly the value an operator configuring `wasl.json`
/// from a node's own `[operator] bind` would naturally copy in. Left
/// unvalidated, that convention-shaped value sails through `load_wasl_config`
/// untouched, into `WaslClientConfig.base_url`, and panics the first time it's
/// actually used. For the streaming connector (`WaslOperatorClient::run`,
/// started on its own background thread) that only silently kills the
/// connector — no more Wasl visibility for that host, no crash. For a human
/// decision (`Core::answer_wasl_gate` -> `handle_answer_wasl_gate` ->
/// `connector.decide()` -> `post_decision` -> this same parse) it is FAR
/// worse: `decide()` runs synchronously on the agent's single
/// command-processing thread, which `main.rs` drives directly on the process's
/// main thread — an unhandled panic there takes down the entire daemon on the
/// first Wasl gate a human ever answers, not just the one malformed host.
///
/// `marsad-wasl`'s own frozen suite never catches this: it always constructs
/// `base_url` from `MockServer`'s own precisely-formatted address, never a
/// hand-typed config value. This is exactly the class of defect `load_wasl_config`
/// already exists to firewall (see its own "config problems must never take
/// the agent down" contract for malformed JSON / duplicate host ids) — so a
/// bad url is normalized (strip one trailing slash, the documented-convention
/// case) or, if still not `http://ip:port` after that, dropped with a loud
/// stderr line, the same disposition as every other malformed-entry case
/// below.
fn normalize_operator_url(url: &str) -> Option<String> {
    let trimmed = url.strip_suffix('/').unwrap_or(url);
    let host_port = trimmed.strip_prefix("http://")?;
    host_port.parse::<std::net::SocketAddr>().ok()?;
    Some(trimmed.to_string())
}

/// Read `<data_root>/wasl.json`. Absent file -> `None` (the inert path).
/// Malformed JSON / empty `operators` -> `None` + one stderr line (config
/// problems must never take the agent down). Duplicate `host_id` -> first
/// entry wins + stderr line. A malformed `url` (anything that isn't
/// `http://ip:port`, allowing one trailing slash) is normalized or, failing
/// that, dropped + stderr line — see `normalize_operator_url`. If every entry
/// is filtered out this way, `None` (same "nothing to wire" disposition as an
/// empty `operators` list).
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
    for mut entry in parsed.operators {
        if !seen.insert(entry.host_id.clone()) {
            eprintln!(
                "marsad-agent: duplicate wasl operator host_id {:?} in {} — first entry wins",
                entry.host_id,
                path.display()
            );
            continue;
        }
        match normalize_operator_url(&entry.url) {
            Some(normalized) => {
                entry.url = normalized;
                operators.push(entry);
            }
            None => {
                eprintln!(
                    "marsad-agent: malformed wasl operator url {:?} for host {:?} in {} \
                     (must be http://ip:port, e.g. \"http://127.0.0.1:4243\") — operator skipped",
                    entry.url,
                    entry.host_id,
                    path.display()
                );
            }
        }
    }

    if operators.is_empty() {
        eprintln!(
            "marsad-agent: wasl config {} has no usable operators after validation",
            path.display()
        );
        return None;
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
        // Boxed -> Arc at the registry boundary (review finding #1: see
        // `WaslConnectorMap`'s doc comment in `lib.rs`) — callers keep
        // passing an owned `Box` (unchanged call sites/signature); only the
        // registry's OWN storage needs to be shareable-without-the-map-lock.
        self.wasl_connectors
            .0
            .lock()
            .unwrap()
            .insert(host_id.to_string(), std::sync::Arc::from(connector));
    }

    /// Additive test seam; default = the real client factory (supplied
    /// lazily by `serve()`, since it is only ever needed from real-IO code).
    /// Setter so no constructor changes.
    pub fn set_wasl_connector_factory(&mut self, f: WaslConnectorFactoryFn) {
        self.wasl_connector_factory.0 = Some(f);
    }

    /// X4b (docs/specs/wasl-connector.md §X4b): adopt-or-spawn the HOST's
    /// receive-side `wasl-mcpd` — the node that binds `[a2a]` (the stable
    /// endpoint peers reach) + `[operator]` (so Marsad observes it, per
    /// X1/X2). Mirrors `apps/desktop`'s `adopt_or_spawn_agent` shape
    /// (try-connect-else-spawn), but reuses the SAME `Container` seam
    /// `Agent` already uses for claude sessions — no new fake/trait/
    /// constructor-arg. No-op (zero container calls) when `spec.wasl` is
    /// false: nothing on this host needs a `wasl-mcpd` for a launch that
    /// never requested one.
    ///
    /// "Marsad launches it, never speaks through it" is structural: the
    /// `Container` trait exposes only exists/attach/spawn/kill — no query/
    /// consult method exists to call, so no A2A traffic can originate from
    /// this fn (or anywhere else in the agent) via this seam.
    pub fn adopt_or_spawn_wasl_mcpd(&mut self, spec: &marsad_proto::LaunchSpec) -> bool {
        if !spec.wasl {
            return false;
        }
        let name = format!("wasl-mcpd-{}", spec.host_id);
        if self.active_container().exists(&name) {
            self.active_container().attach(&name).is_ok()
        } else {
            self.active_container().spawn(&name, "wasl-mcpd").is_ok()
        }
    }
}
