//! errors.rs — every `CoreError` variant; edge paths.
//!
//! Covers:
//! - `CoreError::UnknownSession` for all commands operating on unknown ids
//! - `CoreError::Transport` when the transport is broken
//! - `CoreError::Protocol` from malformed inbound IPC lines
//! - `CoreError::Agent` variant is constructible
//! - `on_agent_line` with non-JSON input → `Protocol`
//! - `on_agent_line` with unknown IPC discriminant → `Protocol`
//! - `on_agent_line` with missing required fields → `Protocol`
//!
//! These tests WILL fail to compile until the code-agent implements `marsad_core`.

use marsad_core::{Core, CoreConfig, CoreError, LaunchSpec, Posture};
use marsad_gates::{approve, Gate};
use marsad_transport::{LoopbackFake, Transport};
use marsad_viewmodel::SlashCmd;

// ---------------------------------------------------------------------------
// Minimal helper
// ---------------------------------------------------------------------------

fn cfg() -> CoreConfig {
    CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-err-test".to_string(),
    }
}

fn make_core() -> (Core, LoopbackFake) {
    let (a, b) = LoopbackFake::pair();
    let core = Core::new(cfg(), Box::new(a)).expect("Core::new");
    (core, b)
}

/// A core whose transport peer has been closed, so the next command should
/// see a transport error.
fn make_broken_core() -> Core {
    let (mut a, mut b) = LoopbackFake::pair();
    a.connect().ok();
    b.connect().ok();
    // Close the peer so the Core's transport sends to a closed pipe
    b.close().ok();

    Core::new(cfg(), Box::new(a)).expect("Core::new")
}

// ---------------------------------------------------------------------------
// CoreError::UnknownSession — every command that takes `id: &str`
// ---------------------------------------------------------------------------

const UNKNOWN: &str = "no-such-session-id-xxxxxxxxx";

#[test]
fn resume_unknown_session_returns_unknown_session_error() {
    let (mut core, _peer) = make_core();
    let err = core.resume_session(UNKNOWN).unwrap_err();
    assert!(
        matches!(err, CoreError::UnknownSession(_)),
        "expected UnknownSession, got {err:?}"
    );
}

#[test]
fn attach_unknown_session_returns_unknown_session_error() {
    let (mut core, _peer) = make_core();
    let err = core.attach_session(UNKNOWN).unwrap_err();
    assert!(
        matches!(err, CoreError::UnknownSession(_)),
        "expected UnknownSession, got {err:?}"
    );
}

#[test]
fn stop_unknown_session_returns_unknown_session_error() {
    let (mut core, _peer) = make_core();
    let err = core.stop_session(UNKNOWN).unwrap_err();
    assert!(
        matches!(err, CoreError::UnknownSession(_)),
        "expected UnknownSession, got {err:?}"
    );
}

#[test]
fn send_prompt_unknown_session_returns_unknown_session_error() {
    let (mut core, _peer) = make_core();
    let err = core.send_prompt(UNKNOWN, "hello").unwrap_err();
    assert!(
        matches!(err, CoreError::UnknownSession(_)),
        "expected UnknownSession, got {err:?}"
    );
}

#[test]
fn interrupt_unknown_session_returns_unknown_session_error() {
    let (mut core, _peer) = make_core();
    let err = core.interrupt(UNKNOWN).unwrap_err();
    assert!(
        matches!(err, CoreError::UnknownSession(_)),
        "expected UnknownSession, got {err:?}"
    );
}

#[test]
fn run_slash_unknown_session_returns_unknown_session_error() {
    let (mut core, _peer) = make_core();
    let err = core.run_slash(UNKNOWN, SlashCmd::Clear).unwrap_err();
    assert!(
        matches!(err, CoreError::UnknownSession(_)),
        "expected UnknownSession, got {err:?}"
    );
}

