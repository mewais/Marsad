//! wasl.rs — X3 core bucket: inbound Wasl arms, `VmUpdate`s, `answer_wasl_gate`.
//!
//! Authoritative spec: `docs/specs/wasl-connector.md` §5.2 ("marsad-core —
//! inbound arms, `VmUpdate`s, `answer_wasl_gate`"), §6 ("Optionality proof",
//! point 3), and §7.6 ("marsad-core — NEW tests/wasl.rs (loopback transport,
//! house pattern)"). Drives `Core` over `LoopbackFake`, exactly like
//! `tests/fakes.rs`/`tests/branches.rs` — inbound lines are fed directly to
//! `on_agent_line` (playing the role of the agent), outbound `AnswerWaslGate`
//! lines are read back via `peer.drain_outbound()`.
//!
//! `VmUpdate` has no `PartialEq` derive (see `crates/marsad-core/src/lib.rs`'s
//! `#[derive(Debug, Clone)] pub enum VmUpdate`), so every assertion below
//! `match`es the variant and compares its fields directly — the same pattern
//! `tests/branches.rs` already uses for the pre-existing variants.
//!
//! This file WILL fail to compile until the code-agent lands `marsad-wasl` as
//! a dependency of `marsad-core`, the `WaslGateView` struct, the 3 new
//! `VmUpdate::Wasl*` variants, the `CoreError::UnknownWaslGate` variant, the
//! new `on_agent_line` arms, and `Core::answer_wasl_gate` (§5.2). That is the
//! expected tests-first compile-failure signal.

use marsad_core::{Core, CoreConfig, CoreError, VmUpdate, WaslGateView};
use marsad_gates::Decision;
use marsad_proto::{encode_ipc, parse_ipc, IpcMsg};
use marsad_transport::LoopbackFake;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn cfg() -> CoreConfig {
    CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-core-wasl-test".to_string(),
    }
}

fn make_core() -> (Core, LoopbackFake) {
    let (a, b) = LoopbackFake::pair();
    let core = Core::new(cfg(), Box::new(a)).expect("Core::new");
    (core, b)
}

#[allow(clippy::too_many_arguments)]
fn gate_requested_line(
    host_id: &str,
    gate_id: &str,
    decidable: bool,
    kind: &str,
    from: &str,
    summary: &str,
    preview: &str,
    requested_at: &str,
    deadline: &str,
) -> String {
    encode_ipc(&IpcMsg::WaslGateRequested {
        host_id: host_id.to_string(),
        gate_id: gate_id.to_string(),
        mode: "external".to_string(),
        decidable,
        request: serde_json::json!({
            "id": "call-9",
            "kind": kind,
            "from": from,
            "summary": summary,
            "preview": preview,
        }),
        requested_at: requested_at.to_string(),
        timeout_ms: 120_000,
        deadline: deadline.to_string(),
    })
}

fn gate_decided_line(
    host_id: &str,
    gate_id: &str,
    outcome: &str,
    decided_by: &str,
    reason: Option<&str>,
    decided_at: &str,
) -> String {
    encode_ipc(&IpcMsg::WaslGateDecided {
        host_id: host_id.to_string(),
        gate_id: gate_id.to_string(),
        outcome: outcome.to_string(),
        decided_by: decided_by.to_string(),
        reason: reason.map(str::to_string),
        decided_at: decided_at.to_string(),
    })
}

fn observation_line(host_id: &str, entry: serde_json::Value) -> String {
    encode_ipc(&IpcMsg::WaslObservation {
        host_id: host_id.to_string(),
        entry,
    })
}

fn view(
    host_id: &str,
    gate_id: &str,
    kind: &str,
    from: &str,
    summary: &str,
    preview: &str,
    requested_at: &str,
    deadline: &str,
) -> WaslGateView {
    WaslGateView {
        host_id: host_id.to_string(),
        gate_id: gate_id.to_string(),
        kind: kind.to_string(),
        from: from.to_string(),
        summary: summary.to_string(),
        preview: preview.to_string(),
        requested_at: requested_at.to_string(),
        deadline: deadline.to_string(),
    }
}

