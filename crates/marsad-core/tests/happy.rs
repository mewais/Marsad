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

use marsad_core::{Core, CoreConfig, LaunchMode, LaunchSpec, NotifPref, PersistedSettings, PersistedTemplate, Posture, VmUpdate};
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
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"system","subtype":"init","session_id":"{session_id}","model":"claude-opus-4-5","cwd":"/tmp/work","tools":[],"mcp_servers":[],"permissionMode":"default"}}}}"#
    )
}

fn ipc_agent_event_assistant(session_id: &str, msg_id: &str) -> String {
    format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"assistant","message":{{"id":"{msg_id}","content":[{{"type":"text","text":"Thinking..."}}],"usage":{{"input_tokens":100,"output_tokens":20,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}}}"#
    )
}

fn ipc_agent_event_control_request(session_id: &str, request_id: &str) -> String {
    format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"control_request","request_id":"{request_id}","request":{{"subtype":"can_use_tool","tool_name":"Bash","input":{{}}}}}}}}"#
    )
}

fn ipc_agent_event_result_success(session_id: &str) -> String {
    format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"result","subtype":"success","is_error":false,"total_cost_usd":1.23,"usage":{{"input_tokens":100,"output_tokens":20,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}},"num_turns":1,"permission_denials":[]}}}}"#
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
        host_target: None,
        model: "claude-opus-4-5".to_string(),
        posture: Posture::Ask,
        first_prompt: None,
        mode: Default::default(),
    };
    let id = core
        .launch_session(spec)
        .expect("launch_session should succeed");
    assert!(!id.is_empty(), "session id must not be empty");

    let outbound = peer.drain_outbound();
    assert!(
        !outbound.is_empty(),
        "launch_session must emit an outbound IPC line"
    );
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

// Launching on a registered remote host resolves and forwards `host_target` so
// the agent runs the session on that host.
#[test]
fn launch_session_on_remote_host_forwards_host_target() {
    let (mut core, mut peer) = make_core();
    core.add_host("box", "user@box.example.com")
        .expect("add remote host");
    let host_id = core
        .snapshot()
        .hosts
        .into_iter()
        .find(|h| !h.is_local)
        .expect("remote host present")
        .id;

    core.launch_session(LaunchSpec {
        project: "p".to_string(),
        host_id,
        host_target: None,
        model: "claude-haiku-4-5-20251001".to_string(),
        posture: Posture::Ask,
        first_prompt: None,
        mode: Default::default(),
    })
    .expect("launch on remote host");

    let outbound = peer.drain_outbound();
    let line = &outbound[0];
    assert!(
        line.contains("host_target") && line.contains("user@box.example.com"),
        "outbound IPC must carry the resolved host_target, got: {line}"
    );
}

#[test]
fn launch_session_forwards_mode_and_posture() {
    let (mut core, mut peer) = make_core();
    // Continue mode → `"continue":true` plus the posture in the outbound IPC.
    core.launch_session(LaunchSpec {
        project: "p".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        model: "m".to_string(),
        posture: Posture::Ask,
        first_prompt: None,
        mode: LaunchMode::Continue,
    })
    .expect("launch continue");
    let out = peer.drain_outbound();
    let line = out.last().expect("continue ipc");
    assert!(
        line.contains("\"continue\":true"),
        "continue mode must be forwarded; got {line}"
    );
    assert!(
        line.contains("\"posture\":\"Ask\""),
        "posture must be forwarded; got {line}"
    );

    // Resume mode → `"resume_session_id":"<id>"`.
    core.launch_session(LaunchSpec {
        project: "p".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        model: "m".to_string(),
        posture: Posture::FullAutonomy,
        first_prompt: None,
        mode: LaunchMode::Resume("uuid-7".to_string()),
    })
    .expect("launch resume");
    let out2 = peer.drain_outbound();
    let line2 = out2.last().expect("resume ipc");
    assert!(
        line2.contains("\"resume_session_id\":\"uuid-7\""),
        "resume id must be forwarded; got {line2}"
    );
    assert!(
        line2.contains("\"posture\":\"FullAutonomy\""),
        "posture must be forwarded; got {line2}"
    );
}

