//! Nominal (happy) paths:
//!   • `plan_reconnect` returns each Action for the right inputs.
//!   • `reconcile` adopt path  → calls attach, never spawn.
//!   • `reconcile` resume path → spawns fresh container with claude --resume.

use marsad_lifecycle::{
    plan_reconnect, reconcile, Action, FakeContainer, Persisted, Probe, RuntimePhase,
    SessionRuntime,
};

// ── helpers ──────────────────────────────────────────────────────────────────

fn persisted_with_session(session_id: &str, container: &str) -> Persisted {
    Persisted {
        session_id: Some(session_id.to_string()),
        container_name: container.to_string(),
        project: "my-project".to_string(),
        model: "claude-opus-4-5".to_string(),
    }
}

fn persisted_no_session(container: &str) -> Persisted {
    Persisted {
        session_id: None,
        container_name: container.to_string(),
        project: "my-project".to_string(),
        model: "claude-opus-4-5".to_string(),
    }
}

fn alive() -> Probe {
    Probe {
        container_alive: true,
    }
}

fn dead() -> Probe {
    Probe {
        container_alive: false,
    }
}

// ── plan_reconnect: Adopt ────────────────────────────────────────────────────

#[test]
fn plan_adopt_when_container_alive_with_session() {
    let p = persisted_with_session("sid-1", "marsad-sid-1");
    let action = plan_reconnect(&p, &alive());
    assert!(matches!(action, Action::Adopt));
}

#[test]
fn plan_adopt_when_container_alive_no_session() {
    // Even if session_id is absent, alive container → Adopt (spec says "container alive → Adopt").
    let p = persisted_no_session("marsad-orphan");
    let action = plan_reconnect(&p, &alive());
    assert!(matches!(action, Action::Adopt));
}

// ── plan_reconnect: Resume ───────────────────────────────────────────────────

#[test]
fn plan_resume_when_dead_and_session_id_known() {
    let p = persisted_with_session("sid-2", "marsad-sid-2");
    let action = plan_reconnect(&p, &dead());
    match action {
        Action::Resume(id) => assert_eq!(id, "sid-2"),
        other => panic!("expected Resume, got {other:?}"),
    }
}

#[test]
fn plan_resume_carries_exact_session_id() {
    let p = persisted_with_session("abc-xyz-999", "marsad-abc");
    let action = plan_reconnect(&p, &dead());
    match action {
        Action::Resume(id) => assert_eq!(id, "abc-xyz-999"),
        other => panic!("expected Resume(abc-xyz-999), got {other:?}"),
    }
}

// ── plan_reconnect: MarkStale ────────────────────────────────────────────────

#[test]
fn plan_mark_stale_when_dead_and_no_session_id() {
    let p = persisted_no_session("marsad-gone");
    let action = plan_reconnect(&p, &dead());
    assert!(matches!(action, Action::MarkStale));
}

// ── reconcile: Adopt path ────────────────────────────────────────────────────

#[test]
fn reconcile_adopt_calls_attach_not_spawn() {
    let p = persisted_with_session("sid-a", "marsad-sid-a");
    let mut container = FakeContainer::new();
    container.set_exists("marsad-sid-a", true);

    let result = reconcile(&p, &mut container).expect("reconcile should succeed");

    // attach must have been called
    assert!(
        container.calls().iter().any(|c| c.contains("attach")),
        "Adopt path must call attach"
    );
    // spawn must NOT have been called (no duplication)
    assert!(
        !container.calls().iter().any(|c| c.contains("spawn")),
        "Adopt path must NEVER call spawn"
    );

    assert!(matches!(result.action, Action::Adopt));
}

#[test]
fn reconcile_adopt_phase_is_detached() {
    // A live-but-unattached container is classified as Detached.
    let p = persisted_with_session("sid-d", "marsad-sid-d");
    let mut container = FakeContainer::new();
    container.set_exists("marsad-sid-d", true);

    let SessionRuntime { phase, .. } = reconcile(&p, &mut container).unwrap();
    assert!(
        matches!(phase, Some(RuntimePhase::Detached)),
        "alive container → Detached phase, got {phase:?}"
    );
}

// ── reconcile: Resume path ───────────────────────────────────────────────────

#[test]
fn reconcile_resume_spawns_fresh_container() {
    let p = persisted_with_session("sid-r", "marsad-sid-r");
    let mut container = FakeContainer::new();
    // container is dead → set_exists not called (defaults false)

    let result = reconcile(&p, &mut container).expect("reconcile should succeed");

    assert!(
        container.calls().iter().any(|c| c.contains("spawn")),
        "Resume path must call spawn"
    );
    assert!(matches!(result.action, Action::Resume(_)));
}

#[test]
fn reconcile_resume_spawn_command_contains_resume_flag() {
    // Verify the spawned command uses `claude --resume <session_id>`.
    // NOTE(test-agent): FakeContainer must record the full spawn argument
    // so we can assert the command string. We look for "resume" in any call.
    let p = persisted_with_session("sid-r2", "marsad-sid-r2");
    let mut container = FakeContainer::new();

    reconcile(&p, &mut container).expect("reconcile should succeed");

    let spawn_calls: Vec<&String> = container
        .calls()
        .iter()
        .filter(|c| c.contains("spawn"))
        .collect();
    assert!(!spawn_calls.is_empty(), "spawn must be called");
    assert!(
        spawn_calls
            .iter()
            .any(|c| c.contains("resume") && c.contains("sid-r2")),
        "spawn command must contain '--resume sid-r2', calls: {spawn_calls:?}"
    );
}

#[test]
fn reconcile_mark_stale_action_when_dead_no_session() {
    let p = persisted_no_session("marsad-lost");
    let mut container = FakeContainer::new();

    let result = reconcile(&p, &mut container).expect("reconcile should succeed");
    assert!(matches!(result.action, Action::MarkStale));
}