fn only_wasl_gates(updates: &[VmUpdate]) -> &Vec<WaslGateView> {
    assert_eq!(
        updates.len(),
        1,
        "expected exactly one VmUpdate, got {updates:?}"
    );
    match &updates[0] {
        VmUpdate::WaslGates(views) => views,
        other => panic!("expected VmUpdate::WaslGates, got {other:?}"),
    }
}

// ===========================================================================
// Part 1 — on_agent_line: WaslGateRequested
// ===========================================================================

#[test]
fn wasl_gate_requested_emits_wasl_gates_with_exact_view() {
    let (mut core, _peer) = make_core();
    let line = gate_requested_line(
        "local",
        "gate-1",
        true,
        "message",
        "wasl-a",
        "message to 1 peer(s)",
        "hi bob",
        "2026-07-02T12:00:00.000Z",
        "2026-07-02T12:02:00.000Z",
    );
    let updates = core.on_agent_line(&line).expect("must parse+apply");
    let views = only_wasl_gates(&updates);
    assert_eq!(
        views,
        &vec![view(
            "local",
            "gate-1",
            "message",
            "wasl-a",
            "message to 1 peer(s)",
            "hi bob",
            "2026-07-02T12:00:00.000Z",
            "2026-07-02T12:02:00.000Z",
        )]
    );
}

#[test]
fn wasl_gate_requested_duplicate_line_is_idempotent_single_entry() {
    let (mut core, _peer) = make_core();
    let line = gate_requested_line(
        "local", "gate-1", true, "message", "wasl-a", "s", "p", "t1", "d1",
    );
    core.on_agent_line(&line).expect("first upsert");
    let updates = core.on_agent_line(&line).expect("duplicate upsert");
    let views = only_wasl_gates(&updates);
    assert_eq!(
        views.len(),
        1,
        "duplicate upsert of the same key must not grow the list"
    );
}

#[test]
fn wasl_gate_requested_second_distinct_gate_yields_two_entry_deterministic_order() {
    let (mut core, _peer) = make_core();
    // Insert out of lexicographic order: host "b" first, then host "a".
    core.on_agent_line(&gate_requested_line(
        "b", "gate-x", true, "command", "wasl-b", "s2", "p2", "t2", "d2",
    ))
    .expect("first gate");
    let updates = core
        .on_agent_line(&gate_requested_line(
            "a", "gate-y", true, "consult", "wasl-a", "s1", "p1", "t1", "d1",
        ))
        .expect("second gate");
    let views = only_wasl_gates(&updates);
    assert_eq!(views.len(), 2);
    assert_eq!(
        (views[0].host_id.as_str(), views[0].gate_id.as_str()),
        ("a", "gate-y"),
        "BTreeMap<(host_id, gate_id)> ordering must be deterministic (lexicographic)"
    );
    assert_eq!(
        (views[1].host_id.as_str(), views[1].gate_id.as_str()),
        ("b", "gate-x"),
    );
}

#[test]
fn wasl_gate_requested_not_decidable_is_tolerant_noop() {
    let (mut core, _peer) = make_core();
    let line = gate_requested_line(
        "local", "gate-1", false, "consult", "wasl-a", "s", "p", "t", "d",
    );
    let updates = core
        .on_agent_line(&line)
        .expect("observe-only gate must not error");
    assert!(
        updates.is_empty(),
        "decidable:false gates are observe-only, never pending, got {updates:?}"
    );
}

#[test]
fn wasl_gate_requested_malformed_request_value_is_tolerant_skip() {
    let (mut core, _peer) = make_core();
    let line = encode_ipc(&IpcMsg::WaslGateRequested {
        host_id: "local".to_string(),
        gate_id: "gate-1".to_string(),
        mode: "external".to_string(),
        decidable: true,
        request: serde_json::json!({}), // missing id/kind/from/summary/preview
        requested_at: "t".to_string(),
        timeout_ms: 0,
        deadline: "d".to_string(),
    });
    let updates = core
        .on_agent_line(&line)
        .expect("malformed request must be a tolerant skip, not an error");
    assert!(updates.is_empty(), "got {updates:?}");
}