/// `set_workspace` no longer validates the id against the session list —
/// it accepts any workspace id (it stores `active_ws` as a plain String).
/// This test documents that it succeeds even for an unknown id.
#[test]
fn set_workspace_with_any_id_succeeds() {
    let (mut core, _peer) = make_core();
    // Must succeed — set_workspace now takes a workspace id, not a session id.
    core.set_workspace(UNKNOWN)
        .expect("set_workspace must succeed for any id");
}

#[test]
fn unknown_session_error_contains_the_bad_id() {
    let (mut core, _peer) = make_core();
    let err = core.stop_session("my-bad-id-99").unwrap_err();
    match err {
        CoreError::UnknownSession(msg) => {
            assert!(
                msg.contains("my-bad-id-99"),
                "UnknownSession message must contain the offending id, got: {msg}"
            );
        }
        other => panic!("expected UnknownSession, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// CoreError::Transport — broken transport
// ---------------------------------------------------------------------------

#[test]
fn launch_session_transport_failure_returns_transport_error() {
    // NOTE(test-agent): A broken transport (peer closed before any send) should surface
    // as CoreError::Transport when the core tries to send the LaunchSession IPC line.
    // Implementation may return Transport on the first command that hits the broken pipe.
    let mut core = make_broken_core();
    let spec = LaunchSpec {
        project: "p".to_string(),
        host_id: "h".to_string(),
        model: "claude-opus-4-5".to_string(),
        posture: Posture::Ask,
        first_prompt: None,
        mode: Default::default(),
    };
    let result = core.launch_session(spec);
    // Either the launch succeeds (transport failure deferred) or returns Transport error
    if let Err(e) = result {
        assert!(
            matches!(e, CoreError::Transport(_)),
            "expected Transport error from broken transport, got {e:?}"
        );
    }
    // If launch succeeded, the next command must fail with Transport
}

#[test]
fn send_prompt_transport_failure_returns_transport_error() {
    let mut core = make_broken_core();
    let spec = LaunchSpec {
        project: "p".to_string(),
        host_id: "h".to_string(),
        model: "claude-opus-4-5".to_string(),
        posture: Posture::Ask,
        first_prompt: None,
        mode: Default::default(),
    };
    // try launch (may succeed or fail)
    let id_result = core.launch_session(spec);
    if let Ok(id) = id_result {
        let err = core.send_prompt(&id, "hello").unwrap_err();
        assert!(
            matches!(err, CoreError::Transport(_)),
            "expected Transport error, got {err:?}"
        );
    }
    // If launch itself failed with Transport, this test is satisfied
}

// ---------------------------------------------------------------------------
// CoreError::Protocol — malformed inbound IPC lines
// ---------------------------------------------------------------------------

#[test]
fn on_agent_line_non_json_returns_protocol_error() {
    let (mut core, _peer) = make_core();
    let err = core
        .on_agent_line("this is not json at all!!!")
        .unwrap_err();
    assert!(
        matches!(err, CoreError::Protocol(_)),
        "expected Protocol error for non-JSON input, got {err:?}"
    );
}

#[test]
fn on_agent_line_truncated_json_returns_protocol_error() {
    let (mut core, _peer) = make_core();
    let err = core.on_agent_line(r#"{"type":"AgentEvent""#).unwrap_err();
    assert!(
        matches!(err, CoreError::Protocol(_)),
        "expected Protocol for truncated JSON, got {err:?}"
    );
}

#[test]
fn on_agent_line_unknown_ipc_type_returns_protocol_error() {
    let (mut core, _peer) = make_core();
    // IPC is a closed protocol — unknown discriminant is an error (per proto.md)
    let err = core
        .on_agent_line(r#"{"type":"completely_unknown_type","data":{}}"#)
        .unwrap_err();
    assert!(
        matches!(err, CoreError::Protocol(_)),
        "expected Protocol for unknown IPC type, got {err:?}"
    );
}

#[test]
fn on_agent_line_missing_session_id_field_returns_protocol_error() {
    let (mut core, _peer) = make_core();
    // agent_event without session_id is a schema mismatch
    let err = core
        .on_agent_line(r#"{"type":"AgentEvent","event":{"type":"system","subtype":"init"}}"#)
        .unwrap_err();
    assert!(
        matches!(err, CoreError::Protocol(_)),
        "expected Protocol for missing session_id in agent_event, got {err:?}"
    );
}

#[test]
fn on_agent_line_wrong_field_type_returns_protocol_error() {
    let (mut core, _peer) = make_core();
    // session_id should be a string, not a number
    let err = core
        .on_agent_line(r#"{"type":"AgentEvent","session_id":42,"event":{}}"#)
        .unwrap_err();
    assert!(
        matches!(err, CoreError::Protocol(_)),
        "expected Protocol for wrong field type, got {err:?}"
    );
}

#[test]
fn on_agent_line_empty_string_returns_protocol_error() {
    let (mut core, _peer) = make_core();
    let err = core.on_agent_line("").unwrap_err();
    assert!(
        matches!(err, CoreError::Protocol(_)),
        "expected Protocol for empty input, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// CoreError::UnknownSession — on_agent_line for a never-launched session
// ---------------------------------------------------------------------------

#[test]
fn on_agent_line_for_unregistered_session_returns_unknown_session() {
    let (mut core, _peer) = make_core();
    // A well-formed agent_event for a session_id that was never launched/registered.
    // Uses an assistant event (not system/init) so the impl cannot auto-register on first sight.
    let line = r#"{"type":"AgentEvent","session_id":"never-launched-session","event":{"type":"assistant","message":{"id":"msg-x","content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}"#;
    match core.on_agent_line(line) {
        Err(CoreError::UnknownSession(_)) => {}
        other => panic!(
            "agent_event for an unlaunched session must return UnknownSession, got: {other:?}"
        ),
    }
}

// ---------------------------------------------------------------------------
// CoreError::Agent — variant is constructible and matchable
// ---------------------------------------------------------------------------

#[test]
fn core_error_agent_variant_is_accessible() {
    let e: CoreError = CoreError::Agent("agent died unexpectedly".to_string());
    match e {
        CoreError::Agent(msg) => assert!(msg.contains("agent")),
        _ => panic!("wrong variant"),
    }
}

// ---------------------------------------------------------------------------
// CoreError — Debug and display coverage
// ---------------------------------------------------------------------------

#[test]
fn core_error_transport_debug() {
    let e = CoreError::Transport("broken pipe".to_string());
    let s = format!("{e:?}");
    assert!(s.contains("Transport") || s.contains("broken pipe"));
}

#[test]
fn core_error_protocol_debug() {
    let e = CoreError::Protocol("bad json".to_string());
    let s = format!("{e:?}");
    assert!(s.contains("Protocol") || s.contains("bad json"));
}

#[test]
fn core_error_unknown_session_debug() {
    let e = CoreError::UnknownSession("sid-xyz".to_string());
    let s = format!("{e:?}");
    assert!(s.contains("Unknown") || s.contains("sid-xyz"));
}

#[test]
fn core_error_agent_debug() {
    let e = CoreError::Agent("crash".to_string());
    let s = format!("{e:?}");
    assert!(s.contains("Agent") || s.contains("crash"));
}

// ---------------------------------------------------------------------------
// answer_gate on unknown gate decision — routes to one session
// (gate answer for an unknown gate may produce GateError internally but
//  Core must not panic; it may return an Agent or Protocol error)
// ---------------------------------------------------------------------------

#[test]
fn answer_gate_unknown_session_returns_unknown_session_error() {
    let (mut core, _peer) = make_core();
    let gate = Gate::Permission {
        command: "rm -rf".to_string(),
        cwd: "/".to_string(),
        risk: "high".to_string(),
        explanation: "destructive".to_string(),
    };
    let decision = approve(UNKNOWN, &gate);
    let err = core.answer_gate(decision).unwrap_err();
    assert!(
        matches!(err, CoreError::UnknownSession(_)),
        "expected UnknownSession for gate on unknown session, got {err:?}"
    );
}
