//! fakes.rs — `FakeWaslSessionClient` / `FakeWaslSessionProvider` conformance
//! (plan §5, test plan §10.1 "fake conformance"): every `WaslSessionClient`
//! method honors its contract against the in-memory double —
//!
//! * `prompt` returns the scripted receipt (Started / Queued / RejectedBusy /
//!   RejectedUnavailable), defaulting to `Started` when nothing is scripted;
//! * `status` returns the scripted status;
//! * `attach` drains scripted events honoring `after_seq` (replay skips earlier
//!   seqs, no gaps / dupes);
//! * `resolve_native_gate` / `interrupt` / `shutdown` record their arguments in
//!   inspectable logs and return the scripted ack / error;
//! * `stop` / drop are idempotent.
//!
//! Also proves `WaslSessionClient` / `WaslSessionProvider` are object-safe
//! (`Box<dyn …>`), the seam the agent's registry stores — mirroring
//! `marsad-wasl/tests/fakes.rs`, its structural analogue.
//!
//! This file WILL fail to compile until a later worker implements
//! `marsad_wasl_session`'s public surface (§4/§5) — expected tests-first signal.
mod support;

use marsad_wasl_session::{
    FakeWaslSessionClient, WaslCancelReason, WaslGateDecision, WaslNativeGateDetail,
    WaslSessionClient, WaslSessionError, WaslSessionEventKind, WaslShutdownMode, WaslStreamItem,
    WaslTargetStatus, WaslTurnId, WaslTurnReceipt,
};

use support::{all_caps, drain, drain_items, idle_status, sample_request};

// ---------------------------------------------------------------------------
// prompt: returns the scripted receipt FIFO; default Started when exhausted.
// ---------------------------------------------------------------------------

#[test]
fn prompt_defaults_to_started_when_nothing_scripted() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());
    let receipt = fake.prompt(sample_request("k1", "hello"), support::queue_policy());
    match receipt {
        WaslTurnReceipt::Started { .. } => {}
        other => panic!("default receipt must be Started, got {other:?}"),
    }
}

#[test]
fn prompt_returns_scripted_receipts_in_fifo_order_then_falls_back_to_default() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());
    fake.script_receipt(WaslTurnReceipt::Queued {
        turn_id: "t1".to_string(),
        position: 2,
    });
    fake.script_receipt(WaslTurnReceipt::RejectedBusy {
        active_turn: "t0".to_string(),
    });

    assert_eq!(
        fake.prompt(sample_request("k1", "a"), support::queue_policy()),
        WaslTurnReceipt::Queued {
            turn_id: "t1".to_string(),
            position: 2
        }
    );
    assert_eq!(
        fake.prompt(sample_request("k2", "b"), support::reject_policy()),
        WaslTurnReceipt::RejectedBusy {
            active_turn: "t0".to_string()
        }
    );
    // Queue exhausted → back to the Started default.
    match fake.prompt(sample_request("k3", "c"), support::queue_policy()) {
        WaslTurnReceipt::Started { .. } => {}
        other => panic!("expected Started default once script is drained, got {other:?}"),
    }
}

#[test]
fn prompt_records_every_request_verbatim_in_order() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());
    fake.prompt(sample_request("k1", "first"), support::queue_policy());
    fake.prompt(sample_request("k2", "second"), support::reject_policy());

    let log = fake.prompts.lock().unwrap().clone();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].idempotency_key, "k1");
    assert_eq!(log[0].text, "first");
    assert_eq!(log[1].idempotency_key, "k2");
    assert_eq!(log[1].text, "second");
}

// ---------------------------------------------------------------------------
// duplicate resend (v3 §4.4): a resent idempotency key returns the ORIGINAL
// receipt wrapped verbatim in `Duplicate{original: Box<WaslTurnReceipt>}` — no
// second injection. This proves the fake's `script_receipt` scripting API can
// carry (and be tested against) the new `Duplicate` variant: `WaslTurnReceipt`
// is just data, so no INTERNAL dedup logic is needed in the fake — the test
// scripts the `Duplicate` outcome directly.
// ---------------------------------------------------------------------------