#[test]
fn wasl_gate_requested_missing_host_id_is_protocol_error() {
    let (mut core, _peer) = make_core();
    let line = r#"{"type":"WaslGateRequested","gate_id":"gate-1","mode":"external","decidable":true,"request":{},"requested_at":"","timeout_ms":0,"deadline":""}"#;
    let err = core
        .on_agent_line(line)
        .expect_err("missing host_id (identity field) must be a hard Protocol error");
    assert!(matches!(err, CoreError::Protocol(_)), "got {err:?}");
}

#[test]
fn wasl_gate_requested_missing_gate_id_is_protocol_error() {
    let (mut core, _peer) = make_core();
    let line = r#"{"type":"WaslGateRequested","host_id":"local","mode":"external","decidable":true,"request":{},"requested_at":"","timeout_ms":0,"deadline":""}"#;
    let err = core
        .on_agent_line(line)
        .expect_err("missing gate_id (identity field) must be a hard Protocol error");
    assert!(matches!(err, CoreError::Protocol(_)), "got {err:?}");
}

// ===========================================================================
// Part 2 — on_agent_line: WaslGateDecided
// ===========================================================================

#[test]
fn wasl_gate_decided_for_pending_gate_emits_decided_then_shrunk_list() {
    let (mut core, _peer) = make_core();
    core.on_agent_line(&gate_requested_line(
        "local", "gate-1", true, "message", "wasl-a", "s", "p", "t", "d",
    ))
    .expect("seed pending gate");

    let line = gate_decided_line(
        "local",
        "gate-1",
        "approved",
        "operator",
        None,
        "2026-07-02T12:00:03.500Z",
    );
    let updates = core.on_agent_line(&line).expect("decided must apply");
    assert_eq!(
        updates.len(),
        2,
        "expected [WaslGateDecided, WaslGates(shrunk)], got {updates:?}"
    );

    match &updates[0] {
        VmUpdate::WaslGateDecided {
            host_id,
            gate_id,
            outcome,
            decided_by,
            reason,
        } => {
            assert_eq!(host_id, "local");
            assert_eq!(gate_id, "gate-1");
            assert_eq!(outcome, "approved");
            assert_eq!(decided_by, "operator");
            assert_eq!(reason, &None);
        }
        other => panic!("expected VmUpdate::WaslGateDecided first, got {other:?}"),
    }
    match &updates[1] {
        VmUpdate::WaslGates(views) => {
            assert!(
                views.is_empty(),
                "the decided gate must be removed from the pending list"
            );
        }
        other => panic!("expected VmUpdate::WaslGates second, got {other:?}"),
    }
}

#[test]
fn wasl_gate_decided_denied_carries_reason() {
    let (mut core, _peer) = make_core();
    core.on_agent_line(&gate_requested_line(
        "local", "gate-1", true, "message", "wasl-a", "s", "p", "t", "d",
    ))
    .expect("seed pending gate");

    let line = gate_decided_line(
        "local",
        "gate-1",
        "denied",
        "operator",
        Some("not during business hours"),
        "2026-07-02T12:00:03.500Z",
    );
    let updates = core.on_agent_line(&line).expect("decided must apply");
    match &updates[0] {
        VmUpdate::WaslGateDecided {
            outcome, reason, ..
        } => {
            assert_eq!(outcome, "denied");
            assert_eq!(reason.as_deref(), Some("not during business hours"));
        }
        other => panic!("expected VmUpdate::WaslGateDecided, got {other:?}"),
    }
}

#[test]
fn wasl_gate_decided_for_unknown_gate_emits_decided_only_no_list_update() {
    let (mut core, _peer) = make_core();
    // No prior WaslGateRequested for this (host_id, gate_id).
    let line = gate_decided_line("local", "gate-ghost", "approved", "operator", None, "t");
    let updates = core
        .on_agent_line(&line)
        .expect("decided must apply even for an unknown gate");
    assert_eq!(
        updates.len(),
        1,
        "an unknown-gate decision must emit ONLY WaslGateDecided (no removal happened), got {updates:?}"
    );
    assert!(matches!(updates[0], VmUpdate::WaslGateDecided { .. }));
}

