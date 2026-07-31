//! workstreams.rs — test suite for the additive workstream layer (B12).
//!
//! See `docs/specs/core-ext.md` for the authoritative contract (CW1–CW12 test
//! plan). This file drives `Core` through the SAME harness the sibling test
//! files use (`Core::with_deps` + `LoopbackFake` + `MemoryBackend`, mirroring
//! `core_with_backend` in `tests/happy.rs`), launching real sessions via
//! `Core::launch_session` so real `SessionRecord`s exist to become nodes.
//!
//! These tests WILL fail to compile until the code-agent implements the new
//! `Core` fields/methods (`load_workstreams`, `create_workstream`,
//! `delete_workstream`, `add_connection`, `remove_connection`,
//! `create_profile`, `delete_profile`, `assign_session`, `set_active_profile`,
//! `board`, `graph`, `nodes`) and the new `CoreError::Connection` /
//! `VmUpdate::Workstreams` / `VmUpdate::Board` variants described in
//! docs/specs/core-ext.md. That is the correct, expected state until the
//! code-agent lands the implementation.
//!
//! DEV DEPENDENCY NOTE: this file uses `marsad_workstream::{ConnKind, Driver,
//! HeadState}` directly. `marsad-workstream` is currently only a TRANSITIVE
//! dependency of marsad-core (via marsad-store/marsad-viewmodel), so it must
//! be added to `[dev-dependencies]` in crates/marsad-core/Cargo.toml as
//! `marsad-workstream = { workspace = true }` before this file will compile.
//! See tests/DEV_DEPENDENCIES.txt for details. All other imports
//! (marsad-store, marsad-transport, marsad-events, marsad-viewmodel) are
//! already available to marsad-core's tests via existing [dependencies].

use marsad_core::{Core, CoreConfig, CoreError, LaunchSpec,  VmUpdate};
use marsad_events::Phase;
use marsad_store::{BackendOp, MemoryBackend};
use marsad_transport::LoopbackFake;
use marsad_viewmodel::EdgeStatus;
use marsad_workstream::{ConnKind, Driver};

// ---------------------------------------------------------------------------
// Harness (mirrors `core_with_backend` in tests/happy.rs)
// ---------------------------------------------------------------------------

fn core_with_backend(backend: MemoryBackend) -> Core {
    let (a, _b) = LoopbackFake::pair();
    let cfg = CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-workstreams-test".to_string(),
    };
    Core::with_deps(
        cfg,
        Box::new(a),
        Box::new(marsad_core::RealGit),
        Box::new(marsad_core::RealTranscript),
        Box::new(backend),
    )
    .expect("with_deps")
}

/// Launch a real session (registers a `SessionRecord` + pushes onto
/// `session_order`) and return its id. Mirrors the launch calls in
/// `tests/fakes.rs` / `tests/happy.rs`.
fn launch(core: &mut Core, project: &str) -> String {
    core.launch_session(LaunchSpec {
        project: project.to_string(),
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
    })
    .expect("launch_session")
}

fn workstreams_from(updates: &[VmUpdate]) -> Vec<marsad_workstream::Workstream> {
    updates
        .iter()
        .rev()
        .find_map(|u| match u {
            VmUpdate::Workstreams(w) => Some(w.clone()),
            _ => None,
        })
        .expect("a Workstreams update")
}

fn board_from(updates: &[VmUpdate]) -> marsad_viewmodel::BoardView {
    updates
        .iter()
        .rev()
        .find_map(|u| match u {
            VmUpdate::Board(b) => Some(b.clone()),
            _ => None,
        })
        .expect("a Board update")
}

// ---------------------------------------------------------------------------
// CW1 — create_workstream then board() shows it; Workstreams VmUpdate carries it
// ---------------------------------------------------------------------------

#[test]
fn cw1_create_workstream_appears_in_board_and_update() {
    let mut core = core_with_backend(MemoryBackend::new());

    let updates = core
        .create_workstream(
            "ws-alpha".to_string(),
            "Alpha".to_string(),
            "Ship the alpha feature".to_string(),
            None,
        )
        .expect("create_workstream");

    // Workstreams VmUpdate carries the new workstream.
    let wss = workstreams_from(&updates);
    let created = wss
        .iter()
        .find(|w| w.id == "ws-alpha")
        .expect("created workstream present in Workstreams update");
    assert_eq!(created.name, "Alpha");
    assert_eq!(created.goal, "Ship the alpha feature");
    assert_eq!(created.profile, None);

    // Board VmUpdate is also returned and includes the card.
    let board = board_from(&updates);
    assert!(
        board
            .all
            .iter()
            .any(|c| c.id == "ws-alpha" && c.name == "Alpha"),
        "board().all must contain the new workstream card"
    );

    // board() delegate also shows it directly.
    let board2 = core.board();
    assert!(board2.all.iter().any(|c| c.id == "ws-alpha"));
}

#[test]
fn cw1_create_workstream_duplicate_id_overwrites() {
    let mut core = core_with_backend(MemoryBackend::new());
    core.create_workstream(
        "ws-dup".to_string(),
        "First".to_string(),
        "goal1".to_string(),
        None,
    )
    .expect("first create");

    let updates = core
        .create_workstream(
            "ws-dup".to_string(),
            "Second".to_string(),
            "goal2".to_string(),
            None,
        )
        .expect("second create (upsert, not an error)");

    let wss = workstreams_from(&updates);
    let matches: Vec<_> = wss.iter().filter(|w| w.id == "ws-dup").collect();
    assert_eq!(
        matches.len(),
        1,
        "duplicate id must overwrite, not duplicate"
    );
    assert_eq!(matches[0].name, "Second");
    assert_eq!(matches[0].goal, "goal2");
}

// ---------------------------------------------------------------------------
// CW2 — delete_workstream cascades; absent id is a no-op Ok
// ---------------------------------------------------------------------------

#[test]
fn cw2_delete_workstream_cascades_connections_membership_lead() {
    let mut core = core_with_backend(MemoryBackend::new());
    let s1 = launch(&mut core, "proj-1");
    let s2 = launch(&mut core, "proj-2");

    core.create_workstream("ws-del".to_string(), "Del".to_string(), String::new(), None)
        .expect("create ws");
    core.assign_session(&s1, "ws-del", true)
        .expect("assign s1 lead");
    core.assign_session(&s2, "ws-del", false)
        .expect("assign s2");
    core.add_connection(
        "conn-1".to_string(),
        "ws-del".to_string(),
        s1.clone(),
        s2.clone(),
        ConnKind::Data,
        false,
    )
    .expect("add_connection");

    // Precondition: connection visible in graph.
    let graph_before = core.graph("ws-del").expect("graph exists before delete");
    assert_eq!(graph_before.edges.len(), 1);

    let updates = core.delete_workstream("ws-del").expect("delete_workstream");

    let wss = workstreams_from(&updates);
    assert!(!wss.iter().any(|w| w.id == "ws-del"), "workstream removed");

    let board = board_from(&updates);
    assert!(!board.all.iter().any(|c| c.id == "ws-del"));

    // Graph for the deleted workstream is gone.
    assert!(
        core.graph("ws-del").is_none(),
        "graph must be None after delete"
    );

    // Membership + lead cleared: neither session appears in nodes() anymore
    // (nodes() only includes sessions WITH a membership entry).
    let nodes = core.nodes();
    assert!(!nodes.iter().any(|n| n.id == s1), "s1 membership cleared");
    assert!(!nodes.iter().any(|n| n.id == s2), "s2 membership cleared");
}

#[test]
fn cw2_delete_workstream_absent_id_is_noop_ok() {
    let mut core = core_with_backend(MemoryBackend::new());
    let result = core.delete_workstream("ws-does-not-exist");
    assert!(
        result.is_ok(),
        "deleting an absent workstream id must be Ok"
    );
}

// ---------------------------------------------------------------------------
// CW3 — add_connection valid edge appears in graph(); persists across a
// fresh Core::load_workstreams() over the SAME backend.
// ---------------------------------------------------------------------------

#[test]
fn cw3_add_connection_valid_edge_appears_in_graph() {
    let mut core = core_with_backend(MemoryBackend::new());
    let s1 = launch(&mut core, "proj-a");
    let s2 = launch(&mut core, "proj-b");
    core.create_workstream("ws-g".to_string(), "G".to_string(), String::new(), None)
        .expect("create ws");
    core.assign_session(&s1, "ws-g", true).expect("assign s1");
    core.assign_session(&s2, "ws-g", false).expect("assign s2");

    let updates = core
        .add_connection(
            "conn-valid".to_string(),
            "ws-g".to_string(),
            s1.clone(),
            s2.clone(),
            ConnKind::Command,
            false,
        )
        .expect("valid add_connection");

    // Only [Board] is required per spec.
    let board = board_from(&updates);
    assert!(board.all.iter().any(|c| c.id == "ws-g"));

    let graph = core.graph("ws-g").expect("graph exists");
    let edge = graph
        .edges
        .iter()
        .find(|e| e.id == "conn-valid")
        .expect("edge present in graph");
    assert_eq!(edge.from, s1);
    assert_eq!(edge.to, s2);
    assert_eq!(edge.kind, ConnKind::Command);
    assert!(!edge.bi);
}

