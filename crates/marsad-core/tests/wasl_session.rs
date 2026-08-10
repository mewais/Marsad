//! wasl_session.rs — Wasl **Backend** (Wasl-managed session) core bucket.
//!
//! Authoritative design: `docs/design/wasl-backend-plan.md` (v3), especially
//! §6 (RuntimeOwner + centralized dedicated routing methods, persisted
//! `WaslSessionBinding` + reconnect, `get_or_start` semantics), §6.3 (status
//! projection → the EXISTING `Phase`, NO new variant), §8 (native gate →
//! EXISTING `Gate::Permission` pipeline, session-scoped, decision-ledger
//! idempotent), and §10.4 (this file's test plan incl. transcript-folding edge
//! cases). Drives `Core` over `LoopbackFake`, exactly like `tests/wasl.rs`/
//! `tests/happy.rs`: inbound lines are fed to `on_agent_line` (playing the
//! agent), outbound `*Wasl*` command lines are read back via
//! `peer.drain_outbound()`. Runs entirely without a real Wasl process.
//!
//! `VmUpdate` has no `PartialEq` derive, so assertions `match` the variant and
//! compare fields directly (the pattern `tests/wasl.rs` already uses).
//!
//! ── NOTE(test-agent): INVENTED / ASSUMED CORE SURFACE ────────────────────────
//! `docs/design/wasl-backend-plan.md` is a DESIGN doc (no `docs/specs/
//! wasl-backend.md` exists yet). §6 pins the BEHAVIOR but not exact Rust seam
//! names on `marsad_core::Core`. This file assumes the seams below, chosen to
//! mirror the existing `launch_session`/`send_prompt`/`answer_wasl_gate` shapes.
//! If the code-agent lands differently-shaped seams it should adjust these call
//! sites — the pinned BEHAVIOR (a WaslManaged session's send/attach/stop/
//! interrupt emit the `*Wasl*` variants; a MarsadManaged session emits EXACTLY
//! today's variants; `WaslSessionStatus` projects per the §6.3 table with NO new
//! `Phase`; a native gate upserts one `Gate::Permission` and answering it emits
//! one ledger-idempotent `AnswerWaslNativeGate`; the host-scoped
//! `VmUpdate::WaslGates` list is untouched; folding edge cases hold) is the
//! actual contract.
//!
//!   * `Core::launch_wasl_session(&mut self, host_id: &str,
//!         wasl_target_key: serde_json::Value, name: Option<String>)
//!         -> Result<String, CoreError>`
//!     — creates a `WaslManaged` session (RuntimeOwner::WaslManaged{binding}),
//!     emits `LaunchWaslSession`, and returns the marsad session id. The
//!     analogue of `launch_session` for the Wasl runtime owner.
//!   * The EXISTING `send_prompt`/`attach_session`/`interrupt`/`stop_session`
//!     route by RuntimeOwner (§6.1 dedicated routing methods): for a
//!     WaslManaged session they emit `SendWaslPrompt`/`AttachWaslSession`/
//!     `InterruptWaslSession`/`StopWaslSession`; for a MarsadManaged session
//!     they emit EXACTLY today's `SendPrompt`/`AttachSession`/`Interrupt`/
//!     `StopSession` (a regression pin).
//!   * `Core::answer_wasl_native_gate(&mut self, session_id: &str,
//!         gate_id: &str, decision: marsad_gates::Decision)
//!         -> Result<Vec<VmUpdate>, CoreError>`
//!     — the session-scoped native-gate answer path (§8): records in the
//!     EXISTING `DecisionLedger` (idempotent) and emits one
//!     `AnswerWaslNativeGate`. `answer_gate` may also route here for a
//!     WaslManaged session; this dedicated method is what this file drives.
//!     The `gate_id` core answers with is the per-occurrence `(turn_id, seq)`
//!     key `native_gate_key(turn_id, seq)` (§8, v3) — the real gate has no
//!     `gate_id` field, so identity is derived from the folding event's own
//!     `turn_id` + stream `seq`.
//!
//! ── V3 CONTRACT NOTES (test-agent, spec-correction cascade) ──────────────────
//! The foundational `marsad-wasl-session` crate was corrected to the v3 Wasl
//! contract (see the plan's v3 revision log). Every `WaslSessionEventKind` now
//! carries `turn_id` and, where applicable, `source`/`certainty`;
//! `WaslNativeGateDetail` is ONLY `{summary, preview}` (no `gate_id`/`tool`);
//! `WaslTurnReceipt` gains `Duplicate{original}`; native-gate identity is
//! `native_gate_key(turn_id, seq)` and `native_gate_command(gate_key, detail)`
//! is 2-arg; `WaslStreamItem{Event, Truncated}` is the stream vocabulary. These
//! tests were revised to construct/match the v3 shapes.
//!
//! IPC note (v3, verified against the shipped `marsad-proto`): the
//! `AnswerWaslNativeGate`/`WaslNativeGateAnswered` wire variants remain keyed by
//! a single `gate_id` string (NOT the doc-§7 `turn_id`+`gate_key` pair). Core
//! puts the per-occurrence `native_gate_key(turn_id, seq)` INTO that `gate_id`
//! field (it is Marsad's ledger identity); Wasl-side resolution by bare `turn_id`
//! happens agent-side (marsad-agent decomposes the key). `SendWaslPrompt` carries
//! NO `causation` — the agent mints the fresh single-hop `WaslCausation` (§7); it
//! is never on the wire.
//!
//! ── STORE-SCHEMA GAP flagged (see the reconnect test) ────────────────────────
//! §6.1 persists a `WaslSessionBinding` on the session record so a fresh Core
//! reconnects after a restart. `marsad_store::PersistedSession` has NO
//! `runtime_owner`/binding field today (grepped). Persisting the binding
//! therefore requires a marsad-store schema addition (out of this crate's edit
//! scope). The reconnect test below asserts the BLACK-BOX behavior (a fresh Core
//! sharing the same store restores the WaslManaged session and reconnects it
//! without duplicating it); the store field is the code-agent/owner's to add.
//!
//! This file WILL fail to compile until the code-agent adds `marsad-wasl-session`
//! as a dependency of `marsad-core`, lands the `RuntimeOwner` field + routing +
//! `launch_wasl_session` + `answer_wasl_native_gate` + the `on_agent_line`
//! `WaslSessionStatus`/`WaslSessionEvent`/`WaslPromptReceipt`/
//! `WaslNativeGateAnswered` arms, and the store-schema binding. That is the
//! expected tests-first compile-failure (red-state) signal.

use marsad_core::{Core, CoreConfig, CoreError, VmUpdate};
use marsad_events::Phase;
use marsad_gates::{Decision, Gate};
use marsad_proto::{encode_ipc, parse_ipc, IpcMsg};
use marsad_transport::LoopbackFake;
use marsad_wasl_session::{
    native_gate_command, native_gate_key, project_status_to_phase, WaslNativeGateDetail,
    WaslSessionEvent, WaslSessionEventKind, WaslStreamItem, WaslTargetStatus, WaslTurnReceipt,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn cfg() -> CoreConfig {
    CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-core-wasl-session-test".to_string(),
    }
}

fn make_core() -> (Core, LoopbackFake) {
    let (a, b) = LoopbackFake::pair();
    let core = Core::new(cfg(), Box::new(a)).expect("Core::new");
    (core, b)
}

/// Launch a WaslManaged session and drain the launch IPC, returning its id.
fn launch_wasl(core: &mut Core, peer: &mut LoopbackFake) -> String {
    let id = core
        .launch_wasl_session(
            "local",
            serde_json::json!({"protocol": "claude", "ctx": "t"}),
            None,
        )
        .expect("launch_wasl_session must succeed");
    let _ = peer.drain_outbound();
    id
}

/// Launch a plain MarsadManaged session (today's path) and drain, returning id.
fn launch_marsad(core: &mut Core, peer: &mut LoopbackFake) -> String {
    let spec = marsad_core::LaunchSpec {
        project: "/home/u/proj".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: "claude-opus-4-5".to_string(),
        session_mode: String::new(),
        first_prompt: None,
        mode: Default::default(),
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
    };
    let id = core.launch_session(spec).expect("launch_session");
    let _ = peer.drain_outbound();
    id
}

// ── inbound line builders (agent → core) ─────────────────────────────────────

fn status_line(session_id: &str, status: &WaslTargetStatus) -> String {
    encode_ipc(&IpcMsg::WaslSessionStatus {
        session_id: session_id.to_string(),
        status: serde_json::to_value(status).unwrap(),
    })
}

fn event_line(session_id: &str, ev: &WaslSessionEvent) -> String {
    encode_ipc(&IpcMsg::WaslSessionEvent {
        session_id: session_id.to_string(),
        seq: ev.seq,
        event: serde_json::to_value(ev).unwrap(),
    })
}

fn receipt_line(session_id: &str, receipt: &WaslTurnReceipt) -> String {
    encode_ipc(&IpcMsg::WaslPromptReceipt {
        session_id: session_id.to_string(),
        receipt: serde_json::to_value(receipt).unwrap(),
    })
}

fn last_session_view(updates: &[VmUpdate], id: &str) -> marsad_viewmodel::SessionView {
    updates
        .iter()
        .rev()
        .find_map(|u| match u {
            VmUpdate::Session(sv) if sv.id == id => Some(sv.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected a VmUpdate::Session for {id}, got {updates:?}"))
}

fn ev(seq: u64, kind: WaslSessionEventKind) -> WaslSessionEvent {
    WaslSessionEvent { seq, kind }
}

// ===========================================================================
// Part 1 — runtime-owner routing via dedicated methods (§6.1, §10.4)
// ===========================================================================

/// A WaslManaged session's `send_prompt` emits `SendWaslPrompt`, NOT
/// `SendPrompt` — the centralized RuntimeOwner routing (§6.1).
#[test]
fn wasl_managed_send_prompt_emits_send_wasl_prompt() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    core.send_prompt(&id, "hello wasl").expect("send_prompt");
    let out = peer.drain_outbound();
    let msgs: Vec<IpcMsg> = out.iter().filter_map(|l| parse_ipc(l).ok()).collect();
    assert!(
        msgs.iter().any(
            |m| matches!(m, IpcMsg::SendWaslPrompt { session_id, text, .. }
            if session_id == &id && text == "hello wasl")
        ),
        "a WaslManaged send_prompt must emit SendWaslPrompt, got {msgs:?}"
    );
    assert!(
        !msgs.iter().any(|m| matches!(m, IpcMsg::SendPrompt { .. })),
        "a WaslManaged send_prompt must NEVER emit the MarsadManaged SendPrompt, got {msgs:?}"
    );
}

/// A WaslManaged `attach_session`/`interrupt`/`stop_session` emit the `*Wasl*`
/// variants, never today's.
#[test]
fn wasl_managed_attach_interrupt_stop_emit_wasl_variants() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    core.attach_session(&id).expect("attach");
    assert!(
        peer.drain_outbound().iter().any(|l| matches!(
            parse_ipc(l),
            Ok(IpcMsg::AttachWaslSession { session_id, .. }) if session_id == id
        )),
        "attach_session on a WaslManaged session must emit AttachWaslSession"
    );

    core.interrupt(&id).expect("interrupt");
    assert!(
        peer.drain_outbound().iter().any(|l| matches!(
            parse_ipc(l),
            Ok(IpcMsg::InterruptWaslSession { session_id }) if session_id == id
        )),
        "interrupt on a WaslManaged session must emit InterruptWaslSession"
    );

    core.stop_session(&id).expect("stop");
    assert!(
        peer.drain_outbound().iter().any(|l| matches!(
            parse_ipc(l),
            Ok(IpcMsg::StopWaslSession { session_id, .. }) if session_id == id
        )),
        "stop_session on a WaslManaged session must emit StopWaslSession"
    );
}

/// REGRESSION PIN: a MarsadManaged session's routing is byte-identical to
/// before this feature — send_prompt/attach/interrupt/stop emit EXACTLY today's
/// variants, never a `*Wasl*` one.
#[test]
fn marsad_managed_routing_is_byte_identical_to_today() {
    let (mut core, mut peer) = make_core();
    let id = launch_marsad(&mut core, &mut peer);

    core.send_prompt(&id, "hi").expect("send");
    let send_out: Vec<IpcMsg> = peer
        .drain_outbound()
        .iter()
        .filter_map(|l| parse_ipc(l).ok())
        .collect();
    assert!(
        send_out
            .iter()
            .any(|m| matches!(m, IpcMsg::SendPrompt { session_id, .. } if session_id == &id)),
        "a MarsadManaged send_prompt must still emit SendPrompt, got {send_out:?}"
    );
    assert!(
        !send_out
            .iter()
            .any(|m| matches!(m, IpcMsg::SendWaslPrompt { .. })),
        "a MarsadManaged session must NEVER emit SendWaslPrompt"
    );

    core.attach_session(&id).expect("attach");
    let attach_out: Vec<IpcMsg> = peer
        .drain_outbound()
        .iter()
        .filter_map(|l| parse_ipc(l).ok())
        .collect();
    assert!(attach_out
        .iter()
        .any(|m| matches!(m, IpcMsg::AttachSession { .. })));
    assert!(!attach_out
        .iter()
        .any(|m| matches!(m, IpcMsg::AttachWaslSession { .. })));

    core.interrupt(&id).expect("interrupt");
    let int_out: Vec<IpcMsg> = peer
        .drain_outbound()
        .iter()
        .filter_map(|l| parse_ipc(l).ok())
        .collect();
    assert!(int_out
        .iter()
        .any(|m| matches!(m, IpcMsg::Interrupt { .. })));
    assert!(!int_out
        .iter()
        .any(|m| matches!(m, IpcMsg::InterruptWaslSession { .. })));

    core.stop_session(&id).expect("stop");
    let stop_out: Vec<IpcMsg> = peer
        .drain_outbound()
        .iter()
        .filter_map(|l| parse_ipc(l).ok())
        .collect();
    assert!(stop_out
        .iter()
        .any(|m| matches!(m, IpcMsg::StopSession { .. })));
    assert!(!stop_out
        .iter()
        .any(|m| matches!(m, IpcMsg::StopWaslSession { .. })));
}

// ===========================================================================
// Part 2 — status projection into SessionView (§6.3, §10.4)
// ===========================================================================

/// Every `WaslTargetStatus` row projects to EXACTLY the §6.3 `Phase` — and the
/// core must USE `marsad_wasl_session::project_status_to_phase`, not
/// reimplement it: the test asserts the projected `SessionView.state` equals the
/// real function's output for the same status.
#[test]
fn wasl_status_projects_into_session_view_phase_per_table() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    let cases = vec![
        WaslTargetStatus::Offline,
        WaslTargetStatus::Starting,
        WaslTargetStatus::Idle {
            since: "t".to_string(),
        },
        WaslTargetStatus::Busy {
            active_turn: "t1".to_string(),
            source: "LocalHuman".to_string(),
            phase: "generating".to_string(),
            since: "t".to_string(),
            queued: 0,
        },
        WaslTargetStatus::AwaitingGate {
            active_turn: "t1".to_string(),
            detail: WaslNativeGateDetail {
                summary: "s".to_string(),
                preview: "p".to_string(),
            },
            since: "t".to_string(),
            queued: 0,
        },
        WaslTargetStatus::Interrupting {
            active_turn: "t1".to_string(),
            since: "t".to_string(),
        },
        WaslTargetStatus::Draining {
            active_turn: "t1".to_string(),
            since: "t".to_string(),
        },
        WaslTargetStatus::Faulted {
            error: "e".to_string(),
            recoverable: true,
        },
        WaslTargetStatus::Faulted {
            error: "e".to_string(),
            recoverable: false,
        },
        WaslTargetStatus::Stopped,
        WaslTargetStatus::Unknown("future-state".to_string()),
    ];

    for st in cases {
        let updates = core
            .on_agent_line(&status_line(&id, &st))
            .unwrap_or_else(|e| panic!("status {st:?} must apply, got {e:?}"));
        let sv = last_session_view(&updates, &id);
        assert_eq!(
            sv.state,
            project_status_to_phase(&st),
            "SessionView.state for {st:?} must equal project_status_to_phase(...) — \
             the core must USE the real projection, not reimplement it"
        );
    }
}