#[test]
fn prompt_can_be_scripted_to_return_duplicate_wrapping_the_original() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());

    // The original admission (as if the first send of this idempotency key).
    let original = WaslTurnReceipt::Started {
        turn_id: "t-orig".to_string(),
        first_seq: 1,
    };
    // A resend of the same idempotency key returns the wrapped original.
    fake.script_receipt(WaslTurnReceipt::Duplicate {
        original: Box::new(original.clone()),
    });

    let receipt = fake.prompt(sample_request("k-dup", "resent"), support::queue_policy());
    match receipt {
        WaslTurnReceipt::Duplicate { original: got } => {
            assert_eq!(
                *got, original,
                "Duplicate must wrap the ORIGINAL receipt verbatim"
            );
        }
        other => panic!("expected Duplicate wrapping the original, got {other:?}"),
    }
}

#[test]
fn duplicate_can_wrap_any_receipt_kind_including_queued() {
    // The wrapped original can be any receipt kind, not just Started — a resend
    // of a key whose original turn was Queued returns Duplicate{Queued}.
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());
    let original = WaslTurnReceipt::Queued {
        turn_id: "t-q".to_string(),
        position: 2,
    };
    fake.script_receipt(WaslTurnReceipt::Duplicate {
        original: Box::new(original.clone()),
    });

    match fake.prompt(sample_request("k-dup2", "resent"), support::queue_policy()) {
        WaslTurnReceipt::Duplicate { original: got } => assert_eq!(*got, original),
        other => panic!("expected Duplicate{{Queued}}, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// status: returns whatever was scripted last.
// ---------------------------------------------------------------------------

#[test]
fn status_returns_the_scripted_status() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());
    fake.script_status(idle_status());
    assert_eq!(fake.status(), idle_status());

    fake.script_status(WaslTargetStatus::Offline);
    assert_eq!(fake.status(), WaslTargetStatus::Offline);
}

// ---------------------------------------------------------------------------
// target_id / capabilities: constant identity + honest cap set.
// ---------------------------------------------------------------------------

#[test]
fn target_id_and_capabilities_are_stable() {
    let caps = all_caps();
    let fake = FakeWaslSessionClient::new("wasl-target-7", caps.clone());
    assert_eq!(fake.target_id(), "wasl-target-7");
    assert_eq!(fake.capabilities(), caps);
}

// ---------------------------------------------------------------------------
// attach: drains scripted events, honoring after_seq (skip earlier seqs).
// ---------------------------------------------------------------------------

#[test]
fn attach_drains_scripted_events_in_order_from_seq_zero() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());
    fake.script_event(WaslSessionEventKind::TurnAccepted {
        turn_id: "t1".to_string(),
        source: "LocalHuman".to_string(),
    });
    fake.script_event(WaslSessionEventKind::AssistantDelta {
        turn_id: "t1".to_string(),
        text: "hi".to_string(),
    });
    fake.script_event(WaslSessionEventKind::TurnCompleted {
        turn_id: "t1".to_string(),
        reply: "hi".to_string(),
        certainty: "committed".to_string(),
    });

    let mut stream = fake.attach(0);
    let events = drain(&mut *stream);

    let kinds: Vec<&WaslSessionEventKind> = events.iter().map(|e| &e.kind).collect();
    assert!(matches!(
        kinds[0],
        WaslSessionEventKind::TurnAccepted { .. }
    ));
    assert!(matches!(
        kinds[1],
        WaslSessionEventKind::AssistantDelta { .. }
    ));
    assert!(matches!(
        kinds[2],
        WaslSessionEventKind::TurnCompleted { .. }
    ));
    // Sequences are monotonically increasing with no duplicate.
    for w in events.windows(2) {
        assert!(
            w[1].seq > w[0].seq,
            "seq must strictly increase: {events:?}"
        );
    }
}