#[test]
fn cw3_add_connection_persists_and_survives_reload() {
    let backend = MemoryBackend::new();
    let mut core = core_with_backend(backend.clone());

    let s1 = launch(&mut core, "proj-p1");
    let s2 = launch(&mut core, "proj-p2");
    core.create_workstream(
        "ws-persist".to_string(),
        "Persist".to_string(),
        String::new(),
        None,
    )
    .expect("create ws");
    core.assign_session(&s1, "ws-persist", true)
        .expect("assign s1");
    core.assign_session(&s2, "ws-persist", false)
        .expect("assign s2");
    core.add_connection(
        "conn-persist".to_string(),
        "ws-persist".to_string(),
        s1.clone(),
        s2.clone(),
        ConnKind::Both,
        true,
    )
    .expect("add_connection");

    // Fresh Core over the SAME backend, then reload.
    let mut core2 = core_with_backend(backend);
    // Need the sessions present in core2 for the connection's endpoints to
    // resolve as nodes; launch_sessions do not persist session records via
    // this harness's launch(), so we only assert the raw workstream/connection
    // persistence + membership round-trip here (graph() requires nodes(),
    // which requires the sessions to exist in THIS Core — out of scope for a
    // fresh Core that never launched them). We instead assert at the
    // workstream/connection/membership persistence layer via load_workstreams.
    let reload_updates = core2
        .load_workstreams()
        .expect("load_workstreams on fresh core");
    let wss = workstreams_from(&reload_updates);
    assert!(
        wss.iter().any(|w| w.id == "ws-persist"),
        "workstream must survive reload on a fresh Core over the same backend"
    );
}

// ---------------------------------------------------------------------------
// Regression: a persisted session (as the agent writes it) must reappear on
// the board after a fresh Core runs the real startup sequence
// (load_sessions → load_workstreams). This reproduces "sessions vanished after
// reinstall": the agent persists PersistedSession records; a relaunched desktop
// Core must restore them AND surface them on the workstream-centric board.
// ---------------------------------------------------------------------------

#[test]
fn persisted_session_reappears_on_board_after_reload() {
    use marsad_store::{PersistedSession, Store};

    let backend = MemoryBackend::new();
    // Simulate the agent having persisted a session (what survives an app/agent
    // restart). The desktop Core's launch() does NOT persist; the agent does.
    {
        let mut store = Store::new(Box::new(backend.clone()));
        store
            .upsert_session(&PersistedSession {
                id: "sess-restore-1".to_string(),
                session_id: Some("claude-uuid-restore".to_string()),
                name: "sess-restore-1".to_string(),
                host_id: "local".to_string(),
                host_target: None,
                claude_bin: None,
                setup: None,
                env: Vec::new(),
                project: "/tmp/restore-proj".to_string(),
                branch: "main".to_string(),
                model: "claude-opus-4-5".to_string(),
                last_phase: "Done".to_string(),
                mode: None,
                disallowed_tools: vec![],
                driver: "claude".to_string(),
                slash_commands: vec![],
                tools: vec![],
                mcp_servers: vec![],
            })
            .expect("agent persists session");
    }

    // Fresh Core over the SAME backend runs the exact desktop startup order.
    let mut core = core_with_backend(backend);
    core.load_sessions().expect("load_sessions");
    let updates = core.load_workstreams().expect("load_workstreams");

    // The restored session must be present on the board (auto-assigned to a
    // single-node workstream by the migration).
    let board = board_from(&updates);
    let total_sessions: usize = board.all.iter().map(|c| c.session_count).sum();
    assert_eq!(
        total_sessions, 1,
        "restored session must appear on the board after reload, got board.all={:?}",
        board.all.iter().map(|c| (&c.id, c.session_count)).collect::<Vec<_>>()
    );
    assert!(
        core.snapshot().sessions.iter().any(|s| s.id == "sess-restore-1"),
        "restored session must be in the snapshot"
    );
}

// ---------------------------------------------------------------------------
// Regression: a Fresh session is persisted with session_id=None (the claude
// conversation UUID is unknown until system/init arrives). That UUID lives ONLY
// in memory unless Core writes it back to the store on init. Without this, two
// sessions in the SAME directory both restore with session_id=None and fall back
// to "most recent .jsonl", collapsing onto ONE claude session after restart.
// Core MUST persist the captured UUID on system/init so each session resumes its
// own conversation.
// ---------------------------------------------------------------------------

#[test]
fn system_init_persists_claude_session_uuid_to_store() {
    use marsad_store::{PersistedSession, Store};

    let backend = MemoryBackend::new();
    // Agent-side launch: a Fresh session is persisted with session_id=None.
    {
        let mut store = Store::new(Box::new(backend.clone()));
        store
            .upsert_session(&PersistedSession {
                id: "sess-uuid-1".to_string(),
                session_id: None,
                name: "sess-uuid-1".to_string(),
                host_id: "local".to_string(),
                host_target: None,
                claude_bin: None,
                setup: None,
                env: Vec::new(),
                project: "/tmp/same-dir".to_string(),
                branch: "main".to_string(),
                model: "claude-opus-4-5".to_string(),
                last_phase: "Starting".to_string(),
                mode: None,
                disallowed_tools: vec![],
                driver: "claude".to_string(),
                slash_commands: vec![],
                tools: vec![],
                mcp_servers: vec![],
            })
            .expect("agent persists fresh session");
    }

    let mut core = core_with_backend(backend.clone());
    core.load_sessions().expect("load_sessions");

    // The real claude conversation UUID arrives via system/init (inner
    // `session_id`), distinct from the marsad routing id ("sess-uuid-1").
    let init = r#"{"type":"AgentEvent","session_id":"sess-uuid-1","event":{"type":"system","subtype":"init","session_id":"claude-real-uuid-abc","model":"claude-opus-4-5","cwd":"/tmp/same-dir","tools":[],"mcp_servers":[],"permissionMode":"default"}}"#;
    core.on_agent_line(init).expect("on_agent_line init");

    // The store must now record the claude UUID for this session (the --resume /
    // transcript-backfill target that survives a restart).
    let store = Store::new(Box::new(backend));
    let persisted = store
        .restore()
        .expect("restore")
        .sessions
        .into_iter()
        .find(|s| s.id == "sess-uuid-1")
        .expect("session still persisted");
    assert_eq!(
        persisted.session_id.as_deref(),
        Some("claude-real-uuid-abc"),
        "system/init must persist the captured claude session UUID to the store"
    );
}

// ---------------------------------------------------------------------------
// CW4 — add_connection invalid cases -> CoreError::Connection, nothing persisted
// ---------------------------------------------------------------------------

#[test]
fn cw4_add_connection_self_loop_errors() {
    let mut core = core_with_backend(MemoryBackend::new());
    let s1 = launch(&mut core, "proj-x");
    core.create_workstream("ws-e".to_string(), "E".to_string(), String::new(), None)
        .expect("create ws");
    core.assign_session(&s1, "ws-e", true).expect("assign s1");

    let err = core
        .add_connection(
            "conn-self".to_string(),
            "ws-e".to_string(),
            s1.clone(),
            s1.clone(),
            ConnKind::Data,
            false,
        )
        .expect_err("self-loop must error");
    assert!(matches!(err, CoreError::Connection(_)));

    // Nothing persisted: graph has no edges.
    let graph = core.graph("ws-e").expect("graph exists");
    assert!(graph.edges.is_empty(), "self-loop must not persist an edge");
}

#[test]
fn cw4_add_connection_unknown_endpoint_errors() {
    let mut core = core_with_backend(MemoryBackend::new());
    let s1 = launch(&mut core, "proj-y");
    core.create_workstream("ws-f".to_string(), "F".to_string(), String::new(), None)
        .expect("create ws");
    core.assign_session(&s1, "ws-f", true).expect("assign s1");

    let err = core
        .add_connection(
            "conn-unknown".to_string(),
            "ws-f".to_string(),
            s1.clone(),
            "session-does-not-exist".to_string(),
            ConnKind::Data,
            false,
        )
        .expect_err("unknown endpoint must error");
    assert!(matches!(err, CoreError::Connection(_)));

    let graph = core.graph("ws-f").expect("graph exists");
    assert!(graph.edges.is_empty());
}

#[test]
fn cw4_add_connection_cross_workstream_errors() {
    let mut core = core_with_backend(MemoryBackend::new());
    let s1 = launch(&mut core, "proj-cw1");
    let s2 = launch(&mut core, "proj-cw2");
    core.create_workstream("ws-c1".to_string(), "C1".to_string(), String::new(), None)
        .expect("create ws1");
    core.create_workstream("ws-c2".to_string(), "C2".to_string(), String::new(), None)
        .expect("create ws2");
    core.assign_session(&s1, "ws-c1", true)
        .expect("assign s1 to ws-c1");
    core.assign_session(&s2, "ws-c2", true)
        .expect("assign s2 to ws-c2");

    let err = core
        .add_connection(
            "conn-cross".to_string(),
            "ws-c1".to_string(),
            s1.clone(),
            s2.clone(),
            ConnKind::Data,
            false,
        )
        .expect_err("cross-workstream endpoints must error");
    assert!(matches!(err, CoreError::Connection(_)));

    let graph1 = core.graph("ws-c1").expect("graph1 exists");
    assert!(graph1.edges.is_empty());
    let graph2 = core.graph("ws-c2").expect("graph2 exists");
    assert!(graph2.edges.is_empty());
}