/// No new `Phase` variant was introduced: every projected phase is one of the
/// seven that existed before this feature.
#[test]
fn wasl_status_never_introduces_a_new_phase_variant() {
    let allowed = [
        Phase::Starting,
        Phase::Working,
        Phase::Needs,
        Phase::Error,
        Phase::Done,
        Phase::Stale,
        Phase::Detached,
    ];
    for st in [
        WaslTargetStatus::Offline,
        WaslTargetStatus::Starting,
        WaslTargetStatus::Idle {
            since: String::new(),
        },
        WaslTargetStatus::Busy {
            active_turn: String::new(),
            source: String::new(),
            phase: String::new(),
            since: String::new(),
            queued: 0,
        },
        WaslTargetStatus::Unknown("x".to_string()),
    ] {
        assert!(
            allowed.contains(&project_status_to_phase(&st)),
            "projection of {st:?} must be one of the seven existing Phase variants"
        );
    }
}

// ===========================================================================
// Part 3 — native gate → EXISTING Gate::Permission pipeline (§8, §10.4)
// ===========================================================================

/// A `NativeGateRequested` event upserts exactly one `Gate::Permission`
/// (session-scoped) into `SessionView.gate`, with `Phase::Needs`. The gate is a
/// `Permission` (NOT `Handoff`), and its `command` encodes the per-occurrence
/// `(turn_id, seq)` gate key via `native_gate_command(gate_key, detail)` so the
/// derived `DecisionId` is unique per occurrence (§8, v3).
#[test]
fn native_gate_event_upserts_one_permission_gate_with_needs_phase() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    let detail = WaslNativeGateDetail {
        summary: "run the test suite".to_string(),
        preview: "$ cargo test".to_string(),
    };
    let seq = 1;
    let turn_id = "turn-abc";
    let updates = core
        .on_agent_line(&event_line(
            &id,
            &ev(
                seq,
                WaslSessionEventKind::NativeGateRequested {
                    turn_id: turn_id.to_string(),
                    detail: detail.clone(),
                },
            ),
        ))
        .expect("native gate event must apply");

    // v3: the per-occurrence gate identity is derived from (turn_id, seq).
    let gate_key = native_gate_key(turn_id, seq);
    let sv = last_session_view(&updates, &id);
    assert_eq!(
        sv.state,
        Phase::Needs,
        "a native gate must light up Phase::Needs"
    );
    let gate = sv
        .gate
        .expect("a native gate must populate SessionView.gate");
    match gate {
        Gate::Permission {
            command,
            explanation,
            is_elicitation,
            ..
        } => {
            assert_eq!(
                command,
                native_gate_command(&gate_key, &detail),
                "the Permission command must be native_gate_command(gate_key, detail) — \
                 gate_key = native_gate_key(turn_id, seq) — so the derived DecisionId is \
                 unique+stable per gate occurrence"
            );
            assert_eq!(
                explanation, detail.preview,
                "explanation carries the CLI preview"
            );
            assert!(
                !is_elicitation,
                "a native permission prompt is a binary approve/deny"
            );
        }
        other => {
            panic!("a native gate must map to Gate::Permission (NOT Gate::Handoff), got {other:?}")
        }
    }
}

/// Answering the native gate emits EXACTLY one `AnswerWaslNativeGate` line, and
/// is decision-ledger idempotent: a second answer (even a different verdict) is
/// a silent no-op that sends nothing.
#[test]
fn answer_native_gate_emits_one_line_and_is_ledger_idempotent() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);
    let gate_key = native_gate_key("turn-1", 1);
    core.on_agent_line(&event_line(
        &id,
        &ev(
            1,
            WaslSessionEventKind::NativeGateRequested {
                turn_id: "turn-1".to_string(),
                detail: WaslNativeGateDetail {
                    summary: "s".to_string(),
                    preview: "p".to_string(),
                },
            },
        ),
    ))
    .expect("seed gate");
    let _ = peer.drain_outbound();

    core.answer_wasl_native_gate(&id, &gate_key, Decision::Approve)
        .expect("first answer");
    let first = peer.drain_outbound();
    let ans: Vec<IpcMsg> = first.iter().filter_map(|l| parse_ipc(l).ok()).collect();
    let native_answers: Vec<&IpcMsg> = ans
        .iter()
        .filter(|m| matches!(m, IpcMsg::AnswerWaslNativeGate { .. }))
        .collect();
    assert_eq!(
        native_answers.len(),
        1,
        "exactly one AnswerWaslNativeGate line"
    );
    assert_eq!(
        native_answers[0],
        &IpcMsg::AnswerWaslNativeGate {
            session_id: id.clone(),
            gate_id: gate_key.clone(),
            outcome: "approved".to_string(),
            reason: None,
        }
    );

    // Second answer (even a different verdict) → ledger dedup → no IPC.
    core.answer_wasl_native_gate(
        &id,
        &gate_key,
        Decision::Reject {
            reason: "changed my mind".to_string(),
        },
    )
    .expect("idempotent second answer must still be Ok");
    let second = peer.drain_outbound();
    assert!(
        !second
            .iter()
            .any(|l| matches!(parse_ipc(l), Ok(IpcMsg::AnswerWaslNativeGate { .. }))),
        "a duplicate native-gate answer must be a silent no-op (ledger dedup), got {second:?}"
    );
}

/// A denied native gate carries the reason and outcome "denied".
#[test]
fn answer_native_gate_deny_carries_reason() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);
    let gate_key = native_gate_key("turn-d", 1);
    core.on_agent_line(&event_line(
        &id,
        &ev(
            1,
            WaslSessionEventKind::NativeGateRequested {
                turn_id: "turn-d".to_string(),
                detail: WaslNativeGateDetail {
                    summary: "s".to_string(),
                    preview: "p".to_string(),
                },
            },
        ),
    ))
    .expect("seed gate");
    let _ = peer.drain_outbound();

    core.answer_wasl_native_gate(
        &id,
        &gate_key,
        Decision::Reject {
            reason: "no".to_string(),
        },
    )
    .expect("deny");
    let out = peer.drain_outbound();
    assert!(out.iter().any(|l| matches!(
        parse_ipc(l),
        Ok(IpcMsg::AnswerWaslNativeGate { outcome, reason, .. })
            if outcome == "denied" && reason.as_deref() == Some("no")
    )));
}

/// REGRESSION PIN: a native gate is SESSION-scoped and must NOT leak into the
/// host-scoped X1 operator-gate list — no `VmUpdate::WaslGates` is ever emitted
/// by the native-gate path.
#[test]
fn native_gate_never_touches_the_host_scoped_wasl_gates_list() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    let gate_key = native_gate_key("turn-1", 1);
    let requested = core
        .on_agent_line(&event_line(
            &id,
            &ev(
                1,
                WaslSessionEventKind::NativeGateRequested {
                    turn_id: "turn-1".to_string(),
                    detail: WaslNativeGateDetail {
                        summary: "s".to_string(),
                        preview: "p".to_string(),
                    },
                },
            ),
        ))
        .expect("gate event");
    assert!(
        !requested
            .iter()
            .any(|u| matches!(u, VmUpdate::WaslGates(_))),
        "a native (session) gate must NOT emit the host-scoped VmUpdate::WaslGates"
    );

    let _ = peer.drain_outbound();
    let answered = core
        .answer_wasl_native_gate(&id, &gate_key, Decision::Approve)
        .expect("answer");
    assert!(
        !answered.iter().any(|u| matches!(u, VmUpdate::WaslGates(_))),
        "answering a native gate must NOT touch the host-scoped operator-gate list"
    );
}

/// Answering a native gate never requested is an error, not a silent success
/// (the session-scoped analogue of `UnknownWaslGate`).
#[test]
fn answer_unknown_native_gate_errors() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);
    let err = core
        .answer_wasl_native_gate(&id, "never-requested", Decision::Approve)
        .expect_err("answering an unseen native gate must error");
    // Accept either the X1-style UnknownWaslGate or a Protocol error — the
    // contract is "not a silent success", the exact variant is the code-agent's.
    assert!(
        matches!(err, CoreError::UnknownWaslGate(_) | CoreError::Protocol(_)),
        "expected an unknown-gate class error, got {err:?}"
    );
}

// ===========================================================================
// Part 4 — persisted WaslSessionBinding + reconnect (§6.1, §10.4)
//
// STORE-SCHEMA GAP: persisting the binding needs a marsad-store field addition
// (see the file header). This test asserts the BLACK-BOX reconnect behavior:
// a fresh Core sharing the same store restores the WaslManaged session and
// reconnects it (routing still emits *Wasl*; a reconnect attach is issued)
// WITHOUT creating a duplicate or losing the WaslManaged binding.
// ===========================================================================