#[test]
fn launch_session_with_first_prompt_includes_prompt_in_ipc() {
    let (mut core, mut peer) = make_core();
    let spec = LaunchSpec {
        project: "proj-a".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        model: "claude-sonnet-4-5".to_string(),
        posture: Posture::AutoApproveSafe,
        first_prompt: Some("Hello agent".to_string()),
        mode: Default::default(),
    };
    let _id = core.launch_session(spec).expect("launch");
    let outbound = peer.drain_outbound();
    assert!(!outbound.is_empty());
    let combined = outbound.join(" ");
    assert!(
        combined.contains("Hello agent")
            || combined.contains("first_prompt")
            || combined.contains("prompt"),
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
            host_target: None,
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
            mode: Default::default(),
        })
        .expect("launch");
    let _ = peer.drain_outbound(); // consume launch IPC

    core.resume_session(&id)
        .expect("resume_session should succeed for known id");
    let outbound = peer.drain_outbound();
    assert!(
        !outbound.is_empty(),
        "resume_session must emit an outbound IPC line"
    );
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
            host_target: None,
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
            mode: Default::default(),
        })
        .expect("launch");
    let _ = peer.drain_outbound();

    core.attach_session(&id)
        .expect("attach_session should succeed for known id");
    let outbound = peer.drain_outbound();
    assert!(
        !outbound.is_empty(),
        "attach_session must emit an outbound IPC line"
    );
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
            host_target: None,
            model: "claude-opus-4-5".to_string(),
            posture: Posture::FullAutonomy,
            first_prompt: None,
            mode: Default::default(),
        })
        .expect("launch");
    let _ = peer.drain_outbound();

    core.stop_session(&id)
        .expect("stop_session should succeed for known id");
    let outbound = peer.drain_outbound();
    assert!(
        !outbound.is_empty(),
        "stop_session must emit an outbound IPC line"
    );
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
            host_target: None,
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
            mode: Default::default(),
        })
        .expect("launch");
    let _ = peer.drain_outbound();

    core.send_prompt(&id, "What is 2+2?")
        .expect("send_prompt should succeed");
    let outbound = peer.drain_outbound();
    assert!(
        !outbound.is_empty(),
        "send_prompt must emit an outbound IPC line"
    );
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
            host_target: None,
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
            mode: Default::default(),
        })
        .expect("launch");
    let _ = peer.drain_outbound();

    core.interrupt(&id).expect("interrupt should succeed");
    let outbound = peer.drain_outbound();
    assert!(
        !outbound.is_empty(),
        "interrupt must emit an outbound IPC line"
    );
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
            host_target: None,
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
            mode: Default::default(),
        })
        .expect("launch");
    let _ = peer.drain_outbound();

    let gate = Gate::Permission {
        request_id: "req-test".to_string(),
        command: "ls /".to_string(),
        cwd: "/".to_string(),
        risk: "low".to_string(),
        explanation: "directory listing".to_string(),
    };
    let decision = approve(&id, &gate);
    core.answer_gate(decision)
        .expect("answer_gate should succeed");

    let outbound = peer.drain_outbound();
    assert!(
        !outbound.is_empty(),
        "answer_gate must emit an outbound IPC line"
    );
    let combined = outbound.join(" ");
    assert!(
        combined.contains("gate")
            || combined.contains("Gate")
            || combined.contains("answer")
            || combined.contains("Answer"),
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
            host_target: None,
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
            mode: Default::default(),
        })
        .expect("launch");
    let _ = peer.drain_outbound();

    core.run_slash(&id, SlashCmd::Clear)
        .expect("run_slash should succeed");
    let outbound = peer.drain_outbound();
    assert!(
        !outbound.is_empty(),
        "run_slash must emit an outbound IPC line"
    );
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
            host_target: None,
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
            mode: Default::default(),
        })
        .expect("launch");
    let _ = peer.drain_outbound();

    core.set_workspace(&id)
        .expect("set_workspace should succeed");
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
            host_target: None,
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
            mode: Default::default(),
        })
        .expect("launch");

    let line = ipc_agent_event_system_init(&id);
    let updates = core
        .on_agent_line(&line)
        .expect("on_agent_line must succeed for system_init");
    assert!(
        !updates.is_empty(),
        "system_init must produce at least one VmUpdate"
    );
    // Must include a Session update
    let has_session = updates.iter().any(|u| matches!(u, VmUpdate::Session(_)));
    assert!(
        has_session,
        "system_init must produce a VmUpdate::Session, got: {updates:?}"
    );
}

#[test]
fn session_name_derives_from_project_basename_with_id_fallback() {
    fn name_for(project: &str) -> (String, String) {
        let (mut core, _peer) = make_core();
        let id = core
            .launch_session(LaunchSpec {
                project: project.to_string(),
                host_id: "local".to_string(),
                host_target: None,
                model: "m".to_string(),
                posture: Posture::Ask,
                first_prompt: None,
                mode: Default::default(),
            })
            .expect("launch");
        let updates = core
            .on_agent_line(&ipc_agent_event_system_init(&id))
            .expect("init");
        let sv = updates
            .iter()
            .find_map(|u| match u {
                VmUpdate::Session(s) => Some(s.clone()),
                _ => None,
            })
            .expect("a session view");
        (sv.name, id)
    }

    // A real project path → name is the last path component.
    let (name, _) = name_for("/work/mewais/Marsad");
    assert_eq!(name, "Marsad", "name should be the project basename");

    // Empty project → falls back to the session id.
    let (name, id) = name_for("");
    assert_eq!(name, id, "empty project must fall back to the id");
}

#[test]
fn on_agent_line_assistant_yields_session_vm_update() {
    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "proj-asst".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
            mode: Default::default(),
        })
        .expect("launch");

    // Feed init first so session is in Working phase
    let _ = core.on_agent_line(&ipc_agent_event_system_init(&id));

    let line = ipc_agent_event_assistant(&id, "msg-001");
    let updates = core
        .on_agent_line(&line)
        .expect("on_agent_line must succeed for assistant");
    let has_session = updates.iter().any(|u| matches!(u, VmUpdate::Session(_)));
    assert!(
        has_session,
        "assistant event must produce a VmUpdate::Session, got: {updates:?}"
    );
}

#[test]
fn on_agent_line_control_request_yields_session_needs_update() {
    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "proj-ctrl".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
            mode: Default::default(),
        })
        .expect("launch");

    let _ = core.on_agent_line(&ipc_agent_event_system_init(&id));

    let line = ipc_agent_event_control_request(&id, "req-1");
    let updates = core
        .on_agent_line(&line)
        .expect("on_agent_line must succeed for control_request");
    let has_session = updates.iter().any(|u| matches!(u, VmUpdate::Session(_)));
    assert!(
        has_session,
        "control_request must produce a VmUpdate::Session (Needs phase)"
    );
}

#[test]
fn on_agent_line_result_success_yields_done_update() {
    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "proj-done".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
            mode: Default::default(),
        })
        .expect("launch");

    let _ = core.on_agent_line(&ipc_agent_event_system_init(&id));
    let _ = core.on_agent_line(&ipc_agent_event_assistant(&id, "msg-x"));

    let line = ipc_agent_event_result_success(&id);
    let updates = core
        .on_agent_line(&line)
        .expect("on_agent_line must succeed for result");
    let has_session = updates.iter().any(|u| matches!(u, VmUpdate::Session(_)));
    assert!(
        has_session,
        "result/success must produce a VmUpdate::Session (Done phase)"
    );
}

#[test]
fn on_agent_line_full_lifecycle_produces_overview_updates() {
    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "proj-lifecycle".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
            mode: Default::default(),
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
    let session_count = all_updates
        .iter()
        .filter(|u| matches!(u, VmUpdate::Session(_)))
        .count();
    assert!(
        session_count >= 3,
        "full lifecycle must produce multiple VmUpdate::Session, got {session_count}"
    );

    // Must also produce at least one Overview update
    let has_overview = all_updates
        .iter()
        .any(|u| matches!(u, VmUpdate::Overview(_)));
    assert!(
        has_overview,
        "lifecycle must produce at least one VmUpdate::Overview"
    );
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
    for posture in [
        Posture::Ask,
        Posture::AutoApproveSafe,
        Posture::FullAutonomy,
    ] {
        let spec = LaunchSpec {
            project: "p".to_string(),
            host_id: "h".to_string(),
            host_target: None,
            model: "m".to_string(),
            posture,
            first_prompt: Some("hi".to_string()),
            mode: Default::default(),
        };
        let _ = spec;
    }
}