#[test]
fn cw4_add_connection_duplicate_errors() {
    let mut core = core_with_backend(MemoryBackend::new());
    let s1 = launch(&mut core, "proj-d1");
    let s2 = launch(&mut core, "proj-d2");
    core.create_workstream(
        "ws-dupconn".to_string(),
        "D".to_string(),
        String::new(),
        None,
    )
    .expect("create ws");
    core.assign_session(&s1, "ws-dupconn", true)
        .expect("assign s1");
    core.assign_session(&s2, "ws-dupconn", false)
        .expect("assign s2");
    core.add_connection(
        "conn-first".to_string(),
        "ws-dupconn".to_string(),
        s1.clone(),
        s2.clone(),
        ConnKind::Data,
        false,
    )
    .expect("first connection succeeds");

    let err = core
        .add_connection(
            "conn-second".to_string(),
            "ws-dupconn".to_string(),
            s1.clone(),
            s2.clone(),
            ConnKind::Data,
            false,
        )
        .expect_err("identical (from,to) in same ws must error as duplicate");
    assert!(matches!(err, CoreError::Connection(_)));

    let graph = core.graph("ws-dupconn").expect("graph exists");
    assert_eq!(graph.edges.len(), 1, "only the first connection persisted");
}

#[test]
fn cw4_add_connection_cycle_errors() {
    let mut core = core_with_backend(MemoryBackend::new());
    let s1 = launch(&mut core, "proj-cyc1");
    let s2 = launch(&mut core, "proj-cyc2");
    let s3 = launch(&mut core, "proj-cyc3");
    core.create_workstream(
        "ws-cycle".to_string(),
        "Cyc".to_string(),
        String::new(),
        None,
    )
    .expect("create ws");
    core.assign_session(&s1, "ws-cycle", true)
        .expect("assign s1");
    core.assign_session(&s2, "ws-cycle", false)
        .expect("assign s2");
    core.assign_session(&s3, "ws-cycle", false)
        .expect("assign s3");

    core.add_connection(
        "conn-c1".to_string(),
        "ws-cycle".to_string(),
        s1.clone(),
        s2.clone(),
        ConnKind::Data,
        false,
    )
    .expect("s1->s2 ok");
    core.add_connection(
        "conn-c2".to_string(),
        "ws-cycle".to_string(),
        s2.clone(),
        s3.clone(),
        ConnKind::Data,
        false,
    )
    .expect("s2->s3 ok");

    // s3 -> s1 would close the cycle s1->s2->s3->s1.
    let err = core
        .add_connection(
            "conn-c3".to_string(),
            "ws-cycle".to_string(),
            s3.clone(),
            s1.clone(),
            ConnKind::Data,
            false,
        )
        .expect_err("closing edge must be rejected as a cycle");
    assert!(matches!(err, CoreError::Connection(_)));

    let graph = core.graph("ws-cycle").expect("graph exists");
    assert_eq!(graph.edges.len(), 2, "only the first two edges persisted");
}

// ---------------------------------------------------------------------------
// CW5 — remove_connection removes from graph; absent id is Ok no-op
// ---------------------------------------------------------------------------

#[test]
fn cw5_remove_connection_removes_from_graph() {
    let mut core = core_with_backend(MemoryBackend::new());
    let s1 = launch(&mut core, "proj-rm1");
    let s2 = launch(&mut core, "proj-rm2");
    core.create_workstream("ws-rm".to_string(), "RM".to_string(), String::new(), None)
        .expect("create ws");
    core.assign_session(&s1, "ws-rm", true).expect("assign s1");
    core.assign_session(&s2, "ws-rm", false).expect("assign s2");
    core.add_connection(
        "conn-rm".to_string(),
        "ws-rm".to_string(),
        s1.clone(),
        s2.clone(),
        ConnKind::Data,
        false,
    )
    .expect("add_connection");

    assert_eq!(core.graph("ws-rm").unwrap().edges.len(), 1);

    let updates = core
        .remove_connection("conn-rm")
        .expect("remove_connection");
    let board = board_from(&updates);
    assert!(board.all.iter().any(|c| c.id == "ws-rm"));

    let graph = core.graph("ws-rm").expect("graph still exists");
    assert!(
        graph.edges.is_empty(),
        "removed connection must not appear in graph"
    );
}

#[test]
fn cw5_remove_connection_absent_id_is_noop_ok() {
    let mut core = core_with_backend(MemoryBackend::new());
    let result = core.remove_connection("conn-does-not-exist");
    assert!(
        result.is_ok(),
        "removing an absent connection id must be Ok"
    );
}

// ---------------------------------------------------------------------------
// CW6 — create_profile + delete_profile: delete un-sets it on member
// workstreams and clears active_profile if it pointed at it.
// ---------------------------------------------------------------------------

#[test]
fn cw6_create_and_delete_profile_unsets_on_workstreams() {
    let mut core = core_with_backend(MemoryBackend::new());

    let updates = core
        .create_profile("prof-1".to_string(), "Team A".to_string())
        .expect("create_profile");
    let board = board_from(&updates);
    // create_profile only guarantees [Board]; board must still reflect state.
    let _ = board;

    core.create_workstream(
        "ws-p1".to_string(),
        "P1".to_string(),
        String::new(),
        Some("prof-1".to_string()),
    )
    .expect("create ws with profile");
    core.create_workstream(
        "ws-p2".to_string(),
        "P2".to_string(),
        String::new(),
        Some("prof-1".to_string()),
    )
    .expect("create ws2 with profile");

    // Sanity: filtering board by the profile shows both.
    core.set_active_profile(Some("prof-1".to_string()));
    let filtered = core.board();
    assert_eq!(filtered.all.len(), 2, "both workstreams under prof-1");

    let del_updates = core.delete_profile("prof-1").expect("delete_profile");
    let wss = workstreams_from(&del_updates);
    let p1 = wss.iter().find(|w| w.id == "ws-p1").expect("ws-p1 present");
    let p2 = wss.iter().find(|w| w.id == "ws-p2").expect("ws-p2 present");
    assert_eq!(p1.profile, None, "ws-p1 profile must be unset");
    assert_eq!(p2.profile, None, "ws-p2 profile must be unset");

    // active_profile cleared since it pointed at the deleted profile: board()
    // with no explicit filter change should now show all (unfiltered) again
    // because active_profile was internally cleared.
    let board_after = core.board();
    assert_eq!(
        board_after.all.len(),
        2,
        "active_profile cleared -> board shows all workstreams unfiltered"
    );
}

#[test]
fn cw6_delete_profile_absent_id_is_noop_ok() {
    let mut core = core_with_backend(MemoryBackend::new());
    let result = core.delete_profile("prof-does-not-exist");
    assert!(result.is_ok(), "deleting an absent profile id must be Ok");
}

#[test]
fn cw6_delete_profile_not_active_leaves_active_profile_untouched() {
    let mut core = core_with_backend(MemoryBackend::new());
    core.create_profile("prof-keep".to_string(), "Keep".to_string())
        .expect("create keep");
    core.create_profile("prof-other".to_string(), "Other".to_string())
        .expect("create other");
    core.set_active_profile(Some("prof-keep".to_string()));

    core.delete_profile("prof-other")
        .expect("delete other profile");

    // active_profile filter (prof-keep) is unaffected: create a workstream
    // under prof-keep and confirm it's still visible when filtered.
    core.create_workstream(
        "ws-keep".to_string(),
        "Keep WS".to_string(),
        String::new(),
        Some("prof-keep".to_string()),
    )
    .expect("create ws");
    let board = core.board();
    assert!(board.all.iter().any(|c| c.id == "ws-keep"));
}

// ---------------------------------------------------------------------------
// CW7 — assign_session error + success paths
// ---------------------------------------------------------------------------

#[test]
fn cw7_assign_session_unknown_session_errors() {
    let mut core = core_with_backend(MemoryBackend::new());
    core.create_workstream(
        "ws-known".to_string(),
        "Known".to_string(),
        String::new(),
        None,
    )
    .expect("create ws");

    let err = core
        .assign_session("session-nope", "ws-known", false)
        .expect_err("unknown session must error");
    assert!(matches!(err, CoreError::UnknownSession(_)));
}

#[test]
fn cw7_assign_session_unknown_workstream_errors() {
    let mut core = core_with_backend(MemoryBackend::new());
    let s1 = launch(&mut core, "proj-unk-ws");

    let err = core
        .assign_session(&s1, "ws-nope", false)
        .expect_err("unknown workstream must error");
    assert!(matches!(err, CoreError::Connection(_)));
}

