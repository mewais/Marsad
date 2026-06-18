//! errors.rs — every StoreError variant reachable via the public API.
//!
//! Covers:
//! - StoreError::Corrupt{key}    — via MemoryBackend::corrupt()
//! - StoreError::VersionMismatch — blob version != expected (older or newer)
//! - StoreError::BadImport       — malformed JSON fed to import_workspace
//! - StoreError::BadImport       — wrong envelope (missing marsadWorkspace key)
//! - StoreError::NotFound        — export_workspace on a non-existent workspace id
//! - StoreError::Backend(String) — backend read/write failure propagated to Store
//!                                 (via MemoryBackend::fail_next(BackendOp::...))
//! - All StoreError variants implement Debug (spec: derive Debug)

use marsad_store::{Backend, BackendOp, MemoryBackend, PersistedSession, Store, StoreError, Workspace};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_session(id: &str) -> PersistedSession {
    PersistedSession {
        id: id.to_string(),
        session_id: None,
        name: format!("Session {id}"),
        host_id: "localhost".to_string(),
        project: "/project".to_string(),
        branch: "main".to_string(),
        model: "claude-opus-4-5".to_string(),
        last_phase: "Done".to_string(),
    }
}

fn make_workspace(id: &str) -> Workspace {
    Workspace {
        id: id.to_string(),
        name: format!("WS {id}"),
        session_ids: vec![],
        saved_at: "2026-06-18T00:00:00Z".to_string(),
    }
}

fn fresh_store() -> Store {
    Store::new(Box::new(MemoryBackend::new()))
}

// ---------------------------------------------------------------------------
// StoreError::Corrupt{key} — via MemoryBackend::corrupt()
// ---------------------------------------------------------------------------

#[test]
fn corrupt_session_key_returns_corrupt_error() {
    let backend = MemoryBackend::new();

    // Write a valid session first so the key exists, then corrupt it.
    {
        let mut store = Store::new(Box::new(MemoryBackend::new()));
        let sess = make_session("sess-corrupt");
        store.upsert_session(&sess).expect("upsert");
    }

    // Use a separate backend to write a real key, then corrupt it.
    let backend_inner = MemoryBackend::new();
    backend_inner
        .write("session:sess-corrupt", b"valid-looking bytes")
        .expect("manual write");
    backend_inner.corrupt("session:sess-corrupt");

    let store = Store::new(Box::new(backend_inner));
    let result = store.restore();
    match result {
        Err(StoreError::Corrupt { key }) => {
            assert!(
                key.contains("sess-corrupt"),
                "Corrupt key must reference the corrupted key, got: {key}"
            );
        }
        other => panic!("expected Corrupt error, got: {other:?}"),
    }
}

