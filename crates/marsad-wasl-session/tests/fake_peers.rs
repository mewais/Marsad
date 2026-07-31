//! fake_peers.rs — CRUD round-trip tests for the three new peer methods on
//! `FakeWaslSessionProvider`: `register_peer`, `unregister_peer`, `list_peers`.
//! Written test-first (RED before implementation, per house test-first rule).

use marsad_wasl_session::{FakeWaslSessionProvider, WaslAgentCard, WaslSessionProvider};

fn card(id: &str, host: &str, endpoint: Option<&str>) -> WaslAgentCard {
    WaslAgentCard {
        id: id.to_string(),
        host: host.to_string(),
        endpoint: endpoint.map(|s| s.to_string()),
    }
}

// ---------------------------------------------------------------------------
// list_peers — initially empty
// ---------------------------------------------------------------------------

#[test]
fn list_peers_is_empty_on_a_fresh_provider() {
    let provider = FakeWaslSessionProvider::new();
    let peers = provider.list_peers().expect("list_peers must not error");
    assert!(
        peers.is_empty(),
        "a fresh provider must have no peers, got {:?}",
        peers
    );
}

// ---------------------------------------------------------------------------
// register_peer + list_peers — insert + ordering
// ---------------------------------------------------------------------------

/// Register two peers out-of-id-order; `list_peers` must return them sorted by
/// id ascending (mirroring the Wasl wire contract).
#[test]
fn list_peers_returns_peers_sorted_by_id_ascending() {
    let provider = FakeWaslSessionProvider::new();
    provider
        .register_peer(card("bravo", "host-b", Some("http://10.0.0.2:8787")))
        .expect("register bravo");
    provider
        .register_peer(card("alpha", "host-a", None))
        .expect("register alpha");

    let peers = provider.list_peers().expect("list_peers");
    assert_eq!(peers.len(), 2, "must have two peers");
    assert_eq!(
        peers[0].id, "alpha",
        "first must be alpha (sorted ascending)"
    );
    assert_eq!(
        peers[1].id, "bravo",
        "second must be bravo (sorted ascending)"
    );
}

// ---------------------------------------------------------------------------
// register_peer — idempotent overwrite
// ---------------------------------------------------------------------------

/// Registering the same id a second time (with a different endpoint) must
/// overwrite — count stays 1, endpoint is updated.
#[test]
fn register_peer_overwrites_existing_entry_for_same_id() {
    let provider = FakeWaslSessionProvider::new();
    provider
        .register_peer(card("node-1", "host-x", None))
        .expect("first register");
    provider
        .register_peer(card("node-1", "host-x", Some("http://10.0.0.1:8787")))
        .expect("overwrite register");

    let peers = provider.list_peers().expect("list_peers");
    assert_eq!(peers.len(), 1, "count must stay 1 after overwrite");
    assert_eq!(
        peers[0].endpoint,
        Some("http://10.0.0.1:8787".to_string()),
        "endpoint must be updated to the new value"
    );
}

// ---------------------------------------------------------------------------
// register_peer — fields are preserved verbatim
// ---------------------------------------------------------------------------

#[test]
fn register_peer_preserves_all_fields_verbatim() {
    let provider = FakeWaslSessionProvider::new();
    let c = card("peer-z", "some-host", Some("http://192.168.1.5:9000"));
    provider.register_peer(c.clone()).expect("register");

    let peers = provider.list_peers().expect("list_peers");
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].id, "peer-z");
    assert_eq!(peers[0].host, "some-host");
    assert_eq!(
        peers[0].endpoint,
        Some("http://192.168.1.5:9000".to_string())
    );
}

// ---------------------------------------------------------------------------
// unregister_peer — present → Ok(true) then again → Ok(false)
// ---------------------------------------------------------------------------

#[test]
fn unregister_peer_present_returns_true_absent_returns_false() {
    let provider = FakeWaslSessionProvider::new();
    provider
        .register_peer(card("to-remove", "host-r", None))
        .expect("register");

    let first = provider
        .unregister_peer("to-remove")
        .expect("unregister present peer must not error");
    assert!(first, "removing a present peer must return Ok(true)");

    let second = provider
        .unregister_peer("to-remove")
        .expect("unregister already-absent peer must not error");
    assert!(
        !second,
        "removing an already-absent peer must return Ok(false)"
    );
}

/// `unregister_peer` on a peer id that was never registered must return
/// `Ok(false)` — not an error.
#[test]
fn unregister_peer_never_registered_returns_false_not_error() {
    let provider = FakeWaslSessionProvider::new();
    let result = provider
        .unregister_peer("ghost-id")
        .expect("unregistering an unknown peer is Ok(false), not an error");
    assert!(!result, "expected Ok(false) for a never-registered peer id");
}

// ---------------------------------------------------------------------------
// list_peers reflects removal
// ---------------------------------------------------------------------------

#[test]
fn list_peers_reflects_removal() {
    let provider = FakeWaslSessionProvider::new();
    provider
        .register_peer(card("keep", "host-k", None))
        .expect("register keep");
    provider
        .register_peer(card("drop", "host-d", None))
        .expect("register drop");

    provider.unregister_peer("drop").expect("unregister drop");

    let peers = provider.list_peers().expect("list_peers after removal");
    assert_eq!(peers.len(), 1, "one peer must remain after removal");
    assert_eq!(peers[0].id, "keep", "the surviving peer must be 'keep'");
}

// ---------------------------------------------------------------------------
// register then unregister then re-register
// ---------------------------------------------------------------------------

#[test]
fn re_register_after_unregister_works() {
    let provider = FakeWaslSessionProvider::new();
    provider
        .register_peer(card("cycled", "host-c", None))
        .expect("first register");
    provider.unregister_peer("cycled").expect("unregister");
    provider
        .register_peer(card("cycled", "host-c", Some("http://10.0.0.3:8787")))
        .expect("re-register");

    let peers = provider.list_peers().expect("list_peers");
    assert_eq!(peers.len(), 1);
    assert_eq!(
        peers[0].endpoint,
        Some("http://10.0.0.3:8787".to_string()),
        "re-registered entry must carry the new endpoint"
    );
}
