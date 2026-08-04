//! wasl_send_gate.rs — X2 send-gate SessionView surface (marsad-core).
//!
//! Tests that:
//! (b) inbound `WaslGateRequested` sets a `Gate::Handoff` on the owning node's
//!     `SessionView` (unique gate_id → stable DecisionId via marsad-gates);
//!     `WaslGateDecided` clears it.
//! (c) `answer_wasl_gate` emits `AnswerWaslGate` IPC.
//!
//! Drives `Core` over `LoopbackFake`, exactly like `tests/wasl.rs` and
//! `tests/wasl_session.rs`. A WaslManaged session is launched on `host_id=local`
//! so the send-gate can be mapped to it. `VmUpdate` has no `PartialEq` derive,
//! so assertions match the variant directly.

use marsad_core::{Core, CoreConfig, VmUpdate};
use marsad_events::Phase;
use marsad_gates::{Decision, Gate};
use marsad_proto::{encode_ipc, parse_ipc, IpcMsg};
use marsad_transport::LoopbackFake;

fn cfg() -> CoreConfig {
    CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-core-wasl-send-gate-test".to_string(),
    }
}

fn make_core() -> (Core, LoopbackFake) {
    let (a, b) = LoopbackFake::pair();
    let core = Core::new(cfg(), Box::new(a)).expect("Core::new");
    (core, b)
}

/// Launch a WaslManaged session on host_id="local" and drain the launch IPC.
fn launch_wasl_local(core: &mut Core, peer: &mut LoopbackFake) -> String {
    let id = core
        .launch_wasl_session(
            "local",
            serde_json::json!({"protocol": "claude", "key": "test-node-1"}),
            Some("test-node".to_string()),
        )
        .expect("launch_wasl_session must succeed");
    let _ = peer.drain_outbound();
    id
}

fn gate_requested_line(
    host_id: &str,
    gate_id: &str,
    decidable: bool,
    kind: &str,
    from: &str,
    summary: &str,
    preview: &str,
) -> String {
    encode_ipc(&IpcMsg::WaslGateRequested {
        host_id: host_id.to_string(),
        gate_id: gate_id.to_string(),
        mode: "external".to_string(),
        decidable,
        request: serde_json::json!({
            "id": "call-42",
            "kind": kind,
            "from": from,
            "summary": summary,
            "preview": preview,
        }),
        requested_at: "2026-08-01T10:00:00.000Z".to_string(),
        timeout_ms: 120_000,
        deadline: "2026-08-01T10:02:00.000Z".to_string(),
    })
}

fn gate_decided_line(
    host_id: &str,
    gate_id: &str,
    outcome: &str,
    decided_by: &str,
) -> String {
    encode_ipc(&IpcMsg::WaslGateDecided {
        host_id: host_id.to_string(),
        gate_id: gate_id.to_string(),
        outcome: outcome.to_string(),
        decided_by: decided_by.to_string(),
        reason: None,
        decided_at: "2026-08-01T10:00:05.000Z".to_string(),
    })
}

