//! workstreams.rs — B06 workstreams-extension test plan (WS1-WS10).
//!
//! Covers the additive `marsad-store` API from `docs/specs/store-ext.md`:
//! - upsert_workstream / delete_workstream, restore().workstreams
//! - upsert_connection / delete_connection, restore().connections
//! - upsert_profile / delete_profile, restore().profiles
//! - corrupt/version-mismatch skip for the three new prefixes
//! - Backend write-error propagation for the three new upsert_* methods
//! - profiles_from_workspaces (pure migration helper)
//! - coexistence with existing sessions/hosts/workspaces in one restore()
//!
//! Style mirrors the sibling `happy.rs` / `errors.rs` / `branches.rs` files:
//! `MemoryBackend` fakes, `BackendOp::fail_next`, `MemoryBackend::corrupt`.

use marsad_store::{
    profiles_from_workspaces, Backend, BackendOp, MemoryBackend, PersistedHost, PersistedSession,
    Store, StoreError, Workspace,
};
use marsad_workstream::{Connection, Handoff, Profile, Workstream};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn fresh_store() -> Store {
    Store::new(Box::new(MemoryBackend::new()))
}

fn workstream(id: &str, profile: Option<&str>) -> Workstream {
    Workstream {
        id: id.to_string(),
        name: format!("Workstream {id}"),
        goal: format!("Goal for {id}"),
        profile: profile.map(|p| p.to_string()),
    }
}

fn connection(
    id: &str,
    ws: &str,
    from: &str,
    to: &str,
    bi: bool,
    gate: Option<Handoff>,
) -> Connection {
    Connection {
        id: id.to_string(),
        ws: ws.to_string(),
        from: from.to_string(),
        to: to.to_string(),
        bi,
        gate,
    }
}

fn profile(id: &str, name: &str) -> Profile {
    Profile {
        id: id.to_string(),
        name: name.to_string(),
    }
}

fn make_session(id: &str) -> PersistedSession {
    PersistedSession {
        id: id.to_string(),
        session_id: Some(format!("claude-{id}")),
        name: format!("Session {id}"),
        host_id: "localhost".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        project: "/project".to_string(),
        branch: "main".to_string(),
        model: "claude-opus-4-5".to_string(),
        last_phase: "Done".to_string(),
        mode: None,
        disallowed_tools: vec![],
        driver: "claude".to_string(),
        slash_commands: vec![],
        tools: vec![],
        mcp_servers: vec![],
    }
}

fn make_host(id: &str) -> PersistedHost {
    PersistedHost {
        id: id.to_string(),
        label: format!("lbl-{id}"),
        target: "u@h".to_string(),
        is_local: false,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        agent_commands: Vec::new(),
    }
}

fn make_workspace(id: &str, session_ids: Vec<String>) -> Workspace {
    Workspace {
        id: id.to_string(),
        name: format!("WS {id}"),
        session_ids,
        saved_at: "2026-06-18T00:00:00Z".to_string(),
    }
}

// ---------------------------------------------------------------------------
// WS1 — upsert_workstream / restore round-trip, incl. profile Some/None
// ---------------------------------------------------------------------------

#[test]
fn ws1_upsert_workstream_then_restore_returns_it_with_profile_some() {
    let mut store = fresh_store();
    let w = workstream("w-1", Some("prof-1"));
    store
        .upsert_workstream(&w)
        .expect("upsert_workstream must succeed");
    let restored = store.restore().expect("restore must succeed");
    let found = restored
        .workstreams
        .iter()
        .find(|ws| ws.id == "w-1")
        .expect("workstream w-1 must be restored");
    assert_eq!(found.name, "Workstream w-1");
    assert_eq!(found.goal, "Goal for w-1");
    assert_eq!(found.profile.as_deref(), Some("prof-1"));
}

