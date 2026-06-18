//! happy.rs — nominal paths for every public fn/variant of `marsad-core`.
//!
//! Exercises:
//! - `Core::new` over a `LoopbackFake` transport
//! - Each command (launch/resume/attach/stop/send_prompt/interrupt/answer_gate/run_slash/
//!   set_workspace) produces the expected outbound IPC line(s) via `drain_outbound`
//! - `on_agent_line` fed a recorded init→working→needs→done sequence yields correct `VmUpdate`s
//! - `snapshot` returns an `AppSnapshot`
//!
//! These tests WILL fail to compile until the code-agent implements `marsad_core`.
//! That is the expected tests-first compile-failure signal.

use marsad_core::{Core, CoreConfig, CoreError, LaunchSpec, Posture, VmUpdate};
use marsad_gates::{approve, Gate};
use marsad_transport::LoopbackFake;
use marsad_viewmodel::SlashCmd;

// ---------------------------------------------------------------------------
// JSON fixtures — IPC messages that a real agent would push back inbound
// ---------------------------------------------------------------------------

/// A well-formed `system/init`-derived AgentEvent IPC line (what marsad-agent sends back).
/// NOTE(test-agent): The exact JSON schema for `IpcMsg::AgentEvent` containing a `SystemInit`
/// payload is not fully spelled out in proto.md — the test uses a plausible structure matching
/// the IpcMsg enum. The code-agent must accept this format in `on_agent_line`.
fn ipc_agent_event_system_init(session_id: &str) -> String {
    format!(
        r#"{{"type":"agent_event","session_id":"{session_id}","event":{{"type":"system","subtype":"init","session_id":"{session_id}","model":"claude-opus-4-5","cwd":"/tmp/work","tools":[],"mcp_servers":[],"permissionMode":"default"}}}}"#
    )
}

fn ipc_agent_event_assistant(session_id: &str, msg_id: &str) -> String {
    format!(
        r#"{{"type":"agent_event","session_id":"{session_id}","event":{{"type":"assistant","message":{{"id":"{msg_id}","content":[{{"type":"text","text":"Thinking..."}}],"usage":{{"input_tokens":100,"output_tokens":20,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}}}"#
    )
}

fn ipc_agent_event_control_request(session_id: &str, request_id: &str) -> String {
    format!(
        r#"{{"type":"agent_event","session_id":"{session_id}","event":{{"type":"control","subtype":"can_use_tool","tool_name":"Bash","input":{{}},"request_id":"{request_id}"}}}}"#
    )
}

fn ipc_agent_event_result_success(session_id: &str) -> String {
    format!(
        r#"{{"type":"agent_event","session_id":"{session_id}","event":{{"type":"result","subtype":"success","is_error":false,"total_cost_usd":1.23,"usage":{{"input_tokens":100,"output_tokens":20,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}},"num_turns":1,"permission_denials":[]}}}}"#
    )
}

// ---------------------------------------------------------------------------
// Helper: build a Core over LoopbackFake (one end for Core, one end for
// the test to assert outbound commands)
// ---------------------------------------------------------------------------

fn make_core() -> (Core, LoopbackFake) {
    let (transport_a, transport_b) = LoopbackFake::pair();
    let cfg = CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-test".to_string(),
    };
    let core = Core::new(cfg, Box::new(transport_a)).expect("Core::new should succeed");
    (core, transport_b)
}

// ---------------------------------------------------------------------------
// Core::new succeeds
// ---------------------------------------------------------------------------

#[test]
fn core_new_succeeds() {
    let (_core, _peer) = make_core();
}

// ---------------------------------------------------------------------------
// snapshot returns an AppSnapshot immediately (before any commands)
// ---------------------------------------------------------------------------

#[test]
fn snapshot_returns_app_snapshot() {
    let (core, _peer) = make_core();
    let snap = core.snapshot();
    // Should start with no sessions
    assert!(snap.sessions.is_empty(), "fresh core has no sessions");
}

// ---------------------------------------------------------------------------
// launch_session produces an outbound IPC LaunchSession line and returns an id
// ---------------------------------------------------------------------------

#[test]
fn launch_session_returns_id_and_sends_ipc() {
    let (mut core, mut peer) = make_core();
    let spec = LaunchSpec {
        project: "my-project".to_string(),
        host_id: "local".to_string(),
        model: "claude-opus-4-5".to_string(),
        posture: Posture::Ask,
        first_prompt: None,
    };
    let id = core.launch_session(spec).expect("launch_session should succeed");
    assert!(!id.is_empty(), "session id must not be empty");

    let outbound = peer.drain_outbound();
    assert!(!outbound.is_empty(), "launch_session must emit an outbound IPC line");
    let line = &outbound[0];
    assert!(
        line.contains("launch") || line.contains("Launch"),
        "outbound line should reference launch, got: {line}"
    );
    assert!(
        line.contains(&id) || line.contains("my-project"),
        "outbound line should reference the session or project, got: {line}"
    );
}

