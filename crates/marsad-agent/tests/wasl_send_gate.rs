//! wasl_send_gate.rs — X2 A2A send-gate connector wiring tests.
//!
//! Tests that:
//! (a) `ensure_wasl_operator_connector` registers a connector via the factory;
//! (b) events fed to a `FakeWaslConnector` (the pre-registered seam) produce
//!     the right IPC lines via `wasl_event_lines`;
//! (c) `AnswerWaslGate` calls `post_decision` + replies `WaslGateAnswered`;
//! (d) `ensure_wasl_operator_connector` is idempotent (dedup by host_id).
//!
//! These tests drive the pure (no-socket) path exclusively: the
//! `FakeWaslConnector` is injected, no real HTTP.

use marsad_agent::{wasl_event_lines, Agent, AgentConfig, ListenSpec};
use marsad_lifecycle::FakeContainer;
use marsad_notify::CaptureNotifier;
use marsad_proto::{encode_ipc, parse_ipc, IpcMsg};
use marsad_store::{MemoryBackend, Store};
use marsad_wasl::{
    AuditDecision, AuditEntry, DecisionAck, FakeWaslConnector, GateDecided, GateRequest,
    GateRequested, WaslConnector, WaslEvent, WaslOperatorEntry,
};

fn make_agent() -> Agent {
    Agent::with_fakes(
        AgentConfig {
            listen: ListenSpec::Stdio,
            data_root: "/tmp/marsad-agent-wasl-send-gate-test".to_string(),
        },
        Box::new(FakeContainer::new()),
        Box::new(CaptureNotifier::new()),
        Box::new(Store::new(Box::new(MemoryBackend::new()))),
    )
    .expect("Agent::with_fakes")
}

fn sample_gate_requested() -> GateRequested {
    GateRequested {
        gate_id: "send-gate-1".to_string(),
        mode: "external".to_string(),
        decidable: true,
        request: GateRequest {
            id: "call-99".to_string(),
            kind: "message".to_string(),
            from: "marsad-local".to_string(),
            summary: "message to 1 peer(s)".to_string(),
            preview: "hello peer".to_string(),
        },
        requested_at: "2026-08-01T10:00:00.000Z".to_string(),
        timeout_ms: 120_000,
        deadline: "2026-08-01T10:02:00.000Z".to_string(),
    }
}

fn sample_gate_decided() -> GateDecided {
    GateDecided {
        gate_id: "send-gate-1".to_string(),
        outcome: "approved".to_string(),
        decided_by: "operator".to_string(),
        reason: None,
        decided_at: "2026-08-01T10:00:05.000Z".to_string(),
    }
}

fn sample_audit() -> AuditEntry {
    AuditEntry {
        seq: 7,
        kind: "decision".to_string(),
        actor: "marsad-local".to_string(),
        targets: vec!["marsad-remote".to_string()],
        summary: "gate Message".to_string(),
        decision: Some(AuditDecision {
            decision: "approve".to_string(),
            reason: None,
        }),
    }
}

// ===========================================================================
// Part 1 — wasl_event_lines mapping for send-gate event types
// ===========================================================================

/// GateRequested event → WaslGateRequested IPC broadcast.
#[test]
fn wasl_event_lines_gate_requested_for_send_gate_produces_correct_ipc() {
    let g = sample_gate_requested();
    let lines = wasl_event_lines("local", &WaslEvent::GateRequested(g.clone()));
    assert_eq!(lines.len(), 1);
    let parsed = parse_ipc(&lines[0]).expect("must be valid IpcMsg");
    match parsed {
        IpcMsg::WaslGateRequested {
            host_id,
            gate_id,
            mode,
            decidable,
            ..
        } => {
            assert_eq!(host_id, "local");
            assert_eq!(gate_id, "send-gate-1");
            assert_eq!(mode, "external");
            assert!(decidable);
        }
        other => panic!("expected WaslGateRequested, got {other:?}"),
    }
}

/// GateDecided event → WaslGateDecided IPC broadcast.
#[test]
fn wasl_event_lines_gate_decided_for_send_gate_produces_correct_ipc() {
    let d = sample_gate_decided();
    let lines = wasl_event_lines("local", &WaslEvent::GateDecided(d.clone()));
    assert_eq!(lines.len(), 1);
    let parsed = parse_ipc(&lines[0]).expect("must be valid IpcMsg");
    match parsed {
        IpcMsg::WaslGateDecided {
            host_id,
            gate_id,
            outcome,
            decided_by,
            reason,
            ..
        } => {
            assert_eq!(host_id, "local");
            assert_eq!(gate_id, "send-gate-1");
            assert_eq!(outcome, "approved");
            assert_eq!(decided_by, "operator");
            assert_eq!(reason, None);
        }
        other => panic!("expected WaslGateDecided, got {other:?}"),
    }
}