/// A launched WaslManaged session survives a simulated "core restart"
/// (construct a fresh `Core` from the SAME persistence backend + `load_sessions`)
/// with its runtime_owner intact: routing on the reloaded session still emits
/// `SendWaslPrompt`, proving the `WaslManaged` binding survived the reload.
#[test]
fn wasl_managed_binding_survives_a_core_restart() {
    // Shared persistence backend across the two Core instances (the "same
    // store" a real restart would reopen).
    let backend = marsad_store::MemoryBackend::new();
    let (t1a, mut t1b) = LoopbackFake::pair();
    let mut core1 = Core::with_deps(
        cfg(),
        Box::new(t1a),
        Box::new(NoGit),
        Box::new(NoTranscript),
        Box::new(backend.clone()),
    )
    .expect("core1");
    let id = core1
        .launch_wasl_session("local", serde_json::json!({"ctx": "survivor"}), None)
        .expect("launch WaslManaged");
    let _ = t1b.drain_outbound();

    // Simulate a restart: a brand-new Core over the SAME backend.
    let (t2a, mut t2b) = LoopbackFake::pair();
    let mut core2 = Core::with_deps(
        cfg(),
        Box::new(t2a),
        Box::new(NoGit),
        Box::new(NoTranscript),
        Box::new(backend.clone()),
    )
    .expect("core2");
    core2.load_sessions().expect("restore persisted sessions");
    let _ = t2b.drain_outbound();

    // The reloaded session must still route as WaslManaged.
    core2
        .send_prompt(&id, "after restart")
        .expect("send after restart");
    let out: Vec<IpcMsg> = t2b
        .drain_outbound()
        .iter()
        .filter_map(|l| parse_ipc(l).ok())
        .collect();
    assert!(
        out.iter()
            .any(|m| matches!(m, IpcMsg::SendWaslPrompt { session_id, .. } if session_id == &id)),
        "after a restart, the restored session must STILL route as WaslManaged \
         (SendWaslPrompt) — the persisted WaslSessionBinding must survive the reload, got {out:?}"
    );
    assert!(
        !out.iter().any(|m| matches!(m, IpcMsg::SendPrompt { .. })),
        "the restored session must NOT have degraded to MarsadManaged"
    );
}

/// Cold restore ⇒ FULL replay from seq 0 (§6.1). A WaslManaged session that
/// folded events up to seq N is re-created by a fresh Core (`load_sessions`) with
/// an EMPTY transcript (the transcript is not persisted for WaslManaged sessions)
/// and an empty `wasl_seen_seqs`. Re-attaching from the persisted `last_seq` (N)
/// would replay NOTHING and leave the transcript empty forever, so the cold
/// restore path must re-pump from `after_seq == 0`; the empty `wasl_seen_seqs`
/// folds each replayed event exactly once. (A warm in-process reconnect keeps its
/// in-memory transcript and does NOT go through `load_sessions`.)
#[test]
fn cold_restore_reattaches_from_seq_zero_full_replay() {
    let backend = marsad_store::MemoryBackend::new();
    let (t1a, mut t1b) = LoopbackFake::pair();
    let mut core1 = Core::with_deps(
        cfg(),
        Box::new(t1a),
        Box::new(NoGit),
        Box::new(NoTranscript),
        Box::new(backend.clone()),
    )
    .expect("core1");
    let id = core1
        .launch_wasl_session("local", serde_json::json!({"ctx": "seqd"}), None)
        .expect("launch");
    // Fold a few events so "last observed seq" advances to 3.
    for seq in 1..=3 {
        core1
            .on_agent_line(&event_line(
                &id,
                &ev(
                    seq,
                    WaslSessionEventKind::AssistantDelta {
                        turn_id: "turn-seqd".to_string(),
                        text: format!("c{seq}"),
                    },
                ),
            ))
            .expect("fold event");
    }
    let _ = t1b.drain_outbound();

    // Simulate a PRE-FIX binding: one written by an older Marsad that did not yet
    // persist the transcript. Clear `transcript_json` on the stored binding so the
    // restore takes the fallback (no stored transcript → full node replay from seq
    // 0). The current-code binding does persist the transcript (covered by
    // `cold_restore_with_stored_transcript_reattaches_from_last_seq`).
    {
        let mut store = marsad_store::Store::new(Box::new(backend.clone()));
        let mut b = store
            .wasl_bindings()
            .expect("wasl_bindings")
            .remove(&id)
            .expect("binding must be persisted for the WaslManaged session");
        b.transcript_json = String::new();
        store.upsert_wasl_binding(&b).expect("rewrite pre-fix binding");
    }

    let (t2a, mut t2b) = LoopbackFake::pair();
    let mut core2 = Core::with_deps(
        cfg(),
        Box::new(t2a),
        Box::new(NoGit),
        Box::new(NoTranscript),
        Box::new(backend.clone()),
    )
    .expect("core2");
    core2.load_sessions().expect("restore");
    let _ = t2b.drain_outbound();

    core2.attach_session(&id).expect("reattach");
    let out: Vec<IpcMsg> = t2b
        .drain_outbound()
        .iter()
        .filter_map(|l| parse_ipc(l).ok())
        .collect();
    let attach = out
        .iter()
        .find_map(|m| match m {
            IpcMsg::AttachWaslSession {
                session_id,
                after_seq,
                ..
            } if session_id == &id => Some(*after_seq),
            _ => None,
        })
        .expect("reconnect must emit AttachWaslSession for the WaslManaged session");
    assert_eq!(
        attach, 0,
        "a cold restore must re-attach from seq 0 (full replay to rebuild the \
         unpersisted transcript), NOT from the persisted last_seq; got {attach}"
    );
}

/// POST-FIX cold restore: a WaslManaged binding persisted WITH its transcript
/// (the user's prompt + folded response) restores that transcript verbatim and
/// reattaches from the persisted `last_seq` (NOT 0). This is the fix for the
/// "prompts vanish after relaunch" bug: the node's observe replay carries no
/// prompt text, so the prompt only survives because the transcript is persisted;
/// and because it IS restored, reattach must pull only NEW events (> last_seq),
/// never re-replay (which would drop the prompt-less TurnAccepted back over it).
#[test]
fn cold_restore_with_stored_transcript_reattaches_from_last_seq() {
    let backend = marsad_store::MemoryBackend::new();
    let (t1a, mut t1b) = LoopbackFake::pair();
    let mut core1 = Core::with_deps(
        cfg(),
        Box::new(t1a),
        Box::new(NoGit),
        Box::new(NoTranscript),
        Box::new(backend.clone()),
    )
    .expect("core1");
    let id = core1
        .launch_wasl_session("local", serde_json::json!({"ctx": "keepprompt"}), None)
        .expect("launch WaslManaged");

    // The user sends a prompt (recorded live + persisted immediately by the fix),
    // then the node streams a turn — advancing the persisted last_seq past 0.
    core1.send_prompt(&id, "my prompt").expect("send prompt");
    core1
        .on_agent_line(&event_line(
            &id,
            &ev(
                1,
                WaslSessionEventKind::TurnAccepted {
                    turn_id: "turn-keep".to_string(),
                    source: "LocalHuman".to_string(),
                },
            ),
        ))
        .expect("accepted");
    core1
        .on_agent_line(&event_line(
            &id,
            &ev(
                2,
                WaslSessionEventKind::AssistantDelta {
                    turn_id: "turn-keep".to_string(),
                    text: "the reply".to_string(),
                },
            ),
        ))
        .expect("delta");
    let _ = t1b.drain_outbound();

    // Sanity: the binding was persisted WITH a non-empty transcript and a
    // non-zero last_seq (this is the post-fix path, distinct from the pre-fix
    // tests above which clear transcript_json).
    let persisted_last_seq = {
        let store = marsad_store::Store::new(Box::new(backend.clone()));
        let b = store
            .wasl_bindings()
            .expect("wasl_bindings")
            .remove(&id)
            .expect("binding must be persisted");
        assert!(
            !b.transcript_json.is_empty(),
            "the current code must persist the folded transcript on the binding"
        );
        assert!(b.last_seq > 0, "last_seq must have advanced past 0");
        b.last_seq
    };

    // Cold restore into a fresh Core over the same backend.
    let (t2a, mut t2b) = LoopbackFake::pair();
    let mut core2 = Core::with_deps(
        cfg(),
        Box::new(t2a),
        Box::new(NoGit),
        Box::new(NoTranscript),
        Box::new(backend.clone()),
    )
    .expect("core2");
    let updates = core2.load_sessions().expect("restore");
    let _ = t2b.drain_outbound();

    // (a) The restored session's transcript still contains the user's prompt.
    let sv = last_session_view(&updates, &id);
    assert!(
        sv.transcript.iter().any(|e| matches!(
            e,
            marsad_events::TranscriptEntry::User { text, .. } if text == "my prompt"
        )),
        "the restored transcript must still contain the user's prompt \
         (the whole point of persisting it); got {:?}",
        sv.transcript
    );

    // (b) Reattach pulls only NEW events (> last_seq), not a full replay.
    core2.attach_session(&id).expect("reattach");
    let out: Vec<IpcMsg> = t2b
        .drain_outbound()
        .iter()
        .filter_map(|l| parse_ipc(l).ok())
        .collect();
    let attach = out
        .iter()
        .find_map(|m| match m {
            IpcMsg::AttachWaslSession {
                session_id,
                after_seq,
                ..
            } if session_id == &id => Some(*after_seq),
            _ => None,
        })
        .expect("reconnect must emit AttachWaslSession for the WaslManaged session");
    assert_eq!(
        attach, persisted_last_seq,
        "a restore WITH a stored transcript must reattach from the persisted \
         last_seq ({persisted_last_seq}), NOT from 0; got {attach}"
    );
}

/// `load_sessions` restores a persisted WaslManaged binding whose stored
/// `last_seq` was NON-ZERO, but resets the restored session's `wasl_last_seq` to
/// 0 so a cold restore does a FULL replay. Proven black-box: after restore, an
/// `attach_session` emits `AttachWaslSession { after_seq: 0 }` — which ALSO
/// proves the restored `runtime_owner` is `WaslManaged` (only that arm emits
/// `AttachWaslSession`; a MarsadManaged restore would emit `AttachSession`).
#[test]
fn load_sessions_restores_wasl_binding_with_seq_zero() {
    let backend = marsad_store::MemoryBackend::new();
    let (t1a, mut t1b) = LoopbackFake::pair();
    let mut core1 = Core::with_deps(
        cfg(),
        Box::new(t1a),
        Box::new(NoGit),
        Box::new(NoTranscript),
        Box::new(backend.clone()),
    )
    .expect("core1");
    let id = core1
        .launch_wasl_session("local", serde_json::json!({"ctx": "seqreset"}), None)
        .expect("launch WaslManaged");
    // Advance the persisted last_seq to a non-zero value (5) by folding events.
    for seq in 1..=5 {
        core1
            .on_agent_line(&event_line(
                &id,
                &ev(
                    seq,
                    WaslSessionEventKind::AssistantDelta {
                        turn_id: "turn-seqreset".to_string(),
                        text: format!("d{seq}"),
                    },
                ),
            ))
            .expect("fold event");
    }
    let _ = t1b.drain_outbound();

    // Simulate a PRE-FIX binding (no persisted transcript) so this test keeps
    // exercising the seq-0 full-replay fallback. Clear `transcript_json`; a
    // current-code binding persists the transcript and reattaches from last_seq
    // (covered by `cold_restore_with_stored_transcript_reattaches_from_last_seq`).
    {
        let mut store = marsad_store::Store::new(Box::new(backend.clone()));
        let mut b = store
            .wasl_bindings()
            .expect("wasl_bindings")
            .remove(&id)
            .expect("binding must be persisted for the WaslManaged session");
        b.transcript_json = String::new();
        store.upsert_wasl_binding(&b).expect("rewrite pre-fix binding");
    }

    // Fresh Core over the SAME backend → a cold restore.
    let (t2a, mut t2b) = LoopbackFake::pair();
    let mut core2 = Core::with_deps(
        cfg(),
        Box::new(t2a),
        Box::new(NoGit),
        Box::new(NoTranscript),
        Box::new(backend.clone()),
    )
    .expect("core2");
    core2.load_sessions().expect("restore");
    let _ = t2b.drain_outbound();

    core2.attach_session(&id).expect("attach after cold restore");
    let out: Vec<IpcMsg> = t2b
        .drain_outbound()
        .iter()
        .filter_map(|l| parse_ipc(l).ok())
        .collect();
    let after_seq = out
        .iter()
        .find_map(|m| match m {
            IpcMsg::AttachWaslSession {
                session_id,
                after_seq,
                ..
            } if session_id == &id => Some(*after_seq),
            _ => None,
        })
        .expect("restored WaslManaged session must attach via AttachWaslSession");
    assert_eq!(
        after_seq, 0,
        "a restored WaslManaged session must have wasl_last_seq reset to 0 even \
         though the persisted binding's last_seq was 5; got {after_seq}"
    );
    assert!(
        !out.iter().any(|m| matches!(m, IpcMsg::AttachSession { .. })),
        "the restored session must NOT degrade to MarsadManaged (AttachSession)"
    );
}