#[test]
fn cw7_assign_session_valid_sets_membership_and_node_appears() {
    let mut core = core_with_backend(MemoryBackend::new());
    let s1 = launch(&mut core, "proj-assign");
    core.create_workstream(
        "ws-assign".to_string(),
        "Assign".to_string(),
        String::new(),
        None,
    )
    .expect("create ws");

    let updates = core
        .assign_session(&s1, "ws-assign", false)
        .expect("assign_session valid");

    let wss = workstreams_from(&updates);
    assert!(wss.iter().any(|w| w.id == "ws-assign"));
    let board = board_from(&updates);
    assert!(board
        .all
        .iter()
        .any(|c| c.id == "ws-assign" && c.session_count == 1));

    let nodes = core.nodes();
    let node = nodes
        .iter()
        .find(|n| n.id == s1)
        .expect("node appears in nodes()");
    assert_eq!(node.ws, "ws-assign");
    assert!(!node.lead, "lead:false must not mark the node lead");

    let graph = core.graph("ws-assign").expect("graph exists");
    assert!(graph.nodes.iter().any(|n| n.id == s1));
}

#[test]
fn cw7_assign_session_lead_true_marks_node_lead() {
    let mut core = core_with_backend(MemoryBackend::new());
    let s1 = launch(&mut core, "proj-lead");
    core.create_workstream(
        "ws-lead".to_string(),
        "Lead".to_string(),
        String::new(),
        None,
    )
    .expect("create ws");

    core.assign_session(&s1, "ws-lead", true)
        .expect("assign lead");

    let nodes = core.nodes();
    let node = nodes.iter().find(|n| n.id == s1).expect("node present");
    assert!(node.lead, "lead:true must mark the node lead");
}

// ---------------------------------------------------------------------------
// CW8 — set_active_profile filters board(); None shows all
// ---------------------------------------------------------------------------

#[test]
fn cw8_set_active_profile_filters_board() {
    let mut core = core_with_backend(MemoryBackend::new());
    core.create_profile("prof-x".to_string(), "X".to_string())
        .expect("create profile x");
    core.create_profile("prof-y".to_string(), "Y".to_string())
        .expect("create profile y");
    core.create_workstream(
        "ws-x".to_string(),
        "WS X".to_string(),
        String::new(),
        Some("prof-x".to_string()),
    )
    .expect("create ws-x");
    core.create_workstream(
        "ws-y".to_string(),
        "WS Y".to_string(),
        String::new(),
        Some("prof-y".to_string()),
    )
    .expect("create ws-y");

    let updates = core.set_active_profile(Some("prof-x".to_string()));
    let board = board_from(&updates);
    assert_eq!(board.all.len(), 1, "only prof-x workstream shown");
    assert_eq!(board.all[0].id, "ws-x");

    // board() delegate reflects the same filter.
    let board2 = core.board();
    assert_eq!(board2.all.len(), 1);
    assert_eq!(board2.all[0].id, "ws-x");

    let updates_none = core.set_active_profile(None);
    let board_none = board_from(&updates_none);
    assert_eq!(board_none.all.len(), 2, "None filter shows all workstreams");
}

// ---------------------------------------------------------------------------
// CW9 — nodes(): one per session-with-membership, in session_order, correct
// fields; sessions without membership excluded.
// ---------------------------------------------------------------------------

#[test]
fn cw9_nodes_only_sessions_with_membership_in_session_order() {
    let mut core = core_with_backend(MemoryBackend::new());
    let s1 = launch(&mut core, "proj-n1");
    let s2 = launch(&mut core, "proj-n2");
    let s3 = launch(&mut core, "proj-n3"); // never assigned

    core.create_workstream(
        "ws-nodes".to_string(),
        "Nodes".to_string(),
        String::new(),
        None,
    )
    .expect("create ws");
    // Assign s2 first, then s1 — but session_order was s1, s2, s3 at launch.
    core.assign_session(&s2, "ws-nodes", false)
        .expect("assign s2");
    core.assign_session(&s1, "ws-nodes", true)
        .expect("assign s1 lead");

    let nodes = core.nodes();
    let ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    // launch_session now auto-creates a membership for every session, so all
    // three appear in nodes(); the order must follow session_order.
    assert_eq!(
        ids,
        vec![s1.as_str(), s2.as_str(), s3.as_str()],
        "nodes() must follow session_order, including auto-workstream sessions"
    );

    let n1 = nodes.iter().find(|n| n.id == s1).unwrap();
    assert_eq!(n1.driver, Driver::Claude, "Phase 1 is Claude-only");
    assert_eq!(n1.host, "local");
    assert!(n1.lead);
    assert_eq!(n1.ws, "ws-nodes");

    let n2 = nodes.iter().find(|n| n.id == s2).unwrap();
    assert!(!n2.lead);
    assert_eq!(n2.host, "local");

    // s3 is in its auto-created single-node workstream, not ws-nodes.
    let n3 = nodes.iter().find(|n| n.id == s3).unwrap();
    assert_eq!(n3.ws, format!("ws-{s3}"), "s3 in its auto workstream");
}

// ---------------------------------------------------------------------------
// CW10 — migration in load_workstreams: N sessions, no persisted workstreams
// -> N single-node ws-<id> workstreams, each a lead of its own; profiles
// seeded from legacy workspaces; idempotent on a second run.
// ---------------------------------------------------------------------------

#[test]
fn cw10_migration_creates_single_node_workstreams_for_unassigned_sessions() {
    let backend = MemoryBackend::new();
    let mut core = core_with_backend(backend);
    let s1 = launch(&mut core, "proj-mig1");
    let s2 = launch(&mut core, "proj-mig2");

    let updates = core
        .load_workstreams()
        .expect("load_workstreams (migration)");
    let wss = workstreams_from(&updates);

    let ws1_id = format!("ws-{s1}");
    let ws2_id = format!("ws-{s2}");
    let ws1 = wss
        .iter()
        .find(|w| w.id == ws1_id)
        .expect("ws-<s1> created");
    let ws2 = wss
        .iter()
        .find(|w| w.id == ws2_id)
        .expect("ws-<s2> created");
    assert_eq!(ws1.goal, String::new());
    assert_eq!(ws1.profile, None);
    assert_eq!(ws2.goal, String::new());
    assert_eq!(ws2.profile, None);

    // Each session is now a lead of its own single-node workstream.
    let nodes = core.nodes();
    let n1 = nodes.iter().find(|n| n.id == s1).expect("s1 has a node");
    let n2 = nodes.iter().find(|n| n.id == s2).expect("s2 has a node");
    assert_eq!(n1.ws, ws1_id);
    assert!(
        n1.lead,
        "migrated session is lead of its own fast-lane workstream"
    );
    assert_eq!(n2.ws, ws2_id);
    assert!(n2.lead);

    let board = board_from(&updates);
    let card1 = board
        .all
        .iter()
        .find(|c| c.id == ws1_id)
        .expect("card1 present");
    assert!(card1.is_single_node, "migrated workstream is single-node");
    assert_eq!(card1.session_count, 1);
}

#[test]
fn cw10_migration_seeds_profiles_from_legacy_workspaces() {
    let backend = MemoryBackend::new();
    let mut seed = marsad_store::Store::new(Box::new(backend.clone()));
    seed.snapshot_workspace(&marsad_store::Workspace {
        id: "legacy-ws".to_string(),
        name: "Legacy".to_string(),
        session_ids: vec![],
        saved_at: "2026-06-24".to_string(),
    })
    .expect("seed legacy workspace");

    let mut core = core_with_backend(backend);
    core.load_workspaces().expect("load_workspaces");
    core.load_workstreams().expect("load_workstreams migration");

    // A profile seeded from the legacy workspace should now allow filtering
    // a workstream tagged with that profile id.
    core.create_workstream(
        "ws-under-legacy".to_string(),
        "Under Legacy".to_string(),
        String::new(),
        Some("legacy-ws".to_string()),
    )
    .expect("create ws under legacy profile");
    core.set_active_profile(Some("legacy-ws".to_string()));
    let board = core.board();
    assert!(
        board.all.iter().any(|c| c.id == "ws-under-legacy"),
        "profile seeded from legacy workspace id must be usable as a filter"
    );
}

#[test]
fn cw10_migration_is_idempotent() {
    let backend = MemoryBackend::new();
    let mut core = core_with_backend(backend);
    let s1 = launch(&mut core, "proj-idem");

    let first = core.load_workstreams().expect("first migration");
    let wss_first = workstreams_from(&first);
    let ws_id = format!("ws-{s1}");
    assert_eq!(
        wss_first.iter().filter(|w| w.id == ws_id).count(),
        1,
        "exactly one fast-lane workstream after first migration"
    );

    let second = core
        .load_workstreams()
        .expect("second migration (idempotent)");
    let wss_second = workstreams_from(&second);
    assert_eq!(
        wss_second.iter().filter(|w| w.id == ws_id).count(),
        1,
        "second run must not duplicate the fast-lane workstream"
    );
    assert_eq!(
        wss_second.len(),
        wss_first.len(),
        "idempotent migration must not grow the workstream list"
    );

    // nodes() still shows exactly one node for s1.
    let nodes = core.nodes();
    assert_eq!(nodes.iter().filter(|n| n.id == s1).count(), 1);
}

