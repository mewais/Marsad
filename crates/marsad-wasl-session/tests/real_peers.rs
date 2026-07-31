//! real_peers.rs — black-box tests for the three new peer CRUD methods on
//! `RealWaslSessionProvider`:
//!   * `register_peer(card)` → POST /v1/peers; 204 = Ok(())
//!   * `list_peers()`         → GET  /v1/peers; parses `{"peers":[…]}`
//!   * `unregister_peer(id)`  → DELETE /v1/peers/:id; 204→Ok(true), 404→Ok(false)
//!
//! Written test-first (RED before implementation). Uses the existing
//! `real_support::MockRuntimeServer` loopback harness, extended with peer
//! queues.

mod real_support;

use marsad_wasl_session::{
    RealWaslSessionProvider, WaslAgentCard, WaslSessionClientConfig, WaslSessionProvider,
};

use real_support::{BufferedResponse, MockRuntimeServer};

fn provider(server: &MockRuntimeServer) -> RealWaslSessionProvider {
    RealWaslSessionProvider::new(WaslSessionClientConfig {
        base_url: server.base_url(),
    })
}

fn card(id: &str, host: &str, endpoint: Option<&str>) -> WaslAgentCard {
    WaslAgentCard {
        id: id.to_string(),
        host: host.to_string(),
        endpoint: endpoint.map(|s| s.to_string()),
    }
}

// ---------------------------------------------------------------------------
// register_peer — POST /v1/peers
// ---------------------------------------------------------------------------

/// `register_peer` sends a POST to `/v1/peers` with the correct JSON body and
/// Content-Type, and returns `Ok(())` on a 204.
#[test]
fn register_peer_posts_to_v1_peers_with_correct_body_and_returns_ok_on_204() {
    let server = MockRuntimeServer::start();
    server.push_peers_register(BufferedResponse::empty(204));

    let p = provider(&server);
    p.register_peer(card("node-a", "host-a", Some("http://10.0.0.1:8787")))
        .expect("register_peer must succeed on a 204");

    let reqs = server.requests_to("POST", "/v1/peers");
    assert_eq!(reqs.len(), 1, "exactly one POST /v1/peers");
    let req = &reqs[0];
    assert_eq!(
        req.header("content-type"),
        Some("application/json"),
        "body must be sent as application/json"
    );
    let body = req.body_json();
    assert_eq!(
        body.get("id").and_then(|v| v.as_str()),
        Some("node-a"),
        "body must carry the peer id"
    );
    assert_eq!(
        body.get("host").and_then(|v| v.as_str()),
        Some("host-a"),
        "body must carry the peer host"
    );
    assert_eq!(
        body.get("endpoint").and_then(|v| v.as_str()),
        Some("http://10.0.0.1:8787"),
        "body must carry the endpoint when present"
    );
}

/// `register_peer` with `endpoint: None` must NOT include the `endpoint` key
/// in the serialized body (or include it as `null` — either is fine; what
/// matters is that the call succeeds and the required fields are present).
#[test]
fn register_peer_with_no_endpoint_succeeds_on_204() {
    let server = MockRuntimeServer::start();
    server.push_peers_register(BufferedResponse::empty(204));

    let p = provider(&server);
    p.register_peer(card("node-b", "host-b", None))
        .expect("register_peer with no endpoint must succeed on a 204");

    let reqs = server.requests_to("POST", "/v1/peers");
    assert_eq!(reqs.len(), 1, "exactly one POST /v1/peers");
    let body = reqs[0].body_json();
    assert_eq!(body.get("id").and_then(|v| v.as_str()), Some("node-b"));
    assert_eq!(body.get("host").and_then(|v| v.as_str()), Some("host-b"));
}

// ---------------------------------------------------------------------------
// list_peers — GET /v1/peers
// ---------------------------------------------------------------------------

/// `list_peers` sends a GET to `/v1/peers`, parses `{"peers":[…]}`, and
/// returns the entries sorted by id ascending (the Wasl wire contract).
#[test]
fn list_peers_gets_v1_peers_and_returns_sorted_cards() {
    let server = MockRuntimeServer::start();
    // Two peers out of id order — client must sort them.
    let body = r#"{"peers":[
        {"id":"zeta","host":"host-z","endpoint":"http://10.0.0.2:8787"},
        {"id":"alpha","host":"host-a"}
    ]}"#;
    server.push_peers_list(BufferedResponse::json(200, body));

    let p = provider(&server);
    let peers = p.list_peers().expect("list_peers must succeed on a 200");

    assert_eq!(peers.len(), 2, "two peers returned");
    assert_eq!(peers[0].id, "alpha", "sorted ascending: first = alpha");
    assert_eq!(peers[1].id, "zeta", "sorted ascending: second = zeta");
    assert_eq!(
        peers[0].endpoint, None,
        "alpha has no endpoint in the fixture"
    );
    assert_eq!(
        peers[1].endpoint,
        Some("http://10.0.0.2:8787".to_string()),
        "zeta's endpoint must be parsed"
    );

    let gets = server.requests_to("GET", "/v1/peers");
    assert_eq!(gets.len(), 1, "exactly one GET /v1/peers");
}