/// `reattach_wasl_on_relaunch` emits `AttachWaslSession { after_seq: 0 }` for a
/// restored WaslManaged session (re-pumping the node's full history) and emits
/// NOTHING for a MarsadManaged session (its transcript is re-hydrated by the
/// agent's replay-on-subscribe backfill, not by us).
#[test]
fn reattach_wasl_on_relaunch_attaches_only_wasl_sessions() {
    let backend = marsad_store::MemoryBackend::new();
    let (t1a, mut t1b) = LoopbackFake::pair();
    let mut core1 = Core::with_deps(
        cfg(),
        Box::new(t1a),
        Box::new(NoGit),
        Box::new(NoTranscript),
        Box::new(backend.clone()),
    )
    .expect("core1");
    let wasl_id = launch_wasl(&mut core1, &mut t1b);
    let marsad_id = launch_marsad(&mut core1, &mut t1b);
    // Advance the Wasl session's persisted last_seq so we can prove the reset.
    core1
        .on_agent_line(&event_line(
            &wasl_id,
            &ev(
                7,
                WaslSessionEventKind::AssistantDelta {
                    turn_id: "turn-relaunch".to_string(),
                    text: "x".to_string(),
                },
            ),
        ))
        .expect("fold event");
    let _ = t1b.drain_outbound();

    // Cold restore into a fresh Core, then trigger the relaunch re-attach.
    let (t2a, mut t2b) = LoopbackFake::pair();
    let mut core2 = Core::with_deps(
        cfg(),
        Box::new(t2a),
        Box::new(NoGit),
        Box::new(NoTranscript),
        Box::new(backend.clone()),
    )
    .expect("core2");
    core2.load_sessions().expect("restore");
    let _ = t2b.drain_outbound();

    core2
        .reattach_wasl_on_relaunch()
        .expect("reattach_wasl_on_relaunch");
    let out: Vec<IpcMsg> = t2b
        .drain_outbound()
        .iter()
        .filter_map(|l| parse_ipc(l).ok())
        .collect();

    // Exactly one AttachWaslSession, for the WaslManaged session, at seq 0.
    let wasl_attaches: Vec<u64> = out
        .iter()
        .filter_map(|m| match m {
            IpcMsg::AttachWaslSession {
                session_id,
                after_seq,
                ..
            } if session_id == &wasl_id => Some(*after_seq),
            _ => None,
        })
        .collect();
    assert_eq!(
        wasl_attaches,
        vec![0],
        "reattach_wasl_on_relaunch must emit exactly one AttachWaslSession(after_seq=0) \
         for the WaslManaged session, got {out:?}"
    );
    // NOTHING for the MarsadManaged session: neither AttachWaslSession nor the
    // MarsadManaged AttachSession is emitted for it.
    assert!(
        !out.iter().any(|m| matches!(
            m,
            IpcMsg::AttachWaslSession { session_id, .. } if session_id == &marsad_id
        )),
        "reattach_wasl_on_relaunch must NOT attach the MarsadManaged session, got {out:?}"
    );
    assert!(
        !out.iter().any(|m| matches!(m, IpcMsg::AttachSession { .. })),
        "reattach_wasl_on_relaunch must emit NO AttachSession (MarsadManaged is \
         untouched), got {out:?}"
    );
}

/// New test: `AttachWaslSession` emitted on `attach_session` carries the
/// binding's `provider_key` as `wasl_target_key` and `host_id` from the
/// binding — so the agent can re-adopt the node + re-attach the observe pump
/// after a cold restart without any session→key map in the agent.
#[test]
fn attach_wasl_session_carries_provider_key_and_host_id() {
    let (mut core, mut peer) = make_core();
    // Launch with a specific host + non-trivial key so we can assert both fields.
    let target_key = serde_json::json!({
        "protocol": "claude",
        "context": {"cwd": "/home/u/proj"},
    });
    let id = core
        .launch_wasl_session("host-42", target_key.clone(), None)
        .expect("launch_wasl_session must succeed");
    let _ = peer.drain_outbound();

    core.attach_session(&id).expect("attach");
    let out: Vec<IpcMsg> = peer
        .drain_outbound()
        .iter()
        .filter_map(|l| parse_ipc(l).ok())
        .collect();

    let found = out.iter().find_map(|m| match m {
        IpcMsg::AttachWaslSession {
            session_id,
            wasl_target_key,
            host_id,
            ..
        } if session_id == &id => Some((wasl_target_key.clone(), host_id.clone())),
        _ => None,
    });
    let (wk, hid) = found
        .expect("attach_session on WaslManaged must emit AttachWaslSession with key+host");
    assert_eq!(
        wk, target_key,
        "AttachWaslSession must carry wasl_target_key == the binding's provider_key"
    );
    assert_eq!(
        hid, "host-42",
        "AttachWaslSession must carry host_id == the binding's host_id"
    );
}

// ===========================================================================
// Part 5 — prompt-busy surfaces (§10.4)
// ===========================================================================

/// A `RejectedBusy` receipt line surfaces as a VISIBLE signal (a toast or a
/// session-view surface), not a silent drop. We assert at least one VmUpdate is
/// produced that carries the busy/rejection to the UI.
#[test]
fn rejected_busy_receipt_surfaces_visibly() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    let updates = core
        .on_agent_line(&receipt_line(
            &id,
            &WaslTurnReceipt::RejectedBusy {
                active_turn: "t-active".to_string(),
            },
        ))
        .expect("a busy receipt must apply");
    assert!(
        !updates.is_empty(),
        "a RejectedBusy receipt must surface a visible signal, not be silently dropped"
    );
    // Accept either a Toast or a Session update carrying the busy state — the
    // exact surface is the code-agent's; the contract is "visible".
    assert!(
        updates
            .iter()
            .any(|u| matches!(u, VmUpdate::Toast { .. } | VmUpdate::Session(_))),
        "the busy rejection must be surfaced via a Toast or a Session update, got {updates:?}"
    );
}

/// A `Queued` receipt likewise surfaces a visible queued indicator.
#[test]
fn queued_receipt_surfaces_visibly() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    let updates = core
        .on_agent_line(&receipt_line(
            &id,
            &WaslTurnReceipt::Queued {
                turn_id: "t-q".to_string(),
                position: 2,
            },
        ))
        .expect("a queued receipt must apply");
    assert!(
        !updates.is_empty(),
        "a Queued receipt must surface a visible queued indicator, not be silently dropped"
    );
}

// ===========================================================================
// Part 6 — transcript folding edge cases (§10.4 — CORE's responsibility)
// ===========================================================================

/// A prompt-and-turn folds: the User prompt appears, then the assistant reply.
#[test]
fn assistant_events_fold_into_transcript() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    core.send_prompt(&id, "do the thing").expect("prompt");
    let _ = peer.drain_outbound();

    core.on_agent_line(&event_line(
        &id,
        &ev(
            1,
            WaslSessionEventKind::TurnAccepted {
                turn_id: "t1".to_string(),
                source: "LocalHuman".to_string(),
            },
        ),
    ))
    .expect("accepted");
    let d1 = core
        .on_agent_line(&event_line(
            &id,
            &ev(
                2,
                WaslSessionEventKind::AssistantDelta {
                    turn_id: "t1".to_string(),
                    text: "part one ".to_string(),
                },
            ),
        ))
        .expect("delta");
    let sv = last_session_view(&d1, &id);
    assert!(
        sv.transcript.iter().any(|e| matches!(
            e,
            marsad_events::TranscriptEntry::User { text, .. } if text == "do the thing"
        )),
        "the user's prompt must appear in the transcript as a User entry"
    );
    assert!(
        sv.transcript.iter().any(|e| matches!(
            e,
            marsad_events::TranscriptEntry::Assistant { text, .. } if text.contains("part one")
        )),
        "an assistant delta must fold into an Assistant transcript entry"
    );
}

/// DEDUP: the same `WaslSessionEvent { seq }` delivered twice folds ONCE — the
/// transcript is identical whether the event arrives once or twice (replay
/// overlap after an ObserveLost/reattach must not double-fold).
#[test]
fn duplicate_event_by_seq_is_not_double_folded() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    let delta = ev(
        5,
        WaslSessionEventKind::AssistantDelta {
            turn_id: "t1".to_string(),
            text: "once".to_string(),
        },
    );
    core.on_agent_line(&event_line(&id, &delta)).expect("first");
    let after_first = last_session_view(
        &core
            .on_agent_line(&event_line(
                &id,
                &ev(
                    6,
                    WaslSessionEventKind::AssistantDelta {
                        turn_id: "t1".to_string(),
                        text: "".to_string(),
                    },
                ),
            ))
            .unwrap_or_default(),
        &id,
    );
    let count_first = assistant_text_occurrences(&after_first.transcript, "once");

    // Re-deliver seq 5 (a replay overlap). Must NOT append a second "once".
    let dup_updates = core.on_agent_line(&event_line(&id, &delta)).expect("dup");
    let after_dup = updates_session_view_or(&dup_updates, &id, after_first.clone());
    let count_dup = assistant_text_occurrences(&after_dup.transcript, "once");
    assert_eq!(
        count_first, count_dup,
        "a duplicate event (same seq) must fold exactly once — dedup by seq"
    );
}

/// APPEND, not re-create: a run of `AssistantDelta`s for one in-progress turn
/// appends to the SAME Assistant entry (concatenating text), not one entry per
/// delta.
#[test]
fn assistant_deltas_append_to_the_same_entry() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);
    core.on_agent_line(&event_line(
        &id,
        &ev(
            1,
            WaslSessionEventKind::TurnAccepted {
                turn_id: "t1".to_string(),
                source: "LocalHuman".to_string(),
            },
        ),
    ))
    .expect("accept");
    core.on_agent_line(&event_line(
        &id,
        &ev(
            2,
            WaslSessionEventKind::AssistantDelta {
                turn_id: "t1".to_string(),
                text: "Hello, ".to_string(),
            },
        ),
    ))
    .expect("d1");
    core.on_agent_line(&event_line(
        &id,
        &ev(
            3,
            WaslSessionEventKind::AssistantDelta {
                turn_id: "t1".to_string(),
                text: "world".to_string(),
            },
        ),
    ))
    .expect("d2");
    let updates = core
        .on_agent_line(&event_line(
            &id,
            &ev(
                4,
                WaslSessionEventKind::AssistantDelta {
                    turn_id: "t1".to_string(),
                    text: "!".to_string(),
                },
            ),
        ))
        .expect("d3");

    let sv = last_session_view(&updates, &id);
    let assistant_entries: Vec<&String> = sv
        .transcript
        .iter()
        .filter_map(|e| match e {
            marsad_events::TranscriptEntry::Assistant { text, .. } => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(
        assistant_entries.len(),
        1,
        "a run of deltas for one in-progress turn must be ONE Assistant entry, got {assistant_entries:?}"
    );
    assert_eq!(
        assistant_entries[0], "Hello, world!",
        "the deltas must be concatenated in order into the single entry"
    );
}

/// A single terminal event (`TurnCompleted`) closes the turn's entry exactly
/// once; the entry reaches its final form a single time (no double-close).
#[test]
fn one_terminal_event_closes_the_turn_once() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);
    core.on_agent_line(&event_line(
        &id,
        &ev(
            1,
            WaslSessionEventKind::TurnAccepted {
                turn_id: "t1".to_string(),
                source: "LocalHuman".to_string(),
            },
        ),
    ))
    .expect("accept");
    core.on_agent_line(&event_line(
        &id,
        &ev(
            2,
            WaslSessionEventKind::AssistantDelta {
                turn_id: "t1".to_string(),
                text: "final answer".to_string(),
            },
        ),
    ))
    .expect("delta");
    let done = core
        .on_agent_line(&event_line(
            &id,
            &ev(
                3,
                WaslSessionEventKind::TurnCompleted {
                    turn_id: "t1".to_string(),
                    reply: "final answer".to_string(),
                    certainty: "committed".to_string(),
                },
            ),
        ))
        .expect("complete");
    let sv = last_session_view(&done, &id);
    let assistant_count = sv
        .transcript
        .iter()
        .filter(|e| matches!(e, marsad_events::TranscriptEntry::Assistant { text, .. } if text.contains("final answer")))
        .count();
    assert_eq!(
        assistant_count, 1,
        "a completed turn must have exactly one Assistant entry (no double-close/reopen)"
    );
    assert_ne!(
        sv.state,
        Phase::Needs,
        "a completed turn is not awaiting input"
    );
}