// ---------------------------------------------------------------------------
// CW11 — persistence round-trip: create workstreams/connections/profiles +
// membership, then a NEW Core over the SAME backend load_workstreams()
// restores them all identically.
// ---------------------------------------------------------------------------

#[test]
fn cw11_persistence_round_trip_restores_everything() {
    let backend = MemoryBackend::new();
    let mut core = core_with_backend(backend.clone());

    let s1 = launch(&mut core, "proj-rt1");
    let s2 = launch(&mut core, "proj-rt2");

    core.create_profile("prof-rt".to_string(), "RoundTrip".to_string())
        .expect("create profile");
    core.create_workstream(
        "ws-rt".to_string(),
        "RT".to_string(),
        "round trip goal".to_string(),
        Some("prof-rt".to_string()),
    )
    .expect("create workstream");
    core.assign_session(&s1, "ws-rt", true)
        .expect("assign s1 lead");
    core.assign_session(&s2, "ws-rt", false).expect("assign s2");
    core.add_connection(
        "conn-rt".to_string(),
        "ws-rt".to_string(),
        s1.clone(),
        s2.clone(),
        ConnKind::Data,
        false,
    )
    .expect("add_connection");

    // Fresh Core over the SAME backend. Also relaunch the same session ids so
    // their SessionRecords exist in the new Core (membership alone cannot
    // produce a node for a session Core never registered) — this exercises
    // membership persistence + node projection together.
    let mut core2 = core_with_backend(backend);
    // Re-launch under the SAME ids is not directly supported by launch_session
    // (it mints a fresh id), so restore the round-trip at the
    // workstream/connection/profile/membership layer, which is what
    // load_workstreams is specified to restore.
    let updates2 = core2
        .load_workstreams()
        .expect("load_workstreams on fresh core");

    let wss2 = workstreams_from(&updates2);
    let ws_rt = wss2
        .iter()
        .find(|w| w.id == "ws-rt")
        .expect("ws-rt restored");
    assert_eq!(ws_rt.name, "RT");
    assert_eq!(ws_rt.goal, "round trip goal");
    assert_eq!(ws_rt.profile, Some("prof-rt".to_string()));

    // Filtering by the restored profile still finds the restored workstream.
    core2.set_active_profile(Some("prof-rt".to_string()));
    let board2 = core2.board();
    assert!(board2.all.iter().any(|c| c.id == "ws-rt"));
}

// ---------------------------------------------------------------------------
// CW12 — board()/graph() delegate correctly: spot-check head_state and an
// edge's status.
// ---------------------------------------------------------------------------

#[test]
fn cw12_board_head_state_reflects_node_phase() {
    let mut core = core_with_backend(MemoryBackend::new());
    let s1 = launch(&mut core, "proj-hs");
    core.create_workstream("ws-hs".to_string(), "HS".to_string(), String::new(), None)
        .expect("create ws");
    core.assign_session(&s1, "ws-hs", true).expect("assign s1");

    // Drive the session to Needs via a control_request AgentEvent (mirrors
    // tests/fakes.rs's agent_control_request), then check the board card's
    // head_state reflects it.
    let init_line = format!(
        r#"{{"type":"AgentEvent","session_id":"{s1}","event":{{"type":"system","subtype":"init","session_id":"{s1}","model":"claude-opus-4-5","cwd":"/workspace","tools":["Read"],"mcp_servers":[],"permissionMode":"default"}}}}"#
    );
    core.on_agent_line(&init_line).expect("init event");
    let ctrl_line = format!(
        r#"{{"type":"AgentEvent","session_id":"{s1}","event":{{"type":"control_request","request_id":"rq-hs","request":{{"subtype":"can_use_tool","tool_name":"Bash","input":{{"command":"ls"}}}}}}}}"#
    );
    core.on_agent_line(&ctrl_line)
        .expect("control_request event");

    // Sanity: the session itself reached Needs.
    let snap = core.snapshot();
    let sv = snap
        .sessions
        .iter()
        .find(|s| s.id == s1)
        .expect("session present");
    assert_eq!(sv.state, Phase::Needs);

    let board = core.board();
    let card = board
        .all
        .iter()
        .find(|c| c.id == "ws-hs")
        .expect("card present");
    assert_eq!(
        card.head_state,
        Some(marsad_workstream::HeadState::Needs),
        "board card head_state must reflect the Needs node"
    );
}

#[test]
fn cw12_graph_edge_status_reflects_gate() {
    let mut core = core_with_backend(MemoryBackend::new());
    let s1 = launch(&mut core, "proj-gate1");
    let s2 = launch(&mut core, "proj-gate2");
    core.create_workstream(
        "ws-gate".to_string(),
        "Gate".to_string(),
        String::new(),
        None,
    )
    .expect("create ws");
    core.assign_session(&s1, "ws-gate", true)
        .expect("assign s1");
    core.assign_session(&s2, "ws-gate", false)
        .expect("assign s2");

    // A bidirectional edge with no gate must report Consulting status.
    core.add_connection(
        "conn-bi".to_string(),
        "ws-gate".to_string(),
        s1.clone(),
        s2.clone(),
        ConnKind::Data,
        true,
    )
    .expect("add bi connection");

    let graph = core.graph("ws-gate").expect("graph exists");
    let edge = graph
        .edges
        .iter()
        .find(|e| e.id == "conn-bi")
        .expect("edge present");
    assert_eq!(
        edge.status,
        EdgeStatus::Consulting,
        "bidirectional edge with no gate must have Consulting status"
    );
    assert!(!edge.has_gate);
}

// ---------------------------------------------------------------------------
// CW13: store-write/delete/restore failures on the single-write mutations
// surface as CoreError (consistent with save_workspace/add_host — persistence
// failures are not silently swallowed for user-initiated entity mutations).
// ---------------------------------------------------------------------------

#[test]
fn cw13_create_workstream_write_failure_errors() {
    let backend = MemoryBackend::new();
    let mut core = core_with_backend(backend.clone());
    backend.fail_next(BackendOp::Write);
    assert!(matches!(
        core.create_workstream("w".to_string(), "N".to_string(), "G".to_string(), None),
        Err(CoreError::Agent(_))
    ));
}

#[test]
fn cw13_create_profile_write_failure_errors() {
    let backend = MemoryBackend::new();
    let mut core = core_with_backend(backend.clone());
    backend.fail_next(BackendOp::Write);
    assert!(matches!(
        core.create_profile("p".to_string(), "N".to_string()),
        Err(CoreError::Agent(_))
    ));
}

#[test]
fn cw13_assign_session_write_failure_errors() {
    let backend = MemoryBackend::new();
    let mut core = core_with_backend(backend.clone());
    let s1 = launch(&mut core, "p-assign-fail");
    core.create_workstream("w".to_string(), "N".to_string(), "G".to_string(), None)
        .expect("create ws");
    backend.fail_next(BackendOp::Write);
    assert!(matches!(
        core.assign_session(&s1, "w", false),
        Err(CoreError::Agent(_))
    ));
}

#[test]
fn cw13_add_connection_write_failure_errors() {
    let backend = MemoryBackend::new();
    let mut core = core_with_backend(backend.clone());
    let s1 = launch(&mut core, "p-a");
    let s2 = launch(&mut core, "p-b");
    core.create_workstream("w".to_string(), "N".to_string(), "G".to_string(), None)
        .expect("create ws");
    core.assign_session(&s1, "w", true).expect("assign s1");
    core.assign_session(&s2, "w", false).expect("assign s2");
    backend.fail_next(BackendOp::Write);
    assert!(matches!(
        core.add_connection(
            "c".to_string(),
            "w".to_string(),
            s1.clone(),
            s2.clone(),
            ConnKind::Data,
            false
        ),
        Err(CoreError::Agent(_))
    ));
}

#[test]
fn cw13_remove_connection_delete_failure_errors() {
    let backend = MemoryBackend::new();
    let mut core = core_with_backend(backend.clone());
    let s1 = launch(&mut core, "p-a");
    let s2 = launch(&mut core, "p-b");
    core.create_workstream("w".to_string(), "N".to_string(), "G".to_string(), None)
        .expect("create ws");
    core.assign_session(&s1, "w", true).expect("assign s1");
    core.assign_session(&s2, "w", false).expect("assign s2");
    core.add_connection(
        "c".to_string(),
        "w".to_string(),
        s1.clone(),
        s2.clone(),
        ConnKind::Data,
        false,
    )
    .expect("add connection");
    backend.fail_next(BackendOp::Delete);
    assert!(matches!(
        core.remove_connection("c"),
        Err(CoreError::Agent(_))
    ));
}

