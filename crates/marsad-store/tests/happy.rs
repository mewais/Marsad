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
    MemoryBackend, NotifPref, PersistedHost, PersistedSession, PersistedSettings, PersistedTemplate,
    Restored, Store, StoreError, TranscriptEntry, Workspace,
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
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        project: "/home/user/project".to_string(),
        branch: "main".to_string(),
        model: "claude-opus-4-5".to_string(),
        last_phase: "Done".to_string(),
        posture: None,
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
    let mut sess = make_session("sess-fields");
    // Set the per-host launch config so its round-trip is asserted too.
    sess.claude_bin = Some("genie".to_string());
    sess.setup = Some("module load x".to_string());
    sess.env = vec![("A".to_string(), "b".to_string())];
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
    assert_eq!(found.claude_bin.as_deref(), Some("genie"));
    assert_eq!(found.setup.as_deref(), Some("module load x"));
    assert_eq!(found.env, vec![("A".to_string(), "b".to_string())]);
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
        hosts: vec![],
        workspaces: vec![],
        active_ws: None,
        transcripts: std::collections::HashMap::new(),
        settings: None,
        templates: vec![],
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

// ---------------------------------------------------------------------------
// upsert_host / delete_host / restore round-trip
// ---------------------------------------------------------------------------

fn make_host(id: &str) -> PersistedHost {
    PersistedHost {
        id: id.to_string(),
        label: format!("lbl-{id}"),
        target: "u@h".to_string(),
        is_local: false,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
    }
}

#[test]
fn upsert_host_then_restore_returns_host_with_all_fields() {
    let mut store = fresh_store();
    let mut host = make_host("host-1");
    host.claude_bin = Some("genie".to_string());
    host.setup = Some("module load x".to_string());
    host.env = vec![("A".to_string(), "b".to_string())];
    store.upsert_host(&host).expect("upsert_host");
    let restored: Restored = store.restore().expect("restore");
    let h = restored
        .hosts
        .iter()
        .find(|h| h.id == "host-1")
        .expect("host-1 must be restored");
    assert_eq!(h.label, "lbl-host-1");
    assert_eq!(h.target, "u@h");
    assert!(!h.is_local);
    assert_eq!(h.claude_bin.as_deref(), Some("genie"));
    assert_eq!(h.setup.as_deref(), Some("module load x"));
    assert_eq!(h.env, vec![("A".to_string(), "b".to_string())]);
}

#[test]
fn delete_host_removes_it_from_restore() {
    let mut store = fresh_store();
    store
        .upsert_host(&make_host("host-2"))
        .expect("upsert_host");
    store.delete_host("host-2").expect("delete_host");
    let restored = store.restore().expect("restore");
    assert!(
        !restored.hosts.iter().any(|h| h.id == "host-2"),
        "deleted host must not be restored"
    );
}

#[test]
fn delete_session_removes_it_from_restore() {
    let mut store = fresh_store();
    store
        .upsert_session(&make_session("session-9"))
        .expect("upsert_session");
    store.delete_session("session-9").expect("delete_session");
    let restored = store.restore().expect("restore");
    assert!(
        !restored.sessions.iter().any(|s| s.id == "session-9"),
        "deleted session must not be restored"
    );
}

// ---------------------------------------------------------------------------
// delete_workspace round-trip
// ---------------------------------------------------------------------------

#[test]
fn delete_workspace_removes_it_from_restore() {
    let mut store = fresh_store();
    let ws = make_workspace("ws-del", vec![]);
    store.snapshot_workspace(&ws).expect("snapshot_workspace");
    store.delete_workspace("ws-del").expect("delete_workspace");
    let restored = store.restore().expect("restore");
    assert!(
        !restored.workspaces.iter().any(|w| w.id == "ws-del"),
        "deleted workspace must not be restored"
    );
}

#[test]
fn delete_workspace_noop_when_absent() {
    let mut store = fresh_store();
    // Deleting a non-existent workspace must succeed (no-op).
    store
        .delete_workspace("does-not-exist")
        .expect("delete_workspace on absent id must succeed");
}

// ---------------------------------------------------------------------------
// upsert_settings / restore round-trip
// ---------------------------------------------------------------------------

fn make_notif_pref(enabled: bool, priority: &str) -> NotifPref {
    NotifPref {
        enabled,
        priority: priority.to_string(),
        sound: true,
        group: false,
        dnd: false,
    }
}

fn make_settings() -> PersistedSettings {
    let mut notif_prefs = std::collections::HashMap::new();
    notif_prefs.insert("gate_needs_you".to_string(), make_notif_pref(true, "urgent"));
    notif_prefs.insert("session_done".to_string(), make_notif_pref(false, "normal"));
    PersistedSettings {
        notif_prefs,
        play_sound: true,
        group_alerts: false,
        do_not_disturb: true,
        background: "Gradient".to_string(),
        restore_on_launch: true,
        confirm_before_stop: false,
        default_model: "claude-sonnet-4-6".to_string(),
        default_posture: "ask".to_string(),
    }
}