/// DEFENSIVE: a delta arriving AFTER the terminal event for the same turn is
/// ignored/logged, not folded — the closed entry is not corrupted or reopened.
#[test]
fn post_terminal_delta_is_ignored_not_folded() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);
    core.on_agent_line(&event_line(
        &id,
        &ev(
            1,
            WaslSessionEventKind::TurnAccepted {
                turn_id: "t1".to_string(),
                source: "LocalHuman".to_string(),
            },
        ),
    ))
    .expect("accept");
    core.on_agent_line(&event_line(
        &id,
        &ev(
            2,
            WaslSessionEventKind::AssistantDelta {
                turn_id: "t1".to_string(),
                text: "the reply".to_string(),
            },
        ),
    ))
    .expect("delta");
    core.on_agent_line(&event_line(
        &id,
        &ev(
            3,
            WaslSessionEventKind::TurnCompleted {
                turn_id: "t1".to_string(),
                reply: "the reply".to_string(),
                certainty: "committed".to_string(),
            },
        ),
    ))
    .expect("complete");

    // A stray delta for the already-completed turn t1.
    let after = core
        .on_agent_line(&event_line(
            &id,
            &ev(
                4,
                WaslSessionEventKind::AssistantDelta {
                    turn_id: "t1".to_string(),
                    text: " LEAKED".to_string(),
                },
            ),
        ))
        .expect("post-terminal delta must not error");
    let sv = last_session_view_or_prev(&after, &id, &mut core);
    assert!(
        sv.transcript.iter().all(|e| !matches!(
            e,
            marsad_events::TranscriptEntry::Assistant { text, .. } if text.contains("LEAKED")
        )),
        "a delta after the terminal event must NOT be folded into the closed entry"
    );
}

/// QUEUED ≠ ACCEPTED: a prompt that got a `Queued` receipt is represented as
/// pending/queued — its content is NOT shown as already-delivered. Only on
/// `TurnAccepted` (or a `Started` receipt) does it become an in-flight turn.
#[test]
fn queued_prompt_is_distinct_from_an_accepted_one() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    core.send_prompt(&id, "queued work").expect("prompt");
    let _ = peer.drain_outbound();

    // The prompt is Queued (not accepted): the session must NOT be shown as
    // actively Working purely from a queued receipt.
    let queued_updates = core
        .on_agent_line(&receipt_line(
            &id,
            &WaslTurnReceipt::Queued {
                turn_id: "t-q".to_string(),
                position: 1,
            },
        ))
        .expect("queued receipt");
    let sv_q = last_session_view_or_prev(&queued_updates, &id, &mut core);
    assert_ne!(
        sv_q.state,
        Phase::Working,
        "a merely-queued prompt must NOT be represented as an in-flight (Working) turn"
    );

    // Now the turn is actually accepted → it becomes in-flight.
    let accepted = core
        .on_agent_line(&event_line(
            &id,
            &ev(
                1,
                WaslSessionEventKind::TurnAccepted {
                    turn_id: "t-q".to_string(),
                    source: "LocalHuman".to_string(),
                },
            ),
        ))
        .expect("accepted");
    let sv_a = last_session_view(&accepted, &id);
    assert_eq!(
        sv_a.state,
        Phase::Working,
        "only on TurnAccepted does the queued prompt become an in-flight turn"
    );
}

// ===========================================================================
// Part 7 — absent = inert, core side (§2, §10.4)
// ===========================================================================

/// A fresh Core with only a MarsadManaged session emits NO `*Wasl*` session
/// command from any existing flow (send/attach/interrupt/stop). The whole
/// existing surface stays byte-identical when no Wasl backend is involved.
#[test]
fn marsad_managed_only_core_emits_no_wasl_session_variant() {
    let (mut core, mut peer) = make_core();
    let id = launch_marsad(&mut core, &mut peer);

    core.send_prompt(&id, "x").expect("send");
    core.attach_session(&id).expect("attach");
    core.interrupt(&id).expect("interrupt");
    core.stop_session(&id).expect("stop");

    let out: Vec<IpcMsg> = peer
        .drain_outbound()
        .iter()
        .filter_map(|l| parse_ipc(l).ok())
        .collect();
    assert!(
        !out.iter().any(|m| matches!(
            m,
            IpcMsg::SendWaslPrompt { .. }
                | IpcMsg::AttachWaslSession { .. }
                | IpcMsg::InterruptWaslSession { .. }
                | IpcMsg::StopWaslSession { .. }
                | IpcMsg::LaunchWaslSession { .. }
        )),
        "a MarsadManaged-only flow must never emit any *Wasl* session command, got {out:?}"
    );
}

/// A `WaslSessionEvent` inbound for a session core never launched is an
/// UnknownSession (or tolerant no-op) — never a panic and never a silent
/// corruption of unrelated state.
#[test]
fn wasl_event_for_unknown_session_does_not_panic() {
    let (mut core, _peer) = make_core();
    let res = core.on_agent_line(&event_line(
        "never-launched",
        &ev(
            1,
            WaslSessionEventKind::AssistantDelta {
                turn_id: "t1".to_string(),
                text: "x".to_string(),
            },
        ),
    ));
    // Either a hard UnknownSession or a tolerant no-op is acceptable; the
    // contract is "no panic, no cross-session corruption".
    match res {
        Ok(updates) => assert!(
            updates.is_empty(),
            "an event for an unknown session must not produce updates, got {updates:?}"
        ),
        Err(e) => assert!(
            matches!(e, CoreError::UnknownSession(_) | CoreError::Protocol(_)),
            "expected UnknownSession/Protocol, got {e:?}"
        ),
    }
}

// ===========================================================================
// Part 8 — coverage-gap closers (audit): public-entry gate delegation,
// TurnFailed folding, WaslNativeGateAnswered ack, and the tolerant-skip
// defensive-parsing branches. These exercise reachable Wasl-feature paths the
// existing buckets left uncovered.
// ===========================================================================

/// GAP: the public `answer_gate` entry point (not the dedicated
/// `answer_wasl_native_gate`) must DELEGATE to the native-gate path for a
/// WaslManaged session — emitting `AnswerWaslNativeGate`, never `AnswerGate`
/// (§6.1/§8, lib.rs answer_gate WaslManaged arm). A native gate is seeded so the
/// session's pending `Gate::Permission` carries the gate_id answer_gate reads.
#[test]
fn answer_gate_delegates_to_native_gate_for_wasl_managed_session() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    // Seed a native gate so state.gate is Some(Permission { request_id: gate_key }).
    // v3: identity is the per-occurrence (turn_id, seq) key, not a detail.gate_id.
    let detail = WaslNativeGateDetail {
        summary: "s".to_string(),
        preview: "p".to_string(),
    };
    let seq = 1;
    let turn_id = "turn-delegate";
    let gate_key = native_gate_key(turn_id, seq);
    core.on_agent_line(&event_line(
        &id,
        &ev(
            seq,
            WaslSessionEventKind::NativeGateRequested {
                turn_id: turn_id.to_string(),
                detail: detail.clone(),
            },
        ),
    ))
    .expect("seed native gate");
    let _ = peer.drain_outbound();

    // Answer through the PUBLIC answer_gate entry point. The decision routes to
    // the session; answer_gate detects WaslManaged and delegates.
    let gate = Gate::Permission {
        request_id: gate_key.clone(),
        command: native_gate_command(&gate_key, &detail),
        cwd: String::new(),
        risk: String::new(),
        explanation: detail.preview.clone(),
        is_elicitation: false,
        elicitation_choices: Vec::new(),
        permission_suggestions: Vec::new(),
    };
    let updates = core
        .answer_gate(marsad_gates::approve(&id, &gate))
        .expect("answer_gate on a WaslManaged session must succeed");

    // Delegation → AnswerWaslNativeGate on the wire, never a plain AnswerGate.
    let out: Vec<IpcMsg> = peer
        .drain_outbound()
        .iter()
        .filter_map(|l| parse_ipc(l).ok())
        .collect();
    assert!(
        out.iter().any(|m| matches!(
            m,
            IpcMsg::AnswerWaslNativeGate { session_id, gate_id, outcome, .. }
                if session_id == &id && gate_id == &gate_key && outcome == "approved"
        )),
        "answer_gate on a WaslManaged session must delegate to the native-gate path \
         (emit AnswerWaslNativeGate), got {out:?}"
    );
    assert!(
        !out.iter().any(|m| matches!(m, IpcMsg::AnswerGate { .. })),
        "the delegated path must NEVER emit the MarsadManaged AnswerGate, got {out:?}"
    );
    // And the pending gate is cleared in the returned Session update.
    let sv = last_session_view(&updates, &id);
    assert!(
        sv.gate.is_none(),
        "answering via delegation must clear the pending gate"
    );
}

/// GAP: `answer_gate` on a WaslManaged session with NO pending native gate is a
/// benign silent no-op (the "nothing to answer" disposition) — no IPC, Ok(empty)
/// (lib.rs answer_gate WaslManaged `gate_id == None` arm).
#[test]
fn answer_gate_wasl_managed_with_no_pending_gate_is_silent_noop() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);
    let _ = peer.drain_outbound();

    // No native gate was ever requested → state.gate is None.
    let gate = Gate::Permission {
        request_id: "nonexistent".to_string(),
        command: "x".to_string(),
        cwd: String::new(),
        risk: String::new(),
        explanation: String::new(),
        is_elicitation: false,
        elicitation_choices: Vec::new(),
        permission_suggestions: Vec::new(),
    };
    let updates = core
        .answer_gate(marsad_gates::approve(&id, &gate))
        .expect("answer_gate with nothing pending must be Ok");
    assert!(
        updates.is_empty(),
        "answer_gate on a WaslManaged session with no pending gate must return no updates"
    );
    let out = peer.drain_outbound();
    assert!(
        !out.iter()
            .any(|l| matches!(parse_ipc(l), Ok(IpcMsg::AnswerWaslNativeGate { .. }))),
        "no native-gate answer must be emitted when there is nothing to answer, got {out:?}"
    );
}

/// GAP: a `TurnFailed` event folds into the transcript via the same
/// `close_wasl_turn` path as `TurnCompleted`, and drives `Phase::Error` (the
/// existing buckets only cover `TurnCompleted`). lib.rs K::TurnFailed arm.
#[test]
fn turn_failed_folds_into_transcript_and_sets_error_phase() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);
    core.on_agent_line(&event_line(
        &id,
        &ev(
            1,
            WaslSessionEventKind::TurnAccepted {
                turn_id: "t1".to_string(),
                source: "LocalHuman".to_string(),
            },
        ),
    ))
    .expect("accept");

    // No prior delta opened an entry → close_wasl_turn appends the error text as
    // the (single) assistant entry for this failed turn.
    let updates = core
        .on_agent_line(&event_line(
            &id,
            &ev(
                2,
                WaslSessionEventKind::TurnFailed {
                    turn_id: "t1".to_string(),
                    error: "boom: the turn failed".to_string(),
                    certainty: "unknown".to_string(),
                },
            ),
        ))
        .expect("failed turn must apply");

    let sv = last_session_view(&updates, &id);
    assert_eq!(
        sv.state,
        Phase::Error,
        "a TurnFailed event must drive Phase::Error"
    );
    let error_entries = sv
        .transcript
        .iter()
        .filter(|e| matches!(
            e,
            marsad_events::TranscriptEntry::Assistant { text, .. } if text.contains("boom: the turn failed")
        ))
        .count();
    assert_eq!(
        error_entries, 1,
        "a TurnFailed must fold its error text into exactly one Assistant entry (close_wasl_turn), got {:?}",
        sv.transcript
    );
}

/// GAP: a `TurnFailed` after deltas already opened an assistant entry closes
/// that SAME entry once (no double-close/reopen), mirroring the TurnCompleted
/// close semantics — and still sets Error phase.
#[test]
fn turn_failed_after_deltas_closes_the_open_entry_once() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);
    core.on_agent_line(&event_line(
        &id,
        &ev(
            1,
            WaslSessionEventKind::TurnAccepted {
                turn_id: "t1".to_string(),
                source: "LocalHuman".to_string(),
            },
        ),
    ))
    .expect("accept");
    core.on_agent_line(&event_line(
        &id,
        &ev(
            2,
            WaslSessionEventKind::AssistantDelta {
                turn_id: "t1".to_string(),
                text: "partial output".to_string(),
            },
        ),
    ))
    .expect("delta");
    let updates = core
        .on_agent_line(&event_line(
            &id,
            &ev(
                3,
                WaslSessionEventKind::TurnFailed {
                    turn_id: "t1".to_string(),
                    error: "then it failed".to_string(),
                    certainty: "unknown".to_string(),
                },
            ),
        ))
        .expect("fail");

    let sv = last_session_view(&updates, &id);
    assert_eq!(sv.state, Phase::Error);
    let assistant_entries = sv
        .transcript
        .iter()
        .filter(|e| matches!(e, marsad_events::TranscriptEntry::Assistant { .. }))
        .count();
    assert_eq!(
        assistant_entries, 1,
        "a delta-opened entry closed by TurnFailed must remain exactly one Assistant entry, got {:?}",
        sv.transcript
    );

    // Post-terminal guard: a stray delta after the failure is not folded.
    let after = core
        .on_agent_line(&event_line(
            &id,
            &ev(
                4,
                WaslSessionEventKind::AssistantDelta {
                    turn_id: "t1".to_string(),
                    text: " LEAKED".to_string(),
                },
            ),
        ))
        .expect("post-terminal delta must not error");
    let sv2 = last_session_view_or_prev(&after, &id, &mut core);
    assert!(
        sv2.transcript.iter().all(|e| !matches!(
            e,
            marsad_events::TranscriptEntry::Assistant { text, .. } if text.contains("LEAKED")
        )),
        "a delta after TurnFailed must NOT be folded into the closed entry"
    );
}