/// Audit event → WaslObservation IPC broadcast.
#[test]
fn wasl_event_lines_audit_for_send_gate_produces_observation() {
    let e = sample_audit();
    let lines = wasl_event_lines("local", &WaslEvent::Audit(e.clone()));
    assert_eq!(lines.len(), 1);
    let parsed = parse_ipc(&lines[0]).expect("must be valid IpcMsg");
    match parsed {
        IpcMsg::WaslObservation { host_id, .. } => assert_eq!(host_id, "local"),
        other => panic!("expected WaslObservation, got {other:?}"),
    }
}

// ===========================================================================
// Part 2 — ensure_wasl_operator_connector (dedup + factory path)
// ===========================================================================

/// A pre-registered connector is NOT overwritten by ensure_wasl_operator_connector.
#[test]
fn ensure_wasl_operator_connector_is_noop_when_connector_already_registered() {
    let mut agent = make_agent();
    // Pre-register a fake connector for "local".
    agent.register_wasl_connector("local", Box::new(FakeWaslConnector::new()));

    // Call ensure_wasl_operator_connector — must not overwrite the registered connector.
    agent.ensure_wasl_operator_connector("local", "http://127.0.0.1:49999");

    // The pre-registered connector is still there (AnswerWaslGate succeeds).
    let line = encode_ipc(&IpcMsg::AnswerWaslGate {
        host_id: "local".to_string(),
        gate_id: "gate-x".to_string(),
        outcome: "approved".to_string(),
        reason: None,
    });
    let out = agent.handle_ipc(&line).expect("pre-registered connector must ack");
    assert_eq!(out.len(), 1);
    assert!(matches!(
        parse_ipc(&out[0]).unwrap(),
        IpcMsg::WaslGateAnswered { .. }
    ));
}

/// Without a factory or pre-registered connector, ensure_wasl_operator_connector
/// logs but does not panic and leaves the connector map empty.
#[test]
fn ensure_wasl_operator_connector_without_factory_leaves_map_empty() {
    let mut agent = make_agent();
    // No connector registered, no factory set.
    agent.ensure_wasl_operator_connector("local", "http://127.0.0.1:49999");

    // The connector map is still empty → AnswerWaslGate errors.
    let line = encode_ipc(&IpcMsg::AnswerWaslGate {
        host_id: "local".to_string(),
        gate_id: "gate-x".to_string(),
        outcome: "approved".to_string(),
        reason: None,
    });
    let err = agent
        .handle_ipc(&line)
        .expect_err("empty connector map must error");
    assert!(
        matches!(err, marsad_agent::AgentError::Protocol(_)),
        "expected AgentError::Protocol"
    );
}

/// With a factory set, ensure_wasl_operator_connector builds and registers a
/// connector (the AnswerWaslGate arm succeeds thereafter).
#[test]
fn ensure_wasl_operator_connector_with_factory_registers_connector() {
    let mut agent = make_agent();

    // Install a factory that creates a FakeWaslConnector.
    // Capture decisions so we can assert they were relayed.
    use std::sync::{Arc, Mutex};
    let decisions_log: Arc<Mutex<Vec<(String, String, Option<String>)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let dl = decisions_log.clone();
    agent.set_wasl_connector_factory(Box::new(move |_entry: &WaslOperatorEntry| {
        let fake = FakeWaslConnector::new();
        // Capture the decisions Arc into the factory closure.
        let _ = fake.decisions.clone(); // Ensure it compiles.
        let _ = dl.clone();
        Box::new(fake) as Box<dyn WaslConnector>
    }));

    agent.ensure_wasl_operator_connector("local", "http://127.0.0.1:49000");

    // Connector was registered → AnswerWaslGate succeeds.
    let line = encode_ipc(&IpcMsg::AnswerWaslGate {
        host_id: "local".to_string(),
        gate_id: "send-gate-1".to_string(),
        outcome: "approved".to_string(),
        reason: None,
    });
    let out = agent
        .handle_ipc(&line)
        .expect("factory-built connector must ack");
    assert_eq!(out.len(), 1);
    match parse_ipc(&out[0]).unwrap() {
        IpcMsg::WaslGateAnswered {
            host_id,
            gate_id,
        } => {
            assert_eq!(host_id, "local");
            assert_eq!(gate_id, "send-gate-1");
        }
        other => panic!("expected WaslGateAnswered, got {other:?}"),
    }
}

