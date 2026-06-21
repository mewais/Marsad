//! Full matrix of branches — drives 100 % region coverage.
//!
//! Matrix (action → phase):
//!   container_alive=T × session_id=Some → Adopt    → Some(Detached)
//!   container_alive=T × session_id=None → Adopt    → Some(Detached)
//!   container_alive=F × session_id=Some → Resume   → None
//!   container_alive=F × session_id=None → MarkStale → Some(Stale)
//!
//! No-duplication invariant: Adopt paths MUST NOT call spawn.

use marsad_lifecycle::{
    plan_reconnect, reconcile, Action, FakeContainer, Persisted, Probe, RuntimePhase,
};

// ── helpers ──────────────────────────────────────────────────────────────────

fn persisted(session_id: Option<&str>, container: &str) -> Persisted {
    Persisted {
        session_id: session_id.map(str::to_string),
        container_name: container.to_string(),
        project: "proj".to_string(),
        model: "model".to_string(),
    }
}

fn probe(alive: bool) -> Probe {
    Probe {
        container_alive: alive,
    }
}

// ── plan_reconnect full matrix ────────────────────────────────────────────────

/// Table-driven: (alive, session_id) → expected Action discriminant label.
#[test]
fn plan_reconnect_matrix() {
    struct Case {
        alive: bool,
        session_id: Option<&'static str>,
        label: &'static str,
    }

    let cases = [
        Case {
            alive: true,
            session_id: Some("s1"),
            label: "Adopt",
        },
        Case {
            alive: true,
            session_id: None,
            label: "Adopt",
        },
        Case {
            alive: false,
            session_id: Some("s2"),
            label: "Resume",
        },
        Case {
            alive: false,
            session_id: None,
            label: "MarkStale",
        },
    ];

    for c in &cases {
        let p = persisted(c.session_id, "marsad-x");
        let action = plan_reconnect(&p, &probe(c.alive));
        let got = match &action {
            Action::Adopt => "Adopt",
            Action::Resume(_) => "Resume",
            Action::MarkStale => "MarkStale",
        };
        assert_eq!(
            got, c.label,
            "alive={} session_id={:?} → expected {}, got {}",
            c.alive, c.session_id, c.label, got
        );
    }
}

// ── reconcile full matrix ─────────────────────────────────────────────────────

/// Row 1: alive=true, session_id=Some → Adopt + Detached, no spawn (no duplication).
#[test]
fn reconcile_alive_with_session_adopt_detached_no_spawn() {
    let p = persisted(Some("sid-A"), "marsad-A");
    let mut f = FakeContainer::new();
    f.set_exists("marsad-A", true);

    let rt = reconcile(&p, &mut f).unwrap();

    assert!(matches!(rt.action, Action::Adopt), "expected Adopt");
    assert!(
        matches!(rt.phase, Some(RuntimePhase::Detached)),
        "expected Detached, got {:?}",
        rt.phase
    );
    // NO-DUPLICATION PROOF: spawn must not appear in the call log
    assert!(
        !f.calls().iter().any(|c| c.contains("spawn")),
        "Adopt must never call spawn — calls: {:?}",
        f.calls()
    );
}

/// Row 2: alive=true, session_id=None → Adopt + Detached, no spawn.
#[test]
fn reconcile_alive_no_session_adopt_detached_no_spawn() {
    let p = persisted(None, "marsad-B");
    let mut f = FakeContainer::new();
    f.set_exists("marsad-B", true);

    let rt = reconcile(&p, &mut f).unwrap();

    assert!(matches!(rt.action, Action::Adopt), "expected Adopt");
    assert!(
        matches!(rt.phase, Some(RuntimePhase::Detached)),
        "expected Detached, got {:?}",
        rt.phase
    );
    assert!(
        !f.calls().iter().any(|c| c.contains("spawn")),
        "Adopt (no session) must never call spawn — calls: {:?}",
        f.calls()
    );
}

