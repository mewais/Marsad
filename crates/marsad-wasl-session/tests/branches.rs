//! branches.rs — edge-path / forward-compat cases from the caller's side
//! (test plan §10.1): attach replay correctness including an `ObserveLost`
//! mid-stream then resubscribe-from-last-seq with no dup/gap; capabilities are
//! informational only (an `interruptible:false` client is still callable, but
//! any affordance helper reports it unavailable); and the v2 UUID idempotency
//! key's black-box uniqueness property. Event-stream *replay* mechanics are
//! THIS crate's concern; transcript folding lives in marsad-core (plan §6.4,
//! §10.4) and is tested there, not here.
//!
//! This file WILL fail to compile until a later worker implements
//! `marsad_wasl_session`'s public surface (§4/§5) — expected tests-first signal.
mod support;

use std::collections::HashSet;

use marsad_wasl_session::{
    new_idempotency_key, FakeWaslSessionClient, WaslCancelReason, WaslSessionClient,
    WaslSessionEventKind, WaslStreamItem,
};

use support::{all_caps, drain, not_interruptible_caps};

// ---------------------------------------------------------------------------
// attach replay: seq 1..=5, after_seq=2 yields exactly 3..=5 (also pinned in
// fakes.rs; here we additionally assert no gap and no dup across a full range).
// ---------------------------------------------------------------------------

#[test]
fn attach_replay_yields_a_contiguous_gapless_dupless_suffix() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());
    for i in 1..=5 {
        fake.script_event(WaslSessionEventKind::AssistantDelta {
            turn_id: "t1".to_string(),
            text: format!("d{i}"),
        });
    }

    let mut stream = fake.attach(2);
    let seqs: Vec<u64> = drain(&mut *stream).iter().map(|e| e.seq).collect();
    assert_eq!(seqs, vec![3, 4, 5]);

    // No duplicate.
    let unique: HashSet<u64> = seqs.iter().copied().collect();
    assert_eq!(unique.len(), seqs.len(), "no seq may repeat");
    // No gap.
    for w in seqs.windows(2) {
        assert_eq!(w[1], w[0] + 1, "no seq may be skipped");
    }
}

// ---------------------------------------------------------------------------
// ObserveLost mid-stream, then resubscribe from the last good seq: the
// resubscription neither repeats an already-seen event nor skips one. This is
// the event-stream replay contract (Wasl events(after_seq)), which is THIS
// crate's responsibility.
// ---------------------------------------------------------------------------

#[test]
fn observe_lost_mid_stream_then_resubscribe_has_no_dup_or_gap() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());
    // Real turn events at seq 1..=2, then a synthesized ObserveLost, then more.
    fake.script_event(WaslSessionEventKind::TurnAccepted {
        turn_id: "t1".to_string(),
        source: "LocalHuman".to_string(),
    });
    fake.script_event(WaslSessionEventKind::AssistantDelta {
        turn_id: "t1".to_string(),
        text: "part-1".to_string(),
    });
    fake.script_event(WaslSessionEventKind::ObserveLost {
        error: "peer reset".to_string(),
    });
    fake.script_event(WaslSessionEventKind::AssistantDelta {
        turn_id: "t1".to_string(),
        text: "part-2".to_string(),
    });
    fake.script_event(WaslSessionEventKind::TurnCompleted {
        turn_id: "t1".to_string(),
        reply: "part-1part-2".to_string(),
        certainty: "committed".to_string(),
    });

    // First subscription: read until (and including) the ObserveLost marker. v3:
    // next() yields a WaslStreamItem; unwrap the Event(..) (no truncation here).
    let mut first = fake.attach(0);
    let mut seen = Vec::new();
    let mut last_good_seq = 0;
    while let Some(item) = first.next() {
        let ev = match item {
            WaslStreamItem::Event(ev) => ev,
            WaslStreamItem::Truncated { .. } => panic!("unexpected truncation in this scenario"),
        };
        let is_lost = matches!(ev.kind, WaslSessionEventKind::ObserveLost { .. });
        if !is_lost {
            last_good_seq = ev.seq;
        }
        seen.push(ev.seq);
        if is_lost {
            break;
        }
    }
    assert_eq!(
        last_good_seq, 2,
        "the last good (non-ObserveLost) seq before the drop must be 2"
    );
    drop(first);

    // Resubscribe from the last good seq: must yield only the tail, no repeat
    // of seq 1..=2, no skip.
    let mut second = fake.attach(last_good_seq);
    let tail: Vec<u64> = drain(&mut *second).iter().map(|e| e.seq).collect();
    for s in &tail {
        assert!(
            *s > last_good_seq,
            "resubscribe must not repeat an already-observed seq (got {s} <= {last_good_seq})"
        );
    }
    // Contiguous from last_good_seq+1 onward (skipping the ObserveLost seq is
    // fine — it is a synthesized marker, not a journaled event, but there must
    // be no gap between the real events that remain).
    assert!(
        tail.contains(&4) && tail.contains(&5),
        "the post-drop real events (part-2 @ delta, completion) must all replay: {tail:?}"
    );
}

// ---------------------------------------------------------------------------
// capabilities are informational only: an interruptible:false client is STILL
// callable (interrupt records + returns per the contract), but the affordance
// helper reports interrupt as unavailable. No test asserts a capability the
// client didn't claim.
// ---------------------------------------------------------------------------

#[test]
fn interrupt_is_callable_even_when_not_interruptible() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", not_interruptible_caps());
    // Per the trait contract, interrupt remains callable (best-effort); the
    // fake records it and returns Ok by default.
    fake.interrupt(WaslCancelReason::Human)
        .expect("interrupt is callable regardless of capability");
    let log = fake.interrupts.lock().unwrap().clone();
    assert_eq!(log, vec![WaslCancelReason::Human]);
}

#[test]
fn affordance_helper_reports_interrupt_unavailable_when_not_interruptible() {
    let caps = not_interruptible_caps();
    assert!(
        !caps.can_interrupt(),
        "an interruptible:false client must report the interrupt affordance unavailable"
    );

    let full = all_caps();
    assert!(
        full.can_interrupt(),
        "an interruptible:true client reports the affordance available"
    );
}

// ---------------------------------------------------------------------------
// idempotency key (v2): UUID-based, minted per send. Black-box: repeated calls
// to the generator must produce distinct, non-empty keys. We assert only the
// observable uniqueness property (not the internal representation).
// ---------------------------------------------------------------------------

#[test]
fn new_idempotency_key_is_unique_across_many_calls() {
    let mut seen = HashSet::new();
    for _ in 0..1000 {
        let k = new_idempotency_key();
        assert!(!k.is_empty(), "an idempotency key must not be empty");
        assert!(
            seen.insert(k.clone()),
            "idempotency keys must be unique across sends (collision on {k})"
        );
    }
}