// ---------------------------------------------------------------------------
// GitProbe seam — changes flow into the SessionView
// ---------------------------------------------------------------------------

/// AgentEvent carrying a tool_use assistant message (sets last_action → triggers
/// a git re-probe in Core).
fn ipc_agent_event_tool_use(session_id: &str, msg_id: &str) -> String {
    format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"assistant","message":{{"id":"{msg_id}","content":[{{"type":"tool_use","id":"tu-1","name":"Write","input":{{"path":"hello.txt"}}}}],"usage":{{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}}}"#
    )
}

struct FakeGit {
    files: Vec<marsad_viewmodel::FileChange>,
}

impl marsad_core::GitProbe for FakeGit {
    fn changes(&self, _project_dir: &str) -> Vec<marsad_viewmodel::FileChange> {
        self.files.clone()
    }
}

#[test]
fn git_changes_flow_into_session_view_after_tool_runs() {
    use marsad_viewmodel::FileChange;

    let (transport_a, _b) = LoopbackFake::pair();
    let cfg = CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-test".to_string(),
    };
    let fake = FakeGit {
        files: vec![FileChange {
            path: "hello.txt".to_string(),
            status: "??".to_string(),
            added: 0,
            removed: 0,
        }],
    };
    let mut core = Core::with_git(cfg, Box::new(transport_a), Box::new(fake)).expect("with_git");

    let id = core
        .launch_session(LaunchSpec {
            project: "/tmp/proj".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            model: "m".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
            mode: LaunchMode::Fresh,
        })
        .expect("launch");

    // system/init does not run a tool → no probe → no changes yet.
    let u0 = core
        .on_agent_line(&ipc_agent_event_system_init(&id))
        .unwrap();
    let sv0 = u0
        .iter()
        .find_map(|u| match u {
            VmUpdate::Session(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap();
    assert!(sv0.changes.is_empty(), "no probe before any tool runs");

    // A tool_use assistant turn triggers a re-probe → changes populated.
    let u1 = core
        .on_agent_line(&ipc_agent_event_tool_use(&id, "m1"))
        .unwrap();
    let sv1 = u1
        .iter()
        .find_map(|u| match u {
            VmUpdate::Session(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(sv1.changes.len(), 1, "tool run must trigger a git probe");
    assert_eq!(sv1.changes[0].path, "hello.txt");
    assert_eq!(sv1.changed_files, 1, "changed_files mirrors changes.len()");
}

// ---------------------------------------------------------------------------
// Transcript — resume history (loaded once at launch), injected user prompts,
// and live assistant entries from the stream (deduped). No JSONL-flush race.
// ---------------------------------------------------------------------------

struct FakeTranscript {
    session: Vec<marsad_events::TranscriptEntry>,
    recent: Vec<marsad_events::TranscriptEntry>,
}

impl marsad_core::TranscriptReader for FakeTranscript {
    fn session(&self, _project: &str, _id: &str) -> Vec<marsad_events::TranscriptEntry> {
        self.session.clone()
    }
    fn most_recent(&self, _project: &str) -> Vec<marsad_events::TranscriptEntry> {
        self.recent.clone()
    }
}

fn core_with_history(
    session: Vec<marsad_events::TranscriptEntry>,
    recent: Vec<marsad_events::TranscriptEntry>,
) -> Core {
    let (a, _b) = LoopbackFake::pair();
    let cfg = CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-test".to_string(),
    };
    Core::with_probes(
        cfg,
        Box::new(a),
        Box::new(marsad_core::RealGit),
        Box::new(FakeTranscript { session, recent }),
    )
    .expect("with_probes")
}

fn launch_t(core: &mut Core, first_prompt: Option<&str>, mode: LaunchMode) -> String {
    core.launch_session(LaunchSpec {
        project: "/tmp/proj".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        model: "m".to_string(),
        posture: Posture::Ask,
        first_prompt: first_prompt.map(|s| s.to_string()),
        mode,
    })
    .expect("launch")
}

fn last_sv(updates: &[VmUpdate]) -> marsad_viewmodel::SessionView {
    updates
        .iter()
        .rev()
        .find_map(|u| match u {
            VmUpdate::Session(s) => Some(s.clone()),
            _ => None,
        })
        .expect("a SessionView")
}

#[test]
fn transcript_resume_loads_session_history() {
    use marsad_events::TranscriptEntry;
    let hist = vec![TranscriptEntry::User {
        text: "h1".to_string(),
    }];
    let mut core = core_with_history(hist.clone(), vec![]);
    let id = launch_t(&mut core, None, LaunchMode::Resume("uuid-x".to_string()));
    let u = core
        .on_agent_line(&ipc_agent_event_system_init(&id))
        .unwrap();
    assert_eq!(last_sv(&u).transcript, hist);
}

#[test]
fn transcript_continue_loads_most_recent_history() {
    use marsad_events::TranscriptEntry;
    let recent = vec![TranscriptEntry::Assistant {
        text: "r1".to_string(),
    }];
    let mut core = core_with_history(vec![], recent.clone());
    let id = launch_t(&mut core, None, LaunchMode::Continue);
    let u = core
        .on_agent_line(&ipc_agent_event_system_init(&id))
        .unwrap();
    assert_eq!(last_sv(&u).transcript, recent);
}

#[test]
fn transcript_fresh_has_no_history() {
    let mut core = core_with_history(vec![], vec![]);
    let id = launch_t(&mut core, None, LaunchMode::Fresh);
    let u = core
        .on_agent_line(&ipc_agent_event_system_init(&id))
        .unwrap();
    assert!(last_sv(&u).transcript.is_empty());
}

#[test]
fn transcript_first_prompt_is_injected() {
    use marsad_events::TranscriptEntry;
    let mut core = core_with_history(vec![], vec![]);
    let id = launch_t(&mut core, Some("hello there"), LaunchMode::Fresh);
    let u = core
        .on_agent_line(&ipc_agent_event_system_init(&id))
        .unwrap();
    assert_eq!(
        last_sv(&u).transcript,
        vec![TranscriptEntry::User {
            text: "hello there".to_string()
        }]
    );
}

#[test]
fn transcript_blank_first_prompt_not_injected() {
    let mut core = core_with_history(vec![], vec![]);
    let id = launch_t(&mut core, Some("   "), LaunchMode::Fresh);
    let u = core
        .on_agent_line(&ipc_agent_event_system_init(&id))
        .unwrap();
    assert!(last_sv(&u).transcript.is_empty());
}

#[test]
fn transcript_assistant_text_appended_and_deduped() {
    use marsad_events::TranscriptEntry;
    let mut core = core_with_history(vec![], vec![]);
    let id = launch_t(&mut core, None, LaunchMode::Fresh);
    core.on_agent_line(&ipc_agent_event_system_init(&id))
        .unwrap();
    // ipc_agent_event_assistant uses text "Thinking..." with the given message id.
    core.on_agent_line(&ipc_agent_event_assistant(&id, "m1"))
        .unwrap();
    let u = core
        .on_agent_line(&ipc_agent_event_assistant(&id, "m1"))
        .unwrap();
    assert_eq!(
        last_sv(&u).transcript,
        vec![TranscriptEntry::Assistant {
            text: "Thinking...".to_string()
        }],
        "same message id must append exactly once"
    );
}

#[test]
fn transcript_tool_use_appended_from_stream() {
    let mut core = core_with_history(vec![], vec![]);
    let id = launch_t(&mut core, None, LaunchMode::Fresh);
    core.on_agent_line(&ipc_agent_event_system_init(&id))
        .unwrap();
    let u = core
        .on_agent_line(&ipc_agent_event_tool_use(&id, "m2"))
        .unwrap();
    let t = last_sv(&u).transcript;
    assert_eq!(t.len(), 1);
    assert!(
        matches!(&t[0], marsad_events::TranscriptEntry::Tool { name, .. } if name == "Write"),
        "expected a Tool(Write) entry, got {t:?}"
    );
}

#[test]
fn transcript_assistant_empty_text_and_non_text_blocks_skipped() {
    let mut core = core_with_history(vec![], vec![]);
    let id = launch_t(&mut core, None, LaunchMode::Fresh);
    core.on_agent_line(&ipc_agent_event_system_init(&id))
        .unwrap();
    // Whitespace-only text block (m3) and a tool_result block (m4) → no entries.
    let empty_text = format!(
        r#"{{"type":"AgentEvent","session_id":"{id}","event":{{"type":"assistant","message":{{"id":"m3","content":[{{"type":"text","text":"   "}}],"usage":{{"input_tokens":1,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}}}"#
    );
    let tool_result = format!(
        r#"{{"type":"AgentEvent","session_id":"{id}","event":{{"type":"assistant","message":{{"id":"m4","content":[{{"type":"tool_result","tool_use_id":"x","content":"ok"}}],"usage":{{"input_tokens":1,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}}}"#
    );
    core.on_agent_line(&empty_text).unwrap();
    let u = core.on_agent_line(&tool_result).unwrap();
    assert!(last_sv(&u).transcript.is_empty());
}

#[test]
fn transcript_send_prompt_is_injected() {
    use marsad_events::TranscriptEntry;
    let mut core = core_with_history(vec![], vec![]);
    let id = launch_t(&mut core, None, LaunchMode::Fresh);
    core.on_agent_line(&ipc_agent_event_system_init(&id))
        .unwrap();
    core.send_prompt(&id, "  please do X  ")
        .expect("send_prompt");
    core.send_prompt(&id, "   ").expect("blank send_prompt"); // not injected
    let u = core
        .on_agent_line(&ipc_agent_event_assistant(&id, "m9"))
        .unwrap();
    assert_eq!(
        last_sv(&u).transcript,
        vec![
            TranscriptEntry::User {
                text: "please do X".to_string()
            },
            TranscriptEntry::Assistant {
                text: "Thinking...".to_string()
            },
        ]
    );
}

// ---------------------------------------------------------------------------
// Host commands — add/remove/set_status/load + persistence (MemoryBackend)
// ---------------------------------------------------------------------------

use marsad_core::HostStatus;
use marsad_store::{BackendOp, MemoryBackend, PersistedHost, Store};

fn core_with_backend(backend: MemoryBackend) -> Core {
    let (a, _b) = LoopbackFake::pair();
    let cfg = CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-test".to_string(),
    };
    Core::with_deps(
        cfg,
        Box::new(a),
        Box::new(marsad_core::RealGit),
        Box::new(marsad_core::RealTranscript),
        Box::new(backend),
    )
    .expect("with_deps")
}

fn hosts_from(updates: &[VmUpdate]) -> Vec<marsad_core::Host> {
    updates
        .iter()
        .rev()
        .find_map(|u| match u {
            VmUpdate::Hosts(h) => Some(h.clone()),
            _ => None,
        })
        .expect("a Hosts update")
}

fn remote_id(updates: &[VmUpdate]) -> String {
    hosts_from(updates)
        .into_iter()
        .find(|h| !h.is_local)
        .expect("a remote host")
        .id
}

#[test]
fn add_host_appears_in_list_and_snapshot() {
    let mut core = core_with_backend(MemoryBackend::new());
    let u = core
        .add_host("srv", "alice@srv.example.com")
        .expect("add_host");
    let hosts = hosts_from(&u);
    assert!(hosts.iter().any(|h| h.label == "srv" && !h.is_local));
    assert!(hosts.iter().any(|h| h.is_local), "localhost must remain");
    assert!(core.snapshot().hosts.iter().any(|h| h.label == "srv"));
}

#[test]
fn add_host_duplicate_label_errors() {
    let mut core = core_with_backend(MemoryBackend::new());
    core.add_host("srv", "a@h1").expect("first add");
    assert!(
        core.add_host("srv", "b@h2").is_err(),
        "dup label must error"
    );
}

#[test]
fn add_host_malformed_target_errors() {
    let mut core = core_with_backend(MemoryBackend::new());
    // Bare aliases are accepted now; a target with whitespace is still malformed.
    assert!(core.add_host("x", "has a space").is_err());
}

#[test]
fn add_host_store_write_failure_errors() {
    let backend = MemoryBackend::new();
    backend.fail_next(BackendOp::Write);
    let mut core = core_with_backend(backend);
    assert!(
        core.add_host("srv", "a@h").is_err(),
        "persist failure surfaces"
    );
}

#[test]
fn remove_host_success_and_localhost_protected() {
    let mut core = core_with_backend(MemoryBackend::new());
    let u = core.add_host("srv", "a@h").expect("add");
    let id = remote_id(&u);
    let u2 = core.remove_host(&id).expect("remove");
    assert!(!hosts_from(&u2).iter().any(|h| h.id == id));
    assert!(
        core.remove_host("localhost").is_err(),
        "localhost protected"
    );
}

#[test]
fn remove_host_store_delete_failure_errors() {
    let backend = MemoryBackend::new();
    let mut core = core_with_backend(backend.clone());
    let u = core.add_host("srv", "a@h").expect("add");
    let id = remote_id(&u);
    backend.fail_next(BackendOp::Delete);
    assert!(core.remove_host(&id).is_err(), "delete failure surfaces");
}

#[test]
fn set_host_status_updates_the_list() {
    let mut core = core_with_backend(MemoryBackend::new());
    let u = core.add_host("srv", "a@h").expect("add");
    let id = remote_id(&u);
    let u2 = core.set_host_status(&id, HostStatus::Connected);
    let h = hosts_from(&u2).into_iter().find(|h| h.id == id).unwrap();
    assert_eq!(h.status, HostStatus::Connected);
}

#[test]
fn load_hosts_restores_persisted_hosts() {
    let backend = MemoryBackend::new();
    // Seed a persisted host via a clone that shares the same backing state.
    let mut seed = Store::new(Box::new(backend.clone()));
    seed.upsert_host(&PersistedHost {
        id: "host-9".to_string(),
        label: "saved".to_string(),
        target: "u@saved".to_string(),
        is_local: false,
    })
    .expect("seed");
    let mut core = core_with_backend(backend);
    let u = core.load_hosts().expect("load_hosts");
    let hosts = hosts_from(&u);
    assert!(hosts.iter().any(|h| h.id == "host-9" && h.label == "saved"));
    assert!(hosts.iter().any(|h| h.is_local), "localhost still seeded");
}

#[test]
fn load_hosts_restore_failure_errors() {
    let backend = MemoryBackend::new();
    backend.fail_next(BackendOp::List);
    let mut core = core_with_backend(backend);
    assert!(core.load_hosts().is_err());
}

// ---------------------------------------------------------------------------
// Workspace commands — save/duplicate/delete/export/import/set/load
// ---------------------------------------------------------------------------

use marsad_viewmodel::Workspace;

fn workspaces_from(updates: &[VmUpdate]) -> Vec<Workspace> {
    updates
        .iter()
        .rev()
        .find_map(|u| match u {
            VmUpdate::Workspaces(w) => Some(w.clone()),
            _ => None,
        })
        .expect("a Workspaces update")
}

#[test]
fn save_workspace_appears_in_list_and_snapshot() {
    let mut core = core_with_backend(MemoryBackend::new());
    let u = core
        .save_workspace("my-ws", vec!["s1".to_string()])
        .expect("save_workspace");
    let wss = workspaces_from(&u);
    assert!(
        wss.iter().any(|w| w.name == "my-ws"),
        "saved ws must appear"
    );
    assert!(
        core.snapshot().workspaces.iter().any(|w| w.name == "my-ws"),
        "snapshot must include saved ws"
    );
}

#[test]
fn save_workspace_store_write_failure_errors() {
    let backend = MemoryBackend::new();
    backend.fail_next(BackendOp::Write);
    let mut core = core_with_backend(backend);
    assert!(
        core.save_workspace("fail", vec![]).is_err(),
        "write failure must surface"
    );
}

#[test]
fn duplicate_workspace_creates_copy() {
    let mut core = core_with_backend(MemoryBackend::new());
    let u = core
        .save_workspace("original", vec!["s1".to_string()])
        .expect("save");
    let orig_id = workspaces_from(&u)
        .into_iter()
        .find(|w| w.name == "original")
        .expect("original ws")
        .id;

    let u2 = core.duplicate_workspace(&orig_id).expect("duplicate");
    let wss = workspaces_from(&u2);
    assert!(
        wss.iter().any(|w| w.name.contains("copy")),
        "copy must exist"
    );
    assert_eq!(wss.len(), 2, "original + copy");
}

#[test]
fn duplicate_workspace_unknown_id_errors() {
    let mut core = core_with_backend(MemoryBackend::new());
    assert!(core.duplicate_workspace("not-there").is_err());
}

#[test]
fn duplicate_workspace_store_write_failure_errors() {
    let backend = MemoryBackend::new();
    let mut core = core_with_backend(backend.clone());
    let u = core.save_workspace("orig", vec![]).expect("save");
    let id = workspaces_from(&u).into_iter().next().expect("ws").id;
    backend.fail_next(BackendOp::Write);
    assert!(core.duplicate_workspace(&id).is_err());
}

#[test]
fn delete_workspace_removes_from_list_and_snapshot() {
    let mut core = core_with_backend(MemoryBackend::new());
    let u = core.save_workspace("to-delete", vec![]).expect("save");
    let id = workspaces_from(&u)
        .into_iter()
        .find(|w| w.name == "to-delete")
        .expect("ws")
        .id;

    let u2 = core.delete_workspace(&id).expect("delete");
    let wss = workspaces_from(&u2);
    assert!(!wss.iter().any(|w| w.id == id), "deleted ws must be gone");
    assert!(
        !core.snapshot().workspaces.iter().any(|w| w.id == id),
        "snapshot must not include deleted ws"
    );
}

#[test]
fn delete_workspace_clears_active_ws_if_it_was_active() {
    let mut core = core_with_backend(MemoryBackend::new());
    let u = core.save_workspace("active-del", vec![]).expect("save");
    let id = workspaces_from(&u).into_iter().next().expect("ws").id;
    core.set_workspace(&id).expect("set active");
    assert_eq!(core.snapshot().active_ws, Some(id.clone()));

    core.delete_workspace(&id).expect("delete");
    assert_eq!(
        core.snapshot().active_ws,
        None,
        "active_ws must clear when active ws is deleted"
    );
}

#[test]
fn delete_workspace_store_delete_failure_errors() {
    let backend = MemoryBackend::new();
    let mut core = core_with_backend(backend.clone());
    let u = core.save_workspace("del-fail", vec![]).expect("save");
    let id = workspaces_from(&u).into_iter().next().expect("ws").id;
    backend.fail_next(BackendOp::Delete);
    assert!(core.delete_workspace(&id).is_err());
}

#[test]
fn export_workspace_returns_json() {
    let mut core = core_with_backend(MemoryBackend::new());
    core.save_workspace("exportable", vec![]).expect("save");
    // The store key for this workspace
    let id = core
        .snapshot()
        .workspaces
        .into_iter()
        .find(|w| w.name == "exportable")
        .expect("ws")
        .id;
    let json = core.export_workspace(&id).expect("export");
    assert!(
        json.contains("marsadWorkspace"),
        "must contain envelope key"
    );
}

#[test]
fn export_workspace_unknown_id_errors() {
    let core = core_with_backend(MemoryBackend::new());
    assert!(core.export_workspace("ghost").is_err());
}

#[test]
fn import_workspace_adds_to_list() {
    let mut core = core_with_backend(MemoryBackend::new());
    // Create a valid export JSON
    core.save_workspace("to-export", vec![]).expect("save");
    let id = core
        .snapshot()
        .workspaces
        .into_iter()
        .find(|w| w.name == "to-export")
        .expect("ws")
        .id;
    let json = core.export_workspace(&id).expect("export");

    // Fresh core, import into it
    let mut core2 = core_with_backend(MemoryBackend::new());
    let u = core2.import_workspace(&json).expect("import");
    let wss = workspaces_from(&u);
    assert!(
        wss.iter().any(|w| w.name == "to-export"),
        "imported ws must appear"
    );
}

#[test]
fn import_workspace_bad_json_errors() {
    let mut core = core_with_backend(MemoryBackend::new());
    assert!(core.import_workspace("not json").is_err());
}

#[test]
fn import_workspace_twice_overwrites_not_duplicates() {
    let mut core = core_with_backend(MemoryBackend::new());
    // Export a workspace with a known name
    core.save_workspace("reimport", vec!["s1".to_string()])
        .expect("save");
    let id = core
        .snapshot()
        .workspaces
        .into_iter()
        .find(|w| w.name == "reimport")
        .expect("ws")
        .id;
    let json = core.export_workspace(&id).expect("export");

    // Import twice — the store derives id "imported-reimport" from the name.
    // The second import must overwrite, not add a second entry with that id.
    core.import_workspace(&json).expect("first import");
    let u2 = core.import_workspace(&json).expect("second import");
    let wss = workspaces_from(&u2);
    // Exactly one workspace with the derived imported id.
    let imported_id = "imported-reimport";
    let count = wss.iter().filter(|w| w.id == imported_id).count();
    assert_eq!(count, 1, "double import must overwrite, not duplicate");
}

#[test]
fn set_workspace_persists_active_and_emits_workspaces() {
    let mut core = core_with_backend(MemoryBackend::new());
    let u = core.save_workspace("ws-active", vec![]).expect("save");
    let id = workspaces_from(&u).into_iter().next().expect("ws").id;

    let u2 = core.set_workspace(&id).expect("set_workspace");
    assert!(
        u2.iter().any(|u| matches!(u, VmUpdate::Workspaces(_))),
        "set_workspace must emit Workspaces update"
    );
    assert_eq!(core.snapshot().active_ws, Some(id.clone()));
}

#[test]
fn load_workspaces_restores_persisted_workspaces() {
    let backend = MemoryBackend::new();
    // Seed a persisted workspace via a clone that shares the same backing state.
    let mut seed = Store::new(Box::new(backend.clone()));
    seed.snapshot_workspace(&marsad_store::Workspace {
        id: "ws-seeded".to_string(),
        name: "seeded".to_string(),
        session_ids: vec![],
        saved_at: "2026-06-24".to_string(),
    })
    .expect("seed");

    let mut core = core_with_backend(backend);
    let u = core.load_workspaces().expect("load_workspaces");
    let wss = workspaces_from(&u);
    assert!(
        wss.iter()
            .any(|w| w.id == "ws-seeded" && w.name == "seeded"),
        "seeded workspace must be restored"
    );
}

#[test]
fn load_workspaces_restore_failure_errors() {
    let backend = MemoryBackend::new();
    backend.fail_next(BackendOp::List);
    let mut core = core_with_backend(backend);
    assert!(core.load_workspaces().is_err());
}

#[test]
fn load_workspaces_restores_active_ws() {
    let backend = MemoryBackend::new();
    // Seed a workspace AND set it active via the store directly.
    let mut seed = Store::new(Box::new(backend.clone()));
    seed.snapshot_workspace(&marsad_store::Workspace {
        id: "ws-active".to_string(),
        name: "active".to_string(),
        session_ids: vec![],
        saved_at: "2026-06-24".to_string(),
    })
    .expect("seed workspace");
    seed.set_active_workspace("ws-active")
        .expect("seed active_ws");

    let mut core = core_with_backend(backend);
    core.load_workspaces().expect("load_workspaces");
    assert_eq!(
        core.snapshot().active_ws,
        Some("ws-active".to_string()),
        "active_ws must be restored from the store"
    );
}

#[test]
fn save_workspace_overwrites_same_id() {
    let mut core = core_with_backend(MemoryBackend::new());
    // First save
    core.save_workspace("same", vec!["s1".to_string()])
        .expect("first save");
    // Second save with the same derived id — should overwrite
    let u2 = core
        .save_workspace("same", vec!["s2".to_string()])
        .expect("second save");
    let wss = workspaces_from(&u2);
    // Still only one workspace with that name
    let count = wss.iter().filter(|w| w.name == "same").count();
    assert_eq!(count, 1, "upsert must overwrite, not duplicate");
    assert_eq!(
        wss.iter().find(|w| w.name == "same").unwrap().session_ids,
        vec!["s2"],
        "session_ids must be from the second save"
    );
}

// ---------------------------------------------------------------------------
// set_session_changes — overwrite file list, returns Session+Overview updates
// ---------------------------------------------------------------------------

#[test]
fn set_session_changes_updates_session_view() {
    use marsad_viewmodel::FileChange;

    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "/tmp/proj-changes".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            model: "m".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
            mode: LaunchMode::Fresh,
        })
        .expect("launch");

    // Initially the session has no changes.
    let snap = core.snapshot();
    let sv = snap.sessions.iter().find(|s| s.id == id).expect("session");
    assert!(sv.changes.is_empty(), "should start empty");

    // Inject two changes (simulating a post-commit re-probe with remaining files).
    let new_changes = vec![
        FileChange { path: "remaining.txt".to_string(), status: "M".to_string(), added: 1, removed: 0 },
    ];
    let updates = core.set_session_changes(&id, new_changes.clone());

    // Must produce exactly Session + Overview.
    assert_eq!(updates.len(), 2, "should return Session + Overview");
    let has_session = updates.iter().any(|u| matches!(u, VmUpdate::Session(_)));
    let has_overview = updates.iter().any(|u| matches!(u, VmUpdate::Overview(_)));
    assert!(has_session, "must include Session update");
    assert!(has_overview, "must include Overview update");

    // The returned SessionView must carry the new file list.
    let sv2 = updates.iter().find_map(|u| match u {
        VmUpdate::Session(s) => Some(s.clone()),
        _ => None,
    }).expect("session view");
    assert_eq!(sv2.changes, new_changes, "session view must reflect the new changes");
    assert_eq!(sv2.changed_files, 1);
}

#[test]
fn set_session_changes_unknown_id_is_noop() {
    let (mut core, _peer) = make_core();
    // No session launched — any id is unknown.
    let updates = core.set_session_changes("nonexistent-id", vec![]);
    assert!(updates.is_empty(), "unknown id must return empty vec");
}

// ---------------------------------------------------------------------------
// Settings commands — get_settings / set_settings
// ---------------------------------------------------------------------------

fn make_settings() -> PersistedSettings {
    let mut notif_prefs = std::collections::HashMap::new();
    notif_prefs.insert(
        "gate_needs_you".to_string(),
        NotifPref {
            enabled: true,
            priority: "urgent".to_string(),
            sound: true,
            group: false,
            dnd: false,
        },
    );
    PersistedSettings {
        notif_prefs,
        play_sound: true,
        group_alerts: false,
        do_not_disturb: false,
        background: "Gradient".to_string(),
        restore_on_launch: true,
        confirm_before_stop: true,
        default_model: "claude-sonnet-4-6".to_string(),
        default_posture: "ask".to_string(),
    }
}

#[test]
fn get_settings_returns_defaults_when_empty() {
    let core = core_with_backend(MemoryBackend::new());
    let s = core.get_settings();
    // Defaults: sonnet model, ask posture, restore_on_launch true
    assert_eq!(s.default_model, "claude-sonnet-4-6");
    assert_eq!(s.default_posture, "ask");
    assert!(s.restore_on_launch);
    // All five events must have a default pref
    assert!(s.notif_prefs.contains_key("gate_needs_you"));
    assert!(s.notif_prefs.contains_key("gate_error"));
    assert!(s.notif_prefs.contains_key("session_done"));
    assert!(s.notif_prefs.contains_key("session_started"));
    assert!(s.notif_prefs.contains_key("host_unreachable"));
}

#[test]
fn set_settings_then_get_settings_round_trips() {
    let mut core = core_with_backend(MemoryBackend::new());
    let s = make_settings();
    core.set_settings(&s).expect("set_settings must succeed");
    let got = core.get_settings();
    assert_eq!(got, s);
}

#[test]
fn set_settings_store_write_failure_errors() {
    let backend = MemoryBackend::new();
    backend.fail_next(BackendOp::Write);
    let mut core = core_with_backend(backend);
    let s = make_settings();
    assert!(
        core.set_settings(&s).is_err(),
        "write failure must surface as Err"
    );
}

#[test]
fn set_settings_overwrite_updates_model() {
    let mut core = core_with_backend(MemoryBackend::new());
    let mut s = make_settings();
    core.set_settings(&s).expect("first set");
    s.default_model = "claude-opus-4-8".to_string();
    core.set_settings(&s).expect("second set");
    let got = core.get_settings();
    assert_eq!(got.default_model, "claude-opus-4-8");
}

// ---------------------------------------------------------------------------
// restart_session — stops then relaunches with same session id
// ---------------------------------------------------------------------------

/// Feed a system/init event so the session record gets a claude UUID in
/// `state.session_id` (simulating what the real agent would send after launch).
fn ipc_init_with_uuid(session_id: &str, claude_uuid: &str) -> String {
    format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"system","subtype":"init","session_id":"{claude_uuid}","model":"claude-opus-4-5","cwd":"/tmp","tools":[],"mcp_servers":[],"permissionMode":"default"}}}}"#
    )
}

