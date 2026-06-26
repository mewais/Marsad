/// i04_persistence — integration scenario
///
/// Tests:
///   1. Persistence round-trip across a simulated crash: upsert sessions,
///      append transcripts, snapshot workspace, set_active_workspace, then
///      construct a NEW Store over the SAME shared MemoryBackend data and call
///      restore() — verifying full replay fidelity.
///
///   2. Stale vs Detached classification (store + lifecycle): take a restored
///      session and drive plan_reconnect / reconcile with a FakeContainer to
///      assert the B06+B09 cross-block contract.
use marsad_lifecycle::{
    plan_reconnect, reconcile, Action, FakeContainer, Persisted, Probe, RuntimePhase,
};
use marsad_store::{
    Backend, MemoryBackend, PersistedSession, Store, StoreError, TranscriptEntry, Workspace,
};
use std::sync::Arc;

// ── SharedBackend ─────────────────────────────────────────────────────────────
//
// A thin `Arc`-wrapper around `MemoryBackend` so the same backing store can be
// handed to two successive `Store` instances (simulating a crash + relaunch).
//
// `MemoryBackend::state` is already an `Arc<Mutex<_>>`, so cloning the outer
// `Arc<MemoryBackend>` gives both Stores a view into the identical heap data.
//
// This is *not* the typical single-Store use case; it is deliberately chosen to
// prove that persist-then-restore round-trips survive a simulated process
// boundary.

struct SharedBackend(Arc<MemoryBackend>);

impl SharedBackend {
    fn new(inner: Arc<MemoryBackend>) -> Self {
        SharedBackend(inner)
    }
}

