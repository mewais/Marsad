//! projection.rs — the pure, total `WaslTargetStatus → marsad_events::Phase`
//! status projection (plan §6.3), the analogue of `wasl_event_lines`. Every row
//! of the §6.3 table is pinned here, including BOTH `Faulted` recoverabilities
//! and `Unknown(tag) → Working`. Capabilities never affect the projection. The
//! v2 `live` axis note is also exercised: an idle-and-ready session is "live"
//! and distinct from a stopped/exited one even though both project to `Done`.
//!
//! Per plan §6.3 / Q3 the projection lives in this crate and returns the shared
//! `marsad_events::Phase` (the new `marsad-wasl-session → marsad-events` edge).
//! If the accepted spec instead keeps the projection private to marsad-core,
//! these tests move there — see the test-author report's assumptions note.
//!
//! This file WILL fail to compile until a later worker implements
//! `marsad_wasl_session`'s public surface (§4/§6.3) — expected tests-first
//! signal.

use marsad_events::Phase;
use marsad_wasl_session::{project_status_to_phase, status_is_live, WaslTargetStatus};

fn awaiting_gate() -> WaslTargetStatus {
    WaslTargetStatus::AwaitingGate {
        active_turn: "t1".to_string(),
        // v3 §4.4: the real NativeGateDetail is only {summary, preview}.
        detail: marsad_wasl_session::WaslNativeGateDetail {
            summary: "s".to_string(),
            preview: "p".to_string(),
        },
        since: "2026-07-02T12:00:00.000Z".to_string(),
        queued: 0,
    }
}

fn busy() -> WaslTargetStatus {
    WaslTargetStatus::Busy {
        active_turn: "t1".to_string(),
        source: "LocalHuman".to_string(),
        phase: "generating".to_string(),
        since: "2026-07-02T12:00:00.000Z".to_string(),
        queued: 0,
    }
}

fn idle() -> WaslTargetStatus {
    WaslTargetStatus::Idle {
        since: "2026-07-02T12:00:00.000Z".to_string(),
    }
}

// ---------------------------------------------------------------------------
// The full §6.3 table, one assertion per row.
// ---------------------------------------------------------------------------

#[test]
fn offline_projects_to_detached() {
    assert_eq!(
        project_status_to_phase(&WaslTargetStatus::Offline),
        Phase::Detached
    );
}

#[test]
fn starting_projects_to_starting() {
    assert_eq!(
        project_status_to_phase(&WaslTargetStatus::Starting),
        Phase::Starting
    );
}

#[test]
fn idle_projects_to_done() {
    assert_eq!(project_status_to_phase(&idle()), Phase::Done);
}

#[test]
fn busy_projects_to_working() {
    assert_eq!(project_status_to_phase(&busy()), Phase::Working);
}

#[test]
fn awaiting_gate_projects_to_needs() {
    assert_eq!(project_status_to_phase(&awaiting_gate()), Phase::Needs);
}

#[test]
fn interrupting_projects_to_working() {
    let s = WaslTargetStatus::Interrupting {
        active_turn: "t1".to_string(),
        since: "2026-07-02T12:00:00.000Z".to_string(),
    };
    assert_eq!(project_status_to_phase(&s), Phase::Working);
}

#[test]
fn draining_projects_to_working() {
    let s = WaslTargetStatus::Draining {
        active_turn: "t1".to_string(),
        since: "2026-07-02T12:00:00.000Z".to_string(),
    };
    assert_eq!(project_status_to_phase(&s), Phase::Working);
}

#[test]
fn faulted_recoverable_projects_to_error() {
    let s = WaslTargetStatus::Faulted {
        error: "transient".to_string(),
        recoverable: true,
    };
    assert_eq!(project_status_to_phase(&s), Phase::Error);
}

#[test]
fn faulted_unrecoverable_projects_to_error() {
    let s = WaslTargetStatus::Faulted {
        error: "fatal".to_string(),
        recoverable: false,
    };
    assert_eq!(
        project_status_to_phase(&s),
        Phase::Error,
        "recoverability drives the retry affordance, not the Phase (§6.3)"
    );
}

#[test]
fn stopped_projects_to_done() {
    assert_eq!(
        project_status_to_phase(&WaslTargetStatus::Stopped),
        Phase::Done
    );
}

#[test]
fn unknown_projects_to_working_conservatively() {
    let s = WaslTargetStatus::Unknown("SomeFutureState".to_string());
    assert_eq!(
        project_status_to_phase(&s),
        Phase::Working,
        "an unrecognized state must not claim Done/Needs; project conservatively to Working"
    );
}

// ---------------------------------------------------------------------------
// The projection is total: every constructible variant maps to SOME Phase and
// never panics. (Belt-and-braces over the table above.)
// ---------------------------------------------------------------------------

#[test]
fn projection_is_total_over_every_variant() {
    let all = vec![
        WaslTargetStatus::Offline,
        WaslTargetStatus::Starting,
        idle(),
        busy(),
        awaiting_gate(),
        WaslTargetStatus::Interrupting {
            active_turn: "t".to_string(),
            since: "x".to_string(),
        },
        WaslTargetStatus::Draining {
            active_turn: "t".to_string(),
            since: "x".to_string(),
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
        WaslTargetStatus::Unknown("x".to_string()),
    ];
    for s in &all {
        // Must not panic; the result is one of the seven known phases.
        let p = project_status_to_phase(s);
        assert!(matches!(
            p,
            Phase::Starting
                | Phase::Working
                | Phase::Needs
                | Phase::Error
                | Phase::Done
                | Phase::Stale
                | Phase::Detached
        ));
    }
}

// ---------------------------------------------------------------------------
// The `live` axis (v2 revision note in §6.3): even though Idle and Stopped both
// project to Phase::Done, an idle-and-ready session is "live" and a
// stopped/exited one is not — so a still-live idle session is never confused
// with a dead one at the `live` level.
// ---------------------------------------------------------------------------

#[test]
fn idle_is_live_but_stopped_and_offline_are_not() {
    assert!(status_is_live(&idle()), "an idle-and-ready session is live");
    assert!(
        !status_is_live(&WaslTargetStatus::Stopped),
        "a cleanly stopped session is not live"
    );
    assert!(
        !status_is_live(&WaslTargetStatus::Offline),
        "an offline session is not live"
    );
}

#[test]
fn active_states_are_live() {
    assert!(status_is_live(&busy()), "a busy session is live");
    assert!(status_is_live(&awaiting_gate()), "a gated session is live");
    assert!(
        status_is_live(&WaslTargetStatus::Starting),
        "a starting session is live"
    );
}

#[test]
fn faulted_and_exited_are_not_live() {
    assert!(
        !status_is_live(&WaslTargetStatus::Faulted {
            error: "e".to_string(),
            recoverable: false,
        }),
        "a non-recoverable fault is not live"
    );
}
