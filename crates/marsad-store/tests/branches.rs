//! branches.rs — table-driven cases closing remaining regions to 100%.
//!
//! Covers:
//! - Empty restore (no data in backend) → empty Restored
//! - Multiple workspaces present after snapshots → all returned
//! - active_ws field set/cleared correctly
//! - Transcript accumulation across many appends for same session
//! - Transcripts for multiple sessions are independent
//! - list_by_prefix returns only keys under the given prefix
//! - Session with session_id = None is stored and returned as-is
//! - upsert of many sessions
//! - Workspace with empty session_ids list
//! - All phases of a PersistedSession last_phase field survive round-trip

use marsad_store::{
    Backend, MemoryBackend, PersistedSession, PersistedSettings, Store, StoreError,
    TranscriptEntry, Workspace,
};
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn session(id: &str) -> PersistedSession {
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
        model: "claude-sonnet-4-5".to_string(),
        last_phase: "Done".to_string(),
        mode: None,
        disallowed_tools: vec![],
        driver: "claude".to_string(),
        slash_commands: vec![],
        tools: vec![],
        mcp_servers: vec![],
    }
}

fn workspace(id: &str, session_ids: &[&str]) -> Workspace {
    Workspace {
        id: id.to_string(),
        name: format!("WS {id}"),
        session_ids: session_ids.iter().map(|s| s.to_string()).collect(),
        saved_at: "2026-06-18T00:00:00Z".to_string(),
    }
}

fn entry(kind: &str, payload: &str) -> TranscriptEntry {
    TranscriptEntry {
        kind: kind.to_string(),
        payload: payload.to_string(),
    }
}

fn fresh_store() -> Store {
    Store::new(Box::new(MemoryBackend::new()))
}

// ---------------------------------------------------------------------------
// Empty restore
// ---------------------------------------------------------------------------

#[test]
fn restore_empty_backend_yields_empty_restored() {
    let store = fresh_store();
    let r = store.restore().expect("restore");
    assert!(r.sessions.is_empty(), "sessions must be empty");
    assert!(r.workspaces.is_empty(), "workspaces must be empty");
    assert!(r.active_ws.is_none(), "active_ws must be None");
    assert!(r.transcripts.is_empty(), "transcripts must be empty");
}

// ---------------------------------------------------------------------------
// Multiple workspaces
// ---------------------------------------------------------------------------

#[test]
fn multiple_workspaces_all_returned_after_snapshot() {
    let mut store = fresh_store();
    for i in 0..5u32 {
        let ws = workspace(&format!("ws-{i}"), &[]);
        store.snapshot_workspace(&ws).expect("snapshot");
    }
    let r = store.restore().expect("restore");
    let ids: HashSet<_> = r.workspaces.iter().map(|w| w.id.clone()).collect();
    for i in 0..5u32 {
        assert!(
            ids.contains(&format!("ws-{i}")),
            "ws-{i} must be present after restore"
        );
    }
}

#[test]
fn workspace_overwrite_does_not_duplicate() {
    let mut store = fresh_store();
    let mut ws = workspace("ws-ow", &["s1"]);
    store.snapshot_workspace(&ws).expect("first snapshot");
    ws.session_ids = vec!["s1".to_string(), "s2".to_string()];
    store.snapshot_workspace(&ws).expect("second snapshot");
    let r = store.restore().expect("restore");
    let matching: Vec<_> = r.workspaces.iter().filter(|w| w.id == "ws-ow").collect();
    assert_eq!(matching.len(), 1, "overwrite must not duplicate workspace");
    assert_eq!(matching[0].session_ids.len(), 2);
}

// ---------------------------------------------------------------------------
// active_ws — set_active_workspace / restore round-trip
// ---------------------------------------------------------------------------

#[test]
fn active_ws_none_when_no_set_active_workspace_called() {
    let mut store = fresh_store();
    let ws = workspace("ws-inactive", &[]);
    store.snapshot_workspace(&ws).expect("snapshot");
    let r = store.restore().expect("restore");
    assert!(
        r.active_ws.is_none(),
        "active_ws must be None when set_active_workspace was never called"
    );
}