/// Get the SessionView from the updates for a specific session id.
fn session_view(updates: &[VmUpdate], id: &str) -> marsad_viewmodel::SessionView {
    updates
        .iter()
        .rev()
        .find_map(|u| match u {
            VmUpdate::Session(sv) if sv.id == id => Some(sv.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no VmUpdate::Session for {id} in {updates:?}"))
}

// ===========================================================================
// Part 1 — WaslGateRequested sets a Gate::Handoff on the owning SessionView
// ===========================================================================

/// WaslGateRequested (decidable=true) on a host that HAS a WaslManaged session
/// BOTH emits WaslGates AND sets a Gate::Handoff on that session's view.
#[test]
fn wasl_gate_requested_sets_handoff_gate_on_owning_wasl_session() {
    let (mut core, mut peer) = make_core();
    let session_id = launch_wasl_local(&mut core, &mut peer);

    let line = gate_requested_line(
        "local",
        "send-gate-1",
        true,
        "message",
        "marsad-local",
        "message to 1 peer(s)",
        "hello peer",
    );
    let updates = core.on_agent_line(&line).expect("must parse+apply");

    // Must have at least WaslGates + Session updates.
    let has_wasl_gates = updates
        .iter()
        .any(|u| matches!(u, VmUpdate::WaslGates(_)));
    let has_session = updates
        .iter()
        .any(|u| matches!(u, VmUpdate::Session(sv) if sv.id == session_id));
    assert!(has_wasl_gates, "must emit WaslGates");
    assert!(
        has_session,
        "must emit VmUpdate::Session for the owning session"
    );

    let sv = session_view(&updates, &session_id);
    match &sv.gate {
        Some(Gate::Handoff {
            gate_id,
            kind,
            from,
            summary,
            preview,
        }) => {
            assert_eq!(gate_id, "send-gate-1");
            assert_eq!(kind, "message");
            assert_eq!(from, "marsad-local");
            assert_eq!(summary, "message to 1 peer(s)");
            assert_eq!(preview, "hello peer");
        }
        other => panic!("expected Gate::Handoff, got {other:?}"),
    }
    // Phase is Needs while gate is open.
    assert_eq!(
        sv.state,
        Phase::Needs,
        "session phase must be Needs while a send-gate is pending"
    );
}

/// WaslGateRequested on a host with NO WaslManaged session only emits WaslGates.
#[test]
fn wasl_gate_requested_no_wasl_session_emits_only_wasl_gates() {
    let (mut core, _peer) = make_core();
    // No WaslManaged session launched.

    let line = gate_requested_line(
        "local",
        "send-gate-1",
        true,
        "message",
        "marsad-local",
        "s",
        "p",
    );
    let updates = core.on_agent_line(&line).expect("must parse+apply");
    assert_eq!(
        updates.len(),
        1,
        "only WaslGates update expected when no WaslManaged session"
    );
    assert!(matches!(&updates[0], VmUpdate::WaslGates(_)));
}

/// A decidable=false gate on a host with a WaslManaged session is a tolerant
/// no-op — observe-only gates are never surfaced on a SessionView.
#[test]
fn wasl_gate_requested_not_decidable_is_noop_even_with_wasl_session() {
    let (mut core, mut peer) = make_core();
    let _sid = launch_wasl_local(&mut core, &mut peer);

    let line = gate_requested_line(
        "local",
        "send-gate-1",
        false, // observe-only
        "consult",
        "marsad-local",
        "s",
        "p",
    );
    let updates = core.on_agent_line(&line).expect("must not error");
    assert!(
        updates.is_empty(),
        "decidable=false must be a no-op, got {updates:?}"
    );
}

// ===========================================================================
// Part 2 — WaslGateDecided clears the Gate::Handoff from the SessionView
// ===========================================================================

/// WaslGateDecided clears the Gate::Handoff set by WaslGateRequested.
#[test]
fn wasl_gate_decided_clears_handoff_gate_on_owning_session() {
    let (mut core, mut peer) = make_core();
    let session_id = launch_wasl_local(&mut core, &mut peer);

    // First: set the gate.
    let req_line = gate_requested_line(
        "local",
        "send-gate-1",
        true,
        "message",
        "marsad-local",
        "s",
        "p",
    );
    core.on_agent_line(&req_line).expect("gate requested");

    // Then: resolve it.
    let dec_line = gate_decided_line("local", "send-gate-1", "approved", "operator");
    let updates = core.on_agent_line(&dec_line).expect("gate decided");

    // Must emit Session update with gate cleared.
    let has_session = updates
        .iter()
        .any(|u| matches!(u, VmUpdate::Session(sv) if sv.id == session_id));
    assert!(
        has_session,
        "WaslGateDecided must emit VmUpdate::Session for owning session"
    );

    let sv = session_view(&updates, &session_id);
    assert!(
        sv.gate.is_none(),
        "gate must be cleared on WaslGateDecided, got {:?}",
        sv.gate
    );
}

/// WaslGateDecided for an unknown gate_id (race / already cleared) still emits
/// WaslGateDecided but no Session update.
#[test]
fn wasl_gate_decided_unknown_gate_id_emits_no_session_update() {
    let (mut core, mut peer) = make_core();
    let _sid = launch_wasl_local(&mut core, &mut peer);

    // No gate was ever set for "nonexistent-gate".
    let dec_line = gate_decided_line("local", "nonexistent-gate", "approved", "operator");
    let updates = core.on_agent_line(&dec_line).expect("must not error");

    // WaslGateDecided must be emitted, but no Session update.
    let has_decided = updates
        .iter()
        .any(|u| matches!(u, VmUpdate::WaslGateDecided { .. }));
    let has_session = updates
        .iter()
        .any(|u| matches!(u, VmUpdate::Session(_)));
    assert!(has_decided, "must emit WaslGateDecided");
    assert!(
        !has_session,
        "must not emit Session update for unknown gate, got {updates:?}"
    );
}

// ===========================================================================
// Part 3 — DecisionId is unique per gate_id
// ===========================================================================

/// Two different gate_ids on the same host produce different gates on the session.
#[test]
fn wasl_send_gate_two_distinct_gate_ids_produce_distinct_session_gates() {
    let (mut core, mut peer) = make_core();
    let session_id = launch_wasl_local(&mut core, &mut peer);

    // Request gate A.
    let req_a = gate_requested_line(
        "local",
        "gate-A",
        true,
        "message",
        "marsad-local",
        "msg A",
        "preview A",
    );
    let updates_a = core.on_agent_line(&req_a).expect("gate A requested");
    let sv_a = session_view(&updates_a, &session_id);
    let gate_id_a = match &sv_a.gate {
        Some(Gate::Handoff { gate_id, .. }) => gate_id.clone(),
        other => panic!("expected Handoff for gate A, got {other:?}"),
    };
    assert_eq!(gate_id_a, "gate-A");

    // Resolve gate A.
    core.on_agent_line(&gate_decided_line("local", "gate-A", "approved", "operator"))
        .expect("gate A decided");

    // Request gate B (different gate_id).
    let req_b = gate_requested_line(
        "local",
        "gate-B",
        true,
        "command",
        "marsad-local",
        "cmd B",
        "preview B",
    );
    let updates_b = core.on_agent_line(&req_b).expect("gate B requested");
    let sv_b = session_view(&updates_b, &session_id);
    let gate_id_b = match &sv_b.gate {
        Some(Gate::Handoff { gate_id, .. }) => gate_id.clone(),
        other => panic!("expected Handoff for gate B, got {other:?}"),
    };
    assert_eq!(gate_id_b, "gate-B");
    assert_ne!(gate_id_a, gate_id_b, "gate_ids must differ");
}

// ===========================================================================
// Part 4 — answer_wasl_gate emits AnswerWaslGate IPC
// ===========================================================================

/// answer_wasl_gate emits AnswerWaslGate IPC to the agent.
#[test]
fn answer_wasl_gate_emits_answer_wasl_gate_ipc() {
    let (mut core, mut peer) = make_core();
    let _sid = launch_wasl_local(&mut core, &mut peer);

    // Set up a pending gate first.
    let req_line = gate_requested_line(
        "local", "send-gate-1", true, "message", "marsad-local", "s", "p",
    );
    core.on_agent_line(&req_line).expect("gate requested");
    let _ = peer.drain_outbound();

    // Answer it.
    let updates = core
        .answer_wasl_gate("local", "send-gate-1", Decision::Approve)
        .expect("answer must succeed");
    assert!(
        updates.is_empty(),
        "answer_wasl_gate returns empty update list per contract"
    );

    // The outbound IPC must be AnswerWaslGate.
    let out = peer.drain_outbound();
    assert_eq!(out.len(), 1, "exactly one outbound line, got {out:?}");
    let parsed = parse_ipc(&out[0]).expect("must be valid IpcMsg");
    match parsed {
        IpcMsg::AnswerWaslGate {
            host_id,
            gate_id,
            outcome,
            reason,
        } => {
            assert_eq!(host_id, "local");
            assert_eq!(gate_id, "send-gate-1");
            assert_eq!(outcome, "approved");
            assert_eq!(reason, None);
        }
        other => panic!("expected AnswerWaslGate, got {other:?}"),
    }
}

/// answer_wasl_gate with Reject carries outcome="denied" and the reason.
#[test]
fn answer_wasl_gate_reject_emits_denied_outcome_with_reason() {
    let (mut core, mut peer) = make_core();
    let _sid = launch_wasl_local(&mut core, &mut peer);

    let req_line = gate_requested_line(
        "local", "send-gate-1", true, "message", "marsad-local", "s", "p",
    );
    core.on_agent_line(&req_line).expect("gate requested");
    let _ = peer.drain_outbound();

    core.answer_wasl_gate(
        "local",
        "send-gate-1",
        Decision::Reject {
            reason: "blocked".to_string(),
        },
    )
    .expect("answer must succeed");

    let out = peer.drain_outbound();
    assert_eq!(out.len(), 1);
    let parsed = parse_ipc(&out[0]).expect("must be valid IpcMsg");
    match parsed {
        IpcMsg::AnswerWaslGate {
            outcome, reason, ..
        } => {
            assert_eq!(outcome, "denied");
            assert_eq!(reason, Some("blocked".to_string()));
        }
        other => panic!("expected AnswerWaslGate, got {other:?}"),
    }
}

/// answer_wasl_gate for an unknown gate_id returns CoreError::UnknownWaslGate.
#[test]
fn answer_wasl_gate_unknown_gate_id_errors_correctly() {
    let (mut core, _peer) = make_core();
    let err = core
        .answer_wasl_gate("local", "nonexistent", Decision::Approve)
        .expect_err("must error for unknown gate");
    assert!(
        matches!(err, marsad_core::CoreError::UnknownWaslGate(_)),
        "expected CoreError::UnknownWaslGate, got {err:?}"
    );
}

/// answer_wasl_gate idempotency: the ledger deduplicates a second identical call.
#[test]
fn answer_wasl_gate_is_idempotent_per_ledger() {
    let (mut core, mut peer) = make_core();
    let _sid = launch_wasl_local(&mut core, &mut peer);

    let req_line = gate_requested_line(
        "local", "send-gate-1", true, "message", "marsad-local", "s", "p",
    );
    core.on_agent_line(&req_line).expect("gate requested");
    let _ = peer.drain_outbound();

    // First answer.
    core.answer_wasl_gate("local", "send-gate-1", Decision::Approve)
        .expect("first answer");
    let out1 = peer.drain_outbound();
    assert_eq!(out1.len(), 1, "first answer must emit IPC");

    // Second answer with the same scope+gate_id: ledger deduplication → no-op.
    let updates2 = core
        .answer_wasl_gate("local", "send-gate-1", Decision::Approve)
        .expect("idempotent second answer must be Ok");
    let out2 = peer.drain_outbound();
    assert!(
        updates2.is_empty(),
        "second answer must return empty updates (ledger seen)"
    );
    assert!(
        out2.is_empty(),
        "second answer must not emit duplicate IPC, got {out2:?}"
    );
}