/// The synthesized `GateVanished` resolution (`outcome:"unknown"`,
/// `decided_by:"resync"`) also clears the pending list, exactly like a real
/// `gate_decided` frame.
#[test]
fn wasl_gate_decided_synthesized_unknown_resync_also_clears_pending_list() {
    let (mut core, _peer) = make_core();
    core.on_agent_line(&gate_requested_line(
        "local", "gate-7", true, "message", "wasl-a", "s", "p", "t", "d",
    ))
    .expect("seed pending gate");

    let line = gate_decided_line("local", "gate-7", "unknown", "resync", None, "");
    let updates = core
        .on_agent_line(&line)
        .expect("synthesized decided must apply");
    assert_eq!(updates.len(), 2);
    match &updates[0] {
        VmUpdate::WaslGateDecided {
            outcome,
            decided_by,
            ..
        } => {
            assert_eq!(outcome, "unknown");
            assert_eq!(decided_by, "resync");
        }
        other => panic!("expected VmUpdate::WaslGateDecided, got {other:?}"),
    }
    match &updates[1] {
        VmUpdate::WaslGates(views) => assert!(views.is_empty()),
        other => panic!("expected VmUpdate::WaslGates, got {other:?}"),
    }
}

// ===========================================================================
// Part 3 — on_agent_line: WaslObservation
// ===========================================================================

#[test]
fn wasl_observation_decision_entry_emits_typed_observation() {
    let (mut core, _peer) = make_core();
    let entry = serde_json::json!({
        "seq": 3,
        "kind": "decision",
        "actor": "wasl-a",
        "targets": ["wasl-b"],
        "summary": "gate Message",
        "decision": {"decision": "approve"},
    });
    let updates = core
        .on_agent_line(&observation_line("local", entry))
        .expect("observation must apply");
    assert_eq!(updates.len(), 1);
    match &updates[0] {
        VmUpdate::WaslObservation { host_id, entry } => {
            assert_eq!(host_id, "local");
            assert_eq!(entry.seq, 3);
            assert_eq!(entry.kind, "decision");
            assert_eq!(entry.actor, "wasl-a");
            assert_eq!(entry.targets, vec!["wasl-b".to_string()]);
            assert_eq!(entry.summary, "gate Message");
            assert_eq!(
                entry.decision.as_ref().map(|d| d.decision.as_str()),
                Some("approve")
            );
        }
        other => panic!("expected VmUpdate::WaslObservation, got {other:?}"),
    }
}

#[test]
fn wasl_observation_message_entry_has_none_decision() {
    let (mut core, _peer) = make_core();
    let entry = serde_json::json!({
        "seq": 4,
        "kind": "message",
        "actor": "wasl-a",
        "targets": ["wasl-b"],
        "summary": "message to 1 peer(s)",
        "decision": null,
    });
    let updates = core
        .on_agent_line(&observation_line("local", entry))
        .expect("observation must apply");
    match &updates[0] {
        VmUpdate::WaslObservation { entry, .. } => {
            assert_eq!(
                entry.decision, None,
                "audit_message-style entries have decision==None"
            );
        }
        other => panic!("expected VmUpdate::WaslObservation, got {other:?}"),
    }
}

#[test]
fn wasl_observation_malformed_entry_is_tolerant_skip() {
    let (mut core, _peer) = make_core();
    let updates = core
        .on_agent_line(&observation_line("local", serde_json::json!({})))
        .expect("malformed entry must be a tolerant skip, not an error");
    assert!(updates.is_empty(), "got {updates:?}");
}

// ===========================================================================
// Part 4 — on_agent_line: WaslGateAnswered (ack no-op) / misdirected AnswerWaslGate
// ===========================================================================

#[test]
fn wasl_gate_answered_inbound_is_noop() {
    let (mut core, _peer) = make_core();
    let line = encode_ipc(&IpcMsg::WaslGateAnswered {
        host_id: "local".to_string(),
        gate_id: "gate-1".to_string(),
    });
    let updates = core.on_agent_line(&line).expect("ack must be accepted");
    assert!(updates.is_empty(), "got {updates:?}");
}