#[test]
fn set_active_workspace_then_restore_returns_that_active_ws() {
    let mut store = fresh_store();
    let ws = workspace("ws-active", &[]);
    store.snapshot_workspace(&ws).expect("snapshot");
    store
        .set_active_workspace("ws-active")
        .expect("set_active_workspace must succeed for a known workspace");
    let r = store.restore().expect("restore");
    assert_eq!(
        r.active_ws,
        Some("ws-active".to_string()),
        "restore must return the active workspace id set via set_active_workspace"
    );
}

#[test]
fn set_active_workspace_unknown_id_returns_not_found() {
    let mut store = fresh_store();
    // No workspaces snapshotted — "ghost-ws" does not exist.
    match store.set_active_workspace("ghost-ws") {
        Err(StoreError::NotFound(id)) => {
            assert!(
                id.contains("ghost-ws"),
                "NotFound message must reference the unknown id, got: {id}"
            );
        }
        other => panic!("expected NotFound for unknown workspace id, got: {other:?}"),
    }
}

#[test]
fn set_active_workspace_overwrites_previous_selection() {
    let mut store = fresh_store();
    for id in &["ws-first", "ws-second"] {
        store
            .snapshot_workspace(&workspace(id, &[]))
            .expect("snapshot");
    }
    store.set_active_workspace("ws-first").expect("set first");
    store.set_active_workspace("ws-second").expect("set second");
    let r = store.restore().expect("restore");
    assert_eq!(
        r.active_ws,
        Some("ws-second".to_string()),
        "second set_active_workspace call must overwrite the first"
    );
}