/// GAP: a `WaslNativeGateAnswered` IPC line received inbound by core is handled
/// as a serve-loop ack no-op (Ok, no updates) — the request path only ever SENT
/// `AnswerWaslNativeGate`; here we drive the RECEIVE side (lib.rs
/// IpcMsg::WaslNativeGateAnswered arm).
#[test]
fn wasl_native_gate_answered_ack_is_handled_as_noop() {
    let (mut core, _peer) = make_core();
    let line = encode_ipc(&IpcMsg::WaslNativeGateAnswered {
        session_id: "any-session".to_string(),
        gate_id: "any-gate".to_string(),
    });
    let updates = core
        .on_agent_line(&line)
        .expect("a WaslNativeGateAnswered ack must be handled, not error");
    assert!(
        updates.is_empty(),
        "the WaslNativeGateAnswered serve-loop ack must be a silent no-op, got {updates:?}"
    );
}

/// GAP (tolerant-skip): a malformed `WaslSessionStatus` payload (a JSON Value
/// that is not a valid `WaslTargetStatus`) arriving for a known session must be
/// logged and no-op'd, NOT error the reader loop (lib.rs handle_wasl_session_status
/// bad-status arm).
#[test]
fn malformed_wasl_status_value_is_tolerantly_skipped() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);
    let _ = peer.drain_outbound();

    // A status Value that does not match any WaslTargetStatus variant.
    let bad = encode_ipc(&IpcMsg::WaslSessionStatus {
        session_id: id.clone(),
        status: serde_json::json!({ "NotARealStatus": { "foo": 1 } }),
    });
    let updates = core
        .on_agent_line(&bad)
        .expect("a malformed status must be tolerantly skipped, not error the loop");
    assert!(
        updates.is_empty(),
        "a malformed WaslSessionStatus must produce no updates (log + no-op), got {updates:?}"
    );
}

/// GAP (tolerant-skip): a malformed `WaslSessionEvent` payload for a known
/// session is logged and no-op'd, not an error (lib.rs handle_wasl_session_event
/// bad-event arm).
#[test]
fn malformed_wasl_event_value_is_tolerantly_skipped() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);
    let _ = peer.drain_outbound();

    // event Value that is not a valid WaslSessionEvent (missing seq/kind shape).
    let bad = encode_ipc(&IpcMsg::WaslSessionEvent {
        session_id: id.clone(),
        seq: 1,
        event: serde_json::json!({ "totally": "wrong shape" }),
    });
    let updates = core
        .on_agent_line(&bad)
        .expect("a malformed event must be tolerantly skipped, not error the loop");
    assert!(
        updates.is_empty(),
        "a malformed WaslSessionEvent must produce no updates (log + no-op), got {updates:?}"
    );
}

/// GAP (tolerant-skip): an unknown event-kind tag (the forward-compat
/// `WaslSessionEventKind::Unknown`) is logged and ignored — folded to no
/// transcript change, no panic (lib.rs K::Unknown arm).
#[test]
fn unknown_wasl_event_kind_is_logged_and_ignored() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);
    let _ = peer.drain_outbound();

    let updates = core
        .on_agent_line(&event_line(
            &id,
            &ev(
                1,
                WaslSessionEventKind::Unknown("SomeFutureEventKind".to_string()),
            ),
        ))
        .expect("an unknown event kind must apply, not error");
    // The handler still emits a (no-change) Session view; the transcript must be
    // untouched — no entry synthesized from an unknown kind.
    let sv = last_session_view(&updates, &id);
    assert!(
        sv.transcript.is_empty(),
        "an unknown event kind must not synthesize any transcript entry, got {:?}",
        sv.transcript
    );
}

/// A `WaslSessionStatus` for a session core never launched is a tolerant no-op
/// (lib.rs handle_wasl_session_status unknown-session arm) — the session-status
/// analogue of the already-covered unknown-session EVENT case.
#[test]
fn wasl_status_for_unknown_session_is_tolerant_noop() {
    let (mut core, _peer) = make_core();
    let updates = core
        .on_agent_line(&status_line("never-launched", &WaslTargetStatus::Starting))
        .expect("a status for an unknown session must not error");
    assert!(
        updates.is_empty(),
        "a status for an unknown session must produce no updates, got {updates:?}"
    );
}

/// The remaining §7 event folding arms — ToolStarted/ToolFinished (a tool row),
/// SessionReady (informational), SessionExited (terminal → Done + not live), and
/// ObserveLost (→ Detached) — fold correctly (lib.rs K::Tool*/SessionReady/
/// SessionExited/ObserveLost arms).
#[test]
fn tool_and_lifecycle_events_fold_into_transcript_and_phase() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    // SessionReady is purely informational (no transcript row).
    core.on_agent_line(&event_line(
        &id,
        &ev(1, WaslSessionEventKind::SessionReady { native: None }),
    ))
    .expect("ready");

    // ToolStarted pushes a Tool row; ToolFinished records no new content.
    let started = core
        .on_agent_line(&event_line(
            &id,
            &ev(
                2,
                WaslSessionEventKind::ToolStarted {
                    turn_id: "t1".to_string(),
                    name: "Bash".to_string(),
                    input: serde_json::json!({ "command": "ls -la" }),
                },
            ),
        ))
        .expect("tool started");
    let sv = last_session_view(&started, &id);
    // The Tool row must carry the folded JSON input (not an empty string) so the
    // Wasl-managed session's expandable tool block shows the arguments, exactly
    // like a native session (#7).
    let tool_rows: Vec<&marsad_events::TranscriptEntry> = sv
        .transcript
        .iter()
        .filter(|e| {
            matches!(
                e,
                marsad_events::TranscriptEntry::Tool { name, .. } if name == "Bash"
            )
        })
        .collect();
    assert_eq!(
        tool_rows.len(),
        1,
        "ToolStarted must push exactly one Tool row"
    );
    match tool_rows[0] {
        marsad_events::TranscriptEntry::Tool { input, .. } => {
            assert_eq!(
                input, r#"{"command":"ls -la"}"#,
                "ToolStarted must fold the tool's JSON input into the Tool row"
            );
        }
        other => panic!("expected a Tool entry, got {other:?}"),
    }

    let before = sv.transcript.len();
    let finished = core
        .on_agent_line(&event_line(
            &id,
            &ev(
                3,
                WaslSessionEventKind::ToolFinished {
                    turn_id: "t1".to_string(),
                    name: "Bash".to_string(),
                    ok: true,
                    result: Some(serde_json::json!("total 0")),
                },
            ),
        ))
        .expect("tool finished");
    let sv_fin = last_session_view(&finished, &id);
    assert_eq!(
        sv_fin.transcript.len(),
        before,
        "ToolFinished must not add a new transcript row"
    );

    // ObserveLost → Detached.
    let lost = core
        .on_agent_line(&event_line(
            &id,
            &ev(
                4,
                WaslSessionEventKind::ObserveLost {
                    error: "conn dropped".to_string(),
                },
            ),
        ))
        .expect("observe lost");
    assert_eq!(
        last_session_view(&lost, &id).state,
        Phase::Detached,
        "ObserveLost must project to Phase::Detached"
    );

    // SessionExited → Done and no longer live.
    let exited = core
        .on_agent_line(&event_line(
            &id,
            &ev(5, WaslSessionEventKind::SessionExited { code: Some(0) }),
        ))
        .expect("session exited");
    let sv_exit = last_session_view(&exited, &id);
    assert_eq!(
        sv_exit.state,
        Phase::Done,
        "SessionExited must project to Phase::Done"
    );
    assert!(
        !sv_exit.live,
        "SessionExited must mark the session as no longer live"
    );
}

/// A `RejectedUnavailable` receipt surfaces visibly (a toast) and drives
/// Phase::Error — the remaining `WaslTurnReceipt` arm the existing Part-5 tests
/// don't cover (lib.rs handle_wasl_prompt_receipt RejectedUnavailable arm).
#[test]
fn rejected_unavailable_receipt_surfaces_error_toast() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);
    let _ = peer.drain_outbound();

    let updates = core
        .on_agent_line(&receipt_line(
            &id,
            &WaslTurnReceipt::RejectedUnavailable {
                status: WaslTargetStatus::Faulted {
                    error: "target gone".to_string(),
                    recoverable: false,
                },
            },
        ))
        .expect("a rejected-unavailable receipt must apply");
    assert!(
        updates.iter().any(|u| matches!(u, VmUpdate::Toast { .. })),
        "RejectedUnavailable must surface a visible Toast, got {updates:?}"
    );
    let sv = last_session_view(&updates, &id);
    assert_eq!(
        sv.state,
        Phase::Error,
        "RejectedUnavailable must drive Phase::Error"
    );
}

/// A `Started` receipt drives Phase::Working and emits no toast (the calm
/// happy-path receipt arm — lib.rs handle_wasl_prompt_receipt Started arm).
#[test]
fn started_receipt_marks_working_without_a_toast() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);
    let _ = peer.drain_outbound();

    let updates = core
        .on_agent_line(&receipt_line(
            &id,
            &WaslTurnReceipt::Started {
                turn_id: "t-started".to_string(),
                first_seq: 1,
            },
        ))
        .expect("a started receipt must apply");
    let sv = last_session_view(&updates, &id);
    assert_eq!(
        sv.state,
        Phase::Working,
        "a Started receipt makes the turn in-flight (Working)"
    );
    assert!(
        !updates.iter().any(|u| matches!(u, VmUpdate::Toast { .. })),
        "a Started receipt is the calm path — no toast, got {updates:?}"
    );
}

/// A prompt receipt for a session core never launched is a tolerant no-op
/// (lib.rs handle_wasl_prompt_receipt unknown-session arm).
#[test]
fn wasl_prompt_receipt_for_unknown_session_is_tolerant_noop() {
    let (mut core, _peer) = make_core();
    let updates = core
        .on_agent_line(&receipt_line(
            "never-launched",
            &WaslTurnReceipt::Queued {
                turn_id: "t".to_string(),
                position: 1,
            },
        ))
        .expect("a receipt for an unknown session must not error");
    assert!(
        updates.is_empty(),
        "a receipt for an unknown session must produce no updates, got {updates:?}"
    );
}

/// GAP: a `WaslSessionEvent` fed to a MarsadManaged session still folds (the
/// session exists) but its `persist_wasl_last_seq` best-effort persist takes the
/// `RuntimeOwner::MarsadManaged => return` early-out — no binding is written for
/// a non-Wasl session (lib.rs persist_wasl_last_seq MarsadManaged arm). This is
/// the reachable defensive branch for a misdirected Wasl event on a plain
/// session; it must not panic and must not persist a spurious binding.
#[test]
fn wasl_event_on_marsad_managed_session_does_not_persist_a_binding() {
    // Shared store so we can prove no waslbinding record was written.
    let backend = marsad_store::MemoryBackend::new();
    let (t1a, mut t1b) = LoopbackFake::pair();
    let mut core = Core::with_deps(
        cfg(),
        Box::new(t1a),
        Box::new(NoGit),
        Box::new(NoTranscript),
        Box::new(backend.clone()),
    )
    .expect("core");

    // A plain MarsadManaged session (today's launch path).
    let spec = marsad_core::LaunchSpec {
        project: "/home/u/proj".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: "claude-opus-4-5".to_string(),
        session_mode: String::new(),
        first_prompt: None,
        mode: Default::default(),
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
    };
    let id = core.launch_session(spec).expect("launch_session");
    let _ = t1b.drain_outbound();

    // A WaslSessionEvent misdirected at the MarsadManaged session: it folds
    // (session exists) but persist_wasl_last_seq early-returns on MarsadManaged.
    core.on_agent_line(&event_line(
        &id,
        &ev(
            1,
            WaslSessionEventKind::AssistantDelta {
                turn_id: "t1".to_string(),
                text: "stray".to_string(),
            },
        ),
    ))
    .expect("event on a MarsadManaged session must not error");

    // No binding record must have been persisted for this non-Wasl session.
    let store = marsad_store::Store::new(Box::new(backend.clone()));
    let bindings = store.wasl_bindings().expect("wasl_bindings");
    assert!(
        !bindings.contains_key(&id),
        "a MarsadManaged session must never get a persisted Wasl binding, got {bindings:?}"
    );
}

