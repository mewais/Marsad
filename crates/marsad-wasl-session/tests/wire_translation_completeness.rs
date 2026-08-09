//! wire_translation_completeness.rs — spec-derived black-box coverage for the
//! wire-translation vocabulary that ships NO golden fixture (design
//! `docs/design/wasl-backend-plan.md` §4.4; authoritative wire shapes from
//! `/work/mewais/Wasl/docs/specs/runtime-control-api.md`).
//!
//! The frozen `real_*.rs` suite deliberately exercises exactly ONE shipped
//! fixture per wire shape, so it leaves the remaining closed-set wire variants
//! (which vendor no `runtime-control-v1/*.json`) undriven. This file closes that
//! gap — still black-box, through `RealWaslSessionClient` + the loopback
//! `MockRuntimeServer` — by hand-constructing minimal valid wire JSON per the
//! spec's documented tag/shape for each missing variant and asserting it parses
//! to the correct public type. Every JSON literal below is authored from the
//! wire spec (`runtime-control-api.md` §Conventions: internally-tagged
//! `{"type":"…"}`, snake_case type tags) and the §4.4 type table — NOT from the
//! implementation.
//!
//! WHAT this covers (audit checklist → spec source):
//!   * `WaslTargetStatus` kinds with no fixture: `offline`, `starting`,
//!     `interrupting{active_turn,since}`, `draining{active_turn,since}`,
//!     `stopped`, and the forward-compat `Unknown(tag)` fallback for an
//!     unrecognized `status.type` (plan §4.4 `WaslTargetStatus`; spec §Conventions
//!     unknown-`type` tolerance).
//!   * `WaslSessionEventKind` kinds with no `turn_event_*.json`: `turn_queued`,
//!     `tool_finished`, `native_gate_resolved`, `turn_cancelled`, `turn_expired`,
//!     `status_changed` (plan §4.4 `WaslSessionEventKind`; spec §Frame
//!     `turn_event` variant list).
//!   * an AGENT-originated turn `source` (`{"type":"agent","from":…}`; spec
//!     §POST …/turns `source` is the R1 `TurnSource`) — fixtures only exercise
//!     `local_human`.
//!   * `WaslBusyPolicy::QueueRelated` (`{"type":"queue_related",…}`; plan §4.4
//!     `WaslBusyPolicy`; spec §POST …/turns `busy_policy`).
//!   * `WaslCancelReason::Superseded` (spec §POST …/cancel `reason` ∈ the R1
//!     `CancelReason` set) — fixtures/tests only exercise the human reason.
//!   * the SSE parser tolerating an unmodelled extra SSE field (a `retry:` line
//!     per the SSE spec, which the turn_event stream doesn't use; spec
//!     §Conventions: consumers MUST ignore unknown SSE lines).
//!   * a non-JSON / malformed error body → the client falls back to a generic
//!     error rather than panicking (spec §Error bodies: framework-level 400 may
//!     carry no JSON body).
//!
//! Loopback-real tier against `real_support::MockRuntimeServer`.

mod real_support;

use marsad_wasl_session::{
    RealWaslSessionProvider, WaslBusyPolicy, WaslCancelReason, WaslCausation, WaslSessionClient,
    WaslSessionClientConfig, WaslSessionError, WaslSessionEventKind, WaslSessionProvider,
    WaslStreamItem, WaslTargetKey, WaslTargetStatus, WaslTurnRequest,
};

use real_support::{
    compact_fixture, compact_json, sse_frame, val, BufferedResponse, EventsScript,
    MockRuntimeServer, SSE_BLANK,
};