impl Backend for SharedBackend {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        self.0.read(key)
    }
    fn write(&self, key: &str, bytes: &[u8]) -> Result<(), StoreError> {
        self.0.write(key, bytes)
    }
    fn list(&self, prefix: &str) -> Result<Vec<String>, StoreError> {
        self.0.list(prefix)
    }
    fn delete(&self, key: &str) -> Result<(), StoreError> {
        self.0.delete(key)
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn make_session(id: &str, session_id: Option<&str>) -> PersistedSession {
    PersistedSession {
        id: id.to_string(),
        session_id: session_id.map(str::to_string),
        name: format!("Session {id}"),
        host_id: "localhost".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        project: "/home/user/proj".to_string(),
        branch: "main".to_string(),
        model: "claude-sonnet-4-5".to_string(),
        last_phase: "running".to_string(),

        posture: None,
    }
}

fn make_entry(kind: &str, payload: &str) -> TranscriptEntry {
    TranscriptEntry {
        kind: kind.to_string(),
        payload: payload.to_string(),
    }
}

fn make_workspace(id: &str, session_ids: Vec<&str>) -> Workspace {
    Workspace {
        id: id.to_string(),
        name: format!("Workspace {id}"),
        session_ids: session_ids.into_iter().map(str::to_string).collect(),
        saved_at: "2024-01-01T00:00:00Z".to_string(),
    }
}

// ── test 1: persistence round-trip ───────────────────────────────────────────

#[test]
fn persistence_round_trip_across_simulated_crash() {
    // --- "pre-crash" Store (Store #1) ---
    let backend = Arc::new(MemoryBackend::new());

    let mut store1 = Store::new(Box::new(SharedBackend::new(Arc::clone(&backend))));

    // Upsert several sessions.
    let sess_a = make_session("sess-a", Some("claude-session-aaa"));
    let sess_b = make_session("sess-b", Some("claude-session-bbb"));
    let sess_c = make_session("sess-c", None); // no session_id → will be Stale after crash
    store1.upsert_session(&sess_a).expect("upsert sess-a");
    store1.upsert_session(&sess_b).expect("upsert sess-b");
    store1.upsert_session(&sess_c).expect("upsert sess-c");

    // Append transcripts.
    let entry1 = make_entry("user", "hello");
    let entry2 = make_entry("assistant", "world");
    let entry3 = make_entry("user", "goodbye");
    store1
        .append_transcript("sess-a", &entry1)
        .expect("append 1");
    store1
        .append_transcript("sess-a", &entry2)
        .expect("append 2");
    store1
        .append_transcript("sess-b", &entry3)
        .expect("append 3");

    // Snapshot a workspace.
    let ws = make_workspace("ws-main", vec!["sess-a", "sess-b"]);
    store1.snapshot_workspace(&ws).expect("snapshot workspace");

    // Set the active workspace.
    store1
        .set_active_workspace("ws-main")
        .expect("set active ws");

    // --- "crash" happens: drop Store #1 ---
    drop(store1);

    // --- "relaunch": Store #2 over the SAME backend data ---
    let store2 = Store::new(Box::new(SharedBackend::new(Arc::clone(&backend))));

    let restored = store2.restore().expect("restore after simulated crash");

    // Sessions: all three came back.
    assert_eq!(restored.sessions.len(), 3, "expected 3 sessions");

    let find = |id: &str| {
        restored
            .sessions
            .iter()
            .find(|s| s.id == id)
            .cloned()
            .unwrap_or_else(|| panic!("session '{id}' missing from restored"))
    };
    let ra = find("sess-a");
    assert_eq!(ra.session_id, Some("claude-session-aaa".to_string()));
    assert_eq!(ra.name, "Session sess-a");

    let rb = find("sess-b");
    assert_eq!(rb.session_id, Some("claude-session-bbb".to_string()));

    let rc = find("sess-c");
    assert_eq!(rc.session_id, None, "sess-c should have no session_id");

    // Workspaces: the one workspace came back.
    assert_eq!(restored.workspaces.len(), 1, "expected 1 workspace");
    let rws = &restored.workspaces[0];
    assert_eq!(rws.id, "ws-main");
    assert_eq!(rws.name, "Workspace ws-main");
    assert!(rws.session_ids.contains(&"sess-a".to_string()));
    assert!(rws.session_ids.contains(&"sess-b".to_string()));

    // Active workspace pointer.
    assert_eq!(
        restored.active_ws,
        Some("ws-main".to_string()),
        "active_ws must survive the crash"
    );

    // Transcripts: order-preserving replay fidelity.
    let transcript_a = restored
        .transcripts
        .get("sess-a")
        .expect("sess-a transcript missing");
    assert_eq!(
        transcript_a.len(),
        2,
        "sess-a should have 2 transcript entries"
    );
    assert_eq!(
        transcript_a[0], entry1,
        "first entry must be the user message"
    );
    assert_eq!(
        transcript_a[1], entry2,
        "second entry must be the assistant reply"
    );

    let transcript_b = restored
        .transcripts
        .get("sess-b")
        .expect("sess-b transcript missing");
    assert_eq!(
        transcript_b.len(),
        1,
        "sess-b should have 1 transcript entry"
    );
    assert_eq!(transcript_b[0], entry3);

    // sess-c has no transcripts.
    assert!(
        !restored.transcripts.contains_key("sess-c"),
        "sess-c must have no transcripts"
    );
}

// ── test 2: Stale vs Detached classification ──────────────────────────────────
//
// For each of the three cases from the spec:
//   - container alive            → plan_reconnect → Adopt → reconcile → Detached
//   - container dead + session_id → plan_reconnect → Resume → reconcile → phase None
//   - container dead + no session_id → plan_reconnect → MarkStale → reconcile → Stale

#[test]
fn classification_detached_when_container_alive() {
    // Persisted session with a known session_id and a named container.
    let known = Persisted {
        session_id: Some("claude-session-aaa".to_string()),
        container_name: "marsad-sess-a".to_string(),
        project: "/home/user/proj".to_string(),
        model: "claude-sonnet-4-5".to_string(),
    };

    // plan_reconnect: container alive → Adopt.
    let probe_alive = Probe {
        container_alive: true,
    };
    let action = plan_reconnect(&known, &probe_alive);
    assert!(
        matches!(action, Action::Adopt),
        "container alive must produce Adopt"
    );

    // reconcile: container exists → attach succeeds → Detached.
    let mut fake = FakeContainer::new();
    fake.set_exists("marsad-sess-a", true);

    let runtime = reconcile(&known, &mut fake).expect("reconcile should succeed");
    assert!(
        matches!(runtime.action, Action::Adopt),
        "reconcile action must be Adopt"
    );
    assert!(
        matches!(runtime.phase, Some(RuntimePhase::Detached)),
        "phase must be Detached when container is alive"
    );

    // No spawn must have been called (no-duplication invariant).
    let calls = fake.calls();
    assert!(
        !calls.iter().any(|c| c.starts_with("spawn:")),
        "Adopt path must never spawn a new container; calls: {calls:?}"
    );
    assert!(
        calls.iter().any(|c| c == "attach:marsad-sess-a"),
        "Adopt path must call attach; calls: {calls:?}"
    );
}

#[test]
fn classification_resume_when_container_dead_and_session_id_known() {
    let known = Persisted {
        session_id: Some("claude-session-bbb".to_string()),
        container_name: "marsad-sess-b".to_string(),
        project: "/home/user/proj".to_string(),
        model: "claude-sonnet-4-5".to_string(),
    };

    // plan_reconnect: container dead, session_id known → Resume.
    let probe_dead = Probe {
        container_alive: false,
    };
    let action = plan_reconnect(&known, &probe_dead);
    assert!(
        matches!(action, Action::Resume(ref sid) if sid == "claude-session-bbb"),
        "dead container + known session_id must produce Resume"
    );

    // reconcile: container absent → spawn claude --resume → phase is None.
    let mut fake = FakeContainer::new();
    // No set_exists call → queue empty → exists returns false.

    let runtime = reconcile(&known, &mut fake).expect("reconcile should succeed");
    assert!(
        matches!(runtime.action, Action::Resume(ref sid) if sid == "claude-session-bbb"),
        "reconcile must Resume with the correct session_id"
    );
    assert!(
        runtime.phase.is_none(),
        "phase must be None after a successful Resume (freshly live container)"
    );

    // A spawn must have been called with the resume command.
    let calls = fake.calls();
    assert!(
        calls
            .iter()
            .any(|c| c == "spawn:marsad-sess-b claude --resume claude-session-bbb"),
        "Resume path must spawn claude --resume <id>; calls: {calls:?}"
    );
}

#[test]
fn classification_stale_when_container_dead_and_no_session_id() {
    let known = Persisted {
        session_id: None, // no session_id → MarkStale
        container_name: "marsad-sess-c".to_string(),
        project: "/home/user/proj".to_string(),
        model: "claude-sonnet-4-5".to_string(),
    };

    // plan_reconnect: container dead, no session_id → MarkStale.
    let probe_dead = Probe {
        container_alive: false,
    };
    let action = plan_reconnect(&known, &probe_dead);
    assert!(
        matches!(action, Action::MarkStale),
        "dead container + no session_id must produce MarkStale"
    );

    // reconcile: container absent, no session_id → Stale.
    let mut fake = FakeContainer::new();

    let runtime = reconcile(&known, &mut fake).expect("reconcile should succeed");
    assert!(
        matches!(runtime.action, Action::MarkStale),
        "reconcile action must be MarkStale"
    );
    assert!(
        matches!(runtime.phase, Some(RuntimePhase::Stale)),
        "phase must be Stale when container is dead and no session_id"
    );

    // No spawn must have been called for a Stale session.
    let calls = fake.calls();
    assert!(
        !calls.iter().any(|c| c.starts_with("spawn:")),
        "MarkStale path must never spawn a container; calls: {calls:?}"
    );
}

// ── test 3: cross-block round-trip (store + lifecycle combined) ───────────────
//
// Restore a set of sessions from the store, then classify each one via
// plan_reconnect using a FakeContainer.  This is the B06+B09 cross-block
// contract assertion.

#[test]
fn store_restore_then_lifecycle_classify_cross_block_contract() {
    let backend = Arc::new(MemoryBackend::new());
    let mut store = Store::new(Box::new(SharedBackend::new(Arc::clone(&backend))));

    // Three sessions with different lifecycle outcomes.
    let sess_alive = make_session("alive", Some("claude-session-alive"));
    let sess_dead_known = make_session("dead-known", Some("claude-session-dead"));
    let sess_dead_unknown = make_session("dead-unknown", None);

    store.upsert_session(&sess_alive).unwrap();
    store.upsert_session(&sess_dead_known).unwrap();
    store.upsert_session(&sess_dead_unknown).unwrap();

    // Simulate crash + relaunch.
    drop(store);
    let store2 = Store::new(Box::new(SharedBackend::new(Arc::clone(&backend))));
    let restored = store2.restore().expect("restore");

    assert_eq!(restored.sessions.len(), 3);

    // Classify each restored session.
    for sess in &restored.sessions {
        let container_name = format!("marsad-{}", sess.id);
        let known = Persisted {
            session_id: sess.session_id.clone(),
            container_name: container_name.clone(),
            project: sess.project.clone(),
            model: sess.model.clone(),
        };

        match sess.id.as_str() {
            "alive" => {
                // Container alive → Adopt → Detached.
                let action = plan_reconnect(
                    &known,
                    &Probe {
                        container_alive: true,
                    },
                );
                assert!(matches!(action, Action::Adopt), "alive → Adopt");

                let mut fake = FakeContainer::new();
                fake.set_exists(&container_name, true);
                let rt = reconcile(&known, &mut fake).unwrap();
                assert!(matches!(rt.action, Action::Adopt));
                assert!(matches!(rt.phase, Some(RuntimePhase::Detached)));
            }
            "dead-known" => {
                // Container dead + session_id → Resume → phase None.
                let action = plan_reconnect(
                    &known,
                    &Probe {
                        container_alive: false,
                    },
                );
                assert!(
                    matches!(action, Action::Resume(ref s) if s == "claude-session-dead"),
                    "dead-known → Resume"
                );

                let mut fake = FakeContainer::new();
                let rt = reconcile(&known, &mut fake).unwrap();
                assert!(matches!(rt.action, Action::Resume(_)));
                assert!(rt.phase.is_none());
            }
            "dead-unknown" => {
                // Container dead + no session_id → MarkStale → Stale.
                let action = plan_reconnect(
                    &known,
                    &Probe {
                        container_alive: false,
                    },
                );
                assert!(
                    matches!(action, Action::MarkStale),
                    "dead-unknown → MarkStale"
                );

                let mut fake = FakeContainer::new();
                let rt = reconcile(&known, &mut fake).unwrap();
                assert!(matches!(rt.action, Action::MarkStale));
                assert!(matches!(rt.phase, Some(RuntimePhase::Stale)));
            }
            other => panic!("unexpected session id: {other}"),
        }
    }
}