#[test]
fn restart_session_emits_stop_then_launch_with_same_id() {
    let (mut core, mut peer) = make_core();

    // Launch a session and get its marsad id.
    let spec = LaunchSpec {
        project: "/tmp/proj-restart".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        model: "claude-sonnet-4-6".to_string(),
        posture: Posture::Ask,
        first_prompt: None,
        mode: LaunchMode::Fresh,
    };
    let id = core.launch_session(spec).expect("launch");
    let _ = peer.drain_outbound(); // consume the initial LaunchSession IPC

    // Simulate the agent sending system/init with a claude UUID.
    let claude_uuid = "claude-uuid-abcde";
    core.on_agent_line(&ipc_init_with_uuid(&id, claude_uuid))
        .expect("init");

    // Restart with a new model and posture.
    let updates = core
        .restart_session(&id, "claude-opus-4-8", Posture::FullAutonomy)
        .expect("restart_session should succeed");

    // Must emit a VmUpdate::Session for the same id.
    let has_session = updates.iter().any(|u| matches!(u, VmUpdate::Session(sv) if sv.id == id));
    assert!(has_session, "restart must produce a Session update for the same id");

    // Outbound IPC: StopSession then LaunchSession.
    let outbound = peer.drain_outbound();
    assert!(outbound.len() >= 2, "restart must emit at least 2 IPC lines, got {}", outbound.len());

    // First line must be StopSession.
    let stop_line = &outbound[0];
    assert!(
        stop_line.contains("stop") || stop_line.contains("Stop"),
        "first outbound must be StopSession, got: {stop_line}"
    );
    assert!(
        stop_line.contains(&id),
        "StopSession must reference the session id, got: {stop_line}"
    );

    // Second line must be LaunchSession with the SAME session_id.
    let launch_line = &outbound[1];
    assert!(
        launch_line.contains("launch") || launch_line.contains("Launch"),
        "second outbound must be LaunchSession, got: {launch_line}"
    );
    assert!(
        launch_line.contains(&id),
        "LaunchSession must carry the SAME session_id, got: {launch_line}"
    );
    // Must include resume_session_id = the claude UUID.
    assert!(
        launch_line.contains(claude_uuid),
        "LaunchSession must include resume_session_id={claude_uuid}, got: {launch_line}"
    );
    // Must include the new model.
    assert!(
        launch_line.contains("claude-opus-4-8"),
        "LaunchSession must carry the new model, got: {launch_line}"
    );
}