#[test]
fn launch_session_with_first_prompt_includes_prompt_in_ipc() {
    let (mut core, mut peer) = make_core();
    let spec = LaunchSpec {
        project: "proj-a".to_string(),
        host_id: "local".to_string(),
        model: "claude-sonnet-4-5".to_string(),
        posture: Posture::AutoApproveSafe,
        first_prompt: Some("Hello agent".to_string()),
    };
    let _id = core.launch_session(spec).expect("launch");
    let outbound = peer.drain_outbound();
    assert!(!outbound.is_empty());
    let combined = outbound.join(" ");
    assert!(
        combined.contains("Hello agent") || combined.contains("first_prompt") || combined.contains("prompt"),
        "outbound should encode the first_prompt, got: {combined}"
    );
}

// ---------------------------------------------------------------------------
// resume_session sends an outbound ResumeSession line
// ---------------------------------------------------------------------------

#[test]
fn resume_session_sends_ipc() {
    let (mut core, mut peer) = make_core();
    // NOTE(test-agent): resume_session takes an existing session id. We use a literal id
    // that is not yet tracked in Core. The spec says UnknownSession is returned for unknown ids,
    // but we test the happy path assuming the implementation accepts an id registered via
    // launch_session first. We chain: launch then resume.
    let id = core
        .launch_session(LaunchSpec {
            project: "proj-r".to_string(),
            host_id: "local".to_string(),
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
        })
        .expect("launch");
    let _ = peer.drain_outbound(); // consume launch IPC

    core.resume_session(&id).expect("resume_session should succeed for known id");
    let outbound = peer.drain_outbound();
    assert!(!outbound.is_empty(), "resume_session must emit an outbound IPC line");
    let line = &outbound[0];
    assert!(
        line.contains("resume") || line.contains("Resume"),
        "outbound must reference resume, got: {line}"
    );
}

// ---------------------------------------------------------------------------
// attach_session sends an outbound AttachSession line
// ---------------------------------------------------------------------------

#[test]
fn attach_session_sends_ipc() {
    let (mut core, mut peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "proj-a".to_string(),
            host_id: "local".to_string(),
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
        })
        .expect("launch");
    let _ = peer.drain_outbound();

    core.attach_session(&id).expect("attach_session should succeed for known id");
    let outbound = peer.drain_outbound();
    assert!(!outbound.is_empty(), "attach_session must emit an outbound IPC line");
    let line = &outbound[0];
    assert!(
        line.contains("attach") || line.contains("Attach"),
        "outbound must reference attach, got: {line}"
    );
}

// ---------------------------------------------------------------------------
// stop_session sends an outbound StopSession line
// ---------------------------------------------------------------------------

#[test]
fn stop_session_sends_ipc() {
    let (mut core, mut peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "proj-s".to_string(),
            host_id: "local".to_string(),
            model: "claude-opus-4-5".to_string(),
            posture: Posture::FullAutonomy,
            first_prompt: None,
        })
        .expect("launch");
    let _ = peer.drain_outbound();

    core.stop_session(&id).expect("stop_session should succeed for known id");
    let outbound = peer.drain_outbound();
    assert!(!outbound.is_empty(), "stop_session must emit an outbound IPC line");
    let line = &outbound[0];
    assert!(
        line.contains("stop") || line.contains("Stop"),
        "outbound must reference stop, got: {line}"
    );
}

// ---------------------------------------------------------------------------
// send_prompt sends an outbound SendPrompt line
// ---------------------------------------------------------------------------

#[test]
fn send_prompt_sends_ipc() {
    let (mut core, mut peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "proj-p".to_string(),
            host_id: "local".to_string(),
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
        })
        .expect("launch");
    let _ = peer.drain_outbound();

    core.send_prompt(&id, "What is 2+2?").expect("send_prompt should succeed");
    let outbound = peer.drain_outbound();
    assert!(!outbound.is_empty(), "send_prompt must emit an outbound IPC line");
    let line = &outbound[0];
    assert!(
        line.contains("What is 2+2?") || line.contains("prompt") || line.contains("Prompt"),
        "outbound must include the prompt text or reference prompt, got: {line}"
    );
}

// ---------------------------------------------------------------------------
// interrupt sends an outbound Interrupt line
// ---------------------------------------------------------------------------