/// Provision a client bound to `tgt-1` the spec-correct way (POST /v1/targets →
/// target.json), matching every `real_*.rs` file's `bound_client` helper.
fn bound_client(server: &MockRuntimeServer) -> Box<dyn WaslSessionClient> {
    server.push_create(BufferedResponse::json(200, compact_fixture("target.json")));
    let provider = RealWaslSessionProvider::new(WaslSessionClientConfig {
        base_url: server.base_url(),
    });
    let (client, _snapshot) = provider
        .get_or_start(&WaslTargetKey(val(r#""ctx-42""#)))
        .expect("create should succeed");
    client
}

/// A minimal valid `GET /v1/targets/{id}` snapshot body carrying an arbitrary
/// `status` object, per the wire spec's target shape (`target.json`) with only
/// the `status` field swapped. `capabilities`/`last_seq`/… are the spec's
/// documented fields; only `status` is under test here.
fn snapshot_with_status(status_json: &str) -> String {
    format!(
        r#"{{
            "target_id": "tgt-1", "key": "ctx-42",
            "capabilities": {{
                "persistent_process": true, "persistent_conversation": true,
                "structured_turns": true, "streaming_output": false, "attachable": true,
                "interruptible": false, "structured_provenance": true, "resumable": true
            }},
            "status": {status_json},
            "last_seq": 12, "queued": 0, "native_session_id": "ses_1"
        }}"#
    )
}

/// Drive one `status()` snapshot GET whose body carries `status_json` and return
/// the parsed `WaslTargetStatus`.
fn status_from(status_json: &str) -> WaslTargetStatus {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    server.push_target(BufferedResponse::json(
        200,
        snapshot_with_status(status_json),
    ));
    client.status()
}

// ===========================================================================
// WaslTargetStatus kinds with no vendored fixture.
// runtime-control-api.md: TargetStatus is internally-tagged snake_case
// ({"type":"idle",…}); plan §4.4 gives each variant's field shape.
// ===========================================================================

#[test]
fn status_parses_offline() {
    // `offline` is a bare unit variant (plan §4.4: `Offline`); wire tag "offline".
    assert_eq!(
        status_from(r#"{ "type": "offline" }"#),
        WaslTargetStatus::Offline
    );
}

#[test]
fn status_parses_starting() {
    // `starting` is a bare unit variant (plan §4.4: `Starting`); wire tag "starting".
    assert_eq!(
        status_from(r#"{ "type": "starting" }"#),
        WaslTargetStatus::Starting
    );
}

#[test]
fn status_parses_stopped() {
    // `stopped` is a bare unit variant (plan §4.4: `Stopped`); wire tag "stopped".
    assert_eq!(
        status_from(r#"{ "type": "stopped" }"#),
        WaslTargetStatus::Stopped
    );
}

#[test]
fn status_parses_interrupting_with_active_turn_and_since() {
    // Plan §4.4 (v3 correction): `Interrupting { active_turn, since }` — NOT a
    // bare unit variant. Wire tag "interrupting", carrying both fields.
    assert_eq!(
        status_from(
            r#"{ "type": "interrupting", "active_turn": "turn-7",
                 "since": "2026-07-02T12:00:03.000Z" }"#
        ),
        WaslTargetStatus::Interrupting {
            active_turn: "turn-7".to_string(),
            since: "2026-07-02T12:00:03.000Z".to_string(),
        }
    );
}

#[test]
fn status_parses_draining_with_active_turn_and_since() {
    // Plan §4.4 (v3 correction): `Draining { active_turn, since }` — NOT a bare
    // unit variant. Wire tag "draining", carrying both fields.
    assert_eq!(
        status_from(
            r#"{ "type": "draining", "active_turn": "turn-7",
                 "since": "2026-07-02T12:00:04.000Z" }"#
        ),
        WaslTargetStatus::Draining {
            active_turn: "turn-7".to_string(),
            since: "2026-07-02T12:00:04.000Z".to_string(),
        }
    );
}

#[test]
fn status_unknown_type_projects_to_forward_compat_unknown() {
    // runtime-control-api.md §Conventions: consumers MUST ignore unknown `type`
    // values; plan §4.4: an unrecognized status kind projects to the
    // forward-compat `Unknown(tag)` carrying the raw tag for logging. A future
    // Wasl could introduce e.g. "hibernating".
    assert_eq!(
        status_from(r#"{ "type": "hibernating", "since": "2026-07-02T12:00:05.000Z" }"#),
        WaslTargetStatus::Unknown("hibernating".to_string())
    );
}

// ===========================================================================
// WaslSessionEventKind kinds with no vendored turn_event fixture.
// runtime-control-api.md §Frame turn_event: the payload is a serialized R1
// TurnEvent (internally-tagged snake_case); plan §4.4 gives each field shape.
// Driven through attach() (the only public path that parses a turn_event).
// ===========================================================================

/// Run one hand-constructed `turn_event` payload through `attach` at SSE id 20
/// and return the single `WaslSessionEventKind` it maps to (mirrors
/// `real_attach.rs::single_event_kind`, but for a spec-derived literal rather
/// than a fixture).
fn single_event_kind_from_wire(payload: &str) -> WaslSessionEventKind {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    let mut lines = sse_frame("hello", None, &compact_fixture("hello.json"));
    lines.extend(sse_frame("turn_event", Some(20), &compact_json(payload)));
    server.push_events(EventsScript::new(lines).drop_after(true));

    let mut stream = client.attach(19);
    let mut kinds: Vec<WaslSessionEventKind> = Vec::new();
    while let Some(item) = stream.next() {
        match item {
            WaslStreamItem::Event(ev) => kinds.push(ev.kind),
            WaslStreamItem::Truncated { earliest, latest } => {
                panic!("unexpected Truncated{{{earliest},{latest}}}")
            }
        }
    }
    assert_eq!(kinds.len(), 1, "one event expected, got {kinds:?}");
    kinds.into_iter().next().unwrap()
}

#[test]
fn attach_maps_turn_queued() {
    // Plan §4.4 `TurnQueued { turn_id, position }`; spec turn_event variant
    // "turn_queued".
    assert_eq!(
        single_event_kind_from_wire(
            r#"{ "type": "turn_queued", "turn_id": "turn-8", "position": 2 }"#
        ),
        WaslSessionEventKind::TurnQueued {
            turn_id: "turn-8".to_string(),
            position: 2,
        }
    );
}

#[test]
fn attach_maps_tool_finished() {
    // Plan §4.4 `ToolFinished { turn_id, name, ok }` + the additive `result`
    // (Wasl `TurnEvent::ToolFinished.result`); spec turn_event variant
    // "tool_finished". (The shipped fixture only covers `tool_started`.) A
    // wire frame WITHOUT `result` maps to `result: None` (driver that reports
    // only success/failure).
    assert_eq!(
        single_event_kind_from_wire(
            r#"{ "type": "tool_finished", "turn_id": "turn-7", "name": "ask_wasl", "ok": true }"#
        ),
        WaslSessionEventKind::ToolFinished {
            turn_id: "turn-7".to_string(),
            name: "ask_wasl".to_string(),
            ok: true,
            result: None,
        }
    );
    // A wire frame WITH `result` carries it through.
    assert_eq!(
        single_event_kind_from_wire(
            r#"{ "type": "tool_finished", "turn_id": "turn-7", "name": "ask_wasl", "ok": true, "result": "done" }"#
        ),
        WaslSessionEventKind::ToolFinished {
            turn_id: "turn-7".to_string(),
            name: "ask_wasl".to_string(),
            ok: true,
            result: Some(serde_json::json!("done")),
        }
    );
}

#[test]
fn attach_maps_native_gate_resolved() {
    // Plan §4.4 `NativeGateResolved { turn_id, decision }` — `decision` is a
    // plain relay String (Marsad renders, does not branch). Spec turn_event
    // variant "native_gate_resolved".
    assert_eq!(
        single_event_kind_from_wire(
            r#"{ "type": "native_gate_resolved", "turn_id": "turn-7", "decision": "approve" }"#
        ),
        WaslSessionEventKind::NativeGateResolved {
            turn_id: "turn-7".to_string(),
            decision: "approve".to_string(),
        }
    );
}

#[test]
fn attach_maps_turn_cancelled() {
    // Plan §4.4 `TurnCancelled { turn_id, reason }` — `reason` a relay String.
    // Spec turn_event variant "turn_cancelled".
    assert_eq!(
        single_event_kind_from_wire(
            r#"{ "type": "turn_cancelled", "turn_id": "turn-7", "reason": "superseded" }"#
        ),
        WaslSessionEventKind::TurnCancelled {
            turn_id: "turn-7".to_string(),
            reason: "superseded".to_string(),
        }
    );
}

#[test]
fn attach_maps_turn_expired() {
    // Plan §4.4 `TurnExpired { turn_id }`; spec turn_event variant "turn_expired".
    assert_eq!(
        single_event_kind_from_wire(r#"{ "type": "turn_expired", "turn_id": "turn-9" }"#),
        WaslSessionEventKind::TurnExpired {
            turn_id: "turn-9".to_string(),
        }
    );
}

#[test]
fn attach_maps_status_changed_carrying_a_target_status() {
    // Plan §4.4 `StatusChanged { status }` — the payload nests a full
    // TargetStatus (internally-tagged), so this also proves the nested-status
    // translation inside an event. Spec turn_event variant "status_changed".
    assert_eq!(
        single_event_kind_from_wire(
            r#"{ "type": "status_changed",
                 "status": { "type": "idle", "since": "2026-07-02T12:00:06.000Z" } }"#
        ),
        WaslSessionEventKind::StatusChanged {
            status: WaslTargetStatus::Idle {
                since: "2026-07-02T12:00:06.000Z".to_string(),
            },
        }
    );
}

// ===========================================================================
// turn_accepted with an AGENT source (fixtures only exercise local_human).
// runtime-control-api.md §POST …/turns: `source` is the R1 `TurnSource`
// = {"type":"agent","from":"…"} | {"type":"local_human"}. The turn_accepted
// event carries the same TurnSource shape (fixture turn_event_turn_accepted.json
// shows {"source":{"type":"local_human"}}).
// ===========================================================================

#[test]
fn attach_maps_turn_accepted_agent_source() {
    // The caller-mirror keeps `source` a plain relay String; an agent source
    // {"type":"agent","from":"wasl-b"} must still parse (Marsad renders it). This
    // asserts only the variant + turn_id; the exact relayed String form is
    // implementation-relay detail, so we assert the event is a TurnAccepted for
    // the right turn and carries a non-empty source relay.
    let kind = single_event_kind_from_wire(
        r#"{ "type": "turn_accepted", "turn_id": "turn-7",
             "source": { "type": "agent", "from": "wasl-b" } }"#,
    );
    match kind {
        WaslSessionEventKind::TurnAccepted { turn_id, source } => {
            assert_eq!(turn_id, "turn-7");
            assert!(
                source.contains("agent"),
                "agent source must be relayed (mentions the agent tag), got {source:?}"
            );
        }
        other => panic!("expected TurnAccepted for an agent source, got {other:?}"),
    }
}

// ===========================================================================
// WaslBusyPolicy::QueueRelated wire form (fixtures/tests only cover Queue/Reject).
// runtime-control-api.md §POST …/turns busy_policy; plan §4.4
// `QueueRelated { max_wait_ms }`. Internally-tagged {"type":"queue_related",…}.
// ===========================================================================

fn sample_request(root: &str, text: &str) -> WaslTurnRequest {
    WaslTurnRequest {
        idempotency_key: "opt-key-qr".to_string(),
        text: text.to_string(),
        causation: WaslCausation::fresh(root.to_string()),
    }
}

#[test]
fn prompt_queue_related_policy_serializes_queue_related_wire_form() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    server.push_turns(BufferedResponse::json(
        200,
        compact_fixture("receipt_started.json"),
    ));

    let _ = client.prompt(
        sample_request("req-qr", "hi"),
        WaslBusyPolicy::QueueRelated { max_wait_ms: 5000 },
    );

    let turns = server.requests_to("POST", "/v1/targets/tgt-1/turns");
    assert_eq!(turns.len(), 1, "one turns POST");
    // Spec busy_policy form for the related-queue policy: tag "queue_related"
    // with an integer max_wait_ms (durations on the wire are integer ms).
    assert_eq!(
        turns[0].body_json().get("busy_policy"),
        Some(&val(r#"{ "type": "queue_related", "max_wait_ms": 5000 }"#)),
        "QueueRelated must serialize to the queue_related wire form"
    );
}

// ===========================================================================
// WaslCancelReason::Superseded wire form (real_control.rs only covers Human).
// runtime-control-api.md §POST …/cancel: `reason` ∈ the R1 CancelReason set.
// ===========================================================================

#[test]
fn interrupt_superseded_serializes_a_reason_distinct_from_human() {
    // Human maps to the wire's human/operator-request reason (real_control.rs);
    // Superseded is a distinct CancelReason. We assert Superseded produces a
    // (non-empty) reason on the wire that differs from Human's — the exact
    // relayed string is a wire-mapping detail, but the two reasons must not
    // collapse to the same value.
    let human_reason = {
        let server = MockRuntimeServer::start();
        let client = bound_client(&server);
        server.push_cancel(BufferedResponse::empty(204));
        client.interrupt(WaslCancelReason::Human).expect("204 → Ok");
        let body = server.requests_to("POST", "/v1/targets/tgt-1/cancel")[0].body_json();
        body.get("reason")
            .and_then(|v| v.as_str())
            .expect("cancel body carries a string reason")
            .to_string()
    };

    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    server.push_cancel(BufferedResponse::empty(204));
    client
        .interrupt(WaslCancelReason::Superseded)
        .expect("204 → Ok");

    let body = server.requests_to("POST", "/v1/targets/tgt-1/cancel")[0].body_json();
    let superseded_reason = body
        .get("reason")
        .and_then(|v| v.as_str())
        .expect("cancel body carries a string reason for Superseded");

    assert!(
        !superseded_reason.is_empty(),
        "Superseded must serialize a non-empty reason, got {body}"
    );
    assert_ne!(
        superseded_reason, human_reason,
        "Superseded and Human must map to distinct cancel reasons on the wire"
    );
}

// ===========================================================================
// SSE parser tolerates an unmodelled/extra SSE field (a `retry:` line).
// runtime-control-api.md §Conventions: consumers MUST ignore unknown SSE lines;
// §Frame turn_event uses only event:/id:/data:. A `retry:` line (standard SSE,
// not used by the turn_event stream) must be skipped without corrupting frames.
// ===========================================================================

#[test]
fn attach_tolerates_retry_line_without_corrupting_frames() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);

    let mut lines = sse_frame("hello", None, &compact_fixture("hello.json"));
    // An unmodelled `retry:` field (SSE reconnection hint) interleaved before a
    // real turn_event. It carries no frame semantics for this stream and must be
    // skipped, leaving the following frame parsed correctly.
    lines.push("retry: 3000\n".to_string());
    lines.push(SSE_BLANK.to_string());
    lines.extend(sse_frame(
        "turn_event",
        Some(21),
        &compact_fixture("turn_event_assistant_delta.json"),
    ));
    server.push_events(EventsScript::new(lines).drop_after(true));

    let mut stream = client.attach(20);
    let mut items = Vec::new();
    while let Some(it) = stream.next() {
        items.push(it);
    }

    // Exactly the one real event survives; the retry line neither errored nor
    // produced a spurious item.
    assert_eq!(
        items.len(),
        1,
        "retry line skipped, one event remains: {items:?}"
    );
    match &items[0] {
        WaslStreamItem::Event(ev) => {
            assert_eq!(ev.seq, 21);
            assert_eq!(
                ev.kind,
                WaslSessionEventKind::AssistantDelta {
                    turn_id: "turn-7".to_string(),
                    text: "PON".to_string(),
                }
            );
        }
        other => panic!("expected the surviving assistant_delta event, got {other:?}"),
    }
}

// ===========================================================================
// Malformed / non-JSON error body → generic error, never a panic.
// runtime-control-api.md §Error bodies: a framework-level 400 may carry NO JSON
// body; the error `detail` is free-form and NOT part of the contract, so a
// consumer must not depend on parsing it and must degrade gracefully.
// ===========================================================================

#[test]
fn control_op_non_json_error_body_falls_back_to_generic_error() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    // A 400 whose body is NOT JSON at all (plain text) — the spec explicitly
    // permits a framework-level 400 with no JSON body. The client must not panic
    // trying to parse it; it must surface a WaslSessionError.
    server.push_gate(BufferedResponse::json(400, "Bad Request — not json at all"));

    let err = client
        .resolve_native_gate(
            &marsad_wasl_session::WaslTurnId("turn-7".to_string()),
            marsad_wasl_session::WaslGateDecision::Approve,
        )
        .expect_err("a 4xx is an error");
    // Any WaslSessionError is acceptable (the disposition, not the exact detail,
    // is what matters) — the point is a graceful fallback, not a panic. Match the
    // enum exhaustively so it stays honest if the error taxonomy changes.
    match err {
        WaslSessionError::Io(_)
        | WaslSessionError::Protocol(_)
        | WaslSessionError::Unavailable(_) => {}
    }
}

#[test]
fn control_op_empty_error_body_falls_back_to_generic_error() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    // A 400 with a completely empty body (the framework-level-400 case). Still an
    // error, still no panic.
    server.push_gate(BufferedResponse::empty(400));

    let err = client
        .resolve_native_gate(
            &marsad_wasl_session::WaslTurnId("turn-7".to_string()),
            marsad_wasl_session::WaslGateDecision::Approve,
        )
        .expect_err("a 4xx is an error");
    match err {
        WaslSessionError::Io(_)
        | WaslSessionError::Protocol(_)
        | WaslSessionError::Unavailable(_) => {}
    }
}
