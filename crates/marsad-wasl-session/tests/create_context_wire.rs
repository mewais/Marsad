//! create_context_wire.rs — the create-target body must carry the worker's
//! spawn context. Marsad's caller nests `{key, protocol, context:{cwd,model,…}}`
//! inside the opaque target key; `create_request_body` must UNPACK `protocol` +
//! `context` to the top level (a node reads top-level `protocol` + `context`, not
//! a nested key) AND send the target `key` as a bare STRING context id — the
//! node's `CreateTargetRequest.key` is a `TargetKey(String)`, so sending the whole
//! object as `key` makes the node reject the create with a 422
//! ("key: invalid type: map, expected a string") and no target is ever created.
//! Regression for BOTH: the worker spawning with defaults (context not lifted),
//! and the object-shaped key that broke every managed create live.

mod real_support;

use marsad_wasl_session::{
    RealWaslSessionProvider, WaslSessionClientConfig, WaslSessionProvider, WaslTargetKey,
};

use real_support::{compact_fixture, val, BufferedResponse, MockRuntimeServer};

fn provider(server: &MockRuntimeServer) -> RealWaslSessionProvider {
    RealWaslSessionProvider::new(WaslSessionClientConfig {
        base_url: server.base_url(),
    })
}

/// An object key shaped exactly like the desktop builds it:
/// `{key, protocol, context:{cwd, model, session_mode}}`. The create body must
/// lift `protocol` and `context` to the top level AND send the STRING `key`
/// identity (not the object) as the wire `key`.
#[test]
fn create_body_unpacks_protocol_and_context_to_top_level() {
    let server = MockRuntimeServer::start();
    server.push_create(BufferedResponse::json(200, compact_fixture("target.json")));

    let key = WaslTargetKey(val(
        r#"{ "key": "tgt-9",
             "protocol": "claude",
             "context": { "cwd": "/w/proj", "model": "claude-opus-4-5", "session_mode": "plan" } }"#,
    ));
    let p = provider(&server);
    let _ = p.get_or_start(&key).expect("create must succeed on a 200");

    let creates = server.requests_to("POST", "/v1/targets");
    assert_eq!(creates.len(), 1, "exactly one POST /v1/targets");
    let body = creates[0].body_json();

    // protocol lifted to the top level.
    assert_eq!(
        body.get("protocol").and_then(|v| v.as_str()),
        Some("claude"),
        "protocol must be unpacked to the top-level create body"
    );
    // context lifted to the top level, verbatim.
    let ctx = body.get("context").expect("context must be present at top level");
    assert_eq!(ctx.get("cwd").and_then(|v| v.as_str()), Some("/w/proj"));
    assert_eq!(
        ctx.get("model").and_then(|v| v.as_str()),
        Some("claude-opus-4-5")
    );
    assert_eq!(
        ctx.get("session_mode").and_then(|v| v.as_str()),
        Some("plan")
    );
    // The wire `key` is the STRING identity lifted from the object's `key` field —
    // NOT the object (the node's `key` is a `TargetKey(String)`).
    assert_eq!(
        body.get("key").and_then(|v| v.as_str()),
        Some("tgt-9"),
        "the string `key` identity must be sent as the wire `key`, not the object"
    );
}

/// An object key WITHOUT a string `key` field falls back to a canonical JSON
/// encoding of the whole value — still a bare STRING on the wire (never a map),
/// so the node accepts it and the create-or-reuse identity is stable.
#[test]
fn create_body_key_without_id_field_is_canonical_string() {
    let server = MockRuntimeServer::start();
    server.push_create(BufferedResponse::json(200, compact_fixture("target.json")));

    let key = WaslTargetKey(val(
        r#"{ "protocol": "claude", "context": { "cwd": "/w/proj" } }"#,
    ));
    let p = provider(&server);
    let _ = p.get_or_start(&key).expect("create must succeed on a 200");

    let body = server.requests_to("POST", "/v1/targets")[0].body_json();
    // Wire `key` is a STRING (the canonical JSON of the whole value), never a map.
    let wire_key = body
        .get("key")
        .and_then(|v| v.as_str())
        .expect("wire key must be a bare string");
    assert!(
        wire_key.contains("\"protocol\":\"claude\""),
        "fallback key is the canonical JSON of the value: {wire_key:?}"
    );
    // protocol + context are still lifted to the top level.
    assert_eq!(body.get("protocol").and_then(|v| v.as_str()), Some("claude"));
    assert!(body.get("context").is_some());
}

/// A key that carries neither `protocol` nor `context` (e.g. a bare string) is
/// relayed verbatim as just `{key}` — the create body gains no stray fields.
#[test]
fn create_body_without_protocol_or_context_is_just_key() {
    let server = MockRuntimeServer::start();
    server.push_create(BufferedResponse::json(200, compact_fixture("target.json")));

    let p = provider(&server);
    let _ = p
        .get_or_start(&WaslTargetKey(val(r#""ctx-42""#)))
        .expect("create must succeed");

    let creates = server.requests_to("POST", "/v1/targets");
    let body = creates[0].body_json();
    assert_eq!(body.get("key").and_then(|v| v.as_str()), Some("ctx-42"));
    assert!(
        body.get("protocol").is_none(),
        "no protocol key when the target key carries none"
    );
    assert!(
        body.get("context").is_none(),
        "no context key when the target key carries none"
    );
}