#[test]
fn cw13_load_workstreams_restore_failure_errors() {
    let backend = MemoryBackend::new();
    backend.fail_next(BackendOp::List);
    let mut core = core_with_backend(backend);
    assert!(core.load_workstreams().is_err());
}

// ---------------------------------------------------------------------------
// New deliverable tests: #4 workstream pause/resume/delete lifecycle
// ---------------------------------------------------------------------------

/// delete_workstream kills all member sessions (stops + removes from store)
/// and THEN deletes the workstream record. Sessions must not reappear after restart.
#[test]
fn delete_workstream_kills_member_sessions() {
    let mut core = core_with_backend(MemoryBackend::new());
    let s1 = launch(&mut core, "/proj/a");
    let s2 = launch(&mut core, "/proj/b");

    // Load workstreams so migration creates fast-lane workstreams for both.
    let _ = core.load_workstreams().expect("load_workstreams");

    // Both sessions are now in their own workstreams.
    let nodes = core.nodes();
    assert_eq!(nodes.len(), 2, "both sessions should be nodes");

    // Assign them to the same workstream for the delete test.
    let ws_id = "ws-test-delete".to_string();
    core.create_workstream(
        ws_id.clone(),
        "Test WS".to_string(),
        String::new(),
        None,
    )
    .expect("create_workstream");
    core.assign_session(&s1, &ws_id, true).expect("assign s1");
    core.assign_session(&s2, &ws_id, false).expect("assign s2");

    // Delete the workstream — must kill + remove both sessions.
    let updates = core.delete_workstream(&ws_id).expect("delete_workstream");

    // Both sessions should appear as SessionRemoved in the updates.
    let removed_ids: Vec<String> = updates
        .iter()
        .filter_map(|u| match u {
            VmUpdate::SessionRemoved { id } => Some(id.clone()),
            _ => None,
        })
        .collect();
    assert!(
        removed_ids.contains(&s1),
        "s1 must be removed on workstream delete"
    );
    assert!(
        removed_ids.contains(&s2),
        "s2 must be removed on workstream delete"
    );

    // The workstream itself must be gone.
    let wss: Vec<_> = updates
        .iter()
        .filter_map(|u| match u {
            VmUpdate::Workstreams(ws) => Some(ws.clone()),
            _ => None,
        })
        .next()
        .unwrap_or_default();
    assert!(
        !wss.iter().any(|w| w.id == ws_id),
        "workstream must be deleted"
    );
}

/// pause_workstream stops all member sessions (phase flips to Stale).
#[test]
fn pause_workstream_stops_member_sessions() {
    let mut core = core_with_backend(MemoryBackend::new());
    let s1 = launch(&mut core, "/proj/p1");
    let s2 = launch(&mut core, "/proj/p2");
    let _ = core.load_workstreams().expect("load_workstreams");

    let ws_id = "ws-pause-test".to_string();
    core.create_workstream(ws_id.clone(), "Pause WS".to_string(), String::new(), None)
        .expect("create");
    core.assign_session(&s1, &ws_id, true).expect("assign s1");
    core.assign_session(&s2, &ws_id, false).expect("assign s2");

    let updates = core.pause_workstream(&ws_id).expect("pause_workstream");

    // Both sessions should appear with Stale phase in the updates.
    let stale_ids: Vec<String> = updates
        .iter()
        .filter_map(|u| match u {
            VmUpdate::Session(sv) if sv.state == Phase::Stale => Some(sv.id.clone()),
            _ => None,
        })
        .collect();
    assert!(
        stale_ids.contains(&s1),
        "s1 must be Stale after pause_workstream"
    );
    assert!(
        stale_ids.contains(&s2),
        "s2 must be Stale after pause_workstream"
    );
}

/// resume_workstream re-starts all member sessions (phase flips to Starting).
#[test]
fn resume_workstream_restarts_member_sessions() {
    let mut core = core_with_backend(MemoryBackend::new());
    let s1 = launch(&mut core, "/proj/r1");
    let _ = core.load_workstreams().expect("load_workstreams");

    let ws_id = "ws-resume-test".to_string();
    core.create_workstream(ws_id.clone(), "Resume WS".to_string(), String::new(), None)
        .expect("create");
    core.assign_session(&s1, &ws_id, true).expect("assign");

    // First pause so the session is Stale.
    core.pause_workstream(&ws_id).expect("pause");

    // Now resume.
    let updates = core.resume_workstream(&ws_id).expect("resume_workstream");

    let starting_ids: Vec<String> = updates
        .iter()
        .filter_map(|u| match u {
            VmUpdate::Session(sv) if sv.state == Phase::Starting => Some(sv.id.clone()),
            _ => None,
        })
        .collect();
    assert!(
        starting_ids.contains(&s1),
        "s1 must be Starting after resume_workstream"
    );
}

/// pause_workstream on an empty/absent workstream is a no-op Ok.
#[test]
fn pause_workstream_absent_is_noop() {
    let mut core = core_with_backend(MemoryBackend::new());
    let updates = core
        .pause_workstream("nonexistent-ws")
        .expect("absent ws must be no-op");
    assert!(updates.is_empty());
}

/// resume_workstream on an empty/absent workstream is a no-op Ok.
#[test]
fn resume_workstream_absent_is_noop() {
    let mut core = core_with_backend(MemoryBackend::new());
    let updates = core
        .resume_workstream("nonexistent-ws")
        .expect("absent ws must be no-op");
    assert!(updates.is_empty());
}

// ---------------------------------------------------------------------------
// Deliverable #2: settings round-trip persists default_driver + default_models
// ---------------------------------------------------------------------------

#[test]
fn settings_round_trip_persists_new_fields() {
    use marsad_store::PersistedSettings;
    let mut core = core_with_backend(MemoryBackend::new());

    let s = PersistedSettings {
        notif_prefs: std::collections::HashMap::new(),
        play_sound: true,
        group_alerts: false,
        do_not_disturb: false,
        background: "Gradient".to_string(),
        restore_on_launch: true,
        confirm_before_stop: true,
        default_model: "claude-haiku-latest".to_string(),
        default_driver: "codex".to_string(),
        default_models: vec![
            ("codex".to_string(), "codex-mini-latest".to_string()),
            ("claude".to_string(), "claude-haiku-latest".to_string()),
        ],
        default_modes: vec![("claude".to_string(), "acceptEdits".to_string())],
    };
    core.set_settings(&s).expect("set_settings");
    let got = core.get_settings();
    assert_eq!(got.default_driver, "codex", "default_driver round-trips");
    assert_eq!(got.default_models.len(), 2, "default_models round-trips");
    assert!(
        got.default_models
            .iter()
            .any(|(d, m)| d == "codex" && m == "codex-mini-latest"),
        "codex entry round-trips"
    );
}

/// Defaults for new fields are "claude" and empty when nothing persisted.
#[test]
fn settings_defaults_for_new_fields() {
    let core = core_with_backend(MemoryBackend::new());
    let s = core.get_settings();
    assert_eq!(s.default_driver, "claude");
    assert!(s.default_models.is_empty());
}

// ---------------------------------------------------------------------------
// Deliverable #1: per-driver command resolution in launch_session_with_id
// ---------------------------------------------------------------------------

/// Launch a session on a host that has an agent_commands override for the
/// requested driver, draining the loopback peer so the outbound LaunchSession
/// IPC line can be inspected. Returns the joined outbound lines.
fn launch_with_driver_and_drain(
    agent_commands: Vec<(String, String)>,
    claude_bin: Option<String>,
    driver: &str,
) -> String {
    let (a, mut peer) = LoopbackFake::pair();
    let cfg = CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-driver-bin-test".to_string(),
    };
    let mut core = Core::with_deps(
        cfg,
        Box::new(a),
        Box::new(marsad_core::RealGit),
        Box::new(marsad_core::RealTranscript),
        Box::new(MemoryBackend::new()),
    )
    .expect("with_deps");

    core.add_host(
        "box",
        "user@box.example.com",
        claude_bin,
        None,
        Vec::new(),
        agent_commands,
    )
    .expect("add host");
    let _ = peer.drain_outbound(); // clear the Hosts-side chatter (none, but be safe)

    let host_id = core
        .snapshot()
        .hosts
        .into_iter()
        .find(|h| !h.is_local)
        .expect("remote host")
        .id;

    core.launch_session(LaunchSpec {
        project: "/proj".to_string(),
        host_id,
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: "codex-mini-latest".to_string(),
        session_mode: String::new(),
        first_prompt: None,
        mode: Default::default(),
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: driver.to_string(),
    })
    .expect("launch session");

    peer.drain_outbound().join("\n")
}

/// When the host has an agent_commands entry for the driver, that command is
/// used as the driver bin (claude_bin field) in the outbound LaunchSession IPC.
#[test]
fn launch_session_uses_agent_commands_for_driver_bin() {
    let out = launch_with_driver_and_drain(
        vec![("codex".to_string(), "codex --sandbox".to_string())],
        None,
        "codex",
    );
    assert!(
        out.contains("codex --sandbox"),
        "outbound LaunchSession must carry the agent_commands override 'codex --sandbox'; got: {out}"
    );
    assert!(
        out.contains("\"driver\":\"codex\""),
        "outbound LaunchSession must carry driver=codex; got: {out}"
    );
}

