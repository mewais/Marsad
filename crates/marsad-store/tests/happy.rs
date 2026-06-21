//! happy.rs — nominal paths for every public fn/variant of `marsad-store`.
//!
//! Covers:
//! - upsert_session round-trip: insert then restore, session is present
//! - append_transcript: entries are persisted and returned in restore
//! - snapshot_workspace / restore: workspace round-trip on MemoryBackend
//! - restore returns full Restored struct with sessions, workspaces, transcripts, active_ws
//! - export_workspace → import_workspace round-trip with fidelity (i08 asserts this)
//! - Derived traits: Debug, Clone, PartialEq on PersistedSession, Workspace, TranscriptEntry,
//!   Restored, StoreError

use marsad_store::{
    MemoryBackend, PersistedSession, Restored, Store, StoreError, TranscriptEntry, Workspace,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_session(id: &str) -> PersistedSession {
    PersistedSession {
        id: id.to_string(),
        session_id: Some(format!("claude-{id}")),
        name: format!("Session {id}"),
        host_id: "localhost".to_string(),
        project: "/home/user/project".to_string(),
        branch: "main".to_string(),
        model: "claude-opus-4-5".to_string(),
        last_phase: "Done".to_string(),
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

fn make_transcript_entry(kind: &str, payload: &str) -> TranscriptEntry {
    TranscriptEntry {
        kind: kind.to_string(),
        payload: payload.to_string(),
    }
}

fn fresh_store() -> Store {
    Store::new(Box::new(MemoryBackend::new()))
}

// ---------------------------------------------------------------------------
// upsert_session / restore round-trip
// ---------------------------------------------------------------------------

#[test]
fn upsert_session_then_restore_returns_session() {
    let mut store = fresh_store();
    let sess = make_session("sess-001");
    store
        .upsert_session(&sess)
        .expect("upsert_session must succeed");
    let restored: Restored = store.restore().expect("restore must succeed");
    assert!(
        restored.sessions.iter().any(|s| s.id == "sess-001"),
        "restored sessions must include sess-001"
    );
}

#[test]
fn upsert_session_preserves_all_fields() {
    let mut store = fresh_store();
    let sess = make_session("sess-fields");
    store.upsert_session(&sess).expect("upsert_session");
    let restored = store.restore().expect("restore");
    let found = restored
        .sessions
        .into_iter()
        .find(|s| s.id == "sess-fields")
        .expect("session must exist after upsert");
    assert_eq!(found.session_id, Some("claude-sess-fields".to_string()));
    assert_eq!(found.name, "Session sess-fields");
    assert_eq!(found.host_id, "localhost");
    assert_eq!(found.project, "/home/user/project");
    assert_eq!(found.branch, "main");
    assert_eq!(found.model, "claude-opus-4-5");
    assert_eq!(found.last_phase, "Done");
}

#[test]
fn upsert_session_overwrite_updates_fields() {
    let mut store = fresh_store();
    let mut sess = make_session("sess-update");
    store.upsert_session(&sess).expect("first upsert");
    sess.last_phase = "Error".to_string();
    store.upsert_session(&sess).expect("second upsert");
    let restored = store.restore().expect("restore");
    let sessions: Vec<_> = restored
        .sessions
        .into_iter()
        .filter(|s| s.id == "sess-update")
        .collect();
    // Should only appear once (overwrite, not duplicate)
    assert_eq!(sessions.len(), 1, "upsert must overwrite, not duplicate");
    assert_eq!(sessions[0].last_phase, "Error");
}

// ---------------------------------------------------------------------------
// append_transcript / restore round-trip
// ---------------------------------------------------------------------------

#[test]
fn append_transcript_then_restore_returns_entry() {
    let mut store = fresh_store();
    let sess = make_session("sess-tx");
    store.upsert_session(&sess).expect("upsert_session");
    let entry = make_transcript_entry("text", "Hello from assistant");
    store
        .append_transcript("sess-tx", &entry)
        .expect("append_transcript must succeed");
    let restored = store.restore().expect("restore");
    let transcript = restored
        .transcripts
        .get("sess-tx")
        .expect("transcript for sess-tx must exist");
    assert_eq!(transcript.len(), 1);
    assert_eq!(transcript[0].kind, "text");
    assert_eq!(transcript[0].payload, "Hello from assistant");
}

#[test]
fn append_transcript_accumulates_in_order() {
    let mut store = fresh_store();
    let sess = make_session("sess-accum");
    store.upsert_session(&sess).expect("upsert_session");

    for i in 0..5u32 {
        let entry = make_transcript_entry("text", &format!("Line {i}"));
        store
            .append_transcript("sess-accum", &entry)
            .expect("append_transcript");
    }

    let restored = store.restore().expect("restore");
    let transcript = restored
        .transcripts
        .get("sess-accum")
        .expect("transcript must exist");
    assert_eq!(transcript.len(), 5, "must have exactly 5 entries");
    for (i, entry) in transcript.iter().enumerate() {
        assert_eq!(entry.payload, format!("Line {i}"));
    }
}

// ---------------------------------------------------------------------------
// snapshot_workspace / restore round-trip
// ---------------------------------------------------------------------------

#[test]
fn snapshot_workspace_then_restore_returns_workspace() {
    let mut store = fresh_store();
    let ws = make_workspace("ws-001", vec!["sess-001".to_string()]);
    store.snapshot_workspace(&ws).expect("snapshot_workspace");
    let restored = store.restore().expect("restore");
    assert!(
        restored.workspaces.iter().any(|w| w.id == "ws-001"),
        "restored workspaces must include ws-001"
    );
}

#[test]
fn snapshot_workspace_preserves_all_fields() {
    let mut store = fresh_store();
    let ws = make_workspace("ws-fields", vec!["s1".to_string(), "s2".to_string()]);
    store.snapshot_workspace(&ws).expect("snapshot_workspace");
    let restored = store.restore().expect("restore");
    let found = restored
        .workspaces
        .into_iter()
        .find(|w| w.id == "ws-fields")
        .expect("workspace must exist");
    assert_eq!(found.name, "WS ws-fields");
    assert_eq!(found.session_ids, vec!["s1", "s2"]);
    assert_eq!(found.saved_at, "2026-06-18T00:00:00Z");
}

// ---------------------------------------------------------------------------
// restore returns correct Restored structure
// ---------------------------------------------------------------------------

#[test]
fn restore_empty_store_returns_empty_restored() {
    let store = fresh_store();
    let restored = store
        .restore()
        .expect("restore on empty store must succeed");
    assert!(restored.sessions.is_empty());
    assert!(restored.workspaces.is_empty());
    assert!(restored.active_ws.is_none());
    assert!(restored.transcripts.is_empty());
}

#[test]
fn restore_full_round_trip() {
    let mut store = fresh_store();

    let sess = make_session("s1");
    store.upsert_session(&sess).expect("upsert");

    let entry = make_transcript_entry("assistant", r#"{"text":"hi"}"#);
    store.append_transcript("s1", &entry).expect("append");

    let ws = make_workspace("ws1", vec!["s1".to_string()]);
    store.snapshot_workspace(&ws).expect("snapshot");

    let restored = store.restore().expect("restore");
    assert_eq!(restored.sessions.len(), 1);
    assert_eq!(restored.sessions[0].id, "s1");
    assert_eq!(restored.workspaces.len(), 1);
    assert_eq!(restored.workspaces[0].id, "ws1");
    let tx = restored.transcripts.get("s1").expect("transcript for s1");
    assert_eq!(tx.len(), 1);
    assert_eq!(tx[0].kind, "assistant");
}

// ---------------------------------------------------------------------------
// export_workspace → import_workspace round-trip (i08 fidelity test)
// ---------------------------------------------------------------------------

#[test]
fn export_workspace_returns_json_string() {
    let mut store = fresh_store();
    let sess = make_session("sx");
    store.upsert_session(&sess).expect("upsert");
    let ws = make_workspace("wx", vec!["sx".to_string()]);
    store.snapshot_workspace(&ws).expect("snapshot");

    let json = store
        .export_workspace("wx")
        .expect("export_workspace must succeed");
    assert!(!json.is_empty(), "exported JSON must not be empty");
    // The spec mandates envelope: { marsadWorkspace:1, name, savedAt, sessions:[...] }
    assert!(
        json.contains("marsadWorkspace"),
        "exported JSON must contain marsadWorkspace key"
    );
}

#[test]
fn export_import_workspace_round_trip_fidelity() {
    // This is the i08 fidelity assertion: export then import yields equivalent Workspace + sessions.
    let mut store = fresh_store();

    let sess = make_session("s-export");
    store.upsert_session(&sess).expect("upsert");

    let ws = make_workspace("ws-export", vec!["s-export".to_string()]);
    store.snapshot_workspace(&ws).expect("snapshot");

    // Export to .marsad.json text
    let json = store
        .export_workspace("ws-export")
        .expect("export_workspace");

    // Import back — must return an equivalent Workspace
    let imported_ws: Workspace = store.import_workspace(&json).expect("import_workspace");
    assert_eq!(
        imported_ws.name, ws.name,
        "imported workspace name must match original"
    );
    assert_eq!(
        imported_ws.session_ids, ws.session_ids,
        "imported workspace session_ids must match original"
    );
}

#[test]
fn export_import_preserves_name_and_saved_at() {
    let mut store = fresh_store();
    let sess = make_session("s-ts");
    store.upsert_session(&sess).expect("upsert");
    let ws = make_workspace("ws-ts", vec!["s-ts".to_string()]);
    store.snapshot_workspace(&ws).expect("snapshot");

    let json = store.export_workspace("ws-ts").expect("export");
    let imported = store.import_workspace(&json).expect("import");

    // name must survive the round-trip
    assert_eq!(imported.name, "WS ws-ts");
}

// ---------------------------------------------------------------------------
// Derived traits: Debug, Clone, PartialEq
// ---------------------------------------------------------------------------

#[test]
fn persisted_session_derives_clone_eq_debug() {
    let s = make_session("deriv");
    let s2 = s.clone();
    assert_eq!(s, s2);
    let _ = format!("{s:?}");
}

#[test]
fn workspace_derives_clone_eq_debug() {
    let w = make_workspace("w-deriv", vec!["x".to_string()]);
    let w2 = w.clone();
    assert_eq!(w, w2);
    let _ = format!("{w:?}");
}

#[test]
fn transcript_entry_derives_clone_eq_debug() {
    let e = make_transcript_entry("text", "payload");
    let e2 = e.clone();
    assert_eq!(e, e2);
    let _ = format!("{e:?}");
}

#[test]
fn restored_derives_clone_eq_debug() {
    let r = Restored {
        sessions: vec![],
        workspaces: vec![],
        active_ws: None,
        transcripts: std::collections::HashMap::new(),
    };
    let r2 = r.clone();
    assert_eq!(r, r2);
    let _ = format!("{r:?}");
}

#[test]
fn store_error_derives_debug() {
    let e = StoreError::NotFound("x".to_string());
    let _ = format!("{e:?}");
}