/// A malformed `WaslPromptReceipt` payload for a known session is tolerantly
/// skipped (lib.rs handle_wasl_prompt_receipt bad-receipt arm).
#[test]
fn malformed_wasl_prompt_receipt_is_tolerantly_skipped() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);
    let _ = peer.drain_outbound();

    let bad = encode_ipc(&IpcMsg::WaslPromptReceipt {
        session_id: id.clone(),
        receipt: serde_json::json!({ "NotAReceipt": true }),
    });
    let updates = core
        .on_agent_line(&bad)
        .expect("a malformed receipt must be tolerantly skipped, not error");
    assert!(
        updates.is_empty(),
        "a malformed WaslPromptReceipt must produce no updates, got {updates:?}"
    );
}

// ===========================================================================
// Part 9 — v3 spec-correction coverage (Duplicate receipt, (turn_id, seq)
// native-gate identity + the two-gates-in-one-turn regression pin, and the
// stream-Truncated responsibility boundary). These are NEW tests added for the
// v3 contract, not patches to pre-v3 ones.
// ===========================================================================

/// v3: a `WaslTurnReceipt::Duplicate { original }` line — returned by Wasl when a
/// resent `(TargetKey, IdempotencyKey)` arrives, with NO second injection — must
/// be handled by core's `handle_wasl_prompt_receipt` WITHOUT panicking on the
/// previously-non-exhaustive `WaslTurnReceipt` match. Core surfaces the WRAPPED
/// original's disposition (here a `Started`), i.e. it "unwraps" the duplicate to
/// the original receipt rather than treating a duplicate as a fresh/failed send.
#[test]
fn duplicate_receipt_is_handled_and_surfaces_the_wrapped_original() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);
    let _ = peer.drain_outbound();

    // A resent idempotency key: Wasl returns the ORIGINAL Started receipt wrapped.
    let updates = core
        .on_agent_line(&receipt_line(
            &id,
            &WaslTurnReceipt::Duplicate {
                original: Box::new(WaslTurnReceipt::Started {
                    turn_id: "t-dup".to_string(),
                    first_seq: 1,
                }),
            },
        ))
        .expect("a Duplicate receipt must apply, not panic on a non-exhaustive match");

    // Unwrapping the Duplicate to its Started original means the session is
    // treated as an in-flight turn (Working), exactly like the bare Started arm —
    // NOT as an error/rejection (a duplicate is a benign resend, not a failure).
    let sv = last_session_view(&updates, &id);
    assert_eq!(
        sv.state,
        Phase::Working,
        "a Duplicate{{Started}} receipt must surface the wrapped original's \
         disposition (Working), not be mishandled as a rejection"
    );
    assert!(
        !updates.iter().any(|u| matches!(u, VmUpdate::Toast { .. })),
        "a benign duplicate resend must not raise an error toast, got {updates:?}"
    );
}

/// v3: a `Duplicate` wrapping a `RejectedBusy` unwraps to the busy disposition —
/// core must react to the ORIGINAL inside the wrapper, whatever it is (proving
/// the unwrap is real, not hard-coded to Started).
#[test]
fn duplicate_receipt_wrapping_a_rejection_surfaces_that_rejection() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);
    let _ = peer.drain_outbound();

    let updates = core
        .on_agent_line(&receipt_line(
            &id,
            &WaslTurnReceipt::Duplicate {
                original: Box::new(WaslTurnReceipt::RejectedBusy {
                    active_turn: "t-active".to_string(),
                }),
            },
        ))
        .expect("a Duplicate{RejectedBusy} receipt must apply");
    // The wrapped RejectedBusy must still surface visibly (same as the bare
    // RejectedBusy arm) — the duplicate wrapper is transparent to disposition.
    assert!(
        !updates.is_empty(),
        "a Duplicate wrapping a RejectedBusy must surface the wrapped busy signal, \
         not be silently dropped"
    );
    assert!(
        updates
            .iter()
            .any(|u| matches!(u, VmUpdate::Toast { .. } | VmUpdate::Session(_))),
        "the wrapped busy rejection must be surfaced, got {updates:?}"
    );
}

/// REGRESSION PIN (v3, §10.4): two `NativeGateRequested` events with the SAME
/// `turn_id` but DIFFERENT `seq` are two DISTINCT gate occurrences and must NOT
/// collide. Wasl does not prevent two native gates per turn (approve tool A → the
/// turn continues → tool B later gates → same `turn_id` both times), so core's
/// folding must derive identity from `(turn_id, seq)` via `native_gate_key`, not
/// from `turn_id` alone. If the two collapsed to one identity, answering the
/// first would (wrongly) satisfy the second; here each is answered independently
/// and each emits its own `AnswerWaslNativeGate` with its own distinct key.
#[test]
fn two_native_gates_in_one_turn_with_different_seq_do_not_collide() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    let turn_id = "turn-shared";
    let detail_a = WaslNativeGateDetail {
        summary: "approve tool A".to_string(),
        preview: "$ tool-a".to_string(),
    };
    let detail_b = WaslNativeGateDetail {
        summary: "approve tool B".to_string(),
        preview: "$ tool-b".to_string(),
    };
    let key_a = native_gate_key(turn_id, 1);
    let key_b = native_gate_key(turn_id, 2);
    assert_ne!(
        key_a, key_b,
        "the whole point: same turn_id + different seq → DISTINCT gate keys"
    );

    // First gate (seq 1) for the shared turn.
    let up_a = core
        .on_agent_line(&event_line(
            &id,
            &ev(
                1,
                WaslSessionEventKind::NativeGateRequested {
                    turn_id: turn_id.to_string(),
                    detail: detail_a.clone(),
                },
            ),
        ))
        .expect("first gate applies");
    let gate_a = last_session_view(&up_a, &id)
        .gate
        .expect("first gate populates SessionView.gate");
    let cmd_a = match gate_a {
        Gate::Permission { command, .. } => command,
        other => panic!("expected Gate::Permission, got {other:?}"),
    };
    assert_eq!(
        cmd_a,
        native_gate_command(&key_a, &detail_a),
        "the first gate's command must encode (turn_id, seq=1)"
    );

    // Answer ONLY the first gate.
    let _ = peer.drain_outbound();
    core.answer_wasl_native_gate(&id, &key_a, Decision::Approve)
        .expect("answer first gate");
    let first_out: Vec<IpcMsg> = peer
        .drain_outbound()
        .iter()
        .filter_map(|l| parse_ipc(l).ok())
        .collect();
    assert!(
        first_out.iter().any(|m| matches!(
            m,
            IpcMsg::AnswerWaslNativeGate { gate_id, .. } if gate_id == &key_a
        )),
        "answering the first gate must carry its own (turn_id, seq=1) key, got {first_out:?}"
    );

    // Second gate (seq 2), SAME turn_id — a genuinely distinct occurrence.
    let up_b = core
        .on_agent_line(&event_line(
            &id,
            &ev(
                2,
                WaslSessionEventKind::NativeGateRequested {
                    turn_id: turn_id.to_string(),
                    detail: detail_b.clone(),
                },
            ),
        ))
        .expect("second gate applies");
    let gate_b = last_session_view(&up_b, &id).gate.expect(
        "second gate populates SessionView.gate — must NOT have been \
                 pre-satisfied by answering the first",
    );
    let cmd_b = match gate_b {
        Gate::Permission { command, .. } => command,
        other => panic!("expected Gate::Permission, got {other:?}"),
    };
    assert_eq!(
        cmd_b,
        native_gate_command(&key_b, &detail_b),
        "the second gate's command must encode (turn_id, seq=2) — distinct from the first"
    );
    assert_ne!(
        cmd_a, cmd_b,
        "the two gates' commands (⇒ DecisionIds) MUST differ so answering one \
         never satisfies the other — the v3 (turn_id, seq) identity fix"
    );

    // Answering the second gate is a fresh, non-deduped decision (distinct key)
    // and emits its own line.
    let _ = peer.drain_outbound();
    core.answer_wasl_native_gate(&id, &key_b, Decision::Approve)
        .expect("answer second gate");
    let second_out: Vec<IpcMsg> = peer
        .drain_outbound()
        .iter()
        .filter_map(|l| parse_ipc(l).ok())
        .collect();
    assert!(
        second_out.iter().any(|m| matches!(
            m,
            IpcMsg::AnswerWaslNativeGate { gate_id, .. } if gate_id == &key_b
        )),
        "the second gate must be answerable independently with its own distinct \
         (turn_id, seq=2) key — a collision would have deduped it to a no-op, got {second_out:?}"
    );
}

/// v3 boundary note (documented as a test): a stream `WaslStreamItem::Truncated`
/// is NOT part of core's inbound IPC surface. Per the plan §4.4/§10.1, reacting
/// to a truncation (re-snapshot + resubscribe from `latest`) is the AGENT observe
/// thread's responsibility, upstream of core — the agent re-fetches a snapshot
/// and re-issues a fresh `AttachWaslSession`, so core only ever sees ordinary
/// `WaslSessionEvent` lines (never a truncated control item folded into the
/// transcript). This test pins that separation: constructing a `Truncated` item
/// is a `marsad-wasl-session` value with no core `IpcMsg` counterpart, so it can
/// never be mis-folded as an event here. (The agent-side handling is covered in
/// `marsad-agent/tests/wasl_session.rs`.)
#[test]
fn stream_truncated_is_not_a_core_folding_concern() {
    // A Truncated stream item is a distinct WaslStreamItem — never a
    // WaslSessionEvent — so it cannot be wrapped into an IpcMsg::WaslSessionEvent
    // and folded. This is a compile+shape pin of the responsibility boundary.
    let item = WaslStreamItem::Truncated {
        earliest: 10,
        latest: 42,
    };
    match item {
        WaslStreamItem::Truncated { earliest, latest } => {
            assert!(latest > earliest, "latest is the resubscribe cursor");
        }
        WaslStreamItem::Event(_) => {
            panic!("a Truncated must never be a normal Event — it is an unmissable control signal")
        }
    }
}

// ===========================================================================
// Part 10 — resume_session and restart_session routing (Bug Fix: §6.1)
//
// resume_session on WaslManaged must NOT emit ResumeSession (native) — it must
// reuse attach_session_events (→ AttachWaslSession). The UI-state effects
// (Phase::Starting, live=true, Board/Overview/Session VmUpdates) are preserved.
//
// restart_session on WaslManaged must NOT emit StopSession/LaunchSession —
// it must emit StopWaslSession then LaunchWaslSession with the new model merged
// into the binding's provider_key context. MarsadManaged paths: regression pins.
// ===========================================================================

/// WaslManaged resume_session emits AttachWaslSession (reuses attach_session_events),
/// NOT ResumeSession, and still returns Phase::Starting + live + Board/Overview/Session
/// VmUpdates (the UI effects resume_session always provided).
#[test]
fn wasl_managed_resume_session_emits_attach_wasl_not_resume() {
    let (mut core, mut peer) = make_core();
    let target_key = serde_json::json!({
        "protocol": "claude",
        "context": {"cwd": "/home/u/proj"},
    });
    let id = core
        .launch_wasl_session("host-resume", target_key.clone(), None)
        .expect("launch_wasl_session");
    let _ = peer.drain_outbound();

    // Call resume_session on the WaslManaged session.
    let updates = core
        .resume_session(&id)
        .expect("resume_session on WaslManaged must succeed");

    // Must emit AttachWaslSession, NOT ResumeSession.
    let out: Vec<IpcMsg> = peer
        .drain_outbound()
        .iter()
        .filter_map(|l| parse_ipc(l).ok())
        .collect();
    assert!(
        out.iter().any(|m| matches!(
            m,
            IpcMsg::AttachWaslSession { session_id, wasl_target_key, host_id, .. }
                if session_id == &id
                    && wasl_target_key == &target_key
                    && host_id == "host-resume"
        )),
        "resume_session on WaslManaged must emit AttachWaslSession with the binding's \
         provider_key and host_id (reusing attach_session_events), got {out:?}"
    );
    assert!(
        !out.iter().any(|m| matches!(m, IpcMsg::ResumeSession { .. })),
        "resume_session on WaslManaged must NEVER emit the native ResumeSession, got {out:?}"
    );

    // UI-state effects: the session update must carry Phase::Starting and live=true,
    // and the board must be updated (moved off Paused).
    let sv = last_session_view(&updates, &id);
    assert_eq!(
        sv.state,
        Phase::Starting,
        "resume_session must optimistically set Phase::Starting on the WaslManaged session"
    );
    assert!(
        sv.live,
        "resume_session must set live=true on the WaslManaged session"
    );
    assert!(
        updates.iter().any(|u| matches!(u, VmUpdate::Board(_))),
        "resume_session must return a Board VmUpdate (the session moves off Paused)"
    );
    assert!(
        updates.iter().any(|u| matches!(u, VmUpdate::Overview(_))),
        "resume_session must return an Overview VmUpdate"
    );
}

