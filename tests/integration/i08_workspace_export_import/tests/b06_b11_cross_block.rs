// Integration scenario i08 — B06 (store export/import) × B11 (viewmodel rescope).
//
// B06 fidelity: export_workspace → .marsad.json → import_workspace must yield an
// equivalent Workspace (id reconstructed from name, name/session_ids/saved_at
// unchanged). Also asserts malformed JSON → BadImport.
//
// B11 rescope: build an AppSnapshot with sessions across 2 workspaces; rescope to
// one workspace → counts.total shrinks; cost/tokens re-scoped; unknown workspace
// id → unchanged snapshot.

use marsad_events::Phase;
use marsad_store::{MemoryBackend, PersistedSession, Store, StoreError, Workspace};
use marsad_viewmodel::{
    counts, rescope, AppSnapshot, Filter, SessionView, Tab, Workspace as VmWorkspace,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_session(id: &str, name: &str) -> PersistedSession {
    PersistedSession {
        id: id.to_string(),
        session_id: Some(format!("claude-{id}")),
        name: name.to_string(),
        host_id: "local".to_string(),
        project: "/projects/test".to_string(),
        branch: "main".to_string(),
        model: "claude-opus-4-5".to_string(),
        last_phase: "done".to_string(),
    }
}

fn make_workspace(id: &str, name: &str, session_ids: Vec<&str>) -> Workspace {
    Workspace {
        id: id.to_string(),
        name: name.to_string(),
        session_ids: session_ids.iter().map(|s| s.to_string()).collect(),
        saved_at: "2026-06-18T12:00:00Z".to_string(),
    }
}

/// Build a SessionView for viewmodel tests.
fn sv(id: &str, cost: f64, tokens: u64, state: Phase) -> SessionView {
    SessionView {
        id: id.to_string(),
        name: format!("Session {id}"),
        host_id: "local".to_string(),
        project: "/p".to_string(),
        branch: "main".to_string(),
        model: "claude-opus-4-5".to_string(),
        state,
        changed_files: 0,
        subagents: 0,
        cost_usd: cost,
        tokens,
        context_pct: 10.0,
        gate: None,
        transcript: vec![],
        changes: vec![],
    }
}

// ---------------------------------------------------------------------------
// B06 — export → import fidelity
// ---------------------------------------------------------------------------

#[test]
fn b06_export_import_fidelity() {
    let backend = MemoryBackend::new();
    let mut store = Store::new(Box::new(backend));

    // Persist sessions s1, s2.
    let s1 = make_session("s1", "Alpha session");
    let s2 = make_session("s2", "Beta session");
    store.upsert_session(&s1).unwrap();
    store.upsert_session(&s2).unwrap();

    // Persist workspace referencing both sessions.
    let ws = make_workspace("ws-fidelity", "My Workspace", vec!["s1", "s2"]);
    store.snapshot_workspace(&ws).unwrap();

    // Export.
    let json = store
        .export_workspace("ws-fidelity")
        .expect("export must succeed");

    // Sanity: the JSON contains the expected envelope fields.
    assert!(json.contains("marsadWorkspace"), "envelope key present");
    assert!(json.contains("My Workspace"), "workspace name present");
    assert!(json.contains("2026-06-18T12:00:00Z"), "savedAt present");

    // Import into a fresh Store (simulates round-trip on a different machine).
    let backend2 = MemoryBackend::new();
    let mut store2 = Store::new(Box::new(backend2));
    let imported = store2.import_workspace(&json).expect("import must succeed");

    // Fidelity assertions: name and saved_at survive the round-trip verbatim.
    assert_eq!(imported.name, ws.name, "name round-trips");
    assert_eq!(imported.saved_at, ws.saved_at, "saved_at round-trips");

    // The spec says id is reconstructed as "imported-{name-with-dashes}".
    assert_eq!(
        imported.id, "imported-My-Workspace",
        "id reconstructed per spec"
    );

    // session_ids list has the same elements (order may vary in export).
    let mut got_ids = imported.session_ids.clone();
    let mut want_ids = ws.session_ids.clone();
    got_ids.sort();
    want_ids.sort();
    assert_eq!(got_ids, want_ids, "session_ids round-trip");

    // The imported sessions are also persisted: restore and verify they are back.
    let restored = store2.restore().expect("restore after import must succeed");
    let restored_ids: Vec<&str> = restored.sessions.iter().map(|s| s.id.as_str()).collect();
    assert!(restored_ids.contains(&"s1"), "s1 persisted after import");
    assert!(restored_ids.contains(&"s2"), "s2 persisted after import");

    // The imported workspace is persisted as well.
    assert!(
        restored
            .workspaces
            .iter()
            .any(|w| w.id == "imported-My-Workspace"),
        "imported workspace is persisted"
    );
}

// ---------------------------------------------------------------------------
// B06 — malformed JSON → BadImport
// ---------------------------------------------------------------------------

#[test]
fn b06_bad_import_malformed_json() {
    let backend = MemoryBackend::new();
    let mut store = Store::new(Box::new(backend));

    let result = store.import_workspace("this is not json at all {{{");
    assert!(
        matches!(result, Err(StoreError::BadImport(_))),
        "malformed JSON must produce BadImport, got: {result:?}"
    );
}

#[test]
fn b06_bad_import_empty() {
    let backend = MemoryBackend::new();
    let mut store = Store::new(Box::new(backend));

    let result = store.import_workspace("");
    assert!(
        matches!(result, Err(StoreError::BadImport(_))),
        "empty string must produce BadImport, got: {result:?}"
    );
}

#[test]
fn b06_bad_import_missing_required_field() {
    let backend = MemoryBackend::new();
    let mut store = Store::new(Box::new(backend));

    // Valid JSON but missing the marsadWorkspace envelope key.
    let json = r#"{"name":"X","savedAt":"now","sessions":[]}"#;
    let result = store.import_workspace(json);
    assert!(
        matches!(result, Err(StoreError::BadImport(_))),
        "missing marsadWorkspace field must produce BadImport, got: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// B11 — rescope re-scopes counts/lists/cost
// ---------------------------------------------------------------------------

fn make_snapshot() -> AppSnapshot {
    // ws-a owns s1, s2; ws-b owns s3, s4; s5 belongs to neither.
    let sessions = vec![
        sv("s1", 1.00, 1000, Phase::Working),
        sv("s2", 2.00, 2000, Phase::Done),
        sv("s3", 0.50, 500, Phase::Needs),
        sv("s4", 0.25, 250, Phase::Error),
        sv("s5", 3.00, 3000, Phase::Done),
    ];
    let workspaces = vec![
        VmWorkspace {
            id: "ws-a".to_string(),
            name: "Workspace A".to_string(),
            session_ids: vec!["s1".to_string(), "s2".to_string()],
            saved_at: "2026-06-18T10:00:00Z".to_string(),
        },
        VmWorkspace {
            id: "ws-b".to_string(),
            name: "Workspace B".to_string(),
            session_ids: vec!["s3".to_string(), "s4".to_string()],
            saved_at: "2026-06-18T11:00:00Z".to_string(),
        },
    ];
    AppSnapshot {
        sessions,
        hosts: vec![],
        workspaces,
        active_ws: None,
        selected_id: None,
        filter: Filter::All,
        search: String::new(),
        tab: Tab::Activity,
    }
}

#[test]
fn b11_rescope_restricts_sessions() {
    let snap = make_snapshot();

    // Full snapshot has 5 sessions.
    let full_counts = counts(&snap);
    assert_eq!(full_counts.total, 5, "baseline total is 5");

    // Rescope to ws-a → only s1, s2.
    let scoped = rescope("ws-a", &snap);
    assert_eq!(scoped.sessions.len(), 2, "ws-a has 2 sessions");

    let ids: Vec<&str> = scoped.sessions.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&"s1"), "s1 in ws-a");
    assert!(ids.contains(&"s2"), "s2 in ws-a");
    assert!(!ids.contains(&"s3"), "s3 excluded from ws-a");
    assert!(!ids.contains(&"s4"), "s4 excluded from ws-a");
    assert!(!ids.contains(&"s5"), "s5 excluded from ws-a");
}

#[test]
fn b11_rescope_recalculates_counts() {
    let snap = make_snapshot();

    // ws-a: s1 (Working, $1.00, 1000 tok) + s2 (Done, $2.00, 2000 tok)
    let scoped_a = rescope("ws-a", &snap);
    let c_a = counts(&scoped_a);

    assert_eq!(c_a.total, 2, "ws-a total = 2");
    assert_eq!(c_a.working, 1, "ws-a working = 1 (s1)");
    assert_eq!(c_a.needs, 0, "ws-a needs = 0");
    assert_eq!(c_a.errors, 0, "ws-a errors = 0");
    assert_eq!(c_a.cost, "$3.00", "ws-a cost = $3.00 (1+2)");

    // ws-b: s3 (Needs, $0.50, 500 tok) + s4 (Error, $0.25, 250 tok)
    let scoped_b = rescope("ws-b", &snap);
    let c_b = counts(&scoped_b);

    assert_eq!(c_b.total, 2, "ws-b total = 2");
    assert_eq!(c_b.needs, 1, "ws-b needs = 1 (s3)");
    assert_eq!(c_b.errors, 1, "ws-b errors = 1 (s4)");
    assert_eq!(c_b.working, 0, "ws-b working = 0");
    assert_eq!(c_b.cost, "$0.75", "ws-b cost = $0.75 (0.50+0.25)");
}

#[test]
fn b11_rescope_unknown_workspace_unchanged() {
    let snap = make_snapshot();
    let before_len = snap.sessions.len();

    // Unknown id → snapshot returned unchanged.
    let scoped = rescope("ws-does-not-exist", &snap);
    assert_eq!(
        scoped.sessions.len(),
        before_len,
        "unchanged session count for unknown ws"
    );

    // The unchanged snapshot's counts should match the original.
    let orig_counts = counts(&snap);
    let scoped_counts = counts(&scoped);
    assert_eq!(
        orig_counts, scoped_counts,
        "counts unchanged for unknown workspace"
    );
}

#[test]
fn b11_rescope_empty_workspace() {
    let snap = make_snapshot();

    // Build a snapshot that includes an empty workspace.
    let mut snap2 = snap.clone();
    snap2.workspaces.push(VmWorkspace {
        id: "ws-empty".to_string(),
        name: "Empty".to_string(),
        session_ids: vec![],
        saved_at: "2026-06-18T09:00:00Z".to_string(),
    });

    let scoped = rescope("ws-empty", &snap2);
    assert_eq!(scoped.sessions.len(), 0, "empty workspace → zero sessions");

    let c = counts(&scoped);
    assert_eq!(c.total, 0);
    assert_eq!(c.cost, "$0.00");
}