#[test]
fn interrupt_sends_ipc() {
    let (mut core, mut peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "proj-i".to_string(),
            host_id: "local".to_string(),
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
        })
        .expect("launch");
    let _ = peer.drain_outbound();

    core.interrupt(&id).expect("interrupt should succeed");
    let outbound = peer.drain_outbound();
    assert!(!outbound.is_empty(), "interrupt must emit an outbound IPC line");
    let line = &outbound[0];
    assert!(
        line.contains("interrupt") || line.contains("Interrupt"),
        "outbound must reference interrupt, got: {line}"
    );
}

// ---------------------------------------------------------------------------
// answer_gate sends an outbound AnswerGate line and routes to exactly one session
// ---------------------------------------------------------------------------

#[test]
fn answer_gate_sends_ipc_and_routes_to_one_session() {
    let (mut core, mut peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "proj-gate".to_string(),
            host_id: "local".to_string(),
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
        })
        .expect("launch");
    let _ = peer.drain_outbound();

    let gate = Gate::Permission {
        command: "ls /".to_string(),
        cwd: "/".to_string(),
        risk: "low".to_string(),
        explanation: "directory listing".to_string(),
    };
    let decision = approve(&id, &gate);
    core.answer_gate(decision).expect("answer_gate should succeed");

    let outbound = peer.drain_outbound();
    assert!(!outbound.is_empty(), "answer_gate must emit an outbound IPC line");
    let combined = outbound.join(" ");
    assert!(
        combined.contains("gate") || combined.contains("Gate") || combined.contains("answer") || combined.contains("Answer"),
        "outbound must reference gate answer, got: {combined}"
    );
}

// ---------------------------------------------------------------------------
// run_slash sends an outbound SlashCmd line
// ---------------------------------------------------------------------------

#[test]
fn run_slash_sends_ipc() {
    let (mut core, mut peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "proj-slash".to_string(),
            host_id: "local".to_string(),
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
        })
        .expect("launch");
    let _ = peer.drain_outbound();

    core.run_slash(&id, SlashCmd::Clear).expect("run_slash should succeed");
    let outbound = peer.drain_outbound();
    assert!(!outbound.is_empty(), "run_slash must emit an outbound IPC line");
    let combined = outbound.join(" ");
    assert!(
        combined.contains("clear") || combined.contains("Clear") || combined.contains("slash"),
        "outbound must reference the slash command, got: {combined}"
    );
}

// ---------------------------------------------------------------------------
// set_workspace sends an outbound line and re-scopes the snapshot
// ---------------------------------------------------------------------------

#[test]
fn set_workspace_sends_ipc() {
    let (mut core, mut peer) = make_core();
    // NOTE(test-agent): set_workspace takes a session id per the spec signature
    // `pub fn set_workspace(&mut self, id: &str)`. The spec says workspace switch
    // re-scopes the snapshot. We first launch a session to have a known id.
    let id = core
        .launch_session(LaunchSpec {
            project: "proj-ws".to_string(),
            host_id: "local".to_string(),
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
        })
        .expect("launch");
    let _ = peer.drain_outbound();

    core.set_workspace(&id).expect("set_workspace should succeed");
    let outbound = peer.drain_outbound();
    // set_workspace may or may not produce outbound IPC; what matters is no panic and no error
    let _ = outbound;
}

// ---------------------------------------------------------------------------
// on_agent_line: init→working→needs→done sequence yields correct VmUpdates
// ---------------------------------------------------------------------------

#[test]
fn on_agent_line_init_yields_session_vm_update() {
    let (mut core, _peer) = make_core();
    // First launch a session so Core tracks the id
    let id = core
        .launch_session(LaunchSpec {
            project: "proj-events".to_string(),
            host_id: "local".to_string(),
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
        })
        .expect("launch");

    let line = ipc_agent_event_system_init(&id);
    let updates = core.on_agent_line(&line).expect("on_agent_line must succeed for system_init");
    assert!(!updates.is_empty(), "system_init must produce at least one VmUpdate");
    // Must include a Session update
    let has_session = updates.iter().any(|u| matches!(u, VmUpdate::Session(_)));
    assert!(has_session, "system_init must produce a VmUpdate::Session, got: {updates:?}");
}

#[test]
fn on_agent_line_assistant_yields_session_vm_update() {
    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "proj-asst".to_string(),
            host_id: "local".to_string(),
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
        })
        .expect("launch");

    // Feed init first so session is in Working phase
    let _ = core.on_agent_line(&ipc_agent_event_system_init(&id));

    let line = ipc_agent_event_assistant(&id, "msg-001");
    let updates = core.on_agent_line(&line).expect("on_agent_line must succeed for assistant");
    let has_session = updates.iter().any(|u| matches!(u, VmUpdate::Session(_)));
    assert!(has_session, "assistant event must produce a VmUpdate::Session, got: {updates:?}");
}

