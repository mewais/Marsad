//! wasl_bindings.rs — X6 Wasl Backend session-binding persistence.
//!
//! Covers the additive `marsad-store` API for `PersistedWaslBinding`
//! (`wasl-backend-plan.md` §6.1): its OWN keyed record under the
//! `waslbinding:<session_id>` prefix, fully additive and backward-compatible
//! (an absent binding restores as `RuntimeOwner::MarsadManaged` elsewhere).
//!
//! - upsert_wasl_binding → wasl_bindings() reflects it (happy round-trip, incl.
//!   the opaque `provider_key: serde_json::Value` and every field)
//! - delete_wasl_binding removes it; wasl_bindings() no longer includes it
//! - multiple bindings for different session ids coexist correctly
//! - a version-mismatch record is surfaced as StoreError::VersionMismatch, not
//!   a panic (the "skip on error" branch the audit flagged in wasl_bindings())
//! - a corrupt record surfaces as StoreError::Corrupt, not a panic
//! - a Backend list/read/write/delete failure surfaces as StoreError::Backend
//! - an absent binding (a session with no binding ever written) doesn't appear
//!   in wasl_bindings() and doesn't break anything
//! - the binding records do NOT pollute restore()'s other collections, and
//!   restore() is unaffected by their presence (backward compatibility)
//!
//! Style mirrors the sibling `happy.rs` / `errors.rs` / `workstreams.rs` files:
//! `MemoryBackend` fakes, `BackendOp::fail_next`, `MemoryBackend::corrupt`.

