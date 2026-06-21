//! Error paths:
//!   • AttachRace  — exists()=true, attach() fails → re-probe once:
//!       still alive → Err(AttachRace)
//!       now gone + session_id  → Ok(Resume)
//!       now gone + no session  → Ok(MarkStale)
//!   • ResumeFailed — spawn returns error during Resume path.
//!   • Spawn failure mapped to appropriate error variant.

use marsad_lifecycle::{reconcile, Action, FakeContainer, LifecycleError, Persisted, RuntimePhase};

// ── helpers ──────────────────────────────────────────────────────────────────

fn persisted_with_session(session_id: &str, container: &str) -> Persisted {
    Persisted {
        session_id: Some(session_id.to_string()),
        container_name: container.to_string(),
        project: "proj".to_string(),
        model: "claude-opus-4-5".to_string(),
    }
}

fn persisted_no_session(container: &str) -> Persisted {
    Persisted {
        session_id: None,
        container_name: container.to_string(),
        project: "proj".to_string(),
        model: "claude-opus-4-5".to_string(),
    }
}

// ── AttachRace ────────────────────────────────────────────────────────────────
//
// Scenario: exists() → true, but between the probe and the attach the container
// disappeared (TOCTOU / race).  The attach() returns AttachRace.
// reconcile then re-probes exactly once:
//   - still alive → return Err(AttachRace)  (transient; caller retries)
//   - now gone + session_id known → Ok(Resume(...))
//   - now gone + no session_id   → Ok(MarkStale)
//
// FakeContainer::set_exists queues return values per name: calling
// set_exists("x", true) then set_exists("x", false) makes the first exists("x")
// return true and the second return false — matching the TOCTOU scenario.

#[test]
fn attach_race_reprobe_still_alive_returns_attach_race_error() {
    // exists()→true, attach()→fail, re-probe→true  ⟹  Err(AttachRace)
    let p = persisted_with_session("sid-persistent-race", "marsad-pr");
    let mut container = FakeContainer::new();
    // Both probes see alive: queue [true, true]
    container.set_exists("marsad-pr", true);
    container.set_exists("marsad-pr", true);
    container.fail_attach("marsad-pr");

    let err = reconcile(&p, &mut container)
        .expect_err("re-probe still alive → must return Err(AttachRace)");
    assert!(
        matches!(err, LifecycleError::AttachRace(_)),
        "re-probe still alive → AttachRace, got {err:?}"
    );
}

#[test]
fn attach_race_with_session_id_falls_to_resume() {
    // exists()→true, attach()→fail, re-probe→false, session_id known  ⟹  Ok(Resume)
    let p = persisted_with_session("sid-race", "marsad-sid-race");
    let mut container = FakeContainer::new();
    // First probe alive, re-probe dead: queue [true, false]
    container.set_exists("marsad-sid-race", true);
    container.set_exists("marsad-sid-race", false);
    container.fail_attach("marsad-sid-race");

    let result = reconcile(&p, &mut container).expect("reconcile must succeed after race recovery");
    assert!(
        matches!(result.action, Action::Resume(_)),
        "after AttachRace + re-probe dead + session_id known → Resume, got {:?}",
        result.action
    );
    // Resume → phase is None (not Stale, not Detached)
    assert!(
        result.phase.is_none(),
        "successful Resume → phase None, got {:?}",
        result.phase
    );
}

#[test]
fn attach_race_without_session_id_falls_to_stale() {
    // exists()→true, attach()→fail, re-probe→false, no session_id  ⟹  Ok(MarkStale)
    let p = persisted_no_session("marsad-race2");
    let mut container = FakeContainer::new();
    // First probe alive, re-probe dead: queue [true, false]
    container.set_exists("marsad-race2", true);
    container.set_exists("marsad-race2", false);
    container.fail_attach("marsad-race2");

    let result =
        reconcile(&p, &mut container).expect("reconcile must succeed after race with no session");
    assert!(
        matches!(result.action, Action::MarkStale),
        "after AttachRace + re-probe dead + no session_id → MarkStale, got {:?}",
        result.action
    );
    assert!(
        matches!(result.phase, Some(RuntimePhase::Stale)),
        "MarkStale action → Stale phase, got {:?}",
        result.phase
    );
}

// ── ResumeFailed ─────────────────────────────────────────────────────────────

#[test]
fn resume_failed_when_spawn_errors() {
    // fail_spawn is part of the spec API (lifecycle.md §Public API).
    // container dead → reconcile takes Resume path → calls spawn → Err(ResumeFailed).
    let p = persisted_with_session("sid-rfail", "marsad-sid-rfail");
    let mut container = FakeContainer::new();
    container.fail_spawn("marsad-sid-rfail");

    let err = reconcile(&p, &mut container).expect_err("spawn failure must propagate as Err");
    assert!(
        matches!(err, LifecycleError::ResumeFailed(_)),
        "spawn failure on resume → ResumeFailed, got {err:?}"
    );
}

// ── Spawn failure ─────────────────────────────────────────────────────────────

#[test]
fn spawn_failure_variant_is_returned() {
    // When spawn() errors on the Resume path, reconcile returns ResumeFailed
    // (the canonical mapping).  Spawn is also acceptable if the implementation
    // distinguishes infrastructure failures from session-resume failures.
    let p = persisted_with_session("sid-spawnfail", "marsad-sf");
    let mut container = FakeContainer::new();
    container.fail_spawn("marsad-sf");

    let err = reconcile(&p, &mut container).expect_err("spawn failure must be an Err");
    assert!(
        matches!(
            err,
            LifecycleError::ResumeFailed(_) | LifecycleError::Spawn(_)
        ),
        "spawn failure must map to ResumeFailed or Spawn, got {err:?}"
    );
}

// ── LifecycleError Display / Debug ────────────────────────────────────────────

#[test]
fn lifecycle_error_variants_are_debuggable() {
    // Ensure each variant formats without panic.
    let variants: &[LifecycleError] = &[
        LifecycleError::AttachRace("r".to_string()),
        LifecycleError::ResumeFailed("f".to_string()),
        LifecycleError::Spawn("s".to_string()),
        LifecycleError::Container("c".to_string()),
    ];
    for v in variants {
        // should not panic
        let _ = format!("{v:?}");
    }
}

#[test]
fn lifecycle_error_variants_implement_display() {
    let variants: &[LifecycleError] = &[
        LifecycleError::AttachRace("race".to_string()),
        LifecycleError::ResumeFailed("fail".to_string()),
        LifecycleError::Spawn("sp".to_string()),
        LifecycleError::Container("c".to_string()),
    ];
    for v in variants {
        let s = format!("{v}");
        assert!(!s.is_empty(), "Display must produce non-empty string");
    }
}