/// REGRESSION PIN: MarsadManaged resume_session still emits ResumeSession (native),
/// NOT AttachWaslSession.
#[test]
fn marsad_managed_resume_session_still_emits_resume_session() {
    let (mut core, mut peer) = make_core();
    let id = launch_marsad(&mut core, &mut peer);

    let _ = core
        .resume_session(&id)
        .expect("resume_session on MarsadManaged must succeed");
    let out: Vec<IpcMsg> = peer
        .drain_outbound()
        .iter()
        .filter_map(|l| parse_ipc(l).ok())
        .collect();
    assert!(
        out.iter().any(|m| matches!(
            m,
            IpcMsg::ResumeSession { session_id, .. } if session_id == &id
        )),
        "MarsadManaged resume_session must still emit the native ResumeSession, got {out:?}"
    );
    assert!(
        !out.iter().any(|m| matches!(m, IpcMsg::AttachWaslSession { .. })),
        "MarsadManaged resume_session must NEVER emit AttachWaslSession, got {out:?}"
    );
}

/// WaslManaged restart_session emits StopWaslSession + LaunchWaslSession with the
/// new model merged into the provider_key's context (NOT native StopSession/LaunchSession).
/// The Marsad session id is preserved (same row stays in place on the board).
#[test]
fn wasl_managed_restart_session_emits_stop_then_launch_wasl() {
    let (mut core, mut peer) = make_core();
    let target_key = serde_json::json!({
        "protocol": "claude",
        "context": {"cwd": "/home/u/proj", "model": "old-model", "session_mode": "default"},
    });
    let id = core
        .launch_wasl_session("host-restart", target_key.clone(), None)
        .expect("launch_wasl_session");
    let _ = peer.drain_outbound();

    let updates = core
        .restart_session(&id, "new-model", "bypassPermissions".to_string(), vec![])
        .expect("restart_session on WaslManaged must succeed");

    let out: Vec<IpcMsg> = peer
        .drain_outbound()
        .iter()
        .filter_map(|l| parse_ipc(l).ok())
        .collect();

    // Must emit StopWaslSession, not StopSession.
    assert!(
        out.iter().any(|m| matches!(
            m,
            IpcMsg::StopWaslSession { session_id, .. } if session_id == &id
        )),
        "restart_session on WaslManaged must emit StopWaslSession, got {out:?}"
    );
    assert!(
        !out.iter().any(|m| matches!(m, IpcMsg::StopSession { .. })),
        "restart_session on WaslManaged must NEVER emit the native StopSession, got {out:?}"
    );

    // Must emit LaunchWaslSession with the same session id and the new model in context.
    let launch = out.iter().find_map(|m| match m {
        IpcMsg::LaunchWaslSession {
            session_id,
            host_id,
            wasl_target_key,
            ..
        } if session_id == &id => Some((host_id.clone(), wasl_target_key.clone())),
        _ => None,
    });
    let (launched_host, launched_key) = launch.expect(
        "restart_session on WaslManaged must emit LaunchWaslSession with the same session id",
    );
    assert_eq!(
        launched_host, "host-restart",
        "LaunchWaslSession must carry the binding's host_id"
    );
    // The new model must be merged into context.model.
    let ctx = launched_key
        .get("context")
        .expect("LaunchWaslSession key must have a 'context' sub-object");
    assert_eq!(
        ctx.get("model").and_then(|v| v.as_str()),
        Some("new-model"),
        "LaunchWaslSession key's context.model must be the new model, got {ctx:?}"
    );
    assert_eq!(
        ctx.get("session_mode").and_then(|v| v.as_str()),
        Some("bypassPermissions"),
        "LaunchWaslSession key's context.session_mode must be the new mode, got {ctx:?}"
    );
    // Other context fields must be preserved (cwd).
    assert_eq!(
        ctx.get("cwd").and_then(|v| v.as_str()),
        Some("/home/u/proj"),
        "LaunchWaslSession key must preserve context.cwd, got {ctx:?}"
    );
    // protocol must be preserved too.
    assert_eq!(
        launched_key.get("protocol").and_then(|v| v.as_str()),
        Some("claude"),
        "LaunchWaslSession key must preserve protocol, got {launched_key:?}"
    );

    // Must NOT emit native LaunchSession.
    assert!(
        !out.iter().any(|m| matches!(m, IpcMsg::LaunchSession { .. })),
        "restart_session on WaslManaged must NEVER emit native LaunchSession, got {out:?}"
    );

    // The session view must show Phase::Starting and the same session id.
    let sv = last_session_view(&updates, &id);
    assert_eq!(
        sv.state,
        Phase::Starting,
        "WaslManaged restart must flip phase to Starting"
    );
    assert!(sv.live, "WaslManaged restart must set live=true");
}

/// REGRESSION PIN: MarsadManaged restart_session still emits StopSession then
/// LaunchSession (native), NOT StopWaslSession/LaunchWaslSession.
#[test]
fn marsad_managed_restart_session_still_native() {
    let (mut core, mut peer) = make_core();
    let id = launch_marsad(&mut core, &mut peer);

    core.restart_session(&id, "new-model", String::new(), vec![])
        .expect("MarsadManaged restart must succeed");
    let out: Vec<IpcMsg> = peer
        .drain_outbound()
        .iter()
        .filter_map(|l| parse_ipc(l).ok())
        .collect();
    assert!(
        out.iter().any(|m| matches!(m, IpcMsg::StopSession { .. })),
        "MarsadManaged restart must still emit StopSession, got {out:?}"
    );
    assert!(
        !out.iter().any(|m| matches!(m, IpcMsg::StopWaslSession { .. })),
        "MarsadManaged restart must NEVER emit StopWaslSession, got {out:?}"
    );
    assert!(
        out.iter().any(|m| matches!(m, IpcMsg::LaunchSession { .. })),
        "MarsadManaged restart must still emit LaunchSession, got {out:?}"
    );
    assert!(
        !out.iter().any(|m| matches!(m, IpcMsg::LaunchWaslSession { .. })),
        "MarsadManaged restart must NEVER emit LaunchWaslSession, got {out:?}"
    );
}

// ---------------------------------------------------------------------------
// Local test doubles for the Core::with_deps injection seams (git + transcript)
// so the reconnect tests don't touch the real filesystem/git.
// ---------------------------------------------------------------------------

struct NoGit;
impl marsad_core::GitProbe for NoGit {
    fn changes(&self, _project_dir: &str) -> Vec<marsad_viewmodel::FileChange> {
        Vec::new()
    }
}

struct NoTranscript;
impl marsad_core::TranscriptReader for NoTranscript {
    fn session(&self, _project: &str, _session_id: &str) -> Vec<marsad_events::TranscriptEntry> {
        Vec::new()
    }
    fn most_recent(&self, _project: &str) -> Vec<marsad_events::TranscriptEntry> {
        Vec::new()
    }
    // `session_usage` uses the trait's default (empty map) — no override needed.
}

// ── small transcript-inspection helpers ──────────────────────────────────────

fn assistant_text_occurrences(
    transcript: &[marsad_events::TranscriptEntry],
    needle: &str,
) -> usize {
    transcript
        .iter()
        .filter(|e| matches!(e, marsad_events::TranscriptEntry::Assistant { text, .. } if text.contains(needle)))
        .count()
}

/// Return the last `SessionView` in `updates` for `id`, or the supplied fallback
/// if this update batch produced none.
fn updates_session_view_or(
    updates: &[VmUpdate],
    id: &str,
    fallback: marsad_viewmodel::SessionView,
) -> marsad_viewmodel::SessionView {
    updates
        .iter()
        .rev()
        .find_map(|u| match u {
            VmUpdate::Session(sv) if sv.id == id => Some(sv.clone()),
            _ => None,
        })
        .unwrap_or(fallback)
}

/// Return the last `SessionView` for `id` from `updates`, or fetch it from the
/// core's snapshot if this batch produced none (for defensive-path assertions
/// where an ignored event legitimately emits nothing).
fn last_session_view_or_prev(
    updates: &[VmUpdate],
    id: &str,
    core: &mut Core,
) -> marsad_viewmodel::SessionView {
    if let Some(sv) = updates.iter().rev().find_map(|u| match u {
        VmUpdate::Session(sv) if sv.id == id => Some(sv.clone()),
        _ => None,
    }) {
        return sv;
    }
    core.snapshot()
        .sessions
        .into_iter()
        .find(|sv| sv.id == id)
        .unwrap_or_else(|| panic!("session {id} must exist in the snapshot"))
}

// ---------------------------------------------------------------------------
// Feature A migration — restart_session re-derives peer names + host_names into
// the frozen provider_key so an EXISTING workstream gains names with no
// re-create. The peer roster in the persisted key predates names (opaque ids
// only); re-derivation maps each peer agent_id -> the sibling session whose
// provider_key["key"] == that agent_id -> its display name + host label.
// ---------------------------------------------------------------------------

/// Extract the host id (`host-N`) from an `add_host` Hosts VmUpdate batch by
/// matching the label we just added.
fn host_id_for_label(updates: &[VmUpdate], label: &str) -> String {
    updates
        .iter()
        .rev()
        .find_map(|u| match u {
            VmUpdate::Hosts(hosts) => hosts
                .iter()
                .find(|h| h.label == label)
                .map(|h| h.id.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no host with label {label} in {updates:?}"))
}

#[test]
fn wasl_restart_rederives_peer_names_and_self_host_name_into_key() {
    let (mut core, mut peer) = make_core();

    // A host with a human label — the peer/self host_name source.
    let hu = core
        .add_host("gpu-box", "user@gpu.example", None, None, vec![], vec![])
        .expect("add_host");
    let host = host_id_for_label(&hu, "gpu-box");
    let _ = peer.drain_outbound();

    // The SIBLING ("Reviewer"): its provider_key's top-level "key" is the
    // agent_id peers address it by (== the desktop's draft-node id).
    let reviewer_key = serde_json::json!({
        "protocol": "claude",
        "key": "draft-node-reviewer",
        "context": {"cwd": "/proj", "model": "m", "session_mode": "default", "peers": []},
    });
    let _reviewer = core
        .launch_wasl_session(&host, reviewer_key, Some("Reviewer".to_string()))
        .expect("launch reviewer");
    let _ = peer.drain_outbound();

    // The session under restart ("Writer"): its FROZEN roster references the
    // reviewer by agent_id ONLY — no name/host_name (the pre-migration shape).
    let writer_key = serde_json::json!({
        "protocol": "claude",
        "key": "draft-node-writer",
        "context": {
            "cwd": "/proj",
            "model": "m",
            "session_mode": "default",
            "peers": [ {"agent_id": "draft-node-reviewer", "host_id": host } ]
        },
    });
    let writer = core
        .launch_wasl_session(&host, writer_key, Some("Writer".to_string()))
        .expect("launch writer");
    let _ = peer.drain_outbound();

    // Restart Writer — the migration must patch names into the launch key.
    let _ = core
        .restart_session(&writer, "m2", "default".to_string(), vec![])
        .expect("restart writer");
    let out: Vec<IpcMsg> = peer
        .drain_outbound()
        .iter()
        .filter_map(|l| parse_ipc(l).ok())
        .collect();

    let launched_key = out
        .iter()
        .find_map(|m| match m {
            IpcMsg::LaunchWaslSession { session_id, wasl_target_key, .. }
                if session_id == &writer =>
            {
                Some(wasl_target_key.clone())
            }
            _ => None,
        })
        .expect("restart must emit LaunchWaslSession for writer");

    let ctx = launched_key.get("context").expect("context present");
    // Self node's host display name was patched onto the key.
    assert_eq!(
        ctx.get("host_name").and_then(|v| v.as_str()),
        Some("gpu-box"),
        "self host_name must be re-derived onto context.host_name, got {ctx:?}"
    );
    // The peer entry gained the sibling's display name + its host label.
    let peers = ctx
        .get("peers")
        .and_then(|p| p.as_array())
        .expect("peers array present");
    assert_eq!(peers.len(), 1, "roster preserved");
    assert_eq!(
        peers[0].get("agent_id").and_then(|v| v.as_str()),
        Some("draft-node-reviewer"),
        "peer agent_id preserved (routing unchanged)"
    );
    assert_eq!(
        peers[0].get("name").and_then(|v| v.as_str()),
        Some("Reviewer"),
        "peer name must be re-derived from the sibling session's display name, got {:?}",
        peers[0]
    );
    assert_eq!(
        peers[0].get("host_name").and_then(|v| v.as_str()),
        Some("gpu-box"),
        "peer host_name must be re-derived from the sibling's host label, got {:?}",
        peers[0]
    );
}