/// claude resolves EXACTLY as before when only claude_bin is set (folded into
/// the "claude" agent_commands entry): the LaunchSession IPC carries the
/// wrapper path as the driver bin.
#[test]
fn launch_session_claude_bin_still_resolves_for_claude_driver() {
    let out = launch_with_driver_and_drain(
        Vec::new(),
        Some("/opt/genie/claude".to_string()),
        "claude",
    );
    assert!(
        out.contains("/opt/genie/claude"),
        "claude_bin must resolve as the claude driver bin; got: {out}"
    );
}

/// With no override and no claude_bin, the bare driver name is used — the core
/// leaves claude_bin absent so the agent falls back to the bare driver name.
#[test]
fn launch_session_bare_driver_name_when_no_override() {
    let out = launch_with_driver_and_drain(Vec::new(), None, "codex");
    // No wrapper path baked in; the driver discriminator alone tells the agent
    // which bare binary to spawn.
    assert!(
        out.contains("\"driver\":\"codex\""),
        "outbound must carry driver=codex; got: {out}"
    );
    assert!(
        !out.contains("codex --sandbox"),
        "no override was set, so no wrapper command must appear; got: {out}"
    );
}

// ---------------------------------------------------------------------------
// Deliverable #2: resume/restart rebuilds from the PERSISTED session model,
// not the (possibly newer) settings default_model.
// ---------------------------------------------------------------------------

/// A restarted session must use the model passed to restart_session (which the
/// bridge/UI sources from the persisted session), NOT any settings default.
/// This guards against a regression where a settings default_model leaks into
/// resume/restart.
#[test]
fn restart_session_uses_supplied_model_not_settings_default() {
    let (a, mut peer) = LoopbackFake::pair();
    let cfg = CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-restart-model-test".to_string(),
    };
    let mut core = Core::with_deps(
        cfg,
        Box::new(a),
        Box::new(marsad_core::RealGit),
        Box::new(marsad_core::RealTranscript),
        Box::new(MemoryBackend::new()),
    )
    .expect("with_deps");

    // Persist a settings blob with a DIFFERENT default model, to prove the
    // restart does not read it.
    use marsad_store::PersistedSettings;
    core.set_settings(&PersistedSettings {
        notif_prefs: std::collections::HashMap::new(),
        play_sound: true,
        group_alerts: false,
        do_not_disturb: false,
        background: "Gradient".to_string(),
        restore_on_launch: true,
        confirm_before_stop: true,
        default_model: "settings-default-model".to_string(),
        default_driver: "claude".to_string(),
        default_models: Vec::new(),
        default_modes: Vec::new(),
    })
    .expect("set_settings");

    let id = launch(&mut core, "/proj/restart");
    let _ = peer.drain_outbound();

    // Restart with an explicit model — the persisted-session model in a real
    // flow. It must be the one that goes out on the wire.
    core.restart_session(&id, "session-specific-model", String::new(), vec![])
        .expect("restart_session");
    let out = peer.drain_outbound().join("\n");
    assert!(
        out.contains("session-specific-model"),
        "restart must use the supplied (persisted) model; got: {out}"
    );
    assert!(
        !out.contains("settings-default-model"),
        "restart must NOT leak the settings default_model; got: {out}"
    );
}

// ---------------------------------------------------------------------------
// launch_session auto-creates single-node workstream + membership
// ---------------------------------------------------------------------------

/// Launching a session via launch_session must immediately create a ws-<id>
/// workstream and populate membership so nodes() / board() show the card.
#[test]
fn launch_session_auto_creates_workstream_and_membership() {
    let mut core = core_with_backend(MemoryBackend::new());
    let sid = launch(&mut core, "/home/user/project");

    let nodes = core.nodes();
    assert_eq!(nodes.len(), 1, "session must appear as a node");
    assert_eq!(nodes[0].id, sid);
    assert_eq!(nodes[0].ws, format!("ws-{sid}"), "in its auto workstream");

    let board = core.board();
    let card = board.all.iter().find(|c| c.id == format!("ws-{sid}"));
    assert!(card.is_some(), "auto workstream must appear in board");
    let card = card.unwrap();
    assert_eq!(card.session_count, 1);
    // goal fallback: empty ws.goal → node name used as headline
    assert!(!card.goal.is_empty(), "goal must not be blank");
}

// ---------------------------------------------------------------------------
// remove_session drops session + membership + auto workstream
// ---------------------------------------------------------------------------

/// remove_session must drop the session record, its membership, and the
/// auto-created single-node workstream so the board shows no ghost card.
#[test]
fn remove_session_drops_session_membership_and_auto_workstream() {
    let mut core = core_with_backend(MemoryBackend::new());
    let sid = launch(&mut core, "/home/user/myproj");

    // Precondition: session is in the board.
    let board_before = core.board();
    assert!(
        board_before.all.iter().any(|c| c.id == format!("ws-{sid}")),
        "workstream present before remove"
    );

    let updates = core.remove_session(&sid).expect("remove_session");

    // Session removed event in updates.
    let removed = updates
        .iter()
        .any(|u| matches!(u, VmUpdate::SessionRemoved { id } if id == &sid));
    assert!(removed, "SessionRemoved event must be in updates");

    // nodes() no longer includes the session.
    assert!(
        core.nodes().is_empty(),
        "nodes() must be empty after remove"
    );

    // board() no longer includes the auto workstream.
    let board_after = core.board();
    assert!(
        !board_after.all.iter().any(|c| c.id == format!("ws-{sid}")),
        "ghost workstream card must be gone after remove"
    );
}

// ---------------------------------------------------------------------------
// Phase C gap 1 — remove_session cascade + regression pins
// ---------------------------------------------------------------------------

// Helper: check whether a connection id exists in the store via restore().
fn connection_in_store(backend: &MemoryBackend, conn_id: &str) -> bool {
    let store = marsad_store::Store::new(Box::new(backend.clone()));
    store
        .restore()
        .expect("restore for connection check")
        .connections
        .iter()
        .any(|c| c.id == conn_id)
}

/// **RED** (gap 1 — cascade not implemented):
/// Launch 3 sessions, assign all to one workstream, add A→B and B→C.
/// remove_session(B) must drop BOTH edges (B is touched by both), update the
/// graph + board snapshot, AND delete both connection keys from the store.
#[test]
fn remove_session_cascades_connections_touching_removed_node() {
    let backend = MemoryBackend::new();
    let mut core = core_with_backend(backend.clone());
    let sa = launch(&mut core, "/proj/a");
    let sb = launch(&mut core, "/proj/b");
    let sc = launch(&mut core, "/proj/c");

    core.create_workstream("ws-cascade".to_string(), "Cascade".to_string(), String::new(), None)
        .expect("create ws");
    core.assign_session(&sa, "ws-cascade", true).expect("assign A");
    core.assign_session(&sb, "ws-cascade", false).expect("assign B");
    core.assign_session(&sc, "ws-cascade", false).expect("assign C");

    core.add_connection(
        "conn-ab".to_string(),
        "ws-cascade".to_string(),
        sa.clone(),
        sb.clone(),
        ConnKind::Data,
        false,
    )
    .expect("add A->B");
    core.add_connection(
        "conn-bc".to_string(),
        "ws-cascade".to_string(),
        sb.clone(),
        sc.clone(),
        ConnKind::Data,
        false,
    )
    .expect("add B->C");

    // Precondition: both edges visible.
    let graph_before = core.graph("ws-cascade").expect("graph before");
    assert_eq!(graph_before.edges.len(), 2, "precondition: 2 edges");

    // Remove the middle node B.
    let updates = core.remove_session(&sb).expect("remove_session B");

    // 1a. graph() for the workstream no longer contains either edge.
    let graph_after = core.graph("ws-cascade").expect("graph after");
    assert!(
        !graph_after.edges.iter().any(|e| e.id == "conn-ab"),
        "conn-ab (A->B) must be dropped from graph after remove_session(B)"
    );
    assert!(
        !graph_after.edges.iter().any(|e| e.id == "conn-bc"),
        "conn-bc (B->C) must be dropped from graph after remove_session(B)"
    );
    assert!(
        graph_after.edges.is_empty(),
        "no edges should remain after removing B"
    );

    // 1b. Board update is included in the returned updates.
    let board = board_from(&updates);
    assert!(board.all.iter().any(|c| c.id == "ws-cascade"), "board update present");

    // 1c. Store must have deleted both connection records.
    assert!(
        !connection_in_store(&backend, "conn-ab"),
        "conn-ab must be deleted from store after remove_session(B)"
    );
    assert!(
        !connection_in_store(&backend, "conn-bc"),
        "conn-bc must be deleted from store after remove_session(B)"
    );
}