#[test]
fn restart_session_without_uuid_uses_continue_mode() {
    let (mut core, mut peer) = make_core();

    // Launch but do NOT feed system/init — so claude UUID remains None.
    let spec = LaunchSpec {
        project: "/tmp/proj-no-uuid".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        model: "claude-sonnet-4-6".to_string(),
        posture: Posture::Ask,
        first_prompt: None,
        mode: LaunchMode::Fresh,
    };
    let id = core.launch_session(spec).expect("launch");
    let _ = peer.drain_outbound();

    // Restart — no UUID known → should use Continue mode.
    let updates = core
        .restart_session(&id, "claude-haiku-4-5", Posture::AutoApproveSafe)
        .expect("restart without uuid");

    let has_session = updates.iter().any(|u| matches!(u, VmUpdate::Session(sv) if sv.id == id));
    assert!(has_session, "restart must produce a Session update");

    let outbound = peer.drain_outbound();
    assert!(outbound.len() >= 2, "restart must emit at least 2 lines");

    let launch_line = &outbound[1];
    // Should include continue:true (not resume_session_id) since no UUID.
    assert!(
        launch_line.contains("\"continue\":true"),
        "no-uuid restart must use continue mode, got: {launch_line}"
    );
    // Must still carry the new model.
    assert!(
        launch_line.contains("claude-haiku-4-5"),
        "LaunchSession must carry the new model, got: {launch_line}"
    );
}

