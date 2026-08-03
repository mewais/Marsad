//! create_context_wire.rs — the create-target body must carry the worker's
//! spawn context. Marsad's caller nests `{protocol, context:{cwd,model,…}}`
//! inside the opaque target key; `create_request_body` must UNPACK those to the
//! top level so the node's create-target actually applies them (a node reads
//! top-level `protocol` + `context`, not a nested key). Regression for the gap
//! where the whole key was sent as `{key}` and the worker spawned with defaults.

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
/// `{protocol, context:{cwd, model, session_mode}}`. The create body must lift
/// `protocol` and `context` to the top level while still sending the whole value
/// as the opaque `key`.
#[test]
fn create_body_unpacks_protocol_and_context_to_top_level() {
    let server = MockRuntimeServer::start();
    server.push_create(BufferedResponse::json(200, compact_fixture("target.json")));

    let key = WaslTargetKey(val(
        r#"{ "protocol": "claude",
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
    // The full value is still relayed as the opaque `key`.
    assert_eq!(
        body.get("key").and_then(|k| k.get("protocol")).and_then(|v| v.as_str()),
        Some("claude"),
        "the whole key value is still sent as `key` for reattach/reuse"
    );
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