#[test]
fn attach_honors_after_seq_skipping_earlier_events() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());
    // Five events, seq 1..=5.
    for i in 1..=5 {
        fake.script_event(WaslSessionEventKind::AssistantDelta {
            turn_id: "t1".to_string(),
            text: format!("d{i}"),
        });
    }

    let mut stream = fake.attach(2);
    let events = drain(&mut *stream);

    let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, vec![3, 4, 5], "after_seq=2 must yield exactly 3..=5");
}

// ---------------------------------------------------------------------------
// truncated stream item (v3 §4.4): the fake can be scripted to emit a
// WaslStreamItem::Truncated{earliest, latest} — simulating the client's cursor
// falling out of the retention window. It surfaces DISTINCTLY from a normal
// Event (never folded into an event or into Unknown), and after a Truncated a
// fresh attach from a new after_seq (e.g. resuming from `latest`) works.
// ---------------------------------------------------------------------------

#[test]
fn attach_yields_truncated_as_a_distinct_stream_item() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());
    // Some real events, then a truncation control item.
    fake.script_event(WaslSessionEventKind::AssistantDelta {
        turn_id: "t1".to_string(),
        text: "d1".to_string(),
    });
    fake.script_truncated(5, 42);

    let mut stream = fake.attach(0);
    let items = drain_items(&mut *stream);

    // The truncation is present as a DISTINCT item, never an Event / Unknown.
    let truncations: Vec<(u64, u64)> = items
        .iter()
        .filter_map(|it| match it {
            WaslStreamItem::Truncated { earliest, latest } => Some((*earliest, *latest)),
            WaslStreamItem::Event(_) => None,
        })
        .collect();
    assert_eq!(
        truncations,
        vec![(5, 42)],
        "the scripted Truncated{{earliest:5, latest:42}} must surface distinctly"
    );
    // A normal event is still an Event (the two are never conflated).
    assert!(items
        .iter()
        .any(|it| matches!(it, WaslStreamItem::Event(ev) if matches!(ev.kind, WaslSessionEventKind::AssistantDelta { .. }))));
}

#[test]
fn a_fresh_attach_from_latest_after_a_truncation_works() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());
    // Events at seq 1..=3, then a truncation, then post-truncation events 4..=5.
    for i in 1..=3 {
        fake.script_event(WaslSessionEventKind::AssistantDelta {
            turn_id: "t1".to_string(),
            text: format!("d{i}"),
        });
    }
    // The client's cursor fell below the retained floor: earliest=2, latest=3.
    fake.script_truncated(2, 3);
    for i in 4..=5 {
        fake.script_event(WaslSessionEventKind::AssistantDelta {
            turn_id: "t1".to_string(),
            text: format!("d{i}"),
        });
    }

    // First attach observes the Truncated and reads its `latest`.
    let mut first = fake.attach(0);
    let items = drain_items(&mut *first);
    let latest = items
        .iter()
        .find_map(|it| match it {
            WaslStreamItem::Truncated { latest, .. } => Some(*latest),
            WaslStreamItem::Event(_) => None,
        })
        .expect("a Truncated item must be present");
    assert_eq!(latest, 3);
    drop(first);

    // Resubscribe from `latest` (the re-snapshot + resubscribe signal): yields
    // exactly the post-truncation tail, no dup / gap.
    let mut resumed = fake.attach(latest);
    let seqs: Vec<u64> = drain(&mut *resumed).iter().map(|e| e.seq).collect();
    assert_eq!(
        seqs,
        vec![4, 5],
        "resuming from `latest` after a truncation must yield exactly the post-truncation tail"
    );
}

// ---------------------------------------------------------------------------
// resolve_native_gate / interrupt / shutdown: record + return scripted result.
// ---------------------------------------------------------------------------