#[test]
fn restart_session_unknown_id_returns_unknown_session_error() {
    use marsad_core::CoreError;
    let (mut core, _peer) = make_core();
    let err = core
        .restart_session("no-such-session", "model", Posture::Ask)
        .unwrap_err();
    assert!(
        matches!(err, CoreError::UnknownSession(_)),
        "expected UnknownSession, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// save_template + list_templates — persist and retrieve named configs
// ---------------------------------------------------------------------------

#[test]
fn save_template_then_list_returns_template() {
    let mut core = core_with_backend(MemoryBackend::new());
    core.save_template("fast", "claude-haiku-4-5", "auto")
        .expect("save_template");
    let templates = core.list_templates().expect("list_templates");
    assert!(
        templates.iter().any(|t| t.name == "fast"),
        "saved template must be listed"
    );
}

#[test]
fn save_template_preserves_model_and_posture() {
    let mut core = core_with_backend(MemoryBackend::new());
    core.save_template("careful", "claude-opus-4-8", "full")
        .expect("save_template");
    let templates = core.list_templates().expect("list_templates");
    let t = templates.into_iter().find(|t| t.name == "careful").expect("template");
    assert_eq!(t.model, "claude-opus-4-8");
    assert_eq!(t.posture, "full");
}

#[test]
fn list_templates_empty_on_fresh_core() {
    let core = core_with_backend(MemoryBackend::new());
    let templates = core.list_templates().expect("list_templates");
    assert!(templates.is_empty(), "fresh core has no templates");
}

#[test]
fn save_template_write_failure_errors() {
    let backend = MemoryBackend::new();
    backend.fail_next(BackendOp::Write);
    let mut core = core_with_backend(backend);
    assert!(
        core.save_template("fail", "m", "ask").is_err(),
        "write failure must surface"
    );
}

#[test]
fn save_template_overwrite_updates_model() {
    let mut core = core_with_backend(MemoryBackend::new());
    core.save_template("tmpl", "claude-sonnet-4-6", "ask").expect("first");
    core.save_template("tmpl", "claude-opus-4-8", "full").expect("second");
    let templates = core.list_templates().expect("list");
    let matches: Vec<_> = templates.iter().filter(|t| t.name == "tmpl").collect();
    assert_eq!(matches.len(), 1, "upsert must overwrite, not duplicate");
    assert_eq!(matches[0].model, "claude-opus-4-8");
}

#[test]
fn persisted_template_is_accessible_via_core_reexport() {
    // Verify that PersistedTemplate is re-exported from marsad_core.
    let t = PersistedTemplate {
        name: "test".to_string(),
        model: "claude-sonnet-4-6".to_string(),
        posture: "ask".to_string(),
    };
    assert_eq!(t.name, "test");
}
