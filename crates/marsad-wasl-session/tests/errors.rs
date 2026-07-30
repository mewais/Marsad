//! errors.rs — the admission-refusal and failure paths from the caller's side
//! (test plan §10.1): prompt-when-busy per BusyPolicy, prompt-when-unavailable,
//! and the crash / offline transition (`script_crash` → Faulted/Offline +
//! SessionExited; a subsequent prompt is RejectedUnavailable). All against the
//! in-memory fake.
//!
//! This file WILL fail to compile until a later worker implements
//! `marsad_wasl_session`'s public surface (§4/§5) — expected tests-first signal.
mod support;

use marsad_wasl_session::{
    FakeWaslSessionClient, WaslSessionClient, WaslSessionEventKind, WaslTargetStatus,
    WaslTurnReceipt,
};

use support::{all_caps, busy_status, drain, sample_request};

// ---------------------------------------------------------------------------
// prompt-when-busy: Reject → RejectedBusy.
// ---------------------------------------------------------------------------

#[test]
fn prompt_when_busy_with_reject_policy_yields_rejected_busy() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());
    fake.script_status(busy_status());
    fake.script_receipt(WaslTurnReceipt::RejectedBusy {
        active_turn: "t-active".to_string(),
    });

    let receipt = fake.prompt(sample_request("k1", "hi"), support::reject_policy());
    assert_eq!(
        receipt,
        WaslTurnReceipt::RejectedBusy {
            active_turn: "t-active".to_string()
        }
    );
}

// ---------------------------------------------------------------------------
// prompt-when-busy: Queue{max_wait} → Queued{position}.
// ---------------------------------------------------------------------------

#[test]
fn prompt_when_busy_with_queue_policy_yields_queued_with_position() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());
    fake.script_status(busy_status());
    fake.script_receipt(WaslTurnReceipt::Queued {
        turn_id: "t2".to_string(),
        position: 1,
    });

    let receipt = fake.prompt(sample_request("k1", "hi"), support::queue_policy());
    assert_eq!(
        receipt,
        WaslTurnReceipt::Queued {
            turn_id: "t2".to_string(),
            position: 1
        }
    );
}

// ---------------------------------------------------------------------------
// prompt-when-busy: QueueRelated → both branches (queued when related,
// rejected when unrelated). The fake honors whatever receipt is scripted, so
// this proves the caller can drive both outcomes under one policy.
// ---------------------------------------------------------------------------

#[test]
fn prompt_when_busy_with_queue_related_can_yield_queued_or_rejected() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());
    fake.script_status(busy_status());

    // Related → queued.
    fake.script_receipt(WaslTurnReceipt::Queued {
        turn_id: "t-related".to_string(),
        position: 0,
    });
    assert_eq!(
        fake.prompt(
            sample_request("k1", "related"),
            support::queue_related_policy()
        ),
        WaslTurnReceipt::Queued {
            turn_id: "t-related".to_string(),
            position: 0
        }
    );

    // Unrelated → rejected as busy.
    fake.script_receipt(WaslTurnReceipt::RejectedBusy {
        active_turn: "t-active".to_string(),
    });
    assert_eq!(
        fake.prompt(
            sample_request("k2", "unrelated"),
            support::queue_related_policy()
        ),
        WaslTurnReceipt::RejectedBusy {
            active_turn: "t-active".to_string()
        }
    );
}

// ---------------------------------------------------------------------------
// prompt-when-unavailable: Offline / Faulted → RejectedUnavailable{status}.
// ---------------------------------------------------------------------------

#[test]
fn prompt_when_offline_yields_rejected_unavailable_carrying_the_status() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());
    fake.script_status(WaslTargetStatus::Offline);
    fake.script_receipt(WaslTurnReceipt::RejectedUnavailable {
        status: WaslTargetStatus::Offline,
    });

    assert_eq!(
        fake.prompt(sample_request("k1", "hi"), support::queue_policy()),
        WaslTurnReceipt::RejectedUnavailable {
            status: WaslTargetStatus::Offline
        }
    );
}

#[test]
fn prompt_when_faulted_yields_rejected_unavailable_carrying_the_fault() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());
    let faulted = WaslTargetStatus::Faulted {
        error: "cli crashed".to_string(),
        recoverable: false,
    };
    fake.script_status(faulted.clone());
    fake.script_receipt(WaslTurnReceipt::RejectedUnavailable {
        status: faulted.clone(),
    });

    assert_eq!(
        fake.prompt(sample_request("k1", "hi"), support::queue_policy()),
        WaslTurnReceipt::RejectedUnavailable { status: faulted }
    );
}

// ---------------------------------------------------------------------------
// crash / offline: script_crash → Faulted{recoverable:false} (or Offline) AND
// a terminal SessionExited on the stream; a subsequent prompt is
// RejectedUnavailable.
// ---------------------------------------------------------------------------

#[test]
fn crash_sets_faulted_status_and_enqueues_session_exited() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());
    fake.script_crash(Some(1));

    // Status is a terminal-fault kind.
    match fake.status() {
        WaslTargetStatus::Faulted { recoverable, .. } => {
            assert!(!recoverable, "a crash is a non-recoverable fault");
        }
        WaslTargetStatus::Offline => {}
        other => panic!("script_crash must set Faulted or Offline, got {other:?}"),
    }

    // A terminal SessionExited is on the stream.
    let mut stream = fake.attach(0);
    let events = drain(&mut *stream);
    assert!(
        events.iter().any(|e| matches!(
            &e.kind,
            WaslSessionEventKind::SessionExited { code } if *code == Some(1)
        )),
        "script_crash must enqueue a SessionExited: {events:?}"
    );
}

#[test]
fn prompt_after_crash_is_rejected_unavailable() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());
    fake.script_crash(None);
    // After a crash the session is unavailable; the scripted receipt reflects
    // the atomic admission refusal.
    let dead = fake.status();
    fake.script_receipt(WaslTurnReceipt::RejectedUnavailable {
        status: dead.clone(),
    });

    match fake.prompt(sample_request("k1", "anyone?"), support::queue_policy()) {
        WaslTurnReceipt::RejectedUnavailable { status } => assert_eq!(status, dead),
        other => panic!("prompt after crash must be RejectedUnavailable, got {other:?}"),
    }
}