#[test]
fn resolve_native_gate_records_turn_id_and_decision_and_defaults_ok() {
    // v3 §8: resolution is keyed by the blocked turn's `turn_id` (the real gate
    // has NO gate id), so the fake's log records `(turn_id, decision)`.
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());
    fake.resolve_native_gate(&WaslTurnId("turn-1".to_string()), WaslGateDecision::Approve)
        .expect("default resolve must be Ok");
    fake.resolve_native_gate(
        &WaslTurnId("turn-2".to_string()),
        WaslGateDecision::Deny {
            reason: "no".to_string(),
        },
    )
    .expect("default resolve must be Ok");

    let log = fake.resolved_gates.lock().unwrap().clone();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].0, "turn-1");
    assert_eq!(log[0].1, WaslGateDecision::Approve);
    assert_eq!(log[1].0, "turn-2");
    assert_eq!(
        log[1].1,
        WaslGateDecision::Deny {
            reason: "no".to_string()
        }
    );
}

#[test]
fn interrupt_records_reason_and_defaults_ok() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());
    fake.interrupt(WaslCancelReason::Human).expect("default Ok");
    fake.interrupt(WaslCancelReason::Superseded)
        .expect("default Ok");

    let log = fake.interrupts.lock().unwrap().clone();
    assert_eq!(
        log,
        vec![WaslCancelReason::Human, WaslCancelReason::Superseded]
    );
}

#[test]
fn shutdown_records_mode_and_defaults_ok() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());
    fake.shutdown(WaslShutdownMode::Drain).expect("default Ok");
    fake.shutdown(WaslShutdownMode::Immediate)
        .expect("default Ok");

    let log = fake.shutdowns.lock().unwrap().clone();
    assert_eq!(
        log,
        vec![WaslShutdownMode::Drain, WaslShutdownMode::Immediate]
    );
}

// ---------------------------------------------------------------------------
// script_error: errors the NEXT recording call (FIFO), still records attempt.
// ---------------------------------------------------------------------------

#[test]
fn script_error_surfaces_on_next_resolve_but_attempt_is_still_recorded() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());
    fake.script_error(WaslSessionError::Protocol("boom".to_string()));

    let err = fake
        .resolve_native_gate(&WaslTurnId("turn-1".to_string()), WaslGateDecision::Approve)
        .expect_err("scripted Err must surface");
    assert!(matches!(err, WaslSessionError::Protocol(_)));

    // The attempt is recorded even though the ack was an error.
    let log = fake.resolved_gates.lock().unwrap().clone();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].0, "turn-1");
}

#[test]
fn script_error_surfaces_on_interrupt_and_shutdown_too() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());
    fake.script_error(WaslSessionError::Unavailable("down".to_string()));
    let err = fake
        .interrupt(WaslCancelReason::Human)
        .expect_err("scripted Err must surface on interrupt");
    assert!(matches!(err, WaslSessionError::Unavailable(_)));

    fake.script_error(WaslSessionError::Io("io".to_string()));
    let err = fake
        .shutdown(WaslShutdownMode::Immediate)
        .expect_err("scripted Err must surface on shutdown");
    assert!(matches!(err, WaslSessionError::Io(_)));
}

// ---------------------------------------------------------------------------
// idempotent shutdown / drop: a second shutdown does not poison the fake.
// ---------------------------------------------------------------------------