use marsad_store::{
    Backend, BackendOp, MemoryBackend, PersistedSession, PersistedWaslBinding, Store, StoreError,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn fresh_store() -> Store {
    Store::new(Box::new(MemoryBackend::new()))
}

fn binding(session_id: &str) -> PersistedWaslBinding {
    PersistedWaslBinding {
        session_id: session_id.to_string(),
        target_id: format!("target-{session_id}"),
        host_id: "local".to_string(),
        provider_key: serde_json::json!({ "protocol": "claude", "ctx": session_id }),
        last_seq: 0,
        transcript_json: String::new(),
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

// ---------------------------------------------------------------------------
// Happy round-trip: upsert_wasl_binding → wasl_bindings()
// ---------------------------------------------------------------------------

#[test]
fn upsert_wasl_binding_then_wasl_bindings_reflects_it() {
    let mut store = fresh_store();
    let b = binding("sess-wb");
    store
        .upsert_wasl_binding(&b)
        .expect("upsert_wasl_binding must succeed");

    let bindings = store.wasl_bindings().expect("wasl_bindings must succeed");
    let got = bindings
        .get("sess-wb")
        .expect("binding for sess-wb must be present");
    assert_eq!(got, &b, "the round-tripped binding must equal the original");
}

#[test]
fn upsert_wasl_binding_preserves_all_fields_incl_provider_key_and_last_seq() {
    let mut store = fresh_store();
    let b = PersistedWaslBinding {
        session_id: "sess-fields".to_string(),
        target_id: "wasl-target-42".to_string(),
        host_id: "node-7".to_string(),
        provider_key: serde_json::json!({
            "protocol": "claude",
            "ctx": { "nested": ["a", 1, true, null] }
        }),
        last_seq: 987_654_321,
        transcript_json: String::new(),
    };
    store.upsert_wasl_binding(&b).expect("upsert");
    let bindings = store.wasl_bindings().expect("wasl_bindings");
    let got = bindings.get("sess-fields").expect("binding present");
    assert_eq!(got.session_id, "sess-fields");
    assert_eq!(got.target_id, "wasl-target-42");
    assert_eq!(got.host_id, "node-7");
    assert_eq!(
        got.provider_key,
        serde_json::json!({
            "protocol": "claude",
            "ctx": { "nested": ["a", 1, true, null] }
        }),
        "the opaque provider_key Value must round-trip verbatim"
    );
    assert_eq!(got.last_seq, 987_654_321);
}

#[test]
fn upsert_wasl_binding_overwrite_updates_fields_not_duplicates() {
    let mut store = fresh_store();
    let mut b = binding("sess-ow");
    store.upsert_wasl_binding(&b).expect("first upsert");
    b.last_seq = 55;
    b.target_id = "target-updated".to_string();
    store.upsert_wasl_binding(&b).expect("second upsert");

    let bindings = store.wasl_bindings().expect("wasl_bindings");
    // Keyed by session_id → overwrite, never a duplicate entry.
    assert_eq!(
        bindings.len(),
        1,
        "an overwrite of the same session id must not create a duplicate binding"
    );
    let got = bindings.get("sess-ow").expect("binding present");
    assert_eq!(got.last_seq, 55);
    assert_eq!(got.target_id, "target-updated");
}

#[test]
fn upsert_wasl_binding_default_empty_provider_key_round_trips() {
    // provider_key defaults to serde_json::Value::Null when not yet known.
    let mut store = fresh_store();
    let b = PersistedWaslBinding {
        session_id: "sess-null-key".to_string(),
        target_id: String::new(),
        host_id: String::new(),
        provider_key: serde_json::Value::Null,
        last_seq: 0,
        transcript_json: String::new(),
    };
    store.upsert_wasl_binding(&b).expect("upsert");
    let bindings = store.wasl_bindings().expect("wasl_bindings");
    let got = bindings.get("sess-null-key").expect("binding present");
    assert_eq!(got.provider_key, serde_json::Value::Null);
    assert!(got.target_id.is_empty());
    assert!(got.host_id.is_empty());
}

// ---------------------------------------------------------------------------
// delete_wasl_binding removes it
// ---------------------------------------------------------------------------

#[test]
fn delete_wasl_binding_removes_it_from_wasl_bindings() {
    let mut store = fresh_store();
    store
        .upsert_wasl_binding(&binding("sess-del"))
        .expect("upsert_wasl_binding");
    store
        .delete_wasl_binding("sess-del")
        .expect("delete_wasl_binding must succeed");

    let bindings = store.wasl_bindings().expect("wasl_bindings");
    assert!(
        !bindings.contains_key("sess-del"),
        "a deleted binding must not appear in wasl_bindings()"
    );
}

#[test]
fn delete_wasl_binding_absent_id_is_ok() {
    let mut store = fresh_store();
    store
        .delete_wasl_binding("never-written")
        .expect("delete_wasl_binding on an absent id must succeed (idempotent no-op)");
    let bindings = store.wasl_bindings().expect("wasl_bindings");
    assert!(
        bindings.is_empty(),
        "no bindings must exist after deleting an absent one"
    );
}

#[test]
fn delete_wasl_binding_only_removes_the_targeted_session() {
    let mut store = fresh_store();
    store
        .upsert_wasl_binding(&binding("sess-keep"))
        .expect("upsert keep");
    store
        .upsert_wasl_binding(&binding("sess-drop"))
        .expect("upsert drop");
    store
        .delete_wasl_binding("sess-drop")
        .expect("delete_wasl_binding");

    let bindings = store.wasl_bindings().expect("wasl_bindings");
    assert!(
        !bindings.contains_key("sess-drop"),
        "the deleted binding must be gone"
    );
    assert!(
        bindings.contains_key("sess-keep"),
        "an unrelated binding must be untouched by a targeted delete"
    );
    assert_eq!(bindings.len(), 1);
}

// ---------------------------------------------------------------------------
// Multiple bindings for different session ids coexist
// ---------------------------------------------------------------------------

#[test]
fn multiple_wasl_bindings_coexist_keyed_by_session_id() {
    let mut store = fresh_store();
    for id in ["sess-a", "sess-b", "sess-c", "sess-d"] {
        store
            .upsert_wasl_binding(&binding(id))
            .expect("upsert_wasl_binding");
    }
    let bindings = store.wasl_bindings().expect("wasl_bindings");
    assert_eq!(bindings.len(), 4, "all four bindings must coexist");
    for id in ["sess-a", "sess-b", "sess-c", "sess-d"] {
        let got = bindings
            .get(id)
            .unwrap_or_else(|| panic!("binding for {id} must be present"));
        assert_eq!(
            got.session_id, id,
            "each binding must be keyed by its own session id"
        );
        assert_eq!(
            got.target_id,
            format!("target-{id}"),
            "no cross-contamination between distinct bindings"
        );
    }
}

// ---------------------------------------------------------------------------
// Absent binding: a session with no binding ever written
// ---------------------------------------------------------------------------

#[test]
fn wasl_bindings_empty_when_none_written() {
    let store = fresh_store();
    let bindings = store
        .wasl_bindings()
        .expect("wasl_bindings on an empty store must succeed");
    assert!(
        bindings.is_empty(),
        "wasl_bindings() must be empty when no binding was ever persisted"
    );
}

#[test]
fn session_without_a_binding_does_not_appear_in_wasl_bindings() {
    // A plain session record (no binding) must not leak into wasl_bindings():
    // the absent binding defaults to MarsadManaged elsewhere in the system.
    let mut store = fresh_store();
    store
        .upsert_session(&make_session("plain-sess"))
        .expect("upsert_session");
    // Only sess-with-binding gets a binding.
    store
        .upsert_wasl_binding(&binding("sess-with-binding"))
        .expect("upsert_wasl_binding");

    let bindings = store.wasl_bindings().expect("wasl_bindings");
    assert!(
        !bindings.contains_key("plain-sess"),
        "a session that never had a binding written must be absent from wasl_bindings()"
    );
    assert!(
        bindings.contains_key("sess-with-binding"),
        "the session that DID get a binding must be present"
    );
    assert_eq!(
        bindings.len(),
        1,
        "wasl_bindings() must contain exactly the bindings that were written"
    );
}

// ---------------------------------------------------------------------------
// Error / skip paths in wasl_bindings() — must not panic
// ---------------------------------------------------------------------------

#[test]
fn wasl_bindings_version_mismatch_returns_version_mismatch_error_not_panic() {
    // A binding blob carrying a schema version != the current one must surface
    // as StoreError::VersionMismatch (the wasl_bindings() version-check branch),
    // not a panic.
    let backend = MemoryBackend::new();
    let blob = r#"{"version":9999,"payload":{"session_id":"sess-v","target_id":"t","host_id":"local","provider_key":null,"last_seq":0}}"#;
    backend
        .write("waslbinding:sess-v", blob.as_bytes())
        .expect("manual write");

    let store = Store::new(Box::new(backend));
    match store.wasl_bindings() {
        Err(StoreError::VersionMismatch { found, expected }) => {
            assert_eq!(found, 9999, "found version must match the blob");
            assert!(expected > 0, "expected version must be positive");
        }
        other => panic!("expected VersionMismatch, got: {other:?}"),
    }
}

#[test]
fn wasl_bindings_version_zero_also_mismatches() {
    // A blob with no/zero version (blob_version() → 0) is a VersionMismatch too.
    let backend = MemoryBackend::new();
    let blob = r#"{"version":0,"payload":{"session_id":"sess-v0"}}"#;
    backend
        .write("waslbinding:sess-v0", blob.as_bytes())
        .expect("manual write");

    let store = Store::new(Box::new(backend));
    match store.wasl_bindings() {
        Err(StoreError::VersionMismatch { found, .. }) => assert_eq!(found, 0),
        other => panic!("expected VersionMismatch, got: {other:?}"),
    }
}

#[test]
fn wasl_bindings_corrupt_record_returns_corrupt_error_not_panic() {
    // A corrupted binding record surfaces from the read? in wasl_bindings() as
    // StoreError::Corrupt (via MemoryBackend::corrupt), not a panic.
    let backend = MemoryBackend::new();
    backend
        .write("waslbinding:sess-corrupt", b"not valid JSON at all!!!")
        .expect("manual write");
    backend.corrupt("waslbinding:sess-corrupt");

    let store = Store::new(Box::new(backend));
    match store.wasl_bindings() {
        Err(StoreError::Corrupt { key }) => {
            assert!(
                key.contains("sess-corrupt"),
                "Corrupt key must reference the corrupted binding key, got: {key}"
            );
        }
        other => panic!("expected Corrupt, got: {other:?}"),
    }
}

#[test]
fn wasl_bindings_backend_list_failure_propagates() {
    let backend = MemoryBackend::new();
    backend.fail_next(BackendOp::List);
    let store = Store::new(Box::new(backend));
    match store.wasl_bindings() {
        Err(StoreError::Backend(_)) => {}
        other => panic!("expected Backend error from the list() in wasl_bindings, got: {other:?}"),
    }
}

#[test]
fn wasl_bindings_backend_read_failure_propagates() {
    // A key exists (so wasl_bindings() attempts a read), then the read is armed
    // to fail: the read? Err branch surfaces as StoreError::Backend.
    let backend = MemoryBackend::new();
    backend
        .write("waslbinding:sess-r", b"whatever")
        .expect("seed key");
    backend.fail_next(BackendOp::Read);
    let store = Store::new(Box::new(backend));
    match store.wasl_bindings() {
        Err(StoreError::Backend(_)) => {}
        other => panic!("expected Backend error from the read() in wasl_bindings, got: {other:?}"),
    }
}

#[test]
fn backend_write_failure_propagates_from_upsert_wasl_binding() {
    let backend = MemoryBackend::new();
    backend.fail_next(BackendOp::Write);
    let mut store = Store::new(Box::new(backend));
    match store.upsert_wasl_binding(&binding("sess-fail-write")) {
        Err(StoreError::Backend(_)) => {}
        other => panic!("expected Backend error from write failure, got: {other:?}"),
    }
}

#[test]
fn backend_delete_failure_propagates_from_delete_wasl_binding() {
    let backend = MemoryBackend::new();
    backend.fail_next(BackendOp::Delete);
    let mut store = Store::new(Box::new(backend));
    match store.delete_wasl_binding("sess-x") {
        Err(StoreError::Backend(_)) => {}
        other => panic!("expected Backend error from delete failure, got: {other:?}"),
    }
}

#[test]
fn wasl_bindings_skips_a_key_that_reads_as_none() {
    // A key returned by list() whose read() yields Ok(None) (a listed key that
    // has since vanished — a list/read race) must be silently skipped, not
    // inserted and not an error. This exercises the None arm of the
    // `if let Some(bytes) = read(...)?` in wasl_bindings(). MemoryBackend can't
    // produce this (its listed keys are always readable), so use a tiny custom
    // Backend that lists one phantom key which reads as None.
    struct PhantomListBackend;
    impl Backend for PhantomListBackend {
        fn read(&self, _key: &str) -> Result<Option<Vec<u8>>, StoreError> {
            // Every read reports "absent" — the list/read race outcome.
            Ok(None)
        }
        fn write(&self, _key: &str, _bytes: &[u8]) -> Result<(), StoreError> {
            Ok(())
        }
        fn list(&self, prefix: &str) -> Result<Vec<String>, StoreError> {
            // Only surface a phantom binding key under the queried prefix.
            if "waslbinding:phantom".starts_with(prefix) {
                Ok(vec!["waslbinding:phantom".to_string()])
            } else {
                Ok(vec![])
            }
        }
        fn delete(&self, _key: &str) -> Result<(), StoreError> {
            Ok(())
        }
    }

    let store = Store::new(Box::new(PhantomListBackend));
    let bindings = store
        .wasl_bindings()
        .expect("a listed-but-absent key must be skipped, not error");
    assert!(
        bindings.is_empty(),
        "a key that reads as None must be skipped, leaving no binding entry"
    );
}

// ---------------------------------------------------------------------------
// Backward compatibility: bindings are a SEPARATE record; restore() unaffected
// ---------------------------------------------------------------------------

#[test]
fn wasl_bindings_do_not_pollute_restore_collections() {
    // The waslbinding: records are a self-contained namespace read only by
    // wasl_bindings(); restore() must ignore them entirely (no stray session /
    // workspace / etc. entry), keeping the addition backward-compatible.
    let mut store = fresh_store();
    store
        .upsert_session(&make_session("s-real"))
        .expect("upsert_session");
    store
        .upsert_wasl_binding(&binding("s-real"))
        .expect("upsert_wasl_binding");
    store
        .upsert_wasl_binding(&binding("s-detached"))
        .expect("upsert_wasl_binding");

    let restored = store.restore().expect("restore must succeed");
    // Exactly the one real session — the two binding records add nothing.
    assert_eq!(
        restored.sessions.len(),
        1,
        "binding records must not be restored as sessions"
    );
    assert_eq!(restored.sessions[0].id, "s-real");
    assert!(restored.workspaces.is_empty());
    assert!(restored.hosts.is_empty());
    assert!(restored.templates.is_empty());
    assert!(restored.workstreams.is_empty());
    assert!(restored.connections.is_empty());
    assert!(restored.profiles.is_empty());
    assert!(restored.transcripts.is_empty());

    // And the bindings are still independently retrievable.
    let bindings = store.wasl_bindings().expect("wasl_bindings");
    assert_eq!(bindings.len(), 2);
}

// ---------------------------------------------------------------------------
// Derived traits: Debug, Clone, PartialEq on PersistedWaslBinding
// ---------------------------------------------------------------------------

#[test]
fn persisted_wasl_binding_derives_clone_eq_debug() {
    let b = binding("deriv");
    let b2 = b.clone();
    assert_eq!(b, b2);
    let _ = format!("{b:?}");
}