#[test]
fn ws1_upsert_workstream_profile_none_round_trips() {
    let mut store = fresh_store();
    let w = workstream("w-none", None);
    store.upsert_workstream(&w).expect("upsert_workstream");
    let restored = store.restore().expect("restore");
    let found = restored
        .workstreams
        .iter()
        .find(|ws| ws.id == "w-none")
        .expect("workstream w-none must be restored");
    assert_eq!(found.profile, None);
}

#[test]
fn ws1_upsert_workstream_overwrite_updates_fields() {
    let mut store = fresh_store();
    let mut w = workstream("w-ow", None);
    store.upsert_workstream(&w).expect("first upsert");
    w.goal = "Updated goal".to_string();
    w.profile = Some("prof-x".to_string());
    store.upsert_workstream(&w).expect("second upsert");
    let restored = store.restore().expect("restore");
    let matches: Vec<_> = restored
        .workstreams
        .iter()
        .filter(|ws| ws.id == "w-ow")
        .collect();
    assert_eq!(matches.len(), 1, "upsert must overwrite, not duplicate");
    assert_eq!(matches[0].goal, "Updated goal");
    assert_eq!(matches[0].profile.as_deref(), Some("prof-x"));
}

// ---------------------------------------------------------------------------
// WS2 — multiple workstreams restore sorted by id ascending
// ---------------------------------------------------------------------------

#[test]
fn ws2_multiple_workstreams_restore_sorted_by_id_ascending() {
    let mut store = fresh_store();
    // Insert in a deliberately unsorted order.
    for id in ["w-charlie", "w-alpha", "w-echo", "w-bravo", "w-delta"] {
        store
            .upsert_workstream(&workstream(id, None))
            .expect("upsert_workstream");
    }
    let restored = store.restore().expect("restore");
    let ids: Vec<&str> = restored.workstreams.iter().map(|w| w.id.as_str()).collect();
    let mut expected = ids.clone();
    expected.sort();
    assert_eq!(
        ids, expected,
        "restored workstreams must be sorted by id ascending"
    );
    assert_eq!(
        ids,
        vec!["w-alpha", "w-bravo", "w-charlie", "w-delta", "w-echo"]
    );
}

// ---------------------------------------------------------------------------
// WS3 — delete_workstream removes it; absent id is Ok(())
// ---------------------------------------------------------------------------

#[test]
fn ws3_delete_workstream_removes_it_from_restore() {
    let mut store = fresh_store();
    store
        .upsert_workstream(&workstream("w-del", None))
        .expect("upsert_workstream");
    store
        .delete_workstream("w-del")
        .expect("delete_workstream must succeed");
    let restored = store.restore().expect("restore");
    assert!(
        !restored.workstreams.iter().any(|w| w.id == "w-del"),
        "deleted workstream must not be restored"
    );
}

#[test]
fn ws3_delete_workstream_absent_id_is_ok() {
    let mut store = fresh_store();
    store
        .delete_workstream("does-not-exist")
        .expect("delete_workstream on absent id must succeed (idempotent)");
}

// ---------------------------------------------------------------------------
// WS4 — upsert_connection / restore round-trip, incl. bi:true, gate:Some(Handoff)
// ---------------------------------------------------------------------------

#[test]
fn ws4_upsert_connection_round_trips_bi_true_and_gate_some() {
    let mut store = fresh_store();
    let gate = Handoff {
        payload: "review diff".to_string(),
        risk: "high".to_string(),
    };
    let c = connection(
        "c-1",
        "w-1",
        "node-a",
        "node-b",
        true,
        Some(gate.clone()),
    );
    store
        .upsert_connection(&c)
        .expect("upsert_connection must succeed");
    let restored = store.restore().expect("restore");
    let found = restored
        .connections
        .iter()
        .find(|conn| conn.id == "c-1")
        .expect("connection c-1 must be restored");
    assert_eq!(found.ws, "w-1");
    assert_eq!(found.from, "node-a");
    assert_eq!(found.to, "node-b");
    assert!(found.bi, "bi must round-trip as true");
    assert_eq!(found.gate, Some(gate));
}

