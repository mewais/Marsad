//! resume_wire.rs — W2b: SpawnContext.resume wire DTO round-trip tests.
//!
//! Tests:
//! 1. `create_request_body` when context has `resume` → the create body carries
//!    `context.resume` at the top level (and inside `key.context.resume`).
//! 2. `create_request_body` when context has NO `resume` → no `resume` key in
//!    the create body's context (clean omission via `skip_serializing_if`).
//! 3. `create_request_body` when context is entirely absent → no `resume` key.
//! 4. End-to-end via MockRuntimeServer: GET /v1/targets receives `context.resume`
//!    from the create body correctly.

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

// ===========================================================================
// §1 — context.resume is lifted to the top-level create body
// ===========================================================================

#[test]
fn create_body_lifts_context_resume_to_top_level() {
    let server = MockRuntimeServer::start();
    server.push_create(BufferedResponse::json(200, compact_fixture("target.json")));

    let key = WaslTargetKey(val(
        r#"{
            "key": "node-abc",
            "protocol": "claude",
            "context": {
                "cwd": "/work/proj",
                "model": "claude-opus-4-5",
                "resume": "sess-uuid-1234"
            }
        }"#,
    ));
    let p = provider(&server);
    let _ = p.get_or_start(&key).expect("create must succeed on 200");

    let creates = server.requests_to("POST", "/v1/targets");
    assert_eq!(creates.len(), 1, "exactly one POST /v1/targets");
    let body = creates[0].body_json();

    // context lifted to the top level, including resume.
    let ctx = body.get("context").expect("context must be at the top level");
    assert_eq!(
        ctx.get("resume").and_then(|v| v.as_str()),
        Some("sess-uuid-1234"),
        "context.resume must be preserved and lifted to the top-level create body"
    );
    assert_eq!(
        ctx.get("cwd").and_then(|v| v.as_str()),
        Some("/work/proj"),
        "context.cwd must still be present alongside resume"
    );

    // Also present nested in the opaque key.
    let key_ctx = body
        .get("key")
        .and_then(|k| k.get("context"))
        .expect("key.context must be present");
    assert_eq!(
        key_ctx.get("resume").and_then(|v| v.as_str()),
        Some("sess-uuid-1234"),
        "resume must also be accessible inside key.context"
    );
}

// ===========================================================================
// §2 — context WITHOUT resume → no resume key in the create body context
// ===========================================================================

#[test]
fn create_body_context_without_resume_omits_resume_key() {
    let server = MockRuntimeServer::start();
    server.push_create(BufferedResponse::json(200, compact_fixture("target.json")));

    let key = WaslTargetKey(val(
        r#"{
            "key": "node-abc",
            "protocol": "claude",
            "context": {
                "cwd": "/work/proj",
                "model": "claude-opus-4-5"
            }
        }"#,
    ));
    let p = provider(&server);
    let _ = p.get_or_start(&key).expect("create must succeed on 200");

    let creates = server.requests_to("POST", "/v1/targets");
    let body = creates[0].body_json();

    let ctx = body.get("context").expect("context must be at the top level");
    assert!(
        ctx.get("resume").is_none(),
        "when no resume in context, the create body context must NOT contain resume; got: {ctx}"
    );
}

// ===========================================================================
// §3 — no context at all → no context in the create body
// ===========================================================================

#[test]
fn create_body_without_context_has_no_context_key() {
    let server = MockRuntimeServer::start();
    server.push_create(BufferedResponse::json(200, compact_fixture("target.json")));

    let key = WaslTargetKey(val(r#""bare-key""#));
    let p = provider(&server);
    let _ = p.get_or_start(&key).expect("create must succeed on 200");

    let creates = server.requests_to("POST", "/v1/targets");
    let body = creates[0].body_json();

    assert!(
        body.get("context").is_none(),
        "a bare key with no context must not produce a context field in the create body"
    );
    assert!(
        body.get("resume").is_none(),
        "no stray top-level resume when context is absent"
    );
}

// ===========================================================================
// §4 — resume carries through with other context fields (mcp_config injection)
//
// This mirrors what handle_launch_wasl_session does: it injects mcp_config
// into context after the frontend sends resume. The test confirms that
// setting mcp_config alongside resume does not drop resume.
// ===========================================================================

#[test]
fn create_body_context_with_resume_and_mcp_config_preserves_both() {
    let server = MockRuntimeServer::start();
    server.push_create(BufferedResponse::json(200, compact_fixture("target.json")));

    // Simulate the key AFTER handle_launch_wasl_session injects mcp_config.
    let key = WaslTargetKey(val(
        r#"{
            "key": "node-xyz",
            "protocol": "codex",
            "context": {
                "cwd": "/work/proj",
                "model": "codex-r1",
                "resume": "thread-abc-def",
                "mcp_config": "{\"mcpServers\":{\"wasl\":{\"command\":\"wasl-mcpd\"}}}"
            }
        }"#,
    ));
    let p = provider(&server);
    let _ = p.get_or_start(&key).expect("create must succeed on 200");

    let creates = server.requests_to("POST", "/v1/targets");
    let body = creates[0].body_json();

    let ctx = body.get("context").expect("context must be at the top level");
    assert_eq!(
        ctx.get("resume").and_then(|v| v.as_str()),
        Some("thread-abc-def"),
        "resume must be preserved even when mcp_config is also injected"
    );
    assert!(
        ctx.get("mcp_config").is_some(),
        "mcp_config must also be present"
    );
    assert_eq!(
        ctx.get("cwd").and_then(|v| v.as_str()),
        Some("/work/proj"),
        "cwd must still be present"
    );
}
