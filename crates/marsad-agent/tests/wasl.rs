//! wasl.rs — X2 agent bucket: runtime-optional Wasl connector.
//!
//! Authoritative spec: `docs/specs/wasl-connector.md` §4 ("B13 marsad-agent —
//! runtime-optional connector"), §6 ("Optionality proof", points 1-2), and
//! §7.5 ("marsad-agent — NEW tests/wasl.rs (fake connector; no sockets)").
//! Uses `marsad_wasl::FakeWaslConnector` for all decision-relay tests — no
//! socket, no real HTTP, matching this crate's existing FAKES-only house
//! convention (`tests/happy.rs`'s own top-of-file note).
//!
//! NOTE(test-agent) — INVENTED API SURFACE: §4.1/§4.2 pin two free/pub items
//! verbatim (`load_wasl_config(data_root: &str) -> Option<marsad_wasl::
//! WaslOperatorsConfig>` and `Agent::set_wasl_connector_factory(&mut self,
//! f: Box<dyn Fn(&marsad_wasl::WaslOperatorEntry) -> Box<dyn
//! marsad_wasl::WaslConnector> + Send>)`), and pins `wasl_event_lines(host_id:
//! &str, ev: &marsad_wasl::WaslEvent) -> Vec<String>` as a pure mapping.
//! `wasl_connectors` itself is described only as PRIVATE `Agent` state (§4.2),
//! wired *inside* `serve()`'s `if let Some(cfg) = load_wasl_config(..)` guard
//! — and `serve()` is `#[cfg(not(coverage))]` (real sockets, blocks forever),
//! unreachable from a unit test. Per §7.5 the intended test route is
//! "register a FakeWaslConnector (via set_wasl_connector_factory + a config
//! file in the temp data_root, OR direct registry seam)" — but the config
//! file / `serve()` route can only wire connectors on a real-IO path, and
//! `handle_ipc`'s `AnswerWaslGate` arm (§4.3) must observably branch on
//! registry membership under coverage instrumentation too (unlike
//! `pending_gates`/`session_writers`, which are `#[cfg(not(coverage))]`-only
//! and get away with a fake path that always no-ops the same way regardless
//! of registry contents — Wasl's two branches, present vs. absent, are
//! semantically different and BOTH need coverage). So this file assumes the
//! "direct registry seam" alternative names a plain, always-compiled method:
//!
//!     pub fn register_wasl_connector(&mut self, host_id: &str,
//!                                     connector: Box<dyn marsad_wasl::WaslConnector>)
//!
//! — the `wasl` analogue of the pre-existing `Agent::with_fakes`/
//! `container_calls()` test seams (see `tests/happy.rs`'s own
//! "NOTE(test-agent): injection" precedent for this exact pattern). Also
//! assumes `load_wasl_config`/`wasl_event_lines` are re-exported at the crate
//! root (`marsad_agent::{load_wasl_config, wasl_event_lines}`), mirroring how
//! `driver::claude_argv` and `reader::reconnect_backoff_next` are re-exported
//! today (`crates/marsad-agent/src/lib.rs`'s `pub use` block) rather than
//! nested under a submodule path. If the code-agent lands a differently
//! shaped seam, it should adjust these call sites — the BEHAVIOR pinned here
//! (absent config == zero connector activity; a registered connector's
//! `decide()` is the only call site of a Wasl decision; the ack is
//! independent of the ack value; a transport-level Err surfaces as a Protocol
//! error) is the actual contract, not the exact method name.
//!
//! This file WILL fail to compile until the code-agent lands `marsad-wasl` as
//! a dependency of `marsad-agent`, `load_wasl_config`, `wasl_event_lines`, and
//! the connector-registration surface (§4). That is the expected tests-first
//! compile-failure signal.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use marsad_agent::{
    load_wasl_config, wasl_event_lines, Agent, AgentConfig, AgentError, ListenSpec,
};
use marsad_lifecycle::FakeContainer;
use marsad_notify::CaptureNotifier;
use marsad_proto::{encode_ipc, parse_ipc, IpcMsg};
use marsad_store::{MemoryBackend, Store};
use marsad_wasl::{
    AuditDecision, AuditEntry, DecisionAck, FakeWaslConnector, GateDecided, GateRequest,
    GateRequested, Hello, WaslConnector, WaslError, WaslEvent, WaslOperatorEntry,
    WaslOperatorsConfig,
};