#[test]
fn ws4_upsert_connection_default_bi_false_gate_none_round_trips() {
    let mut store = fresh_store();
    let c = connection(
        "c-2",
        "w-1",
        "node-a",
        "node-c",
        false,
        None,
    );
    store.upsert_connection(&c).expect("upsert_connection");
    let restored = store.restore().expect("restore");
    let found = restored
        .connections
        .iter()
        .find(|conn| conn.id == "c-2")
        .expect("connection c-2 must be restored");
    assert!(!found.bi);
    assert_eq!(found.gate, None);
}

#[test]
fn ws4_multiple_connections_restore_sorted_by_id() {
    let mut store = fresh_store();
    for id in ["c-z", "c-a", "c-m"] {
        store
            .upsert_connection(&connection(
                id,
                "w-1",
                "a",
                "b",
                false,
                None,
            ))
            .expect("upsert_connection");
    }
    let restored = store.restore().expect("restore");
    let ids: Vec<&str> = restored.connections.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, vec!["c-a", "c-m", "c-z"]);
}

// ---------------------------------------------------------------------------
// WS5 — delete_connection removes; absent id Ok(())
// ---------------------------------------------------------------------------

#[test]
fn ws5_delete_connection_removes_it_from_restore() {
    let mut store = fresh_store();
    store
        .upsert_connection(&connection(
            "c-del",
            "w-1",
            "a",
            "b",
            false,
            None,
        ))
        .expect("upsert_connection");
    store
        .delete_connection("c-del")
        .expect("delete_connection must succeed");
    let restored = store.restore().expect("restore");
    assert!(
        !restored.connections.iter().any(|c| c.id == "c-del"),
        "deleted connection must not be restored"
    );
}

#[test]
fn ws5_delete_connection_absent_id_is_ok() {
    let mut store = fresh_store();
    store
        .delete_connection("ghost-conn")
        .expect("delete_connection on absent id must succeed (idempotent)");
}

// ---------------------------------------------------------------------------
// WS6 — upsert_profile / restore round-trip sorted by id; delete_profile
// ---------------------------------------------------------------------------

#[test]
fn ws6_upsert_profile_then_restore_returns_it() {
    let mut store = fresh_store();
    store
        .upsert_profile(&profile("p-1", "Profile One"))
        .expect("upsert_profile must succeed");
    let restored = store.restore().expect("restore");
    let found = restored
        .profiles
        .iter()
        .find(|p| p.id == "p-1")
        .expect("profile p-1 must be restored");
    assert_eq!(found.name, "Profile One");
}

#[test]
fn ws6_multiple_profiles_restore_sorted_by_id() {
    let mut store = fresh_store();
    for (id, name) in [("p-zulu", "Z"), ("p-alpha", "A"), ("p-mike", "M")] {
        store
            .upsert_profile(&profile(id, name))
            .expect("upsert_profile");
    }
    let restored = store.restore().expect("restore");
    let ids: Vec<&str> = restored.profiles.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, vec!["p-alpha", "p-mike", "p-zulu"]);
}

#[test]
fn ws6_delete_profile_removes_it_from_restore() {
    let mut store = fresh_store();
    store
        .upsert_profile(&profile("p-del", "Deletable"))
        .expect("upsert_profile");
    store
        .delete_profile("p-del")
        .expect("delete_profile must succeed");
    let restored = store.restore().expect("restore");
    assert!(
        !restored.profiles.iter().any(|p| p.id == "p-del"),
        "deleted profile must not be restored"
    );
}

#[test]
fn ws6_delete_profile_absent_id_is_ok() {
    let mut store = fresh_store();
    store
        .delete_profile("ghost-profile")
        .expect("delete_profile on absent id must succeed (idempotent)");
}

// ---------------------------------------------------------------------------
// WS7 — corrupt / version-mismatch skip for the three new prefixes
// (mirrors errors.rs corrupt_*_key_returns_corrupt_error and
//  version_mismatch_returns_version_mismatch_error patterns)
// ---------------------------------------------------------------------------