#[test]
fn shutdown_is_idempotent_and_does_not_poison_the_fake() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());
    fake.shutdown(WaslShutdownMode::Immediate).unwrap();
    fake.shutdown(WaslShutdownMode::Immediate).unwrap();
    // Still usable afterwards.
    match fake.prompt(sample_request("k", "still works"), support::queue_policy()) {
        WaslTurnReceipt::Started { .. } => {}
        other => panic!("fake must remain usable after repeated shutdown, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// richer event vocabulary (v3 §4.4): every real WaslSessionEventKind — the five
// NEW kinds (TurnQueued, NativeGateResolved, TurnCancelled, TurnExpired,
// StatusChanged) plus the turn_id/certainty additions on the existing kinds —
// constructs with its exact v3 shape and serde round-trips identically. This is
// the construction/round-trip proof §10.1 pins; the synthetic ObserveLost /
// Unknown have no real-Wasl counterpart (client-only, by design) but must still
// round-trip.
// ---------------------------------------------------------------------------

/// Serialize → deserialize an event and assert it is byte-identical.
fn round_trip(kind: WaslSessionEventKind) {
    let ev = marsad_wasl_session::WaslSessionEvent { seq: 1, kind };
    let json = serde_json::to_string(&ev).expect("serialize");
    let back: marsad_wasl_session::WaslSessionEvent =
        serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, ev, "event must round-trip identically: {json}");
}

#[test]
fn every_event_kind_constructs_with_its_v3_shape_and_round_trips() {
    // Existing kinds — now with turn_id / certainty per v3.
    round_trip(WaslSessionEventKind::SessionReady {
        native: Some("claude".to_string()),
    });
    round_trip(WaslSessionEventKind::TurnAccepted {
        turn_id: "t1".to_string(),
        source: "LocalHuman".to_string(),
    });
    round_trip(WaslSessionEventKind::AssistantDelta {
        turn_id: "t1".to_string(),
        text: "hello".to_string(),
    });
    round_trip(WaslSessionEventKind::ToolStarted {
        turn_id: "t1".to_string(),
        name: "bash".to_string(),
        input: serde_json::json!({ "command": "ls" }),
    });
    round_trip(WaslSessionEventKind::ToolFinished {
        turn_id: "t1".to_string(),
        name: "bash".to_string(),
        ok: true,
        result: Some(serde_json::json!("file1\nfile2")),
    });
    round_trip(WaslSessionEventKind::NativeGateRequested {
        turn_id: "t1".to_string(),
        detail: WaslNativeGateDetail {
            summary: "s".to_string(),
            preview: "p".to_string(),
        },
    });
    round_trip(WaslSessionEventKind::TurnCompleted {
        turn_id: "t1".to_string(),
        reply: "done".to_string(),
        certainty: "committed".to_string(),
    });
    round_trip(WaslSessionEventKind::TurnFailed {
        turn_id: "t1".to_string(),
        error: "boom".to_string(),
        certainty: "unknown".to_string(),
    });
    // The five NEW kinds.
    round_trip(WaslSessionEventKind::TurnQueued {
        turn_id: "t1".to_string(),
        position: 3,
    });
    round_trip(WaslSessionEventKind::NativeGateResolved {
        turn_id: "t1".to_string(),
        decision: "approved".to_string(),
    });
    round_trip(WaslSessionEventKind::TurnCancelled {
        turn_id: "t1".to_string(),
        reason: "human".to_string(),
    });
    round_trip(WaslSessionEventKind::TurnExpired {
        turn_id: "t1".to_string(),
    });
    round_trip(WaslSessionEventKind::StatusChanged {
        status: idle_status(),
    });
    // Terminal + Marsad-synthesized kinds.
    round_trip(WaslSessionEventKind::SessionExited { code: Some(0) });
    round_trip(WaslSessionEventKind::ObserveLost {
        error: "reset".to_string(),
    });
    round_trip(WaslSessionEventKind::Unknown("SomeFutureKind".to_string()));
}

#[test]
fn the_five_new_event_kinds_carry_their_exact_fields() {
    // TurnQueued{turn_id, position}
    match (WaslSessionEventKind::TurnQueued {
        turn_id: "t".to_string(),
        position: 7,
    }) {
        WaslSessionEventKind::TurnQueued { turn_id, position } => {
            assert_eq!(turn_id, "t");
            assert_eq!(position, 7);
        }
        _ => unreachable!(),
    }
    // NativeGateResolved{turn_id, decision}
    match (WaslSessionEventKind::NativeGateResolved {
        turn_id: "t".to_string(),
        decision: "denied".to_string(),
    }) {
        WaslSessionEventKind::NativeGateResolved { turn_id, decision } => {
            assert_eq!(turn_id, "t");
            assert_eq!(decision, "denied");
        }
        _ => unreachable!(),
    }
    // TurnCancelled{turn_id, reason}
    match (WaslSessionEventKind::TurnCancelled {
        turn_id: "t".to_string(),
        reason: "superseded".to_string(),
    }) {
        WaslSessionEventKind::TurnCancelled { turn_id, reason } => {
            assert_eq!(turn_id, "t");
            assert_eq!(reason, "superseded");
        }
        _ => unreachable!(),
    }
    // TurnExpired{turn_id}
    match (WaslSessionEventKind::TurnExpired {
        turn_id: "t".to_string(),
    }) {
        WaslSessionEventKind::TurnExpired { turn_id } => assert_eq!(turn_id, "t"),
        _ => unreachable!(),
    }
    // StatusChanged{status}
    match (WaslSessionEventKind::StatusChanged {
        status: busy_status_local(),
    }) {
        WaslSessionEventKind::StatusChanged { status } => {
            assert!(matches!(status, WaslTargetStatus::Busy { .. }));
        }
        _ => unreachable!(),
    }
}

/// Local busy status for the StatusChanged shape assertion.
fn busy_status_local() -> WaslTargetStatus {
    WaslTargetStatus::Busy {
        active_turn: "t".to_string(),
        source: "LocalHuman".to_string(),
        phase: "generating".to_string(),
        since: "x".to_string(),
        queued: 0,
    }
}

// ---------------------------------------------------------------------------
// object safety: the agent registry stores Box<dyn WaslSessionClient>.
// ---------------------------------------------------------------------------

#[test]
fn fake_client_is_usable_as_a_boxed_trait_object() {
    let fake = FakeWaslSessionClient::new("wasl-target-9", all_caps());
    let prompts = fake.prompts.clone();

    let boxed: Box<dyn WaslSessionClient> = Box::new(fake);
    assert_eq!(boxed.target_id(), "wasl-target-9");
    let _ = boxed.prompt(
        sample_request("k", "via trait object"),
        support::queue_policy(),
    );

    let log = prompts.lock().unwrap().clone();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].text, "via trait object");

    boxed.shutdown(WaslShutdownMode::Immediate).unwrap();
    boxed.shutdown(WaslShutdownMode::Immediate).unwrap(); // idempotent through the object too
}