// ---------------------------------------------------------------------------
// Shared temp-dir helper (mirrors `tests/capabilities.rs`'s idiom — no
// `tempfile` dev-dependency needed; duplicated here per house convention
// since each file under tests/ is compiled as an independent crate).
// ---------------------------------------------------------------------------

struct TempDir(PathBuf);

impl TempDir {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tempdir() -> TempDir {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("marsad-agent-wasl-test-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    TempDir(path)
}

fn make_agent() -> Agent {
    Agent::with_fakes(
        AgentConfig {
            listen: ListenSpec::Stdio,
            data_root: "/tmp/marsad-agent-wasl-test-agent".to_string(),
        },
        Box::new(FakeContainer::new()),
        Box::new(CaptureNotifier::new()),
        Box::new(Store::new(Box::new(MemoryBackend::new()))),
    )
    .expect("Agent::with_fakes")
}

fn answer_wasl_gate_line(
    host_id: &str,
    gate_id: &str,
    outcome: &str,
    reason: Option<&str>,
) -> String {
    encode_ipc(&IpcMsg::AnswerWaslGate {
        host_id: host_id.to_string(),
        gate_id: gate_id.to_string(),
        outcome: outcome.to_string(),
        reason: reason.map(str::to_string),
    })
}

// ===========================================================================
// Part 1 — `load_wasl_config` (§4.1: pure-ish, unit-testable)
// ===========================================================================

#[test]
fn load_wasl_config_absent_file_is_none() {
    let tmp = tempdir();
    assert_eq!(load_wasl_config(tmp.path().to_str().unwrap()), None);
}

#[test]
fn load_wasl_config_malformed_json_is_none() {
    let tmp = tempdir();
    std::fs::write(tmp.path().join("wasl.json"), b"{ not valid json").unwrap();
    assert_eq!(load_wasl_config(tmp.path().to_str().unwrap()), None);
}

#[test]
fn load_wasl_config_empty_operators_is_none() {
    let tmp = tempdir();
    std::fs::write(tmp.path().join("wasl.json"), br#"{"operators":[]}"#).unwrap();
    assert_eq!(load_wasl_config(tmp.path().to_str().unwrap()), None);
}

#[test]
fn load_wasl_config_valid_file_returns_exact_entries() {
    let tmp = tempdir();
    std::fs::write(
        tmp.path().join("wasl.json"),
        br#"{"operators":[{"host_id":"local","url":"http://127.0.0.1:8787"}]}"#,
    )
    .unwrap();
    let cfg = load_wasl_config(tmp.path().to_str().unwrap()).expect("valid file must load");
    assert_eq!(
        cfg,
        WaslOperatorsConfig {
            operators: vec![WaslOperatorEntry {
                host_id: "local".to_string(),
                url: "http://127.0.0.1:8787".to_string(),
            }],
        }
    );
}

#[test]
fn load_wasl_config_multiple_entries_preserve_order() {
    let tmp = tempdir();
    std::fs::write(
        tmp.path().join("wasl.json"),
        br#"{"operators":[{"host_id":"local","url":"http://127.0.0.1:8787"},{"host_id":"hpc","url":"http://10.0.0.5:8787"}]}"#,
    )
    .unwrap();
    let cfg = load_wasl_config(tmp.path().to_str().unwrap()).expect("valid file must load");
    assert_eq!(
        cfg.operators,
        vec![
            WaslOperatorEntry {
                host_id: "local".to_string(),
                url: "http://127.0.0.1:8787".to_string(),
            },
            WaslOperatorEntry {
                host_id: "hpc".to_string(),
                url: "http://10.0.0.5:8787".to_string(),
            },
        ]
    );
}

#[test]
fn load_wasl_config_duplicate_host_id_first_wins() {
    let tmp = tempdir();
    std::fs::write(
        tmp.path().join("wasl.json"),
        br#"{"operators":[{"host_id":"local","url":"http://127.0.0.1:8787"},{"host_id":"local","url":"http://127.0.0.1:9999"}]}"#,
    )
    .unwrap();
    let cfg = load_wasl_config(tmp.path().to_str().unwrap()).expect("valid file must load");
    assert_eq!(
        cfg.operators.len(),
        1,
        "duplicate host_id must collapse to a single entry"
    );
    assert_eq!(
        cfg.operators[0].url, "http://127.0.0.1:8787",
        "the FIRST entry for a duplicate host_id must win"
    );
}

#[test]
fn load_wasl_config_trailing_slash_url_is_normalized() {
    // docs/specs/node-config.md pins the trailing-slash form as "the existing
    // convention" for Wasl peer URLs — the exact value an operator would
    // naturally copy from a node's own `[operator] bind` into `wasl.json`.
    // `marsad_wasl::WaslClientConfig::base_url` requires the NO-trailing-
    // slash `http://ip:port` shape (its `wire::parse_base_url` panics
    // otherwise) — `load_wasl_config` must normalize this before it ever
    // reaches a connector.
    let tmp = tempdir();
    std::fs::write(
        tmp.path().join("wasl.json"),
        br#"{"operators":[{"host_id":"local","url":"http://127.0.0.1:8787/"}]}"#,
    )
    .unwrap();
    let cfg = load_wasl_config(tmp.path().to_str().unwrap()).expect("valid file must load");
    assert_eq!(
        cfg.operators,
        vec![WaslOperatorEntry {
            host_id: "local".to_string(),
            url: "http://127.0.0.1:8787".to_string(),
        }],
        "a trailing slash must be stripped, not left to panic a connector later"
    );
}

#[test]
fn load_wasl_config_malformed_url_entry_is_dropped_valid_entries_survive() {
    let tmp = tempdir();
    std::fs::write(
        tmp.path().join("wasl.json"),
        br#"{"operators":[
            {"host_id":"bad-scheme","url":"https://127.0.0.1:8787"},
            {"host_id":"bad-host","url":"http://not-an-ip:8787"},
            {"host_id":"bad-shape","url":"not a url at all"},
            {"host_id":"local","url":"http://127.0.0.1:8787"}
        ]}"#,
    )
    .unwrap();
    let cfg = load_wasl_config(tmp.path().to_str().unwrap()).expect("one valid entry must load");
    assert_eq!(
        cfg.operators,
        vec![WaslOperatorEntry {
            host_id: "local".to_string(),
            url: "http://127.0.0.1:8787".to_string(),
        }],
        "only the well-formed http://ip:port entry must survive"
    );
}

#[test]
fn load_wasl_config_all_entries_malformed_url_is_none() {
    let tmp = tempdir();
    std::fs::write(
        tmp.path().join("wasl.json"),
        br#"{"operators":[{"host_id":"local","url":"not-a-url"}]}"#,
    )
    .unwrap();
    assert_eq!(
        load_wasl_config(tmp.path().to_str().unwrap()),
        None,
        "zero usable operators after url validation must be None, same as an empty list"
    );
}

// ===========================================================================
// Part 2 — `wasl_event_lines` (§4.2: pure mapping, `WaslEvent` -> encoded
// broadcast lines)
// ===========================================================================

fn sample_hello() -> Hello {
    Hello {
        api_version: 1,
        agent_id: "wasl-a".to_string(),
        host_id: "wasl-host-1".to_string(),
        version: "0.1.0".to_string(),
        last_seq: 42,
        resume: "snapshot".to_string(),
    }
}

fn sample_gate_requested() -> GateRequested {
    GateRequested {
        gate_id: "gate-1".to_string(),
        mode: "external".to_string(),
        decidable: true,
        request: GateRequest {
            id: "call-9".to_string(),
            kind: "message".to_string(),
            from: "wasl-a".to_string(),
            summary: "message to 1 peer(s)".to_string(),
            preview: "hi bob".to_string(),
        },
        requested_at: "2026-07-02T12:00:00.000Z".to_string(),
        timeout_ms: 120_000,
        deadline: "2026-07-02T12:02:00.000Z".to_string(),
    }
}

fn sample_gate_decided() -> GateDecided {
    GateDecided {
        gate_id: "gate-1".to_string(),
        outcome: "approved".to_string(),
        decided_by: "operator".to_string(),
        reason: None,
        decided_at: "2026-07-02T12:00:03.500Z".to_string(),
    }
}

fn sample_audit_entry() -> AuditEntry {
    AuditEntry {
        seq: 3,
        kind: "decision".to_string(),
        actor: "wasl-a".to_string(),
        targets: vec!["wasl-b".to_string()],
        summary: "gate Message".to_string(),
        decision: Some(AuditDecision {
            decision: "approve".to_string(),
            reason: None,
        }),
    }
}

#[test]
fn wasl_event_lines_connected_yields_empty_vec() {
    let lines = wasl_event_lines(
        "local",
        &WaslEvent::Connected {
            hello: sample_hello(),
        },
    );
    assert_eq!(lines, Vec::<String>::new(), "Connected is stderr-log-only");
}

#[test]
fn wasl_event_lines_disconnected_yields_empty_vec() {
    let lines = wasl_event_lines(
        "local",
        &WaslEvent::Disconnected {
            error: "connection reset".to_string(),
        },
    );
    assert_eq!(
        lines,
        Vec::<String>::new(),
        "Disconnected is stderr-log-only"
    );
}

#[test]
fn wasl_event_lines_gate_requested_yields_typed_broadcast() {
    let g = sample_gate_requested();
    let lines = wasl_event_lines("local", &WaslEvent::GateRequested(g.clone()));
    assert_eq!(lines.len(), 1);
    let parsed = parse_ipc(&lines[0]).expect("broadcast line must be valid IpcMsg");
    assert_eq!(
        parsed,
        IpcMsg::WaslGateRequested {
            host_id: "local".to_string(),
            gate_id: g.gate_id.clone(),
            mode: g.mode.clone(),
            decidable: g.decidable,
            request: serde_json::to_value(&g.request).unwrap(),
            requested_at: g.requested_at.clone(),
            timeout_ms: g.timeout_ms,
            deadline: g.deadline.clone(),
        }
    );
}

#[test]
fn wasl_event_lines_gate_decided_yields_typed_broadcast() {
    let d = sample_gate_decided();
    let lines = wasl_event_lines("local", &WaslEvent::GateDecided(d.clone()));
    assert_eq!(lines.len(), 1);
    let parsed = parse_ipc(&lines[0]).expect("broadcast line must be valid IpcMsg");
    assert_eq!(
        parsed,
        IpcMsg::WaslGateDecided {
            host_id: "local".to_string(),
            gate_id: d.gate_id.clone(),
            outcome: d.outcome.clone(),
            decided_by: d.decided_by.clone(),
            reason: d.reason.clone(),
            decided_at: d.decided_at.clone(),
        }
    );
}

#[test]
fn wasl_event_lines_audit_yields_typed_observation_broadcast() {
    let e = sample_audit_entry();
    let lines = wasl_event_lines("local", &WaslEvent::Audit(e.clone()));
    assert_eq!(lines.len(), 1);
    let parsed = parse_ipc(&lines[0]).expect("broadcast line must be valid IpcMsg");
    assert_eq!(
        parsed,
        IpcMsg::WaslObservation {
            host_id: "local".to_string(),
            entry: serde_json::to_value(&e).unwrap(),
        }
    );
}

/// §4.2: `GateVanished{gate_id}` -> the synthesized "unknown"/"resync"/""
/// `WaslGateDecided` golden (also pinned in `marsad-proto`'s
/// `tests/wasl_ipc.rs`).
#[test]
fn wasl_event_lines_gate_vanished_yields_synthesized_unknown_resync_golden() {
    let lines = wasl_event_lines(
        "local",
        &WaslEvent::GateVanished {
            gate_id: "gate-7".to_string(),
        },
    );
    assert_eq!(lines.len(), 1);
    let parsed = parse_ipc(&lines[0]).expect("broadcast line must be valid IpcMsg");
    assert_eq!(
        parsed,
        IpcMsg::WaslGateDecided {
            host_id: "local".to_string(),
            gate_id: "gate-7".to_string(),
            outcome: "unknown".to_string(),
            decided_by: "resync".to_string(),
            reason: None,
            decided_at: String::new(),
        }
    );
}

#[test]
fn wasl_event_lines_host_id_is_threaded_through_every_variant() {
    let g = sample_gate_requested();
    let lines = wasl_event_lines("hpc", &WaslEvent::GateRequested(g));
    match parse_ipc(&lines[0]).unwrap() {
        IpcMsg::WaslGateRequested { host_id, .. } => assert_eq!(host_id, "hpc"),
        other => panic!("expected WaslGateRequested, got {other:?}"),
    }
}

// ===========================================================================
// Part 3 — absent-config = inert (§6 points 1-2)
// ===========================================================================

/// §6 point 1: "absent `<data_root>/wasl.json` -> `None` -> zero threads,
/// zero connections, empty `wasl_connectors` map." A freshly constructed
/// `Agent` (via `with_fakes`, never wired to any config or connector) is that
/// empty-map state by construction.
#[test]
fn fresh_agent_has_no_wasl_connectors_registered() {
    let mut agent = make_agent();
    let line = answer_wasl_gate_line("local", "gate-1", "approved", None);
    let err = agent
        .handle_ipc(&line)
        .expect_err("AnswerWaslGate with an empty connector map must error");
    assert!(
        matches!(err, AgentError::Protocol(_)),
        "expected AgentError::Protocol, got {err:?}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("no wasl operator configured"),
        "error message must explain the missing operator, got: {msg}"
    );
    assert!(
        msg.contains("local"),
        "error message must name the host_id, got: {msg}"
    );
}

/// §6 point 2: "if hand-fed anyway, the empty connector map yields the
/// standard `Error` reply (no side effects)." No container call must result
/// from an `AnswerWaslGate` line regardless of connector state — this arm
/// never touches the session container.
#[test]
fn answer_wasl_gate_with_no_connector_has_no_container_side_effects() {
    let mut agent = make_agent();
    let before = agent.container_calls().len();
    let line = answer_wasl_gate_line("local", "gate-1", "approved", None);
    let _ = agent.handle_ipc(&line);
    assert_eq!(
        agent.container_calls().len(),
        before,
        "AnswerWaslGate must never touch the session container"
    );
}

/// §4.3 point 1: "the entire no-config surface of the new arm" — any host_id
/// is equally unconfigured when the map is empty, not just "local".
#[test]
fn answer_wasl_gate_unknown_host_id_errors_even_when_map_nonempty_for_other_hosts() {
    let mut agent = make_agent();
    agent.register_wasl_connector("local", Box::new(FakeWaslConnector::new()));

    let line = answer_wasl_gate_line("hpc", "gate-1", "approved", None);
    let err = agent
        .handle_ipc(&line)
        .expect_err("host_id not present in the registry must still error");
    assert!(matches!(err, AgentError::Protocol(_)));
    assert!(err.to_string().contains("hpc"));
}

// ===========================================================================
// Part 4 — decision relay via a registered `FakeWaslConnector` (§4.3, §7.5)
// ===========================================================================

#[test]
fn answer_wasl_gate_approve_calls_decide_with_no_reason_and_acks() {
    let mut agent = make_agent();
    let fake = FakeWaslConnector::new();
    let decisions = fake.decisions.clone();
    agent.register_wasl_connector("local", Box::new(fake));

    let line = answer_wasl_gate_line("local", "gate-1", "approved", None);
    let out = agent
        .handle_ipc(&line)
        .expect("registered connector must ack");

    assert_eq!(out.len(), 1);
    let parsed = parse_ipc(&out[0]).expect("reply must be valid IpcMsg");
    assert_eq!(
        parsed,
        IpcMsg::WaslGateAnswered {
            host_id: "local".to_string(),
            gate_id: "gate-1".to_string(),
        }
    );

    assert_eq!(
        decisions.lock().unwrap().as_slice(),
        &[("gate-1".to_string(), "approved".to_string(), None)]
    );
}

#[test]
fn answer_wasl_gate_deny_calls_decide_with_reason_and_acks() {
    let mut agent = make_agent();
    let fake = FakeWaslConnector::new();
    let decisions = fake.decisions.clone();
    agent.register_wasl_connector("local", Box::new(fake));

    let line = answer_wasl_gate_line(
        "local",
        "gate-1",
        "denied",
        Some("not during business hours"),
    );
    let out = agent
        .handle_ipc(&line)
        .expect("registered connector must ack");

    assert_eq!(out.len(), 1);
    let parsed = parse_ipc(&out[0]).expect("reply must be valid IpcMsg");
    assert_eq!(
        parsed,
        IpcMsg::WaslGateAnswered {
            host_id: "local".to_string(),
            gate_id: "gate-1".to_string(),
        }
    );

    assert_eq!(
        decisions.lock().unwrap().as_slice(),
        &[(
            "gate-1".to_string(),
            "denied".to_string(),
            Some("not during business hours".to_string())
        )]
    );
}

/// §4.3 point 2: "AlreadyDecided (409) — benign race; stderr log" — still
/// acks with `WaslGateAnswered`, exactly like `Delivered`.
#[test]
fn answer_wasl_gate_already_decided_ack_still_replies_with_gate_answered() {
    let mut agent = make_agent();
    let fake = FakeWaslConnector::new();
    fake.script_ack(Ok(DecisionAck::AlreadyDecided));
    agent.register_wasl_connector("local", Box::new(fake));

    let line = answer_wasl_gate_line("local", "gate-1", "approved", None);
    let out = agent
        .handle_ipc(&line)
        .expect("AlreadyDecided must still be an Ok ack");
    assert_eq!(out.len(), 1);
    assert_eq!(
        parse_ipc(&out[0]).unwrap(),
        IpcMsg::WaslGateAnswered {
            host_id: "local".to_string(),
            gate_id: "gate-1".to_string(),
        }
    );
}

/// §4.3 point 2: "UnknownGate (404) — stderr log (a stale UI answered a dead
/// gate; nothing to do)" — still acks.
#[test]
fn answer_wasl_gate_unknown_gate_ack_still_replies_with_gate_answered() {
    let mut agent = make_agent();
    let fake = FakeWaslConnector::new();
    fake.script_ack(Ok(DecisionAck::UnknownGate));
    agent.register_wasl_connector("local", Box::new(fake));

    let line = answer_wasl_gate_line("local", "gate-dead", "approved", None);
    let out = agent
        .handle_ipc(&line)
        .expect("UnknownGate must still be an Ok ack");
    assert_eq!(out.len(), 1);
    assert_eq!(
        parse_ipc(&out[0]).unwrap(),
        IpcMsg::WaslGateAnswered {
            host_id: "local".to_string(),
            gate_id: "gate-dead".to_string(),
        }
    );
}

/// §4.3 point 2: "Err(WaslError) (400/transport) -> Err(AgentError::Protocol(…))
/// -> Error reply (the GUI learns the relay itself failed)."
#[test]
fn answer_wasl_gate_transport_error_surfaces_as_protocol_error() {
    let mut agent = make_agent();
    let fake = FakeWaslConnector::new();
    fake.script_ack(Err(WaslError::Http {
        status: 400,
        body: "malformed".to_string(),
    }));
    agent.register_wasl_connector("local", Box::new(fake));

    let line = answer_wasl_gate_line("local", "gate-1", "approved", None);
    let err = agent
        .handle_ipc(&line)
        .expect_err("a scripted decide() Err must surface as an agent error");
    assert!(
        matches!(err, AgentError::Protocol(_)),
        "expected AgentError::Protocol, got {err:?}"
    );
}

/// The agent never originates a decision (§4.3: "the only call site of
/// `decide()` is this arm") — registering a connector and never sending
/// `AnswerWaslGate` must never invoke `decide()`.
#[test]
fn registering_a_connector_alone_never_calls_decide() {
    let mut agent = make_agent();
    let fake = FakeWaslConnector::new();
    let decisions = fake.decisions.clone();
    agent.register_wasl_connector("local", Box::new(fake));

    // Some unrelated command that touches nothing Wasl-specific.
    let subscribe = encode_ipc(&IpcMsg::Subscribe {
        session_id: String::new(),
    });
    let _ = agent.handle_ipc(&subscribe);

    assert!(
        decisions.lock().unwrap().is_empty(),
        "decide() must only ever be called from the AnswerWaslGate arm"
    );
}

// ===========================================================================
// Part 5 — `set_wasl_connector_factory` (§4.2, pinned signature)
// ===========================================================================

/// Smoke test for the pinned setter shape (§4.2: "Setter so no constructor
/// changes"). The factory itself is only INVOKED from the real-IO
/// `serve()` path (guarded by `load_wasl_config(..) == Some(..)`, §6 point 1)
/// which is `#[cfg(not(coverage))]` and cannot be driven from a unit test —
/// so this test only pins that the setter compiles against the documented
/// closure type and does not itself register a connector or otherwise touch
/// observable state (mirrors `fresh_agent_has_no_wasl_connectors_registered`).
#[test]
fn set_wasl_connector_factory_accepts_documented_closure_and_has_no_side_effects_alone() {
    let mut agent = make_agent();
    agent.set_wasl_connector_factory(Box::new(|_entry: &WaslOperatorEntry| {
        Box::new(FakeWaslConnector::new()) as Box<dyn WaslConnector>
    }));

    let line = answer_wasl_gate_line("local", "gate-1", "approved", None);
    let err = agent
        .handle_ipc(&line)
        .expect_err("setting the factory alone must not register any connector");
    assert!(matches!(err, AgentError::Protocol(_)));
}