#[test]
fn ws7_corrupt_workstream_key_returns_corrupt_error() {
    let backend = MemoryBackend::new();
    backend
        .write("workstream:w-corrupt", b"not valid JSON at all!!!")
        .expect("manual write");
    backend.corrupt("workstream:w-corrupt");

    let store = Store::new(Box::new(backend));
    let result = store.restore();
    match result {
        Err(StoreError::Corrupt { key }) => {
            assert!(
                key.contains("w-corrupt"),
                "Corrupt key must contain the workstream key, got: {key}"
            );
        }
        other => panic!("expected Corrupt, got: {other:?}"),
    }
}

#[test]
fn ws7_corrupt_connection_key_returns_corrupt_error() {
    let backend = MemoryBackend::new();
    backend
        .write("connection:c-corrupt", b"not valid JSON at all!!!")
        .expect("manual write");
    backend.corrupt("connection:c-corrupt");

    let store = Store::new(Box::new(backend));
    let result = store.restore();
    match result {
        Err(StoreError::Corrupt { key }) => {
            assert!(
                key.contains("c-corrupt"),
                "Corrupt key must contain the connection key, got: {key}"
            );
        }
        other => panic!("expected Corrupt, got: {other:?}"),
    }
}

#[test]
fn ws7_corrupt_profile_key_returns_corrupt_error() {
    let backend = MemoryBackend::new();
    backend
        .write("profile:p-corrupt", b"not valid JSON at all!!!")
        .expect("manual write");
    backend.corrupt("profile:p-corrupt");

    let store = Store::new(Box::new(backend));
    let result = store.restore();
    match result {
        Err(StoreError::Corrupt { key }) => {
            assert!(
                key.contains("p-corrupt"),
                "Corrupt key must contain the profile key, got: {key}"
            );
        }
        other => panic!("expected Corrupt, got: {other:?}"),
    }
}

#[test]
fn ws7_version_mismatch_on_workstream_key_returns_version_mismatch_error() {
    let backend = MemoryBackend::new();
    let blob = r#"{"version":9999,"id":"w-v","name":"W","goal":"G","profile":null}"#;
    backend
        .write("workstream:w-v", blob.as_bytes())
        .expect("manual write");

    let store = Store::new(Box::new(backend));
    let result = store.restore();
    match result {
        Err(StoreError::VersionMismatch { found, expected }) => {
            assert_eq!(found, 9999);
            assert!(expected > 0);
        }
        other => panic!("expected VersionMismatch, got: {other:?}"),
    }
}

#[test]
fn ws7_version_mismatch_on_connection_key_returns_version_mismatch_error() {
    let backend = MemoryBackend::new();
    let blob = r#"{"version":9999,"id":"c-v","ws":"w-1","from":"a","to":"b","kind":"data","bi":false,"gate":null}"#;
    backend
        .write("connection:c-v", blob.as_bytes())
        .expect("manual write");

    let store = Store::new(Box::new(backend));
    let result = store.restore();
    match result {
        Err(StoreError::VersionMismatch { found, .. }) => {
            assert_eq!(found, 9999);
        }
        other => panic!("expected VersionMismatch, got: {other:?}"),
    }
}