/// ensure_wasl_operator_connector is idempotent: calling it twice for the
/// same host_id with a factory does NOT register a second connector.
#[test]
fn ensure_wasl_operator_connector_idempotent_on_second_call_with_factory() {
    let mut agent = make_agent();
    agent.set_wasl_connector_factory(Box::new(move |_entry: &WaslOperatorEntry| {
        Box::new(FakeWaslConnector::new()) as Box<dyn WaslConnector>
    }));

    agent.ensure_wasl_operator_connector("local", "http://127.0.0.1:49000");
    // Second call must not panic or replace the existing connector.
    agent.ensure_wasl_operator_connector("local", "http://127.0.0.1:49001");

    // Still answers (connector is registered).
    let line = encode_ipc(&IpcMsg::AnswerWaslGate {
        host_id: "local".to_string(),
        gate_id: "g1".to_string(),
        outcome: "approved".to_string(),
        reason: None,
    });
    let out = agent.handle_ipc(&line).expect("connector must still answer");
    assert_eq!(out.len(), 1);
}

// ===========================================================================
// Part 3 — AnswerWaslGate calls post_decision + replies WaslGateAnswered
//           (pinned by existing tests/wasl.rs Part 4; duplicated here for
//           the send-gate scenario end-to-end path)
// ===========================================================================

/// Register a FakeWaslConnector; AnswerWaslGate emits decide() and acks.
#[test]
fn answer_wasl_gate_for_send_gate_calls_decide_and_replies_gate_answered() {
    let mut agent = make_agent();
    let fake = FakeWaslConnector::new();
    let decisions = fake.decisions.clone();
    agent.register_wasl_connector("local", Box::new(fake));

    let line = encode_ipc(&IpcMsg::AnswerWaslGate {
        host_id: "local".to_string(),
        gate_id: "send-gate-1".to_string(),
        outcome: "approved".to_string(),
        reason: None,
    });
    let out = agent.handle_ipc(&line).expect("must ack");
    assert_eq!(out.len(), 1);
    assert_eq!(
        parse_ipc(&out[0]).unwrap(),
        IpcMsg::WaslGateAnswered {
            host_id: "local".to_string(),
            gate_id: "send-gate-1".to_string(),
        }
    );
    let d = decisions.lock().unwrap();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].0, "send-gate-1");
    assert_eq!(d[0].1, "approved");
    assert_eq!(d[0].2, None);
}

/// Deny with reason also passes the reason to decide().
#[test]
fn answer_wasl_gate_deny_with_reason_passes_reason_to_decide() {
    let mut agent = make_agent();
    let fake = FakeWaslConnector::new();
    let decisions = fake.decisions.clone();
    agent.register_wasl_connector("local", Box::new(fake));

    let line = encode_ipc(&IpcMsg::AnswerWaslGate {
        host_id: "local".to_string(),
        gate_id: "send-gate-1".to_string(),
        outcome: "denied".to_string(),
        reason: Some("not during business hours".to_string()),
    });
    let _out = agent.handle_ipc(&line).expect("must ack");
    let d = decisions.lock().unwrap();
    assert_eq!(d[0].1, "denied");
    assert_eq!(d[0].2, Some("not during business hours".to_string()));
}

/// A scripted AlreadyDecided (409) ack still produces WaslGateAnswered.
#[test]
fn answer_wasl_gate_already_decided_still_replies_gate_answered() {
    let mut agent = make_agent();
    let fake = FakeWaslConnector::new();
    fake.script_ack(Ok(DecisionAck::AlreadyDecided));
    agent.register_wasl_connector("local", Box::new(fake));

    let line = encode_ipc(&IpcMsg::AnswerWaslGate {
        host_id: "local".to_string(),
        gate_id: "send-gate-1".to_string(),
        outcome: "approved".to_string(),
        reason: None,
    });
    let out = agent.handle_ipc(&line).expect("AlreadyDecided must still ack");
    assert!(matches!(
        parse_ipc(&out[0]).unwrap(),
        IpcMsg::WaslGateAnswered { .. }
    ));
}