/// `AnswerWaslGate` is a GUI/core -> agent tag; arriving inbound on the agent
/// channel is a routing bug, same class as every other misdirected tag.
#[test]
fn answer_wasl_gate_inbound_is_misdirected_protocol_error() {
    let (mut core, _peer) = make_core();
    let line = encode_ipc(&IpcMsg::AnswerWaslGate {
        host_id: "local".to_string(),
        gate_id: "gate-1".to_string(),
        outcome: "approved".to_string(),
        reason: None,
    });
    let err = core
        .on_agent_line(&line)
        .expect_err("AnswerWaslGate arriving inbound must be a Protocol error");
    assert!(matches!(err, CoreError::Protocol(_)), "got {err:?}");
}

// ===========================================================================
// Part 5 — Core::answer_wasl_gate (§5.2's human decision path)
// ===========================================================================

#[test]
fn answer_wasl_gate_approve_sends_exactly_one_approve_golden_line() {
    let (mut core, mut peer) = make_core();
    core.on_agent_line(&gate_requested_line(
        "local", "gate-1", true, "message", "wasl-a", "s", "p", "t", "d",
    ))
    .expect("seed pending gate");
    let _ = peer.drain_outbound(); // consume nothing (inbound doesn't send outbound)

    let updates = core
        .answer_wasl_gate("local", "gate-1", Decision::Approve)
        .expect("answer_wasl_gate must succeed for a pending gate");
    assert!(
        updates.is_empty(),
        "node-authoritative: the pending list changes only on WaslGateDecided, not here, got {updates:?}"
    );

    let out = peer.drain_outbound();
    assert_eq!(out.len(), 1, "exactly one outbound line, got {out:?}");
    let parsed = parse_ipc(&out[0]).expect("outbound line must be valid IpcMsg");
    assert_eq!(
        parsed,
        IpcMsg::AnswerWaslGate {
            host_id: "local".to_string(),
            gate_id: "gate-1".to_string(),
            outcome: "approved".to_string(),
            reason: None,
        }
    );
}

#[test]
fn answer_wasl_gate_reject_sends_exactly_one_deny_golden_line() {
    let (mut core, mut peer) = make_core();
    core.on_agent_line(&gate_requested_line(
        "local", "gate-1", true, "message", "wasl-a", "s", "p", "t", "d",
    ))
    .expect("seed pending gate");
    let _ = peer.drain_outbound();

    core.answer_wasl_gate(
        "local",
        "gate-1",
        Decision::Reject {
            reason: "not during business hours".to_string(),
        },
    )
    .expect("answer_wasl_gate must succeed for a pending gate");

    let out = peer.drain_outbound();
    assert_eq!(out.len(), 1);
    let parsed = parse_ipc(&out[0]).expect("outbound line must be valid IpcMsg");
    assert_eq!(
        parsed,
        IpcMsg::AnswerWaslGate {
            host_id: "local".to_string(),
            gate_id: "gate-1".to_string(),
            outcome: "denied".to_string(),
            reason: Some("not during business hours".to_string()),
        }
    );
}

/// Ledger idempotency (§5.2: "identical to answer_gate — seen -> silent
/// no-op, no duplicate IPC"). Also covers the note that approve-then-deny of
/// the SAME gate is deduped too (decision_id ignores the verdict value).
#[test]
fn answer_wasl_gate_second_call_is_idempotent_no_duplicate_ipc() {
    let (mut core, mut peer) = make_core();
    core.on_agent_line(&gate_requested_line(
        "local", "gate-1", true, "message", "wasl-a", "s", "p", "t", "d",
    ))
    .expect("seed pending gate");
    let _ = peer.drain_outbound();

    core.answer_wasl_gate("local", "gate-1", Decision::Approve)
        .expect("first answer");
    let first_out = peer.drain_outbound();
    assert_eq!(first_out.len(), 1);

    // Second call, even with a DIFFERENT verdict, must be a silent no-op:
    // decision_id is derived from (scope, gate_id) only.
    let updates = core
        .answer_wasl_gate(
            "local",
            "gate-1",
            Decision::Reject {
                reason: "changed my mind".to_string(),
            },
        )
        .expect("idempotent second call must still be Ok");
    assert!(updates.is_empty(), "got {updates:?}");
    let second_out = peer.drain_outbound();
    assert!(
        second_out.is_empty(),
        "duplicate answer_wasl_gate must not emit a second outbound line, got: {second_out:?}"
    );
}

