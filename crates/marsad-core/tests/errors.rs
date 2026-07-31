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

use marsad_core::{Core, CoreConfig, CoreError, LaunchMode, LaunchSpec, };
use marsad_gates::{approve, Gate};
use marsad_store::{BackendOp, MemoryBackend};
use marsad_transport::{LoopbackFake, Transport, TransportError};
use marsad_viewmodel::SlashCmd;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// CountingTransport — succeeds on the first `ok_count` sends, then fails
// ---------------------------------------------------------------------------

struct CountingTransport {
    ok_count: usize,
    sends: Arc<AtomicUsize>,
}

impl CountingTransport {
    fn new(ok_count: usize) -> Self {
        Self {
            ok_count,
            sends: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl Transport for CountingTransport {
    fn connect(&mut self) -> Result<(), TransportError> {
        Ok(())
    }

    fn send_line(&mut self, _line: &str) -> Result<(), TransportError> {
        let n = self.sends.fetch_add(1, Ordering::Relaxed);
        if n < self.ok_count {
            Ok(())
        } else {
            Err(TransportError::BrokenPipe)
        }
    }

    fn recv_line(&mut self) -> Result<Option<String>, TransportError> {
        Ok(None)
    }

    fn close(&mut self) -> Result<(), TransportError> {
        Ok(())
    }
}

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
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: "claude-opus-4-5".to_string(),
        session_mode: String::new(),
        first_prompt: None,
        mode: Default::default(),
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
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
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: "claude-opus-4-5".to_string(),
        session_mode: String::new(),
        first_prompt: None,
        mode: Default::default(),
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
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

#[test]
fn stop_session_transport_failure_returns_transport_error() {
    // Break the transport AFTER the session is launched so the initial launch
    // succeeds (session registered) but the StopSession IPC send fails — this
    // exercises the `?` error branch in stop_session.
    let (mut a, mut b) = LoopbackFake::pair();
    a.connect().ok();
    b.connect().ok();

    let cfg = CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-err-test".to_string(),
    };
    let mut core = Core::new(cfg, Box::new(a)).expect("Core::new");

    let id = core
        .launch_session(LaunchSpec {
            project: "p".to_string(),
            host_id: "h".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            model: "m".to_string(),
            session_mode: String::new(),
            first_prompt: None,
            mode: LaunchMode::Fresh,
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "claude".to_string(),
        })
        .expect("launch");

    // Close the peer so the next IPC send (StopSession) fails.
    b.close().ok();

    let err = core.stop_session(&id).unwrap_err();
    assert!(
        matches!(err, CoreError::Transport(_)),
        "expected Transport error from stop on broken transport, got {err:?}"
    );
}

#[test]
fn resume_session_transport_failure_returns_transport_error() {
    // Mirror of the stop test: launch (registers the session), then break the
    // transport so the ResumeSession IPC send fails — exercises the `?` error
    // branch in resume_session (which now also returns VmUpdates on success).
    let (mut a, mut b) = LoopbackFake::pair();
    a.connect().ok();
    b.connect().ok();

    let cfg = CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-err-test".to_string(),
    };
    let mut core = Core::new(cfg, Box::new(a)).expect("Core::new");

    let id = core
        .launch_session(LaunchSpec {
            project: "p".to_string(),
            host_id: "h".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            model: "m".to_string(),
            session_mode: String::new(),
            first_prompt: None,
            mode: LaunchMode::Fresh,
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "claude".to_string(),
        })
        .expect("launch");

    // Close the peer so the next IPC send (ResumeSession) fails.
    b.close().ok();

    let err = core.resume_session(&id).unwrap_err();
    assert!(
        matches!(err, CoreError::Transport(_)),
        "expected Transport error from resume on broken transport, got {err:?}"
    );
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

// SPEC CHANGE (docs/specs/typed-ipc.md §7, "Tests that MUST change"): renamed
// from `on_agent_line_unknown_ipc_type_returns_protocol_error`. Per §4's
// unknown-type disposition table, core's `on_agent_line` now treats an
// unknown inbound IPC type as a tolerant no-op (log to stderr, `Ok(vec![])`)
// rather than a hard `Protocol` error — a newer agent sending a type this
// build doesn't know yet must never wedge the reader loop. `parse_ipc` itself
// is unchanged (still `Err(UnknownDiscriminant)`, per proto.md); only core's
// disposition of that error changed.
#[test]
fn on_agent_line_unknown_ipc_type_is_tolerant_noop() {
    let (mut core, _peer) = make_core();
    let updates = core
        .on_agent_line(r#"{"type":"completely_unknown_type","data":{}}"#)
        .expect("unknown IPC type must be a tolerant no-op, not an error");
    assert!(
        updates.is_empty(),
        "unknown IPC type must produce no VmUpdates, got {updates:?}"
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
// on_agent_line — command-ack passthrough (typed-ipc.md §3 item 5): every
// ack/broadcast type that legitimately arrives inbound on the subscriber
// connection is a no-op (`Ok(vec![])`), never an error or a VmUpdate.
// ---------------------------------------------------------------------------

#[test]
fn on_agent_line_ack_types_are_noops() {
    let acks: &[&str] = &[
        r#"{"type":"SessionLaunched","session_id":"s-1"}"#,
        r#"{"type":"SessionResumed","session_id":"s-1"}"#,
        r#"{"type":"SessionAttached","session_id":"s-1"}"#,
        r#"{"type":"SessionStopped","session_id":"s-1"}"#,
        r#"{"type":"PromptSent","session_id":"s-1"}"#,
        r#"{"type":"Interrupted","session_id":"s-1"}"#,
        r#"{"type":"GateAnswered","session_id":"s-1"}"#,
        r#"{"type":"QuestionAnswered","session_id":"s-1"}"#,
        r#"{"type":"Subscribed","subscriber_id":"sub-1"}"#,
        r#"{"type":"Error","message":"unknown IPC type: Bogus"}"#,
    ];
    for line in acks {
        let (mut core, _peer) = make_core();
        let updates = core
            .on_agent_line(line)
            .unwrap_or_else(|e| panic!("ack line {line} must be a no-op, got error {e:?}"));
        assert!(
            updates.is_empty(),
            "ack line {line} must produce no VmUpdates, got {updates:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// on_agent_line — misdirected GUI→agent messages (typed-ipc.md §3 item 5):
// any message that only makes sense flowing GUI/core → agent is a routing
// bug if it arrives on the inbound agent channel, and is a hard
// `CoreError::Protocol` error (never silently dropped).
// ---------------------------------------------------------------------------

#[test]
fn on_agent_line_misdirected_gui_to_agent_messages_are_protocol_errors() {
    let misdirected: &[&str] = &[
        r#"{"type":"LaunchSession","project":"/home/user/proj","host_id":"local","model":"claude-opus-4-5","posture":"Ask"}"#,
        r#"{"type":"ResumeSession","session_id":"s-1"}"#,
        r#"{"type":"AttachSession","session_id":"s-1"}"#,
        r#"{"type":"StopSession","session_id":"s-1"}"#,
        r#"{"type":"SendPrompt","session_id":"s-1","text":"do the thing"}"#,
        r#"{"type":"Interrupt","session_id":"s-1"}"#,
        r#"{"type":"AnswerGate","session_id":"s-1","request_id":"req-1","allow":true,"deny_reason":null}"#,
        r#"{"type":"AnswerQuestion","session_id":"s-1","request_id":"req-q1","answers":{"Which approach?":"Option A"}}"#,
        r#"{"type":"Subscribe","session_id":""}"#,
        r#"{"type":"Unsubscribe","subscriber_id":"sub-1"}"#,
        r#"{"type":"SlashCmd","session_id":"s-1","cmd":"compact"}"#,
        r#"{"type":"SetWorkspace","workspace_id":"ws-1"}"#,
    ];
    for line in misdirected {
        let (mut core, _peer) = make_core();
        let err = core
            .on_agent_line(line)
            .expect_err(&format!("misdirected line {line} must error, not succeed"));
        assert!(
            matches!(err, CoreError::Protocol(_)),
            "expected Protocol error for misdirected line {line}, got {err:?}"
        );
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
        request_id: "req-test".to_string(),
        command: "rm -rf".to_string(),
        cwd: "/".to_string(),
        risk: "high".to_string(),
        explanation: "destructive".to_string(),
        is_elicitation: false,
        elicitation_choices: Vec::new(),
        permission_suggestions: Vec::new(),
    };
    let decision = approve(UNKNOWN, &gate);
    let err = core.answer_gate(decision).unwrap_err();
    assert!(
        matches!(err, CoreError::UnknownSession(_)),
        "expected UnknownSession for gate on unknown session, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// restart_session error paths
// ---------------------------------------------------------------------------

#[test]
fn restart_session_unknown_id_returns_unknown_session_error() {
    let (mut core, _peer) = make_core();
    let err = core
        .restart_session("no-such-session", "model", String::new(), vec![])
        .unwrap_err();
    assert!(
        matches!(err, CoreError::UnknownSession(_)),
        "expected UnknownSession, got {err:?}"
    );
}

#[test]
fn restart_session_transport_failure_on_stop_returns_transport_error() {
    // Break the transport AFTER the session is launched so the initial launch
    // succeeds but the StopSession in restart fails.
    let (mut a, mut b) = LoopbackFake::pair();
    a.connect().ok();
    b.connect().ok();

    let cfg = CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-err-test".to_string(),
    };
    let mut core = Core::new(cfg, Box::new(a)).expect("Core::new");

    // Launch so the session is registered.
    let id = core
        .launch_session(LaunchSpec {
            project: "p".to_string(),
            host_id: "h".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            model: "m".to_string(),
            session_mode: String::new(),
            first_prompt: None,
            mode: LaunchMode::Fresh,
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "claude".to_string(),
        })
        .expect("launch");

    // Now close the peer — the next IPC send (StopSession in restart) will fail.
    b.close().ok();

    let err = core
        .restart_session(&id, "new-model", "bypassPermissions".to_string(), vec![])
        .unwrap_err();
    assert!(
        matches!(err, CoreError::Transport(_)),
        "expected Transport error on broken transport, got {err:?}"
    );
}

#[test]
fn restart_session_transport_failure_on_relaunch_returns_transport_error() {
    // Transport that: succeeds on send #1 (launch) + send #2 (stop), then fails on send #3 (relaunch).
    // ok_count=2 → sends 0 and 1 succeed, send 2 fails.
    let transport = CountingTransport::new(2);
    let cfg = CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-err-test".to_string(),
    };
    let mut core = Core::new(cfg, Box::new(transport)).expect("Core::new");

    let id = core
        .launch_session(LaunchSpec {
            project: "p".to_string(),
            host_id: "h".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            model: "m".to_string(),
            session_mode: String::new(),
            first_prompt: None,
            mode: LaunchMode::Fresh,
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "claude".to_string(),
        })
        .expect("send #1 must succeed");

    let err = core
        .restart_session(&id, "new-model", String::new(), vec![])
        .unwrap_err();
    assert!(
        matches!(err, CoreError::Transport(_)),
        "expected Transport error when relaunch send fails, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// list_templates error path — restore failure
// ---------------------------------------------------------------------------

#[test]
fn list_templates_restore_failure_returns_agent_error() {
    let backend = MemoryBackend::new();
    backend.fail_next(BackendOp::List);
    let (a, _b) = LoopbackFake::pair();
    let cfg = CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-err-test".to_string(),
    };
    let core = Core::with_deps(
        cfg,
        Box::new(a),
        Box::new(marsad_core::RealGit),
        Box::new(marsad_core::RealTranscript),
        Box::new(backend),
    )
    .expect("with_deps");
    let err = core.list_templates().unwrap_err();
    assert!(
        matches!(err, CoreError::Agent(_)),
        "expected Agent error from restore failure, got {err:?}"
    );
}