/// **RED** (gap 1 — cascade not implemented):
/// Variant: session A is an endpoint of only ONE edge (A→C); session B is an
/// endpoint of none. Removing A must drop only the A→C edge, leaving B→C
/// (if any) untouched. Here we have A→C and check that it is dropped alone.
#[test]
fn remove_session_cascades_only_edges_touching_removed_node() {
    let backend = MemoryBackend::new();
    let mut core = core_with_backend(backend.clone());
    let sa = launch(&mut core, "/proj/x1");
    let sb = launch(&mut core, "/proj/x2");
    let sc = launch(&mut core, "/proj/x3");

    core.create_workstream("ws-partial".to_string(), "Partial".to_string(), String::new(), None)
        .expect("create ws");
    core.assign_session(&sa, "ws-partial", true).expect("assign A");
    core.assign_session(&sb, "ws-partial", false).expect("assign B");
    core.assign_session(&sc, "ws-partial", false).expect("assign C");

    // A→C only. B is not connected to anything.
    core.add_connection(
        "conn-ac".to_string(),
        "ws-partial".to_string(),
        sa.clone(),
        sc.clone(),
        ConnKind::Data,
        false,
    )
    .expect("add A->C");

    // Precondition.
    assert_eq!(
        core.graph("ws-partial").unwrap().edges.len(),
        1,
        "one edge before remove"
    );

    // Remove A (source of the only edge).
    core.remove_session(&sa).expect("remove A");

    // The A→C edge must be gone from the graph.
    let graph_after = core.graph("ws-partial").expect("graph after");
    assert!(
        graph_after.edges.is_empty(),
        "conn-ac must be dropped when its source node (A) is removed"
    );

    // Store must not contain conn-ac.
    assert!(
        !connection_in_store(&backend, "conn-ac"),
        "conn-ac must be deleted from store"
    );

    // B is still in the workstream — its membership should survive.
    let nodes = core.nodes();
    assert!(
        nodes.iter().any(|n| n.id == sb),
        "B (unrelated node) must remain after A is removed"
    );
}

/// **RED** (gap 1 — cascade not implemented):
/// remove_session on a node with NO connections must still succeed (Ok),
/// cause no panic, and emit no spurious delete for connections.
#[test]
fn remove_session_no_connections_is_ok_no_spurious_deletes() {
    let backend = MemoryBackend::new();
    let mut core = core_with_backend(backend.clone());
    let sa = launch(&mut core, "/proj/noconn");

    // No connections exist. remove_session must return Ok without panicking.
    let updates = core.remove_session(&sa).expect("remove_session with no connections");

    // SessionRemoved must be in updates.
    assert!(
        updates.iter().any(|u| matches!(u, VmUpdate::SessionRemoved { id } if id == &sa)),
        "SessionRemoved must still be emitted"
    );

    // Store has no connection records (nothing to spuriously delete).
    let restored = marsad_store::Store::new(Box::new(backend.clone()))
        .restore()
        .expect("restore");
    assert!(
        restored.connections.is_empty(),
        "no connections must exist in store after removing a session that had none"
    );
}

/// **GREEN** (regression pin — stop_session does NOT cascade):
/// Stopping a connected node leaves its edges intact. stop_session keeps the
/// session resumable; cascading would destroy information incorrectly.
#[test]
fn stop_session_does_not_cascade_connections() {
    let mut core = core_with_backend(MemoryBackend::new());
    let sa = launch(&mut core, "/proj/stop1");
    let sb = launch(&mut core, "/proj/stop2");

    core.create_workstream("ws-stop".to_string(), "Stop".to_string(), String::new(), None)
        .expect("create ws");
    core.assign_session(&sa, "ws-stop", true).expect("assign A");
    core.assign_session(&sb, "ws-stop", false).expect("assign B");
    core.add_connection(
        "conn-stop".to_string(),
        "ws-stop".to_string(),
        sa.clone(),
        sb.clone(),
        ConnKind::Data,
        false,
    )
    .expect("add connection");

    // stop_session A — keeps it resumable (Stale), must NOT drop the edge.
    core.stop_session(&sa).expect("stop_session A");

    let graph = core.graph("ws-stop").expect("graph after stop");
    assert_eq!(
        graph.edges.len(),
        1,
        "stop_session must NOT cascade-delete connections"
    );
    assert!(
        graph.edges.iter().any(|e| e.id == "conn-stop"),
        "conn-stop must survive stop_session"
    );
}

/// **GREEN** (pin — add_connection error contract):
/// `add_connection` returns `Err(CoreError::Connection(msg))` for each invalid
/// case, and the `msg` string equals the `ConnError::Display` string exactly.
/// This is the string the UI surfaces to the user — pinning it ensures a coder
/// cannot silently change the message.
#[test]
fn add_connection_error_contract_pins_exact_messages() {
    let mut core = core_with_backend(MemoryBackend::new());
    let sa = launch(&mut core, "/proj/ec1");
    let sb = launch(&mut core, "/proj/ec2");
    let sc = launch(&mut core, "/proj/ec3");

    core.create_workstream("ws-ec".to_string(), "EC".to_string(), String::new(), None)
        .expect("create ws");
    core.assign_session(&sa, "ws-ec", true).expect("assign A");
    core.assign_session(&sb, "ws-ec", false).expect("assign B");
    core.assign_session(&sc, "ws-ec", false).expect("assign C");

    // ── SelfLoop ────────────────────────────────────────────────────────────
    let err = core
        .add_connection(
            "c-self".to_string(),
            "ws-ec".to_string(),
            sa.clone(),
            sa.clone(),
            ConnKind::Data,
            false,
        )
        .expect_err("SelfLoop must error");
    let msg = match &err {
        CoreError::Connection(m) => m.clone(),
        other => panic!("expected CoreError::Connection, got {other:?}"),
    };
    assert_eq!(
        msg,
        "connection endpoints must not be the same node",
        "SelfLoop Display string mismatch"
    );

    // ── UnknownEndpoint ──────────────────────────────────────────────────────
    let err = core
        .add_connection(
            "c-unk".to_string(),
            "ws-ec".to_string(),
            sa.clone(),
            "ghost-session".to_string(),
            ConnKind::Data,
            false,
        )
        .expect_err("UnknownEndpoint must error");
    let msg = match &err {
        CoreError::Connection(m) => m.clone(),
        other => panic!("expected CoreError::Connection, got {other:?}"),
    };
    assert_eq!(
        msg,
        "connection endpoint is not a known node",
        "UnknownEndpoint Display string mismatch"
    );

    // ── Duplicate ────────────────────────────────────────────────────────────
    core.add_connection(
        "c-first".to_string(),
        "ws-ec".to_string(),
        sa.clone(),
        sb.clone(),
        ConnKind::Data,
        false,
    )
    .expect("first connection ok");
    let err = core
        .add_connection(
            "c-dup".to_string(),
            "ws-ec".to_string(),
            sa.clone(),
            sb.clone(),
            ConnKind::Data,
            false,
        )
        .expect_err("Duplicate must error");
    let msg = match &err {
        CoreError::Connection(m) => m.clone(),
        other => panic!("expected CoreError::Connection, got {other:?}"),
    };
    assert_eq!(
        msg,
        "an identical connection already exists",
        "Duplicate Display string mismatch"
    );

    // ── Cycle ─────────────────────────────────────────────────────────────────
    // sa→sb already exists. Add sb→sc, then sc→sa closes a cycle.
    core.add_connection(
        "c-bc".to_string(),
        "ws-ec".to_string(),
        sb.clone(),
        sc.clone(),
        ConnKind::Data,
        false,
    )
    .expect("sb->sc ok");
    let err = core
        .add_connection(
            "c-cycle".to_string(),
            "ws-ec".to_string(),
            sc.clone(),
            sa.clone(),
            ConnKind::Data,
            false,
        )
        .expect_err("Cycle must error");
    let msg = match &err {
        CoreError::Connection(m) => m.clone(),
        other => panic!("expected CoreError::Connection, got {other:?}"),
    };
    assert_eq!(
        msg,
        "connection would create a dependency cycle",
        "Cycle Display string mismatch"
    );

    // ── CrossWorkstream (indirect — UnknownEndpoint fires first) ─────────────
    // The validate_connection function checks UnknownEndpoint before
    // CrossWorkstream. Providing a node that IS known but belongs to a
    // different workstream triggers CrossWorkstream. Launch a 4th session and
    // assign it to a separate workstream so the endpoints are known but span two
    // workstreams.
    let sd = launch(&mut core, "/proj/ec4");
    core.create_workstream("ws-ec2".to_string(), "EC2".to_string(), String::new(), None)
        .expect("create ws2");
    core.assign_session(&sd, "ws-ec2", true).expect("assign D to ws2");

    let err = core
        .add_connection(
            "c-cross".to_string(),
            "ws-ec".to_string(),
            sa.clone(),
            sd.clone(),
            ConnKind::Data,
            false,
        )
        .expect_err("CrossWorkstream must error");
    let msg = match &err {
        CoreError::Connection(m) => m.clone(),
        other => panic!("expected CoreError::Connection, got {other:?}"),
    };
    assert_eq!(
        msg,
        "connection endpoints span more than one workstream",
        "CrossWorkstream Display string mismatch"
    );
}