#[test]
fn ws7_version_mismatch_on_profile_key_returns_version_mismatch_error() {
    let backend = MemoryBackend::new();
    let blob = r#"{"version":9999,"id":"p-v","name":"P"}"#;
    backend
        .write("profile:p-v", blob.as_bytes())
        .expect("manual write");

    let store = Store::new(Box::new(backend));
    let result = store.restore();
    match result {
        Err(StoreError::VersionMismatch { found, .. }) => {
            assert_eq!(found, 9999);
        }
        other => panic!("expected VersionMismatch, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// WS8 — Backend write error on upsert_* surfaces as StoreError::Backend
// (mirrors errors.rs backend_write_failure_propagates_from_upsert_session)
// ---------------------------------------------------------------------------

#[test]
fn ws8_backend_write_failure_propagates_from_upsert_workstream() {
    let backend = MemoryBackend::new();
    backend.fail_next(BackendOp::Write);
    let mut store = Store::new(Box::new(backend));
    let w = workstream("w-fail-write", None);
    match store.upsert_workstream(&w) {
        Err(StoreError::Backend(_)) => {}
        other => panic!("expected Backend error from write failure, got: {other:?}"),
    }
}

#[test]
fn ws8_backend_write_failure_propagates_from_upsert_connection() {
    let backend = MemoryBackend::new();
    backend.fail_next(BackendOp::Write);
    let mut store = Store::new(Box::new(backend));
    let c = connection("c-fail-write", "w-1", "a", "b", false, None);
    match store.upsert_connection(&c) {
        Err(StoreError::Backend(_)) => {}
        other => panic!("expected Backend error from write failure, got: {other:?}"),
    }
}

#[test]
fn ws8_backend_write_failure_propagates_from_upsert_profile() {
    let backend = MemoryBackend::new();
    backend.fail_next(BackendOp::Write);
    let mut store = Store::new(Box::new(backend));
    let p = profile("p-fail-write", "Fail");
    match store.upsert_profile(&p) {
        Err(StoreError::Backend(_)) => {}
        other => panic!("expected Backend error from write failure, got: {other:?}"),
    }
}

#[test]
fn ws8_backend_delete_failure_propagates_from_delete_workstream() {
    let backend = MemoryBackend::new();
    backend.fail_next(BackendOp::Delete);
    let mut store = Store::new(Box::new(backend));
    match store.delete_workstream("w-x") {
        Err(StoreError::Backend(_)) => {}
        other => panic!("expected Backend error from delete failure, got: {other:?}"),
    }
}

#[test]
fn ws8_backend_delete_failure_propagates_from_delete_connection() {
    let backend = MemoryBackend::new();
    backend.fail_next(BackendOp::Delete);
    let mut store = Store::new(Box::new(backend));
    match store.delete_connection("c-x") {
        Err(StoreError::Backend(_)) => {}
        other => panic!("expected Backend error from delete failure, got: {other:?}"),
    }
}

#[test]
fn ws8_backend_delete_failure_propagates_from_delete_profile() {
    let backend = MemoryBackend::new();
    backend.fail_next(BackendOp::Delete);
    let mut store = Store::new(Box::new(backend));
    match store.delete_profile("p-x") {
        Err(StoreError::Backend(_)) => {}
        other => panic!("expected Backend error from delete failure, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// WS9 — profiles_from_workspaces: pure migration helper
// ---------------------------------------------------------------------------

#[test]
fn ws9_profiles_from_workspaces_empty_returns_empty() {
    let result = profiles_from_workspaces(&[]);
    assert!(result.is_empty(), "empty input must yield empty output");
}

#[test]
fn ws9_profiles_from_workspaces_maps_id_and_name_preserving_order() {
    let workspaces = vec![
        make_workspace("ws-c", vec!["s1".to_string()]),
        make_workspace("ws-a", vec![]),
        make_workspace("ws-b", vec!["s2".to_string(), "s3".to_string()]),
    ];
    let profiles = profiles_from_workspaces(&workspaces);
    assert_eq!(profiles.len(), 3, "N workspaces must yield N profiles");
    // Order must be preserved (input order, NOT sorted).
    assert_eq!(profiles[0].id, "ws-c");
    assert_eq!(profiles[0].name, "WS ws-c");
    assert_eq!(profiles[1].id, "ws-a");
    assert_eq!(profiles[1].name, "WS ws-a");
    assert_eq!(profiles[2].id, "ws-b");
    assert_eq!(profiles[2].name, "WS ws-b");
}

#[test]
fn ws9_profiles_from_workspaces_independent_of_saved_at_and_session_ids() {
    // Two workspaces with identical id/name but different saved_at/session_ids
    // must map to identical Profiles (those fields have no analog on Profile).
    let mut ws1 = make_workspace("ws-same", vec!["s1".to_string(), "s2".to_string()]);
    ws1.saved_at = "2020-01-01T00:00:00Z".to_string();
    let mut ws2 = make_workspace("ws-same", vec![]);
    ws2.saved_at = "2099-12-31T23:59:59Z".to_string();

    let p1 = profiles_from_workspaces(&[ws1]);
    let p2 = profiles_from_workspaces(&[ws2]);
    assert_eq!(
        p1, p2,
        "profiles_from_workspaces must be independent of saved_at/session_ids"
    );
}

// ---------------------------------------------------------------------------
// WS10 — the three new entity kinds coexist with existing sessions/hosts/
// workspaces in one restore() — no cross-prefix contamination.
// ---------------------------------------------------------------------------

#[test]
fn ws10_mixed_store_restores_all_entity_kinds_correctly() {
    let mut store = fresh_store();

    // Existing entities.
    store
        .upsert_session(&make_session("sess-mix"))
        .expect("upsert_session");
    store
        .upsert_host(&make_host("host-mix"))
        .expect("upsert_host");
    store
        .snapshot_workspace(&make_workspace("ws-mix", vec!["sess-mix".to_string()]))
        .expect("snapshot_workspace");

    // New entities.
    store
        .upsert_workstream(&workstream("wstream-mix", Some("prof-mix")))
        .expect("upsert_workstream");
    store
        .upsert_connection(&connection(
            "conn-mix",
            "wstream-mix",
            "node-a",
            "node-b",
            true,
            None,
        ))
        .expect("upsert_connection");
    store
        .upsert_profile(&profile("prof-mix", "Profile Mix"))
        .expect("upsert_profile");

    let restored = store.restore().expect("restore");

    // Existing entities present, unaffected.
    assert_eq!(restored.sessions.len(), 1);
    assert_eq!(restored.sessions[0].id, "sess-mix");
    assert_eq!(restored.hosts.len(), 1);
    assert_eq!(restored.hosts[0].id, "host-mix");
    assert_eq!(restored.workspaces.len(), 1);
    assert_eq!(restored.workspaces[0].id, "ws-mix");

    // New entities present, exactly one each — no cross-prefix contamination.
    assert_eq!(restored.workstreams.len(), 1);
    assert_eq!(restored.workstreams[0].id, "wstream-mix");
    assert_eq!(restored.workstreams[0].profile.as_deref(), Some("prof-mix"));

    assert_eq!(restored.connections.len(), 1);
    assert_eq!(restored.connections[0].id, "conn-mix");
    assert_eq!(restored.connections[0].ws, "wstream-mix");

    assert_eq!(restored.profiles.len(), 1);
    assert_eq!(restored.profiles[0].id, "prof-mix");
    assert_eq!(restored.profiles[0].name, "Profile Mix");
}

// ---------------------------------------------------------------------------
// WS11: set_membership — round-trip + write-error propagation
// ---------------------------------------------------------------------------

#[test]
fn ws11_set_membership_round_trips_through_restore() {
    let mut store = Store::new(Box::new(MemoryBackend::new()));
    let mut map = std::collections::HashMap::new();
    map.insert("sess-a".to_string(), "wstream-1".to_string());
    map.insert("sess-b".to_string(), "wstream-2".to_string());
    store
        .set_membership(&map)
        .expect("set_membership must succeed");

    let restored = store.restore().expect("restore must succeed");
    assert_eq!(restored.membership, map);
}

#[test]
fn ws11_membership_absent_restores_empty_map() {
    let store = Store::new(Box::new(MemoryBackend::new()));
    let restored = store.restore().expect("restore must succeed");
    assert!(
        restored.membership.is_empty(),
        "membership must default to empty when never persisted"
    );
}

#[test]
fn ws11_backend_write_failure_propagates_from_set_membership() {
    let backend = MemoryBackend::new();
    backend.fail_next(BackendOp::Write);
    let mut store = Store::new(Box::new(backend));
    let mut map = std::collections::HashMap::new();
    map.insert("sess-a".to_string(), "wstream-1".to_string());
    let err = store
        .set_membership(&map)
        .expect_err("write failure must propagate");
    assert!(matches!(err, StoreError::Backend(_)));
}