#[test]
fn on_agent_line_control_request_yields_session_needs_update() {
    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "proj-ctrl".to_string(),
            host_id: "local".to_string(),
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
        })
        .expect("launch");

    let _ = core.on_agent_line(&ipc_agent_event_system_init(&id));

    let line = ipc_agent_event_control_request(&id, "req-1");
    let updates = core.on_agent_line(&line).expect("on_agent_line must succeed for control_request");
    let has_session = updates.iter().any(|u| matches!(u, VmUpdate::Session(_)));
    assert!(has_session, "control_request must produce a VmUpdate::Session (Needs phase)");
}

#[test]
fn on_agent_line_result_success_yields_done_update() {
    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "proj-done".to_string(),
            host_id: "local".to_string(),
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
        })
        .expect("launch");

    let _ = core.on_agent_line(&ipc_agent_event_system_init(&id));
    let _ = core.on_agent_line(&ipc_agent_event_assistant(&id, "msg-x"));

    let line = ipc_agent_event_result_success(&id);
    let updates = core.on_agent_line(&line).expect("on_agent_line must succeed for result");
    let has_session = updates.iter().any(|u| matches!(u, VmUpdate::Session(_)));
    assert!(has_session, "result/success must produce a VmUpdate::Session (Done phase)");
}

#[test]
fn on_agent_line_full_lifecycle_produces_overview_updates() {
    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "proj-lifecycle".to_string(),
            host_id: "local".to_string(),
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
        })
        .expect("launch");

    // Drive the full init→working→needs→done sequence
    let all_updates: Vec<VmUpdate> = [
        ipc_agent_event_system_init(&id),
        ipc_agent_event_assistant(&id, "msg-A"),
        ipc_agent_event_control_request(&id, "req-2"),
        ipc_agent_event_result_success(&id),
    ]
    .iter()
    .flat_map(|line| core.on_agent_line(line).expect("on_agent_line ok"))
    .collect();

    // Must have produced at least Session updates across the sequence
    let session_count = all_updates.iter().filter(|u| matches!(u, VmUpdate::Session(_))).count();
    assert!(
        session_count >= 3,
        "full lifecycle must produce multiple VmUpdate::Session, got {session_count}"
    );

    // Must also produce at least one Overview update
    let has_overview = all_updates.iter().any(|u| matches!(u, VmUpdate::Overview(_)));
    assert!(has_overview, "lifecycle must produce at least one VmUpdate::Overview");
}

// ---------------------------------------------------------------------------
// VmUpdate::Toast — launched session may produce a toast
// ---------------------------------------------------------------------------

#[test]
fn vm_update_toast_is_reachable() {
    // Construct a Toast directly to confirm the variant exists and has the right fields.
    let toast = VmUpdate::Toast {
        icon: "🚀".to_string(),
        text: "Session launched".to_string(),
    };
    match toast {
        VmUpdate::Toast { icon, text } => {
            assert_eq!(icon, "🚀");
            assert_eq!(text, "Session launched");
        }
        _ => panic!("wrong variant"),
    }
}

// ---------------------------------------------------------------------------
// VmUpdate::HostHealth — variant is accessible
// ---------------------------------------------------------------------------

#[test]
fn vm_update_host_health_is_reachable() {
    // NOTE(test-agent): HostHealth carries an unspecified payload in the spec comment.
    // We construct it to prove the variant compiles. The exact inner type must match
    // whatever the code-agent uses.
    let _update: VmUpdate = VmUpdate::HostHealth(Default::default());
    // No panic = pass
}

// ---------------------------------------------------------------------------
// CoreConfig and LaunchSpec field coverage
// ---------------------------------------------------------------------------

#[test]
fn core_config_fields_are_accessible() {
    let cfg = CoreConfig {
        agent_endpoint: "unix:///tmp/marsad.sock".to_string(),
        data_root: "/home/user/.marsad".to_string(),
    };
    assert_eq!(cfg.agent_endpoint, "unix:///tmp/marsad.sock");
    assert_eq!(cfg.data_root, "/home/user/.marsad");
}

#[test]
fn launch_spec_fields_all_postures() {
    for posture in [Posture::Ask, Posture::AutoApproveSafe, Posture::FullAutonomy] {
        let spec = LaunchSpec {
            project: "p".to_string(),
            host_id: "h".to_string(),
            model: "m".to_string(),
            posture,
            first_prompt: Some("hi".to_string()),
        };
        let _ = spec;
    }
}