// ---------------------------------------------------------------------------
// A native gate detail round-trips through the log unchanged (used by §8).
// ---------------------------------------------------------------------------

#[test]
fn native_gate_detail_is_carried_verbatim_on_the_event_stream() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());
    // v3 §4.4: NativeGateRequested carries {turn_id, detail}; detail is only
    // {summary, preview} (no gate_id/tool).
    let detail = WaslNativeGateDetail {
        summary: "run rm -rf".to_string(),
        preview: "$ rm -rf /tmp/x".to_string(),
    };
    fake.script_event(WaslSessionEventKind::NativeGateRequested {
        turn_id: "t-gate".to_string(),
        detail: detail.clone(),
    });

    let mut stream = fake.attach(0);
    let events = drain(&mut *stream);
    assert_eq!(events.len(), 1);
    match &events[0].kind {
        WaslSessionEventKind::NativeGateRequested {
            turn_id,
            detail: got,
        } => {
            assert_eq!(turn_id, "t-gate");
            assert_eq!(*got, detail);
        }
        other => panic!("expected NativeGateRequested, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Arc forwarding: the blanket impl makes Arc<T> itself a WaslSessionClient so a
// shared handle satisfies the trait bound. Exercise all 8 methods through the
// Arc and confirm each delegates to the inner fake (via its shared logs /
// scripted state), then confirm the ?Sized case (Arc<dyn WaslSessionClient>).
// ---------------------------------------------------------------------------

#[test]
fn arc_forwards_every_method_to_the_inner_client() {
    use std::sync::Arc;

    let fake = FakeWaslSessionClient::new("wasl-target-arc", all_caps());
    // Clone the shared log handles BEFORE wrapping — Arc clones share them, so
    // we can observe the inner fake's state after forwarding through the Arc.
    let prompts = fake.prompts.clone();
    let resolved_gates = fake.resolved_gates.clone();
    let interrupts = fake.interrupts.clone();
    let shutdowns = fake.shutdowns.clone();

    // Script the read-side responses so we can assert forwarding returns them.
    fake.script_status(idle_status());
    fake.script_receipt(WaslTurnReceipt::Queued {
        turn_id: "t-arc".to_string(),
        position: 1,
    });
    fake.script_event(WaslSessionEventKind::AssistantDelta {
        turn_id: "t-arc".to_string(),
        text: "d1".to_string(),
    });
    fake.script_event(WaslSessionEventKind::AssistantDelta {
        turn_id: "t-arc".to_string(),
        text: "d2".to_string(),
    });

    let caps = all_caps();
    let client: Arc<FakeWaslSessionClient> = Arc::new(fake);

    // 1. target_id — forwarded &str.
    assert_eq!(client.target_id(), "wasl-target-arc");
    // 2. capabilities — forwarded value.
    assert_eq!(client.capabilities(), caps);
    // 3. status — forwarded scripted status.
    assert_eq!(client.status(), idle_status());
    // 4. prompt — forwarded scripted receipt; request recorded on inner log.
    assert_eq!(
        client.prompt(sample_request("k-arc", "via arc"), support::queue_policy()),
        WaslTurnReceipt::Queued {
            turn_id: "t-arc".to_string(),
            position: 1
        }
    );
    // 5. attach — forwarded, honoring after_seq against the inner event log.
    let mut stream = client.attach(1);
    let events = drain(&mut *stream);
    let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
    assert_eq!(
        seqs,
        vec![2],
        "after_seq=1 must yield exactly seq 2 through Arc"
    );
    // 6. resolve_native_gate — forwarded, recorded on inner log (keyed by turn_id).
    client
        .resolve_native_gate(
            &WaslTurnId("turn-arc".to_string()),
            WaslGateDecision::Approve,
        )
        .expect("forwarded resolve must be Ok");
    // 7. interrupt — forwarded, recorded on inner log.
    client
        .interrupt(WaslCancelReason::Human)
        .expect("forwarded interrupt must be Ok");
    // 8. shutdown — forwarded, recorded on inner log.
    client
        .shutdown(WaslShutdownMode::Drain)
        .expect("forwarded shutdown must be Ok");

    // Each write-side call reached the SAME inner fake.
    let prompt_log = prompts.lock().unwrap().clone();
    assert_eq!(prompt_log.len(), 1);
    assert_eq!(prompt_log[0].idempotency_key, "k-arc");
    assert_eq!(prompt_log[0].text, "via arc");

    let gate_log = resolved_gates.lock().unwrap().clone();
    assert_eq!(gate_log.len(), 1);
    assert_eq!(gate_log[0].0, "turn-arc");
    assert_eq!(gate_log[0].1, WaslGateDecision::Approve);

    assert_eq!(*interrupts.lock().unwrap(), vec![WaslCancelReason::Human]);
    assert_eq!(*shutdowns.lock().unwrap(), vec![WaslShutdownMode::Drain]);
}

#[test]
fn arc_dyn_client_is_usable_as_a_trait_object_via_the_blanket_impl() {
    use std::sync::Arc;

    let fake = FakeWaslSessionClient::new("wasl-target-arc-dyn", all_caps());
    let prompts = fake.prompts.clone();
    let shutdowns = fake.shutdowns.clone();

    // The ?Sized case: Arc<dyn WaslSessionClient> must satisfy the trait via the
    // blanket impl and be usable as a trait object.
    let client: Arc<dyn WaslSessionClient> = Arc::new(fake);

    assert_eq!(client.target_id(), "wasl-target-arc-dyn");
    let _ = client.prompt(
        sample_request("k-dyn", "via arc dyn"),
        support::queue_policy(),
    );
    client
        .shutdown(WaslShutdownMode::Immediate)
        .expect("forwarded shutdown through Arc<dyn> must be Ok");

    let prompt_log = prompts.lock().unwrap().clone();
    assert_eq!(prompt_log.len(), 1);
    assert_eq!(prompt_log[0].text, "via arc dyn");
    assert_eq!(
        *shutdowns.lock().unwrap(),
        vec![WaslShutdownMode::Immediate]
    );
}