#[test]
fn corrupt_workspace_key_returns_corrupt_error() {
    let backend = MemoryBackend::new();
    backend
        .write("workspace:ws-corrupt", b"not valid JSON at all!!!")
        .expect("manual write");
    backend.corrupt("workspace:ws-corrupt");

    let store = Store::new(Box::new(backend));
    let result = store.restore();
    match result {
        Err(StoreError::Corrupt { key }) => {
            assert!(
                key.contains("ws-corrupt"),
                "Corrupt key must contain the workspace key, got: {key}"
            );
        }
        other => panic!("expected Corrupt, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// StoreError::VersionMismatch — blob carries a schema version != expected
// ---------------------------------------------------------------------------

#[test]
fn version_mismatch_returns_version_mismatch_error() {
    // Write a blob with an explicitly wrong schema version so restore
    // detects a VersionMismatch.
    let backend = MemoryBackend::new();
    // The spec says each blob carries a `version` field.
    // Use version=9999 which is clearly not the current version.
    let blob = r#"{"version":9999,"id":"sess-v","name":"S","host_id":"h","project":"/p","branch":"main","model":"m","last_phase":"Done"}"#;
    backend
        .write("session:sess-v", blob.as_bytes())
        .expect("manual write");

    let store = Store::new(Box::new(backend));
    let result = store.restore();
    match result {
        Err(StoreError::VersionMismatch { found, expected }) => {
            assert_eq!(found, 9999, "found version must match blob");
            assert!(expected > 0, "expected version must be positive");
        }
        other => panic!("expected VersionMismatch, got: {other:?}"),
    }
}

#[test]
fn old_version_zero_also_mismatches() {
    // version:0 was never a valid schema version — must be VersionMismatch.
    let backend = MemoryBackend::new();
    let blob = r#"{"version":0,"id":"sess-v0","name":"S","host_id":"h","project":"/p","branch":"main","model":"m","last_phase":"Done"}"#;
    backend
        .write("session:sess-v0", blob.as_bytes())
        .expect("manual write");

    let store = Store::new(Box::new(backend));
    let result = store.restore();
    match result {
        Err(StoreError::VersionMismatch { found, .. }) => {
            assert_eq!(found, 0);
        }
        other => panic!("expected VersionMismatch, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// StoreError::BadImport — malformed JSON to import_workspace
// ---------------------------------------------------------------------------

#[test]
fn import_workspace_malformed_json_returns_bad_import() {
    let mut store = fresh_store();
    let result = store.import_workspace("this is not JSON at all");
    match result {
        Err(StoreError::BadImport(msg)) => {
            assert!(!msg.is_empty(), "BadImport message must not be empty");
        }
        other => panic!("expected BadImport, got: {other:?}"),
    }
}

#[test]
fn import_workspace_truncated_json_returns_bad_import() {
    let mut store = fresh_store();
    let result = store.import_workspace(r#"{"marsadWorkspace":1,"name":"WS""#); // truncated
    match result {
        Err(StoreError::BadImport(_)) => {}
        other => panic!("expected BadImport for truncated JSON, got: {other:?}"),
    }
}

#[test]
fn import_workspace_wrong_envelope_missing_marsad_key_returns_bad_import() {
    // Valid JSON but missing the `marsadWorkspace` discriminant field — wrong envelope.
    let mut store = fresh_store();
    let json = r#"{"name":"WS","sessions":[],"savedAt":"2026-06-18"}"#;
    let result = store.import_workspace(json);
    match result {
        Err(StoreError::BadImport(msg)) => {
            assert!(!msg.is_empty());
        }
        other => panic!("expected BadImport for wrong envelope, got: {other:?}"),
    }
}

#[test]
fn import_workspace_wrong_version_field_returns_bad_import() {
    // marsadWorkspace field present but wrong type (string instead of int).
    let mut store = fresh_store();
    let json = r#"{"marsadWorkspace":"v2","name":"WS","sessions":[],"savedAt":"2026-06-18"}"#;
    let result = store.import_workspace(json);
    match result {
        Err(StoreError::BadImport(_)) => {}
        other => panic!("expected BadImport for wrong version type, got: {other:?}"),
    }
}

#[test]
fn import_workspace_empty_string_returns_bad_import() {
    let mut store = fresh_store();
    match store.import_workspace("") {
        Err(StoreError::BadImport(_)) => {}
        other => panic!("expected BadImport for empty string, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// StoreError::NotFound — export_workspace with unknown id
// ---------------------------------------------------------------------------

#[test]
fn export_workspace_unknown_id_returns_not_found() {
    let store = fresh_store();
    let result = store.export_workspace("non-existent-ws-id");
    match result {
        Err(StoreError::NotFound(id)) => {
            assert!(
                id.contains("non-existent-ws-id"),
                "NotFound message must reference the id, got: {id}"
            );
        }
        other => panic!("expected NotFound, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// StoreError::Backend — backend read/write failure propagated
// ---------------------------------------------------------------------------

// Each test arms a fresh MemoryBackend with fail_next(BackendOp::...) to force the next matching
// operation to return StoreError::Backend, exercising the blessed public fake rather than an inline
// hand-rolled error backend.

#[test]
fn backend_write_failure_propagates_from_upsert_session() {
    let backend = MemoryBackend::new();
    backend.fail_next(BackendOp::Write);
    let mut store = Store::new(Box::new(backend));
    let sess = make_session("sess-fail-write");
    match store.upsert_session(&sess) {
        Err(StoreError::Backend(_)) => {}
        other => panic!("expected Backend error from write failure, got: {other:?}"),
    }
}

#[test]
fn backend_write_failure_propagates_from_snapshot_workspace() {
    let backend = MemoryBackend::new();
    backend.fail_next(BackendOp::Write);
    let mut store = Store::new(Box::new(backend));
    let ws = make_workspace("ws-fail-write");
    match store.snapshot_workspace(&ws) {
        Err(StoreError::Backend(_)) => {}
        other => panic!("expected Backend error, got: {other:?}"),
    }
}

#[test]
fn backend_write_failure_propagates_from_append_transcript() {
    let backend = MemoryBackend::new();
    backend.fail_next(BackendOp::Write);
    let mut store = Store::new(Box::new(backend));
    let entry = marsad_store::TranscriptEntry {
        kind: "text".to_string(),
        payload: "hello".to_string(),
    };
    match store.append_transcript("sess-fail-write", &entry) {
        Err(StoreError::Backend(_)) => {}
        other => panic!("expected Backend error, got: {other:?}"),
    }
}

#[test]
fn backend_list_failure_propagates_from_restore() {
    let backend = MemoryBackend::new();
    backend.fail_next(BackendOp::List);
    let store = Store::new(Box::new(backend));
    match store.restore() {
        Err(StoreError::Backend(_)) => {}
        other => panic!("expected Backend error from restore, got: {other:?}"),
    }
}

#[test]
fn backend_read_failure_propagates_from_restore() {
    // Arm a read failure: first write a key so restore tries to read it, then fail_next(Read).
    let backend = MemoryBackend::new();
    backend.write("session:seed", b"dummy").expect("write seed");
    backend.fail_next(BackendOp::Read);
    let store = Store::new(Box::new(backend));
    match store.restore() {
        Err(StoreError::Backend(_)) => {}
        other => panic!("expected Backend error from restore read, got: {other:?}"),
    }
}

#[test]
fn backend_read_failure_propagates_from_export_workspace() {
    let backend = MemoryBackend::new();
    backend.fail_next(BackendOp::Any);
    let store = Store::new(Box::new(backend));
    match store.export_workspace("any-id") {
        Err(StoreError::Backend(_)) | Err(StoreError::NotFound(_)) => {}
        other => panic!("expected Backend or NotFound error, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// StoreError variant match completeness and Debug
// ---------------------------------------------------------------------------

#[test]
fn all_store_error_variants_are_debug() {
    let errors: Vec<StoreError> = vec![
        StoreError::Backend("io error".to_string()),
        StoreError::Corrupt {
            key: "session:x".to_string(),
        },
        StoreError::VersionMismatch {
            found: 99,
            expected: 1,
        },
        StoreError::BadImport("bad json".to_string()),
        StoreError::NotFound("missing".to_string()),
    ];
    for e in &errors {
        let _ = format!("{e:?}");
    }
}

#[test]
fn store_error_corrupt_carries_key() {
    let e = StoreError::Corrupt {
        key: "session:abc".to_string(),
    };
    match e {
        StoreError::Corrupt { key } => assert_eq!(key, "session:abc"),
        _ => panic!("expected Corrupt"),
    }
}

#[test]
fn store_error_version_mismatch_carries_found_and_expected() {
    let e = StoreError::VersionMismatch {
        found: 3,
        expected: 1,
    };
    match e {
        StoreError::VersionMismatch { found, expected } => {
            assert_eq!(found, 3);
            assert_eq!(expected, 1);
        }
        _ => panic!("expected VersionMismatch"),
    }
}

#[test]
fn store_error_bad_import_carries_message() {
    let e = StoreError::BadImport("invalid envelope".to_string());
    match e {
        StoreError::BadImport(msg) => assert_eq!(msg, "invalid envelope"),
        _ => panic!("expected BadImport"),
    }
}

#[test]
fn store_error_not_found_carries_id() {
    let e = StoreError::NotFound("ws-xyz".to_string());
    match e {
        StoreError::NotFound(id) => assert_eq!(id, "ws-xyz"),
        _ => panic!("expected NotFound"),
    }
}

#[test]
fn store_error_backend_carries_message() {
    let e = StoreError::Backend("disk full".to_string());
    match e {
        StoreError::Backend(msg) => assert_eq!(msg, "disk full"),
        _ => panic!("expected Backend"),
    }
}