#[test]
fn answer_wasl_gate_unknown_gate_returns_unknown_wasl_gate_error() {
    let (mut core, _peer) = make_core();
    let err = core
        .answer_wasl_gate("local", "gate-never-requested", Decision::Approve)
        .expect_err("answering a gate never seen must error");
    assert!(matches!(err, CoreError::UnknownWaslGate(_)), "got {err:?}");
}

/// §5.2: "NO session lookup (Wasl gates are host-scoped, not session-scoped
/// — deliberately not require_session)" — this whole file never calls
/// `launch_session`; every test above already proves this works with zero
/// sessions. This test makes the absence of any session explicit and central.
#[test]
fn answer_wasl_gate_works_with_zero_sessions_ever_launched() {
    let (mut core, mut peer) = make_core();
    core.on_agent_line(&gate_requested_line(
        "local", "gate-1", true, "message", "wasl-a", "s", "p", "t", "d",
    ))
    .expect("seed pending gate");
    let _ = peer.drain_outbound();

    // No `launch_session` call anywhere in this test.
    core.answer_wasl_gate("local", "gate-1", Decision::Approve)
        .expect("must succeed without any session ever having been launched");
}

/// §5.2: "pending list NOT shrunk by answering (only by the decided
/// broadcast)" — after answering, the gate is still tracked (a re-request
/// still collapses to the same one-entry list, proving it was never removed).
#[test]
fn answer_wasl_gate_does_not_shrink_the_pending_list() {
    let (mut core, mut peer) = make_core();
    core.on_agent_line(&gate_requested_line(
        "local", "gate-1", true, "message", "wasl-a", "s", "p", "t", "d",
    ))
    .expect("seed pending gate");
    let _ = peer.drain_outbound();

    core.answer_wasl_gate("local", "gate-1", Decision::Approve)
        .expect("answer must succeed");

    let updates = core
        .on_agent_line(&gate_requested_line(
            "local", "gate-1", true, "message", "wasl-a", "s", "p", "t", "d",
        ))
        .expect("re-request after answering");
    let views = only_wasl_gates(&updates);
    assert_eq!(
        views.len(),
        1,
        "the gate must still be tracked after answer_wasl_gate — only WaslGateDecided removes it"
    );
    assert_eq!(views[0].gate_id, "gate-1");
}

// ===========================================================================
// Part 6 — absent-config = inert, core side (§6 point 3)
// ===========================================================================

fn is_wasl_variant(u: &VmUpdate) -> bool {
    matches!(
        u,
        VmUpdate::WaslGates(_)
            | VmUpdate::WaslGateDecided { .. }
            | VmUpdate::WaslObservation { .. }
    )
}

/// §6 point 3: "a fresh Core emits no Wasl* VmUpdate from any existing flow."
/// A spot-check over an ordinary non-Wasl inbound flow (U1 capabilities
/// update — chosen because, like Wasl, it is a broadcast-only inbound tag
/// with no session requirement) confirms no `Wasl*` variant is ever
/// constructed by code paths a Wasl-less agent could actually exercise.
#[test]
fn ordinary_non_wasl_inbound_flow_never_emits_a_wasl_variant() {
    let (mut core, _peer) = make_core();
    let line = r#"{"type":"CapabilitiesUpdate","host_id":"local","driver":"claude","capabilities":{"driver":"claude","host_id":"local"}}"#;
    let updates = core
        .on_agent_line(line)
        .expect("ordinary flow must still work");
    assert!(
        !updates.iter().any(is_wasl_variant),
        "an ordinary capabilities flow must never emit a Wasl* VmUpdate, got {updates:?}"
    );
}

/// A brand-new `Core`'s `wasl_gates` state starts empty: answering ANY gate
/// id before any `WaslGateRequested` was ever observed is `UnknownWaslGate`,
/// not a panic or a silent success — the strongest available black-box proof
/// that nothing pre-populates Wasl state.
#[test]
fn fresh_core_has_no_pending_wasl_gates() {
    let (mut core, _peer) = make_core();
    let err = core
        .answer_wasl_gate("local", "gate-1", Decision::Approve)
        .expect_err("a fresh Core must have zero pending Wasl gates");
    assert!(matches!(err, CoreError::UnknownWaslGate(_)));
}