/// Row 3: alive=false, session_id=Some → Resume, spawn called once, no extra claude process.
#[test]
fn reconcile_dead_with_session_resume_no_extra_spawn() {
    let p = persisted(Some("sid-R"), "marsad-R");
    let mut f = FakeContainer::new();
    // container dead: set_exists defaults false; leave it

    let rt = reconcile(&p, &mut f).unwrap();

    assert!(matches!(rt.action, Action::Resume(_)), "expected Resume");

    let spawn_count = f.calls().iter().filter(|c| c.contains("spawn")).count();
    assert_eq!(
        spawn_count, 1,
        "Resume must call spawn exactly once, got {spawn_count}"
    );

    // attach must NOT be called (no container existed)
    assert!(
        !f.calls().iter().any(|c| c.contains("attach")),
        "Resume must not call attach — calls: {:?}",
        f.calls()
    );
}

/// Row 4: alive=false, session_id=None → MarkStale + Stale phase.
#[test]
fn reconcile_dead_no_session_mark_stale() {
    let p = persisted(None, "marsad-S");
    let mut f = FakeContainer::new();

    let rt = reconcile(&p, &mut f).unwrap();

    assert!(matches!(rt.action, Action::MarkStale), "expected MarkStale");
    assert!(
        matches!(rt.phase, Some(RuntimePhase::Stale)),
        "MarkStale → Stale phase, got {:?}",
        rt.phase
    );
    // Neither attach nor spawn should be called
    assert!(
        !f.calls()
            .iter()
            .any(|c| c.contains("attach") || c.contains("spawn")),
        "MarkStale must not call attach or spawn — calls: {:?}",
        f.calls()
    );
}

// ── Detached vs Stale classification ────────────────────────────────────────

#[test]
fn detached_phase_only_when_container_alive() {
    let p = persisted(Some("live"), "marsad-live");
    let mut f = FakeContainer::new();
    f.set_exists("marsad-live", true);

    let rt = reconcile(&p, &mut f).unwrap();
    assert!(
        matches!(rt.phase, Some(RuntimePhase::Detached)),
        "alive container → Detached, got {:?}",
        rt.phase
    );
}

#[test]
fn stale_phase_only_when_container_dead_and_no_session() {
    let p = persisted(None, "marsad-dead");
    let mut f = FakeContainer::new();

    let rt = reconcile(&p, &mut f).unwrap();
    assert!(
        matches!(rt.phase, Some(RuntimePhase::Stale)),
        "dead + no session → Stale, got {:?}",
        rt.phase
    );
}

#[test]
fn resume_action_phase_is_none() {
    // A successful Resume means the container is freshly live and attached.
    // The phase is None — neither Detached nor Stale.  The caller derives
    // the running phase from the event stream (spec §RuntimePhase mapping).
    let p = persisted(Some("sid-nr"), "marsad-nr");
    let mut f = FakeContainer::new();

    let rt = reconcile(&p, &mut f).unwrap();
    assert!(
        rt.phase.is_none(),
        "successful Resume → phase must be None, got {:?}",
        rt.phase
    );
}

// ── No-duplication: Adopt never spawns — comprehensive check ─────────────────

#[test]
fn no_duplication_on_adopt_with_any_session_id() {
    // Run multiple named sessions; each must never spawn.
    for name in &["alpha", "beta", "gamma"] {
        let container_name = format!("marsad-{name}");
        let p = persisted(Some(name), &container_name);
        let mut f = FakeContainer::new();
        f.set_exists(&container_name, true);

        let rt = reconcile(&p, &mut f).unwrap();
        assert!(
            matches!(rt.action, Action::Adopt),
            "session {name}: expected Adopt"
        );
        assert!(
            !f.calls().iter().any(|c| c.contains("spawn")),
            "session {name}: Adopt must not spawn — calls: {:?}",
            f.calls()
        );
    }
}

// ── plan_reconnect Resume carries correct session_id ─────────────────────────

#[test]
fn plan_resume_id_matches_persisted_session_id() {
    let p = persisted(Some("exact-id-9999"), "marsad-exact");
    let action = plan_reconnect(&p, &probe(false));
    match action {
        Action::Resume(id) => assert_eq!(id, "exact-id-9999"),
        other => panic!("expected Resume(exact-id-9999), got {other:?}"),
    }
}

// ── SessionRuntime fields are accessible ─────────────────────────────────────

#[test]
fn session_runtime_action_field_accessible() {
    let p = persisted(None, "marsad-field");
    let mut f = FakeContainer::new();
    let rt = reconcile(&p, &mut f).unwrap();
    // Just access the field to prove it is public
    let _action = rt.action;
    let _phase = rt.phase;
}