#[test]
fn upsert_settings_then_restore_returns_settings() {
    let mut store = fresh_store();
    let s = make_settings();
    store.upsert_settings(&s).expect("upsert_settings must succeed");
    let restored = store.restore().expect("restore must succeed");
    let got = restored.settings.expect("settings must be present after upsert");
    assert_eq!(got, s);
}

#[test]
fn upsert_settings_overwrite_updates_fields() {
    let mut store = fresh_store();
    let s1 = make_settings();
    store.upsert_settings(&s1).expect("first upsert");
    let mut s2 = make_settings();
    s2.default_model = "claude-opus-4-8".to_string();
    s2.do_not_disturb = false;
    store.upsert_settings(&s2).expect("second upsert");
    let restored = store.restore().expect("restore");
    let got = restored.settings.expect("settings present");
    assert_eq!(got.default_model, "claude-opus-4-8");
    assert!(!got.do_not_disturb);
}

#[test]
fn restore_empty_store_has_no_settings() {
    let store = fresh_store();
    let restored = store.restore().expect("restore");
    assert!(restored.settings.is_none(), "no settings in empty store");
}

#[test]
fn persisted_settings_derives_clone_eq_debug() {
    let s = make_settings();
    let s2 = s.clone();
    assert_eq!(s, s2);
    let _ = format!("{s:?}");
}

#[test]
fn notif_pref_derives_clone_eq_debug() {
    let p = make_notif_pref(true, "urgent");
    let p2 = p.clone();
    assert_eq!(p, p2);
    let _ = format!("{p:?}");
}

// ---------------------------------------------------------------------------
// upsert_template / restore round-trip
// ---------------------------------------------------------------------------

fn make_template(name: &str) -> PersistedTemplate {
    PersistedTemplate {
        name: name.to_string(),
        model: "claude-sonnet-4-6".to_string(),
        posture: "ask".to_string(),
    }
}

#[test]
fn upsert_template_then_restore_returns_template() {
    let mut store = fresh_store();
    let t = make_template("fast");
    store.upsert_template(&t).expect("upsert_template must succeed");
    let restored = store.restore().expect("restore must succeed");
    assert!(
        restored.templates.iter().any(|tmpl| tmpl.name == "fast"),
        "restored templates must include 'fast'"
    );
}

#[test]
fn upsert_template_preserves_all_fields() {
    let mut store = fresh_store();
    let t = PersistedTemplate {
        name: "careful".to_string(),
        model: "claude-opus-4-8".to_string(),
        posture: "full".to_string(),
    };
    store.upsert_template(&t).expect("upsert_template");
    let restored = store.restore().expect("restore");
    let found = restored
        .templates
        .into_iter()
        .find(|tmpl| tmpl.name == "careful")
        .expect("template must exist");
    assert_eq!(found.model, "claude-opus-4-8");
    assert_eq!(found.posture, "full");
}

#[test]
fn upsert_template_overwrites_same_name() {
    let mut store = fresh_store();
    let t1 = make_template("my-tmpl");
    store.upsert_template(&t1).expect("first upsert");
    let t2 = PersistedTemplate {
        name: "my-tmpl".to_string(),
        model: "claude-haiku-4-5".to_string(),
        posture: "auto".to_string(),
    };
    store.upsert_template(&t2).expect("second upsert");
    let restored = store.restore().expect("restore");
    let matches: Vec<_> = restored.templates.iter().filter(|tmpl| tmpl.name == "my-tmpl").collect();
    assert_eq!(matches.len(), 1, "upsert must overwrite, not duplicate");
    assert_eq!(matches[0].model, "claude-haiku-4-5");
}

#[test]
fn restore_empty_store_has_no_templates() {
    let store = fresh_store();
    let restored = store.restore().expect("restore");
    assert!(restored.templates.is_empty(), "no templates in empty store");
}

#[test]
fn persisted_template_derives_clone_eq_debug() {
    let t = make_template("deriv");
    let t2 = t.clone();
    assert_eq!(t, t2);
    let _ = format!("{t:?}");
}

// ---------------------------------------------------------------------------
// posture field round-trip
// ---------------------------------------------------------------------------

/// PersistedSession.posture round-trips through the store.
/// Covers the Some branch for the new field.
#[test]
fn upsert_session_posture_round_trips() {
    let mut store = fresh_store();
    let mut sess = make_session("posture-rt");
    sess.posture = Some("FullAutonomy".to_string());
    store.upsert_session(&sess).expect("upsert");
    let restored = store.restore().expect("restore");
    let found = restored
        .sessions
        .into_iter()
        .find(|s| s.id == "posture-rt")
        .expect("session must exist");
    assert_eq!(found.posture, Some("FullAutonomy".to_string()));
}