/// `list_peers` on an empty registry is an empty `Vec`, not an error.
#[test]
fn list_peers_empty_registry_is_empty_vec_not_error() {
    let server = MockRuntimeServer::start();
    server.push_peers_list(BufferedResponse::json(200, r#"{"peers":[]}"#));

    let p = provider(&server);
    let peers = p.list_peers().expect("empty list is valid");
    assert!(peers.is_empty(), "empty peers list → empty Vec, not error");
}

/// `list_peers` on a malformed body surfaces as a `Protocol` error.
#[test]
fn list_peers_malformed_body_is_protocol_error() {
    let server = MockRuntimeServer::start();
    server.push_peers_list(BufferedResponse::json(200, r#"{"not":"peers"}"#));

    let p = provider(&server);
    let err = p
        .list_peers()
        .expect_err("malformed 200 body must surface as a Protocol error");
    assert!(
        matches!(err, marsad_wasl_session::WaslSessionError::Protocol(_)),
        "malformed body → Protocol, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// unregister_peer — DELETE /v1/peers/:id
// ---------------------------------------------------------------------------

/// `unregister_peer` sends a DELETE to `/v1/peers/:id` and returns `Ok(true)`
/// on a 204 (peer existed and was removed).
#[test]
fn unregister_peer_returns_true_on_204() {
    let server = MockRuntimeServer::start();
    server.push_peers_delete(BufferedResponse::empty(204));

    let p = provider(&server);
    let result = p
        .unregister_peer("node-a")
        .expect("unregister_peer must not error on 204");
    assert!(result, "Ok(true) = peer existed and was removed (204)");

    let deletes = server.requests_with_path_prefix("DELETE", "/v1/peers/");
    assert_eq!(deletes.len(), 1, "exactly one DELETE /v1/peers/…");
    assert!(
        deletes[0].path.contains("node-a"),
        "DELETE path must include the peer id"
    );
}

/// `unregister_peer` returns `Ok(false)` on a 404 with
/// `{"error":"unknown_peer","detail":"…"}` — this is NOT an error condition.
#[test]
fn unregister_peer_returns_false_on_404_unknown_peer() {
    let server = MockRuntimeServer::start();
    server.push_peers_delete(BufferedResponse::json(
        404,
        r#"{"error":"unknown_peer","detail":"node-ghost"}"#,
    ));

    let p = provider(&server);
    let result = p
        .unregister_peer("node-ghost")
        .expect("unregister_peer on unknown_peer 404 must not error");
    assert!(
        !result,
        "Ok(false) = peer was already absent (404 unknown_peer)"
    );
}

/// A non-404 non-2xx on DELETE maps to the appropriate error (not silently
/// swallowed as Ok(false)).
#[test]
fn unregister_peer_non_404_error_maps_to_error() {
    let server = MockRuntimeServer::start();
    server.push_peers_delete(BufferedResponse::json(
        400,
        r#"{"error":"malformed","detail":"bad id"}"#,
    ));

    let p = provider(&server);
    let err = p
        .unregister_peer("bad")
        .expect_err("a 400 on DELETE must surface as an error, not Ok(false)");
    assert!(
        matches!(err, marsad_wasl_session::WaslSessionError::Protocol(_)),
        "a 400 malformed DELETE maps to Protocol, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Non-2xx error dispositions for register / list (mirrors the get_or_start
// 429/400 error tests — the control-error mapping is proven per call site).
// ---------------------------------------------------------------------------

/// A non-2xx on `POST /v1/peers` surfaces as an error (here a 429 →
/// `Unavailable`), never a silent `Ok(())`.
#[test]
fn register_peer_non_2xx_maps_to_error() {
    let server = MockRuntimeServer::start();
    server.push_peers_register(BufferedResponse::json(
        429,
        r#"{"error":"admission_refused","detail":"node busy"}"#,
    ));

    let p = provider(&server);
    let err = p
        .register_peer(card("node-a", "host-a", None))
        .expect_err("a non-2xx register must surface as an error");
    assert!(
        matches!(err, marsad_wasl_session::WaslSessionError::Unavailable(_)),
        "429 register → Unavailable, got {err:?}"
    );
}

/// A non-2xx on `GET /v1/peers` surfaces as an error (here a 429 →
/// `Unavailable`), never an empty list.
#[test]
fn list_peers_non_2xx_maps_to_error() {
    let server = MockRuntimeServer::start();
    server.push_peers_list(BufferedResponse::json(
        429,
        r#"{"error":"admission_refused","detail":"node busy"}"#,
    ));

    let p = provider(&server);
    let err = p
        .list_peers()
        .expect_err("a non-2xx list must surface as an error");
    assert!(
        matches!(err, marsad_wasl_session::WaslSessionError::Unavailable(_)),
        "429 list → Unavailable, got {err:?}"
    );
}