#[test]
fn set_active_workspace_unknown_id_when_some_workspaces_exist_returns_not_found() {
    let mut store = fresh_store();
    store
        .snapshot_workspace(&workspace("ws-real", &[]))
        .expect("snapshot");
    // "ws-ghost" was never snapshotted.
    match store.set_active_workspace("ws-ghost") {
        Err(StoreError::NotFound(_)) => {}
        other => panic!("expected NotFound even when other workspaces exist, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Transcript accumulation across appends (multiple entries, same session)
// ---------------------------------------------------------------------------

#[test]
fn transcript_accumulation_20_entries() {
    let mut store = fresh_store();
    let sess = session("tx-accum");
    store.upsert_session(&sess).expect("upsert");

    for i in 0..20u32 {
        let e = entry("text", &format!("msg-{i}"));
        store.append_transcript("tx-accum", &e).expect("append");
    }

    let r = store.restore().expect("restore");
    let tx = r
        .transcripts
        .get("tx-accum")
        .expect("transcript must exist");
    assert_eq!(tx.len(), 20, "must have 20 transcript entries");
    for (i, e) in tx.iter().enumerate() {
        assert_eq!(e.payload, format!("msg-{i}"), "entries must be in order");
    }
}

#[test]
fn transcript_accumulation_different_kinds() {
    let mut store = fresh_store();
    let sess = session("tx-kinds");
    store.upsert_session(&sess).expect("upsert");

    store
        .append_transcript("tx-kinds", &entry("system", "init"))
        .expect("append system");
    store
        .append_transcript("tx-kinds", &entry("assistant", "response"))
        .expect("append assistant");
    store
        .append_transcript("tx-kinds", &entry("tool_use", "bash"))
        .expect("append tool_use");
    store
        .append_transcript("tx-kinds", &entry("tool_result", "exit 0"))
        .expect("append tool_result");

    let r = store.restore().expect("restore");
    let tx = r.transcripts.get("tx-kinds").expect("transcript");
    assert_eq!(tx.len(), 4);
    assert_eq!(tx[0].kind, "system");
    assert_eq!(tx[1].kind, "assistant");
    assert_eq!(tx[2].kind, "tool_use");
    assert_eq!(tx[3].kind, "tool_result");
}

// ---------------------------------------------------------------------------
// Transcripts for multiple sessions are independent
// ---------------------------------------------------------------------------

#[test]
fn transcripts_for_multiple_sessions_are_independent() {
    let mut store = fresh_store();

    for sess_id in &["s-alpha", "s-beta", "s-gamma"] {
        let s = session(sess_id);
        store.upsert_session(&s).expect("upsert");
    }

    store
        .append_transcript("s-alpha", &entry("text", "alpha-msg"))
        .expect("append alpha");
    store
        .append_transcript("s-beta", &entry("text", "beta-msg-1"))
        .expect("append beta-1");
    store
        .append_transcript("s-beta", &entry("text", "beta-msg-2"))
        .expect("append beta-2");
    // s-gamma gets no transcript entries

    let r = store.restore().expect("restore");

    let alpha_tx = r.transcripts.get("s-alpha").expect("s-alpha transcript");
    assert_eq!(alpha_tx.len(), 1);
    assert_eq!(alpha_tx[0].payload, "alpha-msg");

    let beta_tx = r.transcripts.get("s-beta").expect("s-beta transcript");
    assert_eq!(beta_tx.len(), 2);

    // s-gamma may have no entry in the map or an empty vec
    let gamma_len = r.transcripts.get("s-gamma").map(|v| v.len()).unwrap_or(0);
    assert_eq!(gamma_len, 0, "s-gamma must have 0 transcript entries");
}

// ---------------------------------------------------------------------------
// list-by-prefix returns the right keys (via Backend directly)
// ---------------------------------------------------------------------------

#[test]
fn list_by_prefix_returns_only_matching_keys() {
    let backend = MemoryBackend::new();
    backend.write("session:a", b"1").expect("write a");
    backend.write("session:b", b"2").expect("write b");
    backend.write("workspace:x", b"3").expect("write x");
    backend.write("transcript:a:0", b"4").expect("write tx");

    let session_keys = backend.list("session:").expect("list session:");
    for k in &session_keys {
        assert!(
            k.starts_with("session:"),
            "key '{k}' does not start with 'session:'"
        );
    }
    assert!(session_keys.contains(&"session:a".to_string()));
    assert!(session_keys.contains(&"session:b".to_string()));
    assert!(!session_keys.contains(&"workspace:x".to_string()));
    assert!(!session_keys.contains(&"transcript:a:0".to_string()));
}

#[test]
fn list_by_prefix_empty_prefix_returns_all_keys() {
    let backend = MemoryBackend::new();
    backend.write("alpha", b"1").expect("write alpha");
    backend.write("beta", b"2").expect("write beta");
    backend.write("gamma", b"3").expect("write gamma");

    let keys = backend.list("").expect("list all");
    assert!(keys.contains(&"alpha".to_string()));
    assert!(keys.contains(&"beta".to_string()));
    assert!(keys.contains(&"gamma".to_string()));
}

#[test]
fn list_by_prefix_no_match_returns_empty_vec() {
    let backend = MemoryBackend::new();
    backend.write("session:a", b"1").expect("write");

    let keys = backend.list("workspace:").expect("list workspace:");
    assert!(
        keys.is_empty(),
        "list with non-matching prefix must return empty vec"
    );
}

// ---------------------------------------------------------------------------
// Session with session_id = None
// ---------------------------------------------------------------------------

#[test]
fn session_with_none_session_id_round_trips() {
    let mut store = fresh_store();
    let sess = PersistedSession {
        id: "sess-no-claude-id".to_string(),
        session_id: None,
        name: "New session".to_string(),
        host_id: "localhost".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        project: "/p".to_string(),
        branch: "main".to_string(),
        model: "claude-opus-4-5".to_string(),
        last_phase: "Starting".to_string(),
        mode: None,
        disallowed_tools: vec![],
        driver: "claude".to_string(),
        slash_commands: vec![],
        tools: vec![],
        mcp_servers: vec![],
    };
    store.upsert_session(&sess).expect("upsert");
    let r = store.restore().expect("restore");
    let found = r
        .sessions
        .into_iter()
        .find(|s| s.id == "sess-no-claude-id")
        .expect("session must exist");
    assert!(
        found.session_id.is_none(),
        "session_id must remain None after round-trip"
    );
}

// ---------------------------------------------------------------------------
// Many sessions upserted all survive restore
// ---------------------------------------------------------------------------

#[test]
fn many_sessions_all_survive_restore() {
    let mut store = fresh_store();
    let count = 50usize;
    for i in 0..count {
        let s = session(&format!("bulk-{i}"));
        store.upsert_session(&s).expect("upsert");
    }
    let r = store.restore().expect("restore");
    assert_eq!(
        r.sessions.len(),
        count,
        "all {count} sessions must survive restore"
    );
}

// ---------------------------------------------------------------------------
// Workspace with empty session_ids
// ---------------------------------------------------------------------------

#[test]
fn workspace_with_empty_session_ids_round_trips() {
    let mut store = fresh_store();
    let ws = workspace("ws-empty", &[]);
    store.snapshot_workspace(&ws).expect("snapshot");
    let r = store.restore().expect("restore");
    let found = r
        .workspaces
        .into_iter()
        .find(|w| w.id == "ws-empty")
        .expect("workspace must exist");
    assert!(
        found.session_ids.is_empty(),
        "empty session_ids must round-trip"
    );
}

// ---------------------------------------------------------------------------
// last_phase round-trips for all documented phase strings
// ---------------------------------------------------------------------------

#[test]
fn last_phase_all_values_round_trip() {
    let phases = [
        "Starting", "Working", "Needs", "Error", "Done", "Stale", "Detached",
    ];
    let mut store = fresh_store();
    for phase in &phases {
        let sess = PersistedSession {
            id: format!("phase-{phase}"),
            session_id: None,
            name: "S".to_string(),
            host_id: "h".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            project: "/p".to_string(),
            branch: "main".to_string(),
            model: "claude-opus-4-5".to_string(),
            last_phase: phase.to_string(),
            mode: None,
            disallowed_tools: vec![],
            driver: "claude".to_string(),
            slash_commands: vec![],
            tools: vec![],
            mcp_servers: vec![],
        };
        store.upsert_session(&sess).expect("upsert");
    }
    let r = store.restore().expect("restore");
    for phase in &phases {
        let found = r
            .sessions
            .iter()
            .find(|s| s.id == format!("phase-{phase}"))
            .unwrap_or_else(|| panic!("session for phase {phase} must exist"));
        assert_eq!(
            found.last_phase, *phase,
            "last_phase '{phase}' must survive round-trip"
        );
    }
}

// ---------------------------------------------------------------------------
// Back-compat: PersistedSettings without the new default_driver / default_models
// fields (serde default path for the new fields added by deliverable #2).
// ---------------------------------------------------------------------------

/// Deserializing an old-format PersistedSettings JSON (no `default_driver` or
/// `default_models` keys) must use the serde defaults: "claude" and [].
/// This exercises the `default_driver_str()` function which is only called by
/// the `#[serde(default = "default_driver_str")]` attribute when the field is
/// absent from the stored JSON.
#[test]
fn persisted_settings_missing_default_driver_falls_back_to_claude() {
    // A raw JSON blob as an old (pre-deliverable-#2) settings record would have
    // stored — deliberately omitting "default_driver" and "default_models".
    let old_json = r#"{
        "notif_prefs": {},
        "play_sound": true,
        "group_alerts": false,
        "do_not_disturb": false,
        "background": "Gradient",
        "restore_on_launch": true,
        "confirm_before_stop": true,
        "default_model": "claude-sonnet-4-6",
        "default_posture": "ask"
    }"#;
    let s: PersistedSettings =
        serde_json::from_str(old_json).expect("old settings JSON must deserialize");
    assert_eq!(
        s.default_driver, "claude",
        "absent default_driver must fall back to 'claude' via default_driver_str()"
    );
    assert!(
        s.default_models.is_empty(),
        "absent default_models must fall back to empty vec"
    );
}
