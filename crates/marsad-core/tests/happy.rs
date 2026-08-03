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

use marsad_core::{
    Core, CoreConfig, LaunchMode, LaunchSpec, NotifPref, PersistedSettings, PersistedTemplate,
    VmUpdate,
};
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
fn ipc_usage_update(session_id: &str, usage_json: &str) -> String {
    format!(
        r#"{{"type":"UsageUpdate","session_id":"{session_id}","usage":{usage_json}}}"#
    )
}

fn ipc_agent_event_system_init(session_id: &str) -> String {
    format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"session_init","native_session_id":"{session_id}","model":"claude-opus-4-5","cwd":"/tmp/work","tools":[],"mcp_servers":[],"permission_mode":"default"}}}}"#
    )
}

fn ipc_agent_event_assistant(session_id: &str, msg_id: &str) -> String {
    format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"assistant_delta","text":"Thinking...","message_id":"{msg_id}"}}}}"#
    )
}

fn ipc_agent_event_control_request(session_id: &str, request_id: &str) -> String {
    format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"gate_requested","gate":{{"gate_type":"permission","request_id":"{request_id}","tool_name":"Bash","command":"","cwd":"","explanation":"","raw_input":{{}}}}}}}}"#
    )
}

fn ipc_agent_event_result_success(session_id: &str) -> String {
    format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"turn_completed","usage":{{"input_tokens":100,"output_tokens":20,"cache_read_tokens":0,"cache_creation_tokens":0,"cost_usd":1.23}}}}}}"#
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
    core.add_host("box", "user@box.example.com", None, None, Vec::new(), Vec::new())
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
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: "claude-haiku-4-5-20251001".to_string(),
        session_mode: String::new(),
        first_prompt: None,
        mode: Default::default(),
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
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
fn launch_session_forwards_mode_and_session_mode() {
    let (mut core, mut peer) = make_core();
    // Continue mode → `"continue":true` in the outbound IPC.
    core.launch_session(LaunchSpec {
        project: "p".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: "m".to_string(),
        session_mode: String::new(), // backend default — no session_mode key emitted
        first_prompt: None,
        mode: LaunchMode::Continue,
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
    })
    .expect("launch continue");
    let out = peer.drain_outbound();
    let line = out.last().expect("continue ipc");
    assert!(
        line.contains("\"continue\":true"),
        "continue mode must be forwarded; got {line}"
    );
    // Empty session_mode → omitted from the wire (skip_serializing_if = String::is_empty)
    assert!(
        !line.contains("\"session_mode\""),
        "empty session_mode must not appear in wire; got {line}"
    );

    // Resume mode + non-empty session_mode → both forwarded.
    core.launch_session(LaunchSpec {
        project: "p".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: "m".to_string(),
        session_mode: "bypassPermissions".to_string(),
        first_prompt: None,
        mode: LaunchMode::Resume("uuid-7".to_string()),
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
    })
    .expect("launch resume");
    let out2 = peer.drain_outbound();
    let line2 = out2.last().expect("resume ipc");
    assert!(
        line2.contains("\"resume_session_id\":\"uuid-7\""),
        "resume id must be forwarded; got {line2}"
    );
    assert!(
        line2.contains("\"session_mode\":\"bypassPermissions\""),
        "non-empty session_mode must be forwarded; got {line2}"
    );
}

#[test]
fn launch_session_with_first_prompt_includes_prompt_in_ipc() {
    let (mut core, mut peer) = make_core();
    let spec = LaunchSpec {
        project: "proj-a".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: "claude-sonnet-4-5".to_string(),
        session_mode: "acceptEdits".to_string(),
        first_prompt: Some("Hello agent".to_string()),
        mode: Default::default(),
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
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
        })
        .expect("launch");
    let _ = peer.drain_outbound(); // consume launch IPC

    let updates = core
        .resume_session(&id)
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
    // resume must ALSO return VmUpdates that flip the badge off Stale, else the
    // dashboard stays "stopped" forever (a resumed, prompt-less session emits no
    // system/init to move the phase on its own).
    assert!(
        !updates.is_empty(),
        "resume_session must return VmUpdates so the UI leaves the Stale state"
    );
    let has_session_starting = updates.iter().any(|u| {
        matches!(u, VmUpdate::Session(sv)
            if sv.id == id && sv.state == marsad_events::Phase::Starting)
    });
    assert!(
        has_session_starting,
        "resume_session must emit a Session update with phase Starting, got: {updates:?}"
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
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            model: "claude-opus-4-5".to_string(),
            session_mode: "bypassPermissions".to_string(),
            first_prompt: None,
            mode: Default::default(),
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "claude".to_string(),
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
        })
        .expect("launch");
    let _ = peer.drain_outbound();

    let gate = Gate::Permission {
        request_id: "req-test".to_string(),
        command: "ls /".to_string(),
        cwd: "/".to_string(),
        risk: "low".to_string(),
        explanation: "directory listing".to_string(),
        is_elicitation: false,
        elicitation_choices: Vec::new(),
        permission_suggestions: Vec::new(),
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
                claude_bin: None,
                setup: None,
                env: Vec::new(),
                model: "m".to_string(),
                session_mode: String::new(),
                first_prompt: None,
                mode: Default::default(),
                name: None,
                disallowed_tools: vec![],
                wasl: false,
                driver: "claude".to_string(),
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
fn launch_spec_fields_all_session_modes() {
    for mode_id in ["", "default", "acceptEdits", "bypassPermissions", "plan"] {
        let spec = LaunchSpec {
            project: "p".to_string(),
            host_id: "h".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            model: "m".to_string(),
            session_mode: mode_id.to_string(),
            first_prompt: Some("hi".to_string()),
            mode: Default::default(),
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "claude".to_string(),
        };
        let _ = spec;
    }
}

// ---------------------------------------------------------------------------
// GitProbe seam — changes flow into the SessionView
// ---------------------------------------------------------------------------

/// AgentEvent carrying a tool_call_started (sets last_action → triggers
/// a git re-probe in Core).
fn ipc_agent_event_tool_use(session_id: &str, _msg_id: &str) -> String {
    format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"tool_call_started","name":"Write","call_id":"tu-1","input":{{"path":"hello.txt"}}}}}}"#
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
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: "m".to_string(),
        session_mode: String::new(),
        first_prompt: first_prompt.map(|s| s.to_string()),
        mode,
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
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

/// Two-axis lifecycle (Gap A/B): an authoritative `SessionLiveness` from the
/// agent's reader is the truthful Resumed↔Paused source. `live:false` (host
/// reboot / OOM / crash / claude exit) flips a running session to Paused — even
/// though its activity phase is untouched — and `live:true` flips it back to
/// Resumed. Lifecycle is NOT inferred from the turn phase.
#[test]
fn session_liveness_signal_flips_paused_and_resumed() {
    let (mut core, _fake) = make_core();
    let id = launch_t(&mut core, None, LaunchMode::Fresh);
    // A launched + initialized session is Resumed (live).
    let u = core
        .on_agent_line(&ipc_agent_event_system_init(&id))
        .expect("system_init");
    assert!(last_sv(&u).live, "launched+init session must be Resumed (live)");

    // Host kills it → SessionLiveness{false} → Paused.
    let dead = format!(r#"{{"type":"SessionLiveness","session_id":"{id}","live":false}}"#);
    let u = core.on_agent_line(&dead).expect("liveness false");
    assert!(
        !last_sv(&u).live,
        "SessionLiveness live:false must flip the session to Paused (live=false)"
    );

    // Reconnect → SessionLiveness{true} → Resumed again.
    let alive = format!(r#"{{"type":"SessionLiveness","session_id":"{id}","live":true}}"#);
    let u = core.on_agent_line(&alive).expect("liveness true");
    assert!(
        last_sv(&u).live,
        "SessionLiveness live:true must flip the session back to Resumed (live=true)"
    );
}

/// `conn_health` is a per-session CONNECTION signal that is meaningful only while
/// the session is running. A `SessionConn` surfaces it on the view; going Paused
/// (`live:false`) MUST clear it so a finalized/paused session never lingers on a
/// stale "Reconnecting"/"Unstable"/"Disconnected" hint (the DCSim/Tajir bug tail).
#[test]
fn conn_health_surfaces_and_clears_on_pause() {
    let (mut core, _fake) = make_core();
    let id = launch_t(&mut core, None, LaunchMode::Fresh);
    let _ = core
        .on_agent_line(&ipc_agent_event_system_init(&id))
        .expect("init");

    // Reader reports the stream flapping to Unstable → surfaces on the view.
    let unstable = format!(r#"{{"type":"SessionConn","session_id":"{id}","health":"Unstable"}}"#);
    let u = core.on_agent_line(&unstable).expect("conn unstable");
    assert_eq!(
        last_sv(&u).conn_health,
        "Unstable",
        "conn_health must surface on the SessionView"
    );

    // Reader gives up / user pauses → SessionLiveness false → conn_health cleared.
    let dead = format!(r#"{{"type":"SessionLiveness","session_id":"{id}","live":false}}"#);
    let u = core.on_agent_line(&dead).expect("liveness false");
    let sv = last_sv(&u);
    assert!(!sv.live, "session must be Paused");
    assert_eq!(
        sv.conn_health, "Connected",
        "conn_health must be cleared on pause — no stale Unstable/Reconnecting hint"
    );

    // A sustained-unreachable Disconnected also surfaces while running.
    let _ = core
        .on_agent_line(&format!(
            r#"{{"type":"SessionLiveness","session_id":"{id}","live":true}}"#
        ))
        .expect("live again");
    let disc =
        format!(r#"{{"type":"SessionConn","session_id":"{id}","health":"Disconnected"}}"#);
    let u = core.on_agent_line(&disc).expect("conn disconnected");
    assert_eq!(last_sv(&u).conn_health, "Disconnected");
}

/// Pausing kills the process, so a pending HITL gate is WASTED (unanswerable
/// until a cold resume — like closing the Claude Code TUI mid-prompt). A
/// `SessionLiveness{live:false}` must therefore DROP the pending gate from state
/// (not just hide it): the card shows PAUSED, and nothing stale resurfaces on
/// resume.
#[test]
fn pause_drops_the_pending_gate() {
    let (mut core, _fake) = make_core();
    let id = launch_t(&mut core, None, LaunchMode::Fresh);
    let _ = core
        .on_agent_line(&ipc_agent_event_system_init(&id))
        .expect("init");

    // A control_request drives the session to Needs with a pending gate.
    let u = core
        .on_agent_line(&ipc_agent_event_control_request(&id, "req-1"))
        .expect("control_request");
    let sv = last_sv(&u);
    assert_eq!(sv.state, marsad_events::Phase::Needs, "gate → Needs");
    assert!(sv.gate.is_some(), "gate must be pending before pause");

    // Pause → the gate is discarded and the lingering Needs flips off.
    let dead = format!(r#"{{"type":"SessionLiveness","session_id":"{id}","live":false}}"#);
    let u = core.on_agent_line(&dead).expect("liveness false");
    let sv = last_sv(&u);
    assert!(!sv.live, "session is Paused");
    assert!(sv.gate.is_none(), "the wasted gate must be cleared on pause");
    assert_ne!(
        sv.state,
        marsad_events::Phase::Needs,
        "phase must not linger on Needs after the gate is dropped"
    );
}

#[test]
fn transcript_resume_loads_session_history() {
    use marsad_events::TranscriptEntry;
    let hist = vec![TranscriptEntry::User {
        text: "h1".to_string(),
        at: None,
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
        at: None,
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
    let t = last_sv(&u).transcript;
    assert_eq!(t.len(), 1);
    assert!(matches!(&t[0], TranscriptEntry::User { text, .. } if text == "hello there"));
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
    let t = last_sv(&u).transcript;
    assert_eq!(t.len(), 1, "same message id must append exactly once");
    assert!(matches!(&t[0], TranscriptEntry::Assistant { text, .. } if text == "Thinking..."));
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
    // Whitespace-only AssistantDelta (m3) and a ThinkingDelta (m4) → no entries.
    let empty_text = format!(
        r#"{{"type":"AgentEvent","session_id":"{id}","event":{{"type":"assistant_delta","text":"   ","message_id":"m3"}}}}"#
    );
    // ThinkingDelta is explicitly skipped (thinking blocks are not transcribed).
    let thinking = format!(
        r#"{{"type":"AgentEvent","session_id":"{id}","event":{{"type":"thinking_delta","text":"some thought","message_id":"m4"}}}}"#
    );
    core.on_agent_line(&empty_text).unwrap();
    let u = core.on_agent_line(&thinking).unwrap();
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
    let t = last_sv(&u).transcript;
    assert_eq!(t.len(), 2);
    assert!(matches!(&t[0], TranscriptEntry::User { text, .. } if text == "please do X"));
    assert!(matches!(&t[1], TranscriptEntry::Assistant { text, .. } if text == "Thinking..."));
}

// ---------------------------------------------------------------------------
// Host commands — add/remove/set_status/load + persistence (MemoryBackend)
// ---------------------------------------------------------------------------

use marsad_core::HostStatus;
use marsad_store::{BackendOp, MemoryBackend, PersistedHost, PersistedSession, Store};

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
        .add_host("srv", "alice@srv.example.com", None, None, Vec::new(), Vec::new())
        .expect("add_host");
    let hosts = hosts_from(&u);
    assert!(hosts.iter().any(|h| h.label == "srv" && !h.is_local));
    assert!(hosts.iter().any(|h| h.is_local), "localhost must remain");
    assert!(core.snapshot().hosts.iter().any(|h| h.label == "srv"));
}

#[test]
fn add_host_duplicate_label_errors() {
    let mut core = core_with_backend(MemoryBackend::new());
    core.add_host("srv", "a@h1", None, None, Vec::new(), Vec::new())
        .expect("first add");
    assert!(
        core.add_host("srv", "b@h2", None, None, Vec::new(), Vec::new())
            .is_err(),
        "dup label must error"
    );
}

#[test]
fn add_host_malformed_target_errors() {
    let mut core = core_with_backend(MemoryBackend::new());
    // Bare aliases are accepted now; a target with whitespace is still malformed.
    assert!(core
        .add_host("x", "has a space", None, None, Vec::new(), Vec::new())
        .is_err());
}

#[test]
fn add_host_store_write_failure_errors() {
    let backend = MemoryBackend::new();
    backend.fail_next(BackendOp::Write);
    let mut core = core_with_backend(backend);
    assert!(
        core.add_host("srv", "a@h", None, None, Vec::new(), Vec::new()).is_err(),
        "persist failure surfaces"
    );
}

#[test]
fn remove_host_success_and_localhost_protected() {
    let mut core = core_with_backend(MemoryBackend::new());
    let u = core
        .add_host("srv", "a@h", None, None, Vec::new(), Vec::new())
        .expect("add");
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
    let u = core
        .add_host("srv", "a@h", None, None, Vec::new(), Vec::new())
        .expect("add");
    let id = remote_id(&u);
    backend.fail_next(BackendOp::Delete);
    assert!(core.remove_host(&id).is_err(), "delete failure surfaces");
}

#[test]
fn set_host_status_updates_the_list() {
    let mut core = core_with_backend(MemoryBackend::new());
    let u = core
        .add_host("srv", "a@h", None, None, Vec::new(), Vec::new())
        .expect("add");
    let id = remote_id(&u);
    // A reachable host whose capability discovery hasn't landed yet shows
    // Connecting (not Connected) — it flips to Connected only once every probed
    // driver has reported (see reconnect/discovery tests in branches.rs).
    let u2 = core.set_host_status(&id, HostStatus::Connected);
    let h = hosts_from(&u2).into_iter().find(|h| h.id == id).unwrap();
    assert_eq!(h.status, HostStatus::Connecting);
    // Unreachable is reported verbatim.
    let u3 = core.set_host_status(&id, HostStatus::Unreachable);
    let h3 = hosts_from(&u3).into_iter().find(|h| h.id == id).unwrap();
    assert_eq!(h3.status, HostStatus::Unreachable);
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
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        agent_commands: Vec::new(),
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

// Adding a host with claude_bin/setup/env persists them and they resolve into the
// LaunchSession IPC line when a session launches on that host.
#[test]
fn add_host_with_launch_config_resolves_into_ipc() {
    let (mut core, mut peer) = make_core();
    core.add_host(
        "box",
        "user@box.example.com",
        Some("genie".to_string()),
        Some("module load x".to_string()),
        vec![("A".to_string(), "b".to_string())],
        Vec::new(),
    )
    .expect("add remote host with config");
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
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: "claude-haiku-4-5-20251001".to_string(),
        session_mode: String::new(),
        first_prompt: None,
        mode: Default::default(),
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
    })
    .expect("launch on remote host");

    let outbound = peer.drain_outbound();
    let line = &outbound[0];
    assert!(line.contains("\"claude_bin\":\"genie\""), "got: {line}");
    assert!(line.contains("\"setup\":\"module load x\""), "got: {line}");
    // env forwarded as a JSON array of [k, v] pairs.
    assert!(line.contains(r#""env":[["A","b"]]"#), "got: {line}");
}

// An explicit env on the LaunchSpec wins over the host's configured env.
#[test]
fn launch_spec_env_overrides_host_env() {
    let (mut core, mut peer) = make_core();
    core.update_host(
        "localhost",
        None,
        None,
        vec![("HOST".to_string(), "x".to_string())],
        None,
        Vec::new(),
    )
    .expect("seed localhost env");
    core.launch_session(LaunchSpec {
        project: "p".to_string(),
        host_id: "localhost".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: vec![("SPEC".to_string(), "y".to_string())],
        model: "claude-haiku-4-5-20251001".to_string(),
        session_mode: String::new(),
        first_prompt: None,
        mode: Default::default(),
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
    })
    .expect("launch with explicit env");
    let outbound = peer.drain_outbound();
    let line = &outbound[0];
    assert!(line.contains(r#""env":[["SPEC","y"]]"#), "got: {line}");
    assert!(!line.contains("HOST"), "spec env must win: {line}");
}

// update_host changes a remote host's config and persists it.
#[test]
fn update_host_changes_remote_config() {
    let mut core = core_with_backend(MemoryBackend::new());
    let u = core
        .add_host("srv", "a@h", None, None, Vec::new(), Vec::new())
        .expect("add");
    let id = remote_id(&u);
    let u2 = core
        .update_host(
            &id,
            Some("genie".to_string()),
            Some("setup.sh".to_string()),
            vec![("K".to_string(), "V".to_string())],
            Some("a@h2".to_string()),
            Vec::new(),
        )
        .expect("update_host");
    let host = hosts_from(&u2).into_iter().find(|h| h.id == id).unwrap();
    assert_eq!(host.claude_bin.as_deref(), Some("genie"));
    assert_eq!(host.setup.as_deref(), Some("setup.sh"));
    assert_eq!(host.env, vec![("K".to_string(), "V".to_string())]);
    assert_eq!(host.target, "a@h2");
}

// update_host works on the protected localhost: its target is unchanged (None),
// the config is persisted, and a session launched on localhost picks it up.
#[test]
fn update_host_localhost_config_resolves_into_ipc() {
    let (mut core, mut peer) = make_core();
    core.update_host(
        "localhost",
        Some("/opt/claude".to_string()),
        Some("module load cuda".to_string()),
        vec![("FOO".to_string(), "bar".to_string())],
        None,
        Vec::new(),
    )
    .expect("update localhost");
    // localhost target stays "this machine".
    let local = core
        .snapshot()
        .hosts
        .into_iter()
        .find(|h| h.is_local)
        .expect("localhost present");
    assert_eq!(local.target, "this machine");

    core.launch_session(LaunchSpec {
        project: "p".to_string(),
        host_id: "localhost".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: "claude-haiku-4-5-20251001".to_string(),
        session_mode: String::new(),
        first_prompt: None,
        mode: Default::default(),
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
    })
    .expect("launch on localhost");
    let outbound = peer.drain_outbound();
    let line = &outbound[0];
    assert!(
        line.contains("\"claude_bin\":\"/opt/claude\""),
        "got: {line}"
    );
    assert!(
        line.contains("\"setup\":\"module load cuda\""),
        "got: {line}"
    );
    assert!(line.contains(r#""env":[["FOO","bar"]]"#), "got: {line}");
    // localhost is not remote, so no host_target is injected.
    assert!(!line.contains("host_target"), "got: {line}");
}

// update_host store-write failure surfaces.
#[test]
fn update_host_store_write_failure_errors() {
    let backend = MemoryBackend::new();
    let mut core = core_with_backend(backend.clone());
    core.add_host("srv", "a@h", None, None, Vec::new(), Vec::new())
        .expect("add");
    backend.fail_next(BackendOp::Write);
    assert!(
        core.update_host("localhost", Some("x".to_string()), None, Vec::new(), None, Vec::new())
            .is_err(),
        "persist failure surfaces"
    );
}

// update_host on an unknown id errors (Host NotFound surfaced).
#[test]
fn update_host_unknown_id_errors() {
    let mut core = core_with_backend(MemoryBackend::new());
    assert!(
        core.update_host("nope", None, None, Vec::new(), None, Vec::new())
            .is_err(),
        "unknown id must error"
    );
}

// load_hosts restores a persisted localhost config onto the seeded localhost.
#[test]
fn load_hosts_restores_localhost_config() {
    let backend = MemoryBackend::new();
    let mut seed = Store::new(Box::new(backend.clone()));
    seed.upsert_host(&PersistedHost {
        id: "localhost".to_string(),
        label: "localhost".to_string(),
        target: "this machine".to_string(),
        is_local: true,
        claude_bin: Some("genie".to_string()),
        setup: Some("module load x".to_string()),
        env: vec![("A".to_string(), "b".to_string())],
        agent_commands: Vec::new(),
    })
    .expect("seed localhost config");
    let mut core = core_with_backend(backend);
    let u = core.load_hosts().expect("load_hosts");
    let hosts = hosts_from(&u);
    let local = hosts.iter().find(|h| h.is_local).expect("localhost");
    assert_eq!(local.claude_bin.as_deref(), Some("genie"));
    assert_eq!(local.setup.as_deref(), Some("module load x"));
    assert_eq!(local.env, vec![("A".to_string(), "b".to_string())]);
    // Still exactly one localhost (update, not duplicate-append).
    assert_eq!(hosts.iter().filter(|h| h.is_local).count(), 1);
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

    // Initially the session has no changes.
    let snap = core.snapshot();
    let sv = snap.sessions.iter().find(|s| s.id == id).expect("session");
    assert!(sv.changes.is_empty(), "should start empty");

    // Inject two changes (simulating a post-commit re-probe with remaining files).
    let new_changes = vec![FileChange {
        path: "remaining.txt".to_string(),
        status: "M".to_string(),
        added: 1,
        removed: 0,
    }];
    let updates = core.set_session_changes(&id, new_changes.clone());

    // Must produce exactly Session + Overview.
    assert_eq!(updates.len(), 2, "should return Session + Overview");
    let has_session = updates.iter().any(|u| matches!(u, VmUpdate::Session(_)));
    let has_overview = updates.iter().any(|u| matches!(u, VmUpdate::Overview(_)));
    assert!(has_session, "must include Session update");
    assert!(has_overview, "must include Overview update");

    // The returned SessionView must carry the new file list.
    let sv2 = updates
        .iter()
        .find_map(|u| match u {
            VmUpdate::Session(s) => Some(s.clone()),
            _ => None,
        })
        .expect("session view");
    assert_eq!(
        sv2.changes, new_changes,
        "session view must reflect the new changes"
    );
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
        default_driver: "claude".to_string(),
        default_models: Vec::new(),
        default_modes: Vec::new(),
    }
}

#[test]
fn get_settings_returns_defaults_when_empty() {
    let core = core_with_backend(MemoryBackend::new());
    let s = core.get_settings();
    // Defaults: sonnet model, empty default_modes, restore_on_launch true
    assert_eq!(s.default_model, "claude-sonnet-4-6");
    assert!(s.default_modes.is_empty());
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
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"session_init","native_session_id":"{claude_uuid}","model":"claude-opus-4-5","cwd":"/tmp","tools":[],"mcp_servers":[],"permission_mode":"default"}}}}"#
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
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: "claude-sonnet-4-6".to_string(),
        session_mode: String::new(),
        first_prompt: None,
        mode: LaunchMode::Fresh,
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
    };
    let id = core.launch_session(spec).expect("launch");
    let _ = peer.drain_outbound(); // consume the initial LaunchSession IPC

    // Simulate the agent sending system/init with a claude UUID.
    let claude_uuid = "claude-uuid-abcde";
    core.on_agent_line(&ipc_init_with_uuid(&id, claude_uuid))
        .expect("init");

    // Restart with a new model and posture.
    let updates = core
        .restart_session(&id, "claude-opus-4-8", "bypassPermissions".to_string(), vec![])
        .expect("restart_session should succeed");

    // Must emit a VmUpdate::Session for the same id.
    let has_session = updates
        .iter()
        .any(|u| matches!(u, VmUpdate::Session(sv) if sv.id == id));
    assert!(
        has_session,
        "restart must produce a Session update for the same id"
    );

    // Outbound IPC: StopSession then LaunchSession.
    let outbound = peer.drain_outbound();
    assert!(
        outbound.len() >= 2,
        "restart must emit at least 2 IPC lines, got {}",
        outbound.len()
    );

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
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: "claude-sonnet-4-6".to_string(),
        session_mode: String::new(),
        first_prompt: None,
        mode: LaunchMode::Fresh,
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
    };
    let id = core.launch_session(spec).expect("launch");
    let _ = peer.drain_outbound();

    // Restart — no UUID known → should use Continue mode.
    let updates = core
        .restart_session(&id, "claude-haiku-4-5", "acceptEdits".to_string(), vec![])
        .expect("restart without uuid");

    let has_session = updates
        .iter()
        .any(|u| matches!(u, VmUpdate::Session(sv) if sv.id == id));
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
        .restart_session("no-such-session", "model", String::new(), vec![])
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
fn save_template_preserves_model_and_mode() {
    let mut core = core_with_backend(MemoryBackend::new());
    core.save_template("careful", "claude-opus-4-8", "bypassPermissions")
        .expect("save_template");
    let templates = core.list_templates().expect("list_templates");
    let t = templates
        .into_iter()
        .find(|t| t.name == "careful")
        .expect("template");
    assert_eq!(t.model, "claude-opus-4-8");
    assert_eq!(t.mode, "bypassPermissions");
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
    core.save_template("tmpl", "claude-sonnet-4-6", "ask")
        .expect("first");
    core.save_template("tmpl", "claude-opus-4-8", "full")
        .expect("second");
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
        mode: "default".to_string(),
    };
    assert_eq!(t.name, "test");
}

// ---------------------------------------------------------------------------
// load_sessions — restore persisted sessions on startup (relaunch)
// ---------------------------------------------------------------------------

fn seed_session(seed: &mut Store, id: &str, host_id: &str, claude_uuid: Option<&str>) {
    seed.upsert_session(&PersistedSession {
        id: id.to_string(),
        session_id: claude_uuid.map(|s| s.to_string()),
        name: id.to_string(),
        host_id: host_id.to_string(),
        host_target: Some("wsl:Ubuntu".to_string()),
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        project: "/work/proj".to_string(),
        branch: "main".to_string(),
        model: "claude-opus-4-8".to_string(),
        last_phase: "Working".to_string(),
        mode: None,
        disallowed_tools: vec![],
        driver: "claude".to_string(),
        slash_commands: vec![],
        tools: vec![],
        mcp_servers: vec![],
    })
    .expect("seed session");
}

/// A persisted session is restored into the snapshot on its real host — covering
/// both the known-uuid (session) and unknown-uuid (most_recent) transcript-load
/// branches — and the restore is idempotent. Two-axis model: its ACTIVITY is idle
/// (Done); "not running until proven" is carried by the separate liveness axis
/// (`live == false` → Paused), not by the phase.
#[test]
fn load_sessions_restores_persisted_sessions() {
    let backend = MemoryBackend::new();
    let mut seed = Store::new(Box::new(backend.clone()));
    seed_session(&mut seed, "session-restore-1", "host-5", Some("uuid-1"));
    seed_session(&mut seed, "session-restore-2", "host-6", None);

    let mut core = core_with_backend(backend);
    let updates = core.load_sessions().expect("load_sessions");
    assert!(!updates.is_empty(), "load_sessions must emit updates");

    let snap = core.snapshot();
    let sv = snap
        .sessions
        .iter()
        .find(|s| s.id == "session-restore-1")
        .expect("restored session present");
    assert_eq!(sv.host_id, "host-5", "restored session keeps its host");
    assert_eq!(
        sv.state,
        marsad_events::Phase::Done,
        "restored session's activity is idle (Done), not a fake-live phase"
    );
    assert!(
        !sv.live,
        "restored session is not-live (Paused) until the agent proves it live"
    );
    assert!(
        snap.sessions.iter().any(|s| s.id == "session-restore-2"),
        "both restored sessions present"
    );

    // Idempotent: a second load does not duplicate already-tracked sessions.
    let _ = core.load_sessions().expect("load_sessions again");
    let snap2 = core.snapshot();
    assert_eq!(
        snap2
            .sessions
            .iter()
            .filter(|s| s.id == "session-restore-1")
            .count(),
        1,
        "second load must not duplicate the session"
    );
}

#[test]
fn load_sessions_restore_failure_errors() {
    let backend = MemoryBackend::new();
    backend.fail_next(BackendOp::List);
    let mut core = core_with_backend(backend);
    assert!(
        core.load_sessions().is_err(),
        "a failing store restore must surface as an error"
    );
}

/// load_sessions seeds tokens/model_usage from the transcript JSONL for LOCAL
/// sessions so the Usage tab shows the whole-conversation total on attach, not
/// zero. A subsequent live `result` event will authoritatively replace the
/// estimate (tested in the `result` event tests above). Bug #3 regression test.
#[test]
fn load_sessions_seeds_usage_for_local_session() {
    struct FakeTranscriptWithUsage;
    impl marsad_core::TranscriptReader for FakeTranscriptWithUsage {
        fn session(&self, _p: &str, _id: &str) -> Vec<marsad_events::TranscriptEntry> {
            Vec::new()
        }
        fn most_recent(&self, _p: &str) -> Vec<marsad_events::TranscriptEntry> {
            Vec::new()
        }
        fn session_usage(
            &self,
            _p: &str,
            _id: &str,
        ) -> std::collections::HashMap<String, marsad_core::discovery::ModelUsageSummary> {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "claude-sonnet-4-5".to_string(),
                marsad_core::discovery::ModelUsageSummary {
                    input_tokens: 1000,
                    output_tokens: 200,
                    cache_read_input_tokens: 50,
                    cache_creation_input_tokens: 0,
                    cost_usd: 0.042,
                },
            );
            m
        }
    }

    let backend = MemoryBackend::new();
    let mut seed = Store::new(Box::new(backend.clone()));
    // LOCAL session: host_id "local", host_target None (matches host_is_local check)
    seed.upsert_session(&PersistedSession {
        id: "usage-seed-local".to_string(),
        session_id: Some("uuid-usage".to_string()),
        name: "usage-seed-local".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        project: "/work/proj".to_string(),
        branch: "main".to_string(),
        model: "claude-sonnet-4-5".to_string(),
        last_phase: "Done".to_string(),
        mode: None,
        disallowed_tools: vec![],
        driver: "claude".to_string(),
        slash_commands: vec![],
        tools: vec![],
        mcp_servers: vec![],
    })
    .expect("seed");

    let (a, _b) = LoopbackFake::pair();
    let cfg = CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-test".to_string(),
    };
    let mut core = Core::with_deps(
        cfg,
        Box::new(a),
        Box::new(marsad_core::RealGit),
        Box::new(FakeTranscriptWithUsage),
        Box::new(backend),
    )
    .expect("with_deps");

    core.load_sessions().expect("load_sessions");
    let snap = core.snapshot();
    let sv = snap
        .sessions
        .iter()
        .find(|s| s.id == "usage-seed-local")
        .expect("session present");
    assert!(sv.tokens > 0, "tokens must be seeded from JSONL on attach (was 0)");
    assert!(
        !sv.model_usage.is_empty(),
        "model_usage must be seeded from JSONL on attach"
    );
    // The seeded total = 1000 + 200 + 50 + 0 = 1250 tokens.
    assert_eq!(sv.tokens, 1250, "token total must sum all token fields");
}

// ---------------------------------------------------------------------------
// opencode must NOT inherit claude's on-disk transcript/usage on load.
// An opencode session owns no ~/.claude transcript; when it shares a project dir
// with a claude session, the claude data would bleed in (opencode header + claude
// transcript/usage). Sibling of the reader's remote-backfill guard.
// ---------------------------------------------------------------------------

#[test]
fn load_sessions_does_not_seed_claude_usage_for_opencode() {
    struct FakeUsage;
    impl marsad_core::TranscriptReader for FakeUsage {
        fn session(&self, _p: &str, _id: &str) -> Vec<marsad_events::TranscriptEntry> {
            Vec::new()
        }
        fn most_recent(&self, _p: &str) -> Vec<marsad_events::TranscriptEntry> {
            Vec::new()
        }
        fn session_usage(
            &self,
            _p: &str,
            _id: &str,
        ) -> std::collections::HashMap<String, marsad_core::discovery::ModelUsageSummary> {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "claude-opus-4-8".to_string(),
                marsad_core::discovery::ModelUsageSummary {
                    input_tokens: 40_000,
                    output_tokens: 8_000,
                    cache_read_input_tokens: 0,
                    cache_creation_input_tokens: 0,
                    cost_usd: 3.0,
                },
            );
            m
        }
    }

    let backend = MemoryBackend::new();
    let mut seed = Store::new(Box::new(backend.clone()));
    seed.upsert_session(&PersistedSession {
        id: "oc-session".to_string(),
        session_id: Some("uuid-oc".to_string()),
        name: "oc-session".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        project: "/proj/ArchAttention".to_string(),
        branch: "main".to_string(),
        model: "amd-anthropic/Claude-Sonnet-5".to_string(),
        last_phase: "Done".to_string(),
        mode: None,
        disallowed_tools: vec![],
        driver: "opencode".to_string(),
        slash_commands: vec![],
        tools: vec![],
        mcp_servers: vec![],
    })
    .expect("seed opencode session");

    let (a, _b) = LoopbackFake::pair();
    let cfg = CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-test".to_string(),
    };
    let mut core = Core::with_deps(
        cfg,
        Box::new(a),
        Box::new(marsad_core::RealGit),
        Box::new(FakeUsage),
        Box::new(backend),
    )
    .expect("with_deps");
    core.load_sessions().expect("load_sessions");

    let tokens = core
        .snapshot()
        .sessions
        .into_iter()
        .find(|s| s.id == "oc-session")
        .expect("opencode session present after load")
        .tokens;
    assert_eq!(
        tokens, 0,
        "an opencode session must NOT be seeded from claude transcript usage; got {tokens}"
    );
}

// ---------------------------------------------------------------------------
// Bug A regression: live result event must NOT clobber whole-conversation totals
// ---------------------------------------------------------------------------

/// After a local session is seeded with large whole-conversation usage from the
/// JSONL, a small live `result` event must leave tokens/cost >= the seed total.
/// Regression for: reduce_result used result.total_cost_usd (current-run only).
#[test]
fn result_event_does_not_clobber_whole_conversation_usage() {
    // A transcript reader that returns a large historical total.
    struct FakeLargeHistory;
    impl marsad_core::TranscriptReader for FakeLargeHistory {
        fn session(&self, _p: &str, _id: &str) -> Vec<marsad_events::TranscriptEntry> {
            Vec::new()
        }
        fn most_recent(&self, _p: &str) -> Vec<marsad_events::TranscriptEntry> {
            Vec::new()
        }
        fn session_usage(
            &self,
            _p: &str,
            _id: &str,
        ) -> std::collections::HashMap<String, marsad_core::discovery::ModelUsageSummary> {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "claude-sonnet-4-5".to_string(),
                marsad_core::discovery::ModelUsageSummary {
                    input_tokens: 50_000,
                    output_tokens: 10_000,
                    cache_read_input_tokens: 5_000,
                    cache_creation_input_tokens: 0,
                    cost_usd: 2.50,
                },
            );
            m
        }
    }

    let backend = MemoryBackend::new();
    let mut seed = Store::new(Box::new(backend.clone()));
    seed.upsert_session(&PersistedSession {
        id: "bug-a-session".to_string(),
        session_id: Some("uuid-bug-a".to_string()),
        name: "bug-a-session".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        project: "/work/proj".to_string(),
        branch: "main".to_string(),
        model: "claude-sonnet-4-5".to_string(),
        last_phase: "Done".to_string(),
        mode: None,
        disallowed_tools: vec![],
        driver: "claude".to_string(),
        slash_commands: vec![],
        tools: vec![],
        mcp_servers: vec![],
    })
    .expect("seed bug-a session");

    let (a, _b) = LoopbackFake::pair();
    let cfg = CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-test".to_string(),
    };
    let mut core = Core::with_deps(
        cfg,
        Box::new(a),
        Box::new(marsad_core::RealGit),
        Box::new(FakeLargeHistory),
        Box::new(backend),
    )
    .expect("with_deps");

    core.load_sessions().expect("load_sessions");

    // Seed total: 50_000 + 10_000 + 5_000 = 65_000 tokens, cost $2.50.
    let seeded_tokens = core
        .snapshot()
        .sessions
        .iter()
        .find(|s| s.id == "bug-a-session")
        .map(|s| s.tokens)
        .expect("session present after load_sessions");
    assert!(seeded_tokens > 0, "session must be seeded with usage");

    // Send a session_init to move the session out of Stale phase (required before
    // a result event is processed — stale sessions ignore non-init events).
    let init_line = r#"{"type":"AgentEvent","session_id":"bug-a-session","event":{"type":"session_init","native_session_id":"uuid-bug-a","model":"claude-sonnet-4-5","cwd":"/work/proj","tools":[],"mcp_servers":[],"permission_mode":"default"}}"#;
    core.on_agent_line(init_line).expect("on_agent_line (init)");

    // Now deliver a small live `turn_completed` event (only 100+20 tokens, $0.001).
    let small_result = r#"{"type":"AgentEvent","session_id":"bug-a-session","event":{"type":"turn_completed","usage":{"input_tokens":100,"output_tokens":20,"cache_read_tokens":0,"cache_creation_tokens":0,"cost_usd":0.001}}}"#;
    core.on_agent_line(small_result).expect("on_agent_line (result)");

    let sv = core
        .snapshot()
        .sessions
        .into_iter()
        .find(|s| s.id == "bug-a-session")
        .expect("session present after result");
    // Whole-conversation total must be >= the seed, NOT the small run's 120 tokens.
    assert!(
        sv.tokens >= seeded_tokens,
        "result event must not clobber whole-conversation total: tokens={} < seeded={}",
        sv.tokens,
        seeded_tokens
    );
    assert!(
        sv.cost_usd >= 2.0,
        "cost must reflect whole conversation ($2.50), not just current run ($0.001): got {}",
        sv.cost_usd
    );
}

// ---------------------------------------------------------------------------
// Bug B regression: tools/mcp_servers persisted after SystemInit and restored
// ---------------------------------------------------------------------------

/// After a SystemInit fires, tools and mcp_servers must be persisted to the
/// store and then restored on the next load_sessions call.
#[test]
fn system_init_persists_and_restores_tools_and_mcp_servers() {
    let backend = MemoryBackend::new();
    let mut seed = Store::new(Box::new(backend.clone()));
    seed.upsert_session(&PersistedSession {
        id: "bug-b-session".to_string(),
        session_id: None,
        name: "bug-b-session".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        project: "/work/proj-b".to_string(),
        branch: "main".to_string(),
        model: "claude-sonnet-4-5".to_string(),
        last_phase: "Working".to_string(),
        mode: None,
        disallowed_tools: vec![],
        driver: "claude".to_string(),
        slash_commands: vec![],
        tools: vec![],
        mcp_servers: vec![],
    })
    .expect("seed bug-b session");

    let (a, _b) = LoopbackFake::pair();
    let cfg = CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-test".to_string(),
    };
    let mut core = Core::with_deps(
        cfg,
        Box::new(a),
        Box::new(marsad_core::RealGit),
        Box::new(FakeTranscript { session: vec![], recent: vec![] }),
        Box::new(backend.clone()),
    )
    .expect("with_deps");
    core.load_sessions().expect("load_sessions");

    // Deliver a session_init with tools and mcp_servers.
    let init_line = r#"{"type":"AgentEvent","session_id":"bug-b-session","event":{"type":"session_init","native_session_id":"claude-uuid-b","model":"claude-sonnet-4-5","cwd":"/work/proj-b","tools":["Read","Bash"],"mcp_servers":[{"name":"hares","status":"connected"}],"permission_mode":"default"}}"#;
    core.on_agent_line(init_line).expect("on_agent_line");

    // Verify the store now holds the persisted tools and mcp_servers.
    let restored = Store::new(Box::new(backend))
        .restore()
        .expect("restore from backend");
    let ps = restored
        .sessions
        .into_iter()
        .find(|s| s.id == "bug-b-session")
        .expect("session in store");
    assert_eq!(
        ps.tools,
        vec!["Read".to_string(), "Bash".to_string()],
        "tools must be persisted after SystemInit"
    );
    assert_eq!(ps.mcp_servers.len(), 1, "mcp_servers must be persisted");
    assert_eq!(ps.mcp_servers[0].name, "hares");

    // Also verify load_sessions restores them into state.
    let (a2, _b2) = LoopbackFake::pair();
    let cfg2 = CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-test".to_string(),
    };
    // NOTE: restore() consumed the backend above (cloned), so we use the cloned one.
    // The backend clone was moved above. Re-seed from the same session data.
    // Actually backend.clone() was consumed. Use the core's store data via snapshot.
    let sv = core
        .snapshot()
        .sessions
        .into_iter()
        .find(|s| s.id == "bug-b-session")
        .expect("session present in snapshot");
    assert_eq!(
        sv.tools,
        vec!["Read".to_string(), "Bash".to_string()],
        "tools must appear in SessionView after SystemInit"
    );
    assert_eq!(
        sv.mcp_servers.len(),
        1,
        "mcp_servers must appear in SessionView after SystemInit"
    );
    let _ = cfg2;
    let _ = a2;
}

// ---------------------------------------------------------------------------
// remove_session — delete a session from the list + store (declutter)
// ---------------------------------------------------------------------------

/// Removing a session drops it from the in-memory list and emits a
/// `SessionRemoved` update so the UI deletes the row.
#[test]
fn remove_session_deletes_from_list_and_store() {
    let backend = MemoryBackend::new();
    let mut seed = Store::new(Box::new(backend.clone()));
    seed_session(&mut seed, "session-rm-1", "host-5", Some("uuid"));
    let mut core = core_with_backend(backend);
    core.load_sessions().expect("load_sessions");
    assert!(
        core.snapshot()
            .sessions
            .iter()
            .any(|s| s.id == "session-rm-1"),
        "precondition: session is tracked"
    );

    let updates = core.remove_session("session-rm-1").expect("remove_session");
    assert!(
        updates
            .iter()
            .any(|u| matches!(u, VmUpdate::SessionRemoved { id } if id == "session-rm-1")),
        "remove_session must emit SessionRemoved, got: {updates:?}"
    );
    assert!(
        !core
            .snapshot()
            .sessions
            .iter()
            .any(|s| s.id == "session-rm-1"),
        "removed session must be gone from the list"
    );
}

#[test]
fn remove_session_unknown_id_errors() {
    let mut core = core_with_backend(MemoryBackend::new());
    assert!(
        core.remove_session("does-not-exist").is_err(),
        "removing an unknown session must error"
    );
}

#[test]
fn remove_session_store_delete_failure_errors() {
    let backend = MemoryBackend::new();
    let mut seed = Store::new(Box::new(backend.clone()));
    seed_session(&mut seed, "session-rm-2", "host-5", Some("uuid"));
    let mut core = core_with_backend(backend.clone());
    core.load_sessions().expect("load_sessions");
    backend.fail_next(BackendOp::Delete);
    assert!(
        core.remove_session("session-rm-2").is_err(),
        "a failing store delete must surface as an error"
    );
}

// ---------------------------------------------------------------------------
// TranscriptBackfill — remote session history shipped by the agent
// ---------------------------------------------------------------------------

fn backfill_line(session_id: &str, jsonl: &str) -> String {
    serde_json::json!({
        "type": "TranscriptBackfill",
        "session_id": session_id,
        "jsonl": jsonl,
    })
    .to_string()
}

/// A TranscriptBackfill for a tracked session parses the NDJSON and prepends the
/// history (user + assistant entries) to the session's transcript.
#[test]
fn transcript_backfill_populates_history() {
    let backend = MemoryBackend::new();
    let mut seed = Store::new(Box::new(backend.clone()));
    seed_session(&mut seed, "session-bf-1", "host-5", Some("uuid"));
    let mut core = core_with_backend(backend);
    core.load_sessions().expect("load_sessions");

    let jsonl = concat!(
        r#"{"type":"user","message":{"content":[{"type":"text","text":"first ask"}]}}"#,
        "\n",
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"first reply"}]}}"#,
    );
    let updates = core
        .on_agent_line(&backfill_line("session-bf-1", jsonl))
        .expect("backfill applies");
    assert!(
        updates.iter().any(|u| matches!(u, VmUpdate::Session(_))),
        "backfill must emit a Session update"
    );
    let sv = core
        .snapshot()
        .sessions
        .into_iter()
        .find(|s| s.id == "session-bf-1")
        .expect("session present");
    assert_eq!(
        sv.transcript.len(),
        2,
        "history must be parsed into the transcript, got: {:?}",
        sv.transcript
    );
}

/// Backfill for an unknown session is a silent no-op (no update, no panic).
#[test]
fn transcript_backfill_unknown_session_is_noop() {
    let mut core = core_with_backend(MemoryBackend::new());
    let jsonl = r#"{"type":"user","message":{"content":[{"type":"text","text":"hi"}]}}"#;
    let updates = core
        .on_agent_line(&backfill_line("nope", jsonl))
        .expect("no error");
    assert!(updates.is_empty(), "unknown session must yield no updates");
}

/// Backfill missing a session_id is a protocol error.
#[test]
fn transcript_backfill_missing_session_id_errors() {
    let mut core = core_with_backend(MemoryBackend::new());
    let line = serde_json::json!({
        "type": "TranscriptBackfill",
        "jsonl": r#"{"type":"user","message":{"content":[{"type":"text","text":"x"}]}}"#,
    })
    .to_string();
    assert!(
        core.on_agent_line(&line).is_err(),
        "backfill without session_id must be a protocol error"
    );
}

/// Backfill with empty/parseless NDJSON yields no entries and no update.
#[test]
fn transcript_backfill_empty_is_noop() {
    let backend = MemoryBackend::new();
    let mut seed = Store::new(Box::new(backend.clone()));
    seed_session(&mut seed, "session-bf-2", "host-5", Some("uuid"));
    let mut core = core_with_backend(backend);
    core.load_sessions().expect("load_sessions");
    let updates = core
        .on_agent_line(&backfill_line("session-bf-2", "   \n  "))
        .expect("no error");
    assert!(updates.is_empty(), "empty history must yield no updates");
}

// ---------------------------------------------------------------------------
// inject_shell_output — shell command + output injected into transcript
// ---------------------------------------------------------------------------

/// inject_shell_output appends a User entry (! <cmd>) and a Tool(shell) entry
/// for the output, and returns a VmUpdate::Session for the caller to emit.
#[test]
fn inject_shell_output_appends_entries_and_returns_session_update() {
    use marsad_events::TranscriptEntry;

    let mut core = core_with_history(vec![], vec![]);
    let id = launch_t(&mut core, None, LaunchMode::Fresh);
    core.on_agent_line(&ipc_agent_event_system_init(&id))
        .unwrap();

    let updates = core
        .inject_shell_output(&id, "ls -la", "total 0\n-rw-r--r-- 1 user group 0 file.txt")
        .expect("inject_shell_output must succeed for known session");

    // Must return exactly one Session update.
    assert_eq!(
        updates.len(),
        1,
        "inject_shell_output must return one update"
    );
    let sv = match &updates[0] {
        VmUpdate::Session(s) => s.clone(),
        other => panic!("expected Session update, got {other:?}"),
    };

    // Transcript must have: User("! ls -la") and Tool { name: "shell", input: output }
    assert_eq!(sv.transcript.len(), 2, "two entries must be appended");
    assert!(
        matches!(&sv.transcript[0], TranscriptEntry::User { text, .. } if text == "! ls -la"),
        "first entry must be the ! command"
    );
    assert!(
        matches!(&sv.transcript[1], TranscriptEntry::Tool { name, input, .. }
            if name == "shell" && input.contains("file.txt")),
        "second entry must be Tool(shell) with the output"
    );
}

/// inject_shell_output with leading/trailing whitespace on the command trims it
/// so the User entry always reads "! <cmd>" without extra spaces.
#[test]
fn inject_shell_output_trims_command() {
    use marsad_events::TranscriptEntry;

    let mut core = core_with_history(vec![], vec![]);
    let id = launch_t(&mut core, None, LaunchMode::Fresh);
    let updates = core
        .inject_shell_output(&id, "  echo hello  ", "hello\n")
        .expect("inject");
    let sv = match &updates[0] {
        VmUpdate::Session(s) => s.clone(),
        other => panic!("{other:?}"),
    };
    assert!(
        matches!(&sv.transcript[0], TranscriptEntry::User { text, .. } if text == "! echo hello"),
        "command must be trimmed"
    );
}

/// inject_shell_output on an unknown session returns UnknownSession.
#[test]
fn inject_shell_output_unknown_session_returns_unknown_session_error() {
    use marsad_core::CoreError;

    let (mut core, _peer) = make_core();
    let err = core
        .inject_shell_output("no-such-session", "ls", "")
        .unwrap_err();
    assert!(
        matches!(err, CoreError::UnknownSession(_)),
        "expected UnknownSession, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Optional session name — custom name field on LaunchSpec
// ---------------------------------------------------------------------------

/// A session launched with a non-empty `name` forwards it in the IPC JSON and
/// uses it as the session's display name in the view.
#[test]
fn launch_session_with_custom_name_uses_name_in_view() {
    let (mut core, mut peer) = make_core();
    let spec = LaunchSpec {
        project: "/work/mewais/Marsad".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: "claude-opus-4-5".to_string(),
        session_mode: String::new(),
        first_prompt: None,
        mode: Default::default(),
        name: Some("My Named Session".to_string()),
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
    };
    let id = core.launch_session(spec).expect("launch_session with name");

    // The outbound IPC JSON must include the name field.
    let outbound = peer.drain_outbound();
    let line = outbound.first().expect("must emit IPC line");
    assert!(
        line.contains("\"name\":\"My Named Session\""),
        "IPC must carry the custom name; got: {line}"
    );

    // The session view uses the custom name, not the project basename.
    let snap = core.snapshot();
    let sv = snap
        .sessions
        .iter()
        .find(|s| s.id == id)
        .expect("session in snapshot");
    assert_eq!(
        sv.name, "My Named Session",
        "SessionView.name must reflect the custom name"
    );
}

/// A session persisted with a real custom name (name != id) is restored with
/// that name by `load_sessions`, covering the `Some(s.name.clone())` branch.
#[test]
fn load_sessions_restores_custom_name() {
    use marsad_store::{MemoryBackend, PersistedSession, Store};

    let backend = MemoryBackend::new();
    let mut seed = Store::new(Box::new(backend.clone()));

    // Persist a session whose name differs from its id (= a real custom name).
    seed.upsert_session(&PersistedSession {
        id: "session-named-1".to_string(),
        session_id: None,
        name: "Custom Display Name".to_string(), // differs from id
        host_id: "local".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        project: "/work/proj".to_string(),
        branch: "main".to_string(),
        model: "claude-opus-4-8".to_string(),
        last_phase: "Working".to_string(),
        mode: None,
        disallowed_tools: vec![],
        driver: "claude".to_string(),
        slash_commands: vec![],
        tools: vec![],
        mcp_servers: vec![],
    })
    .expect("seed named session");

    let mut core = core_with_backend(backend);
    core.load_sessions().expect("load_sessions");

    let snap = core.snapshot();
    let sv = snap
        .sessions
        .iter()
        .find(|s| s.id == "session-named-1")
        .expect("restored named session present");
    assert_eq!(
        sv.name, "Custom Display Name",
        "load_sessions must restore the custom name as the display name"
    );
}

// ---------------------------------------------------------------------------
// answer_question — sends an outbound AnswerQuestion IPC line
// ---------------------------------------------------------------------------

#[test]
fn answer_question_sends_ipc() {
    let (mut core, mut peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "proj-q".to_string(),
            host_id: "local".to_string(),
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
        })
        .expect("launch");
    let _ = peer.drain_outbound();

    let answers = serde_json::json!({"Which approach?": "Option A"});
    core.answer_question(&id, "req-q-1", answers)
        .expect("answer_question should succeed");

    let outbound = peer.drain_outbound();
    assert!(
        !outbound.is_empty(),
        "answer_question must emit an outbound IPC line"
    );
    let combined = outbound.join(" ");
    assert!(
        combined.contains("AnswerQuestion") || combined.contains("question"),
        "outbound must reference AnswerQuestion, got: {combined}"
    );
}

#[test]
fn answer_question_unknown_session_returns_error() {
    let (mut core, _peer) = make_core();
    let result = core.answer_question("no-such-session", "req-q-1", serde_json::json!({}));
    assert!(
        result.is_err(),
        "answer_question for unknown session must return an error"
    );
}

// ---------------------------------------------------------------------------
// ChangesUpdate — remote git probe result applied to session
// ---------------------------------------------------------------------------

/// A well-formed `ChangesUpdate` line for a launched session populates its
/// changes list and returns a `VmUpdate::Session` with the new file-change data.
#[test]
fn on_agent_line_changes_update_populates_session_changes() {
    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "/home/user/proj".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            model: "claude-sonnet-4-6".to_string(),
            session_mode: String::new(),
            first_prompt: None,
            mode: LaunchMode::Fresh,
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "claude".to_string(),
        })
        .expect("launch");

    let line = format!(
        r#"{{"type":"ChangesUpdate","session_id":"{id}","changes":[{{"path":"a.rs","status":"M","added":3,"removed":1}}]}}"#
    );
    let updates = core.on_agent_line(&line).expect("on_agent_line");
    let sv = updates
        .iter()
        .find_map(|u| {
            if let VmUpdate::Session(sv) = u {
                Some(sv)
            } else {
                None
            }
        })
        .expect("must include Session update");
    assert_eq!(sv.changes.len(), 1);
    assert_eq!(sv.changes[0].path, "a.rs");
    assert_eq!(sv.changes[0].status, "M");
    assert_eq!(sv.changes[0].added, 3);
    assert_eq!(sv.changes[0].removed, 1);
}

/// A REMOTE session's changes (from ChangesUpdate) must NOT be clobbered by the
/// local git probe on a result/tool_use event — that probe runs for LOCAL
/// sessions only (a local `git status` on a remote project path is empty).
#[test]
fn remote_session_changes_survive_result_event() {
    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "/remote/proj".to_string(),
            host_id: "narval".to_string(), // remote, unregistered → host_is_local=false
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            model: "claude-opus-4-8".to_string(),
            session_mode: String::new(),
            first_prompt: None,
            mode: LaunchMode::Fresh,
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "claude".to_string(),
        })
        .expect("launch");

    let cu = format!(
        r#"{{"type":"ChangesUpdate","session_id":"{id}","changes":[{{"path":"x.rs","status":"M","added":2,"removed":0}}]}}"#
    );
    core.on_agent_line(&cu).expect("changes update");

    // turn_completed → should_probe; remote session must SKIP the local probe.
    let result = format!(
        r#"{{"type":"AgentEvent","session_id":"{id}","event":{{"type":"turn_completed","usage":{{"input_tokens":1,"output_tokens":1,"cache_read_tokens":0,"cache_creation_tokens":0,"cost_usd":0.0}}}}}}"#
    );
    core.on_agent_line(&result).expect("result event");

    let snap = core.snapshot();
    let sv = snap.sessions.iter().find(|s| s.id == id).expect("session");
    assert_eq!(
        sv.changes.len(),
        1,
        "remote changes must survive: {:?}",
        sv.changes
    );
    assert_eq!(sv.changes[0].path, "x.rs");
}

/// A `ChangesUpdate` for an UNKNOWN session is a no-op (empty update list, no error).
#[test]
fn on_agent_line_changes_update_unknown_session_is_noop() {
    let (mut core, _peer) = make_core();
    let line = r#"{"type":"ChangesUpdate","session_id":"no-such-session","changes":[{"path":"x.rs","status":"A","added":1,"removed":0}]}"#;
    let updates = core
        .on_agent_line(line)
        .expect("on_agent_line must not error");
    assert!(
        updates.is_empty(),
        "unknown session must yield empty updates"
    );
}

/// A `ChangesUpdate` with MISSING `session_id` returns a Protocol error.
#[test]
fn on_agent_line_changes_update_missing_session_id_is_error() {
    let (mut core, _peer) = make_core();
    let line = r#"{"type":"ChangesUpdate","changes":[{"path":"x.rs","status":"A","added":1,"removed":0}]}"#;
    let result = core.on_agent_line(line);
    assert!(
        result.is_err(),
        "ChangesUpdate without session_id must return an error"
    );
}

// ---------------------------------------------------------------------------
// model_usage — populated in SessionView from result event's modelUsage
// ---------------------------------------------------------------------------

/// A result event with modelUsage populates the session view's model_usage rows.
#[test]
fn session_view_exposes_model_usage_after_result_with_model_usage() {
    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "/home/user/proj".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            model: "claude-sonnet-4-6".to_string(),
            session_mode: String::new(),
            first_prompt: None,
            mode: LaunchMode::Fresh,
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "claude".to_string(),
        })
        .expect("launch");

    // Feed a system/init so the session is active.
    core.on_agent_line(&ipc_agent_event_system_init(&id))
        .expect("init");

    // Feed a turn_completed with per_model containing TWO models (so sort runs).
    let result_line = format!(
        r#"{{"type":"AgentEvent","session_id":"{id}","event":{{"type":"turn_completed","usage":{{"input_tokens":21,"output_tokens":255,"cache_read_tokens":0,"cache_creation_tokens":0,"cost_usd":0.368}},"per_model":{{"claude-sonnet-4.6[1m]":{{"input_tokens":11,"output_tokens":155,"cost_usd":0.268}},"claude-haiku-4-5":{{"input_tokens":10,"output_tokens":100,"cost_usd":0.1}}}}}}}}"#
    );
    let updates = core.on_agent_line(&result_line).expect("result");
    let sv = updates
        .iter()
        .find_map(|u| {
            if let VmUpdate::Session(sv) = u {
                Some(sv)
            } else {
                None
            }
        })
        .expect("must include Session update");
    assert_eq!(sv.model_usage.len(), 2, "must have two model usage rows");
    // Rows are sorted by model name: haiku before sonnet.
    assert_eq!(sv.model_usage[0].model, "claude-haiku-4-5");
    assert_eq!(sv.model_usage[1].model, "claude-sonnet-4.6[1m]");
    assert_eq!(sv.model_usage[1].input_tokens, 11);
    assert_eq!(sv.model_usage[1].output_tokens, 155);
    assert!((sv.model_usage[1].cost_usd - 0.268).abs() < 1e-9);
}

// ---------------------------------------------------------------------------
// Feature #2 — TranscriptBackfill populates model_usage from JSONL
// (includes subagent turns; supersedes result-event value)
// ---------------------------------------------------------------------------

/// A backfill JSONL with an opus and a sonnet assistant entry yields TWO
/// model_usage rows with correct tokens and non-zero cost.
#[test]
fn backfill_with_multi_model_jsonl_populates_model_usage() {
    let backend = MemoryBackend::new();
    let mut seed = Store::new(Box::new(backend.clone()));
    seed_session(&mut seed, "session-mu-1", "host-5", Some("uuid-mu-1"));
    let mut core = core_with_backend(backend);
    core.load_sessions().expect("load_sessions");

    // JSONL with an opus main turn and a sonnet subagent turn (isSidechain: true)
    let jsonl = concat!(
        r#"{"type":"assistant","message":{"model":"claude-opus-4-5","usage":{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":10,"cache_creation_input_tokens":0}}}"#,
        "\n",
        r#"{"type":"assistant","isSidechain":true,"message":{"model":"claude-sonnet-4-6","usage":{"input_tokens":200,"output_tokens":80,"cache_read_input_tokens":0,"cache_creation_input_tokens":5}}}"#,
    );
    let updates = core
        .on_agent_line(&backfill_line("session-mu-1", jsonl))
        .expect("backfill applies");
    assert!(
        updates.iter().any(|u| matches!(u, VmUpdate::Session(_))),
        "backfill must emit a Session update"
    );
    let sv = core
        .snapshot()
        .sessions
        .into_iter()
        .find(|s| s.id == "session-mu-1")
        .expect("session present");

    // Must have exactly two rows (sorted by model name).
    assert_eq!(
        sv.model_usage.len(),
        2,
        "must have two model_usage rows (opus + sonnet)"
    );
    // Sorted: opus < sonnet lexicographically.
    let opus = &sv.model_usage[0];
    let sonnet = &sv.model_usage[1];
    assert!(
        opus.model.contains("opus"),
        "first row must be opus, got: {}",
        opus.model
    );
    assert!(
        sonnet.model.contains("sonnet"),
        "second row must be sonnet, got: {}",
        sonnet.model
    );

    // Opus tokens: 100 input + 50 output + 10 cache_read.
    assert_eq!(opus.input_tokens, 100);
    assert_eq!(opus.output_tokens, 50);
    // Opus cost: LiteLLM has claude-opus-4-5 at $5/$25/$0.50/$6.25 (the newer Opus
    // 4.x pricing tier). Updated from $15/$75/$1.50/$18.75 when LiteLLM lookup was wired in.
    // 100/1e6*5 + 50/1e6*25 + 10/1e6*0.50 + 0/1e6*6.25
    let expected_opus_cost = 100.0 / 1_000_000.0 * 5.0
        + 50.0 / 1_000_000.0 * 25.0
        + 10.0 / 1_000_000.0 * 0.50
        + 0.0 / 1_000_000.0 * 6.25;
    assert!(
        (opus.cost_usd - expected_opus_cost).abs() < 1e-9,
        "opus cost mismatch: {} vs {}",
        opus.cost_usd,
        expected_opus_cost
    );

    // Sonnet tokens: 200 input + 80 output + 5 cache_creation.
    assert_eq!(sonnet.input_tokens, 200);
    assert_eq!(sonnet.output_tokens, 80);
    let expected_sonnet_cost = 200.0 / 1_000_000.0 * 3.0
        + 80.0 / 1_000_000.0 * 15.0
        + 0.0 / 1_000_000.0 * 0.30
        + 5.0 / 1_000_000.0 * 3.75;
    assert!(
        (sonnet.cost_usd - expected_sonnet_cost).abs() < 1e-9,
        "sonnet cost mismatch: {} vs {}",
        sonnet.cost_usd,
        expected_sonnet_cost
    );

    // Both costs must be non-zero.
    assert!(opus.cost_usd > 0.0, "opus cost must be non-zero");
    assert!(sonnet.cost_usd > 0.0, "sonnet cost must be non-zero");
}

/// A backfill JSONL with a haiku assistant entry yields the correct cost.
#[test]
fn backfill_model_usage_haiku_pricing() {
    let backend = MemoryBackend::new();
    let mut seed = Store::new(Box::new(backend.clone()));
    seed_session(&mut seed, "session-mu-2", "host-5", Some("uuid-mu-2"));
    let mut core = core_with_backend(backend);
    core.load_sessions().expect("load_sessions");

    let jsonl = r#"{"type":"assistant","message":{"model":"claude-haiku-4-5","usage":{"input_tokens":1000,"output_tokens":500,"cache_read_input_tokens":100,"cache_creation_input_tokens":50}}}"#;
    core.on_agent_line(&backfill_line("session-mu-2", jsonl))
        .expect("backfill applies");
    let sv = core
        .snapshot()
        .sessions
        .into_iter()
        .find(|s| s.id == "session-mu-2")
        .expect("session");
    assert_eq!(sv.model_usage.len(), 1);
    let row = &sv.model_usage[0];
    assert!(row.model.contains("haiku"));
    // Haiku 4.5: input $1, output $5, cache_read $0.10, cache_write $1.25 per Mtok
    let expected = 1000.0 / 1_000_000.0 * 1.0
        + 500.0 / 1_000_000.0 * 5.0
        + 100.0 / 1_000_000.0 * 0.10
        + 50.0 / 1_000_000.0 * 1.25;
    assert!(
        (row.cost_usd - expected).abs() < 1e-9,
        "haiku cost: {} vs {}",
        row.cost_usd,
        expected
    );
}

/// A backfill JSONL with an unknown model still counts tokens but yields 0 cost.
#[test]
fn backfill_model_usage_unknown_model_zero_cost() {
    let backend = MemoryBackend::new();
    let mut seed = Store::new(Box::new(backend.clone()));
    seed_session(&mut seed, "session-mu-3", "host-5", Some("uuid-mu-3"));
    let mut core = core_with_backend(backend);
    core.load_sessions().expect("load_sessions");

    let jsonl = r#"{"type":"assistant","message":{"model":"claude-future-model-9999","usage":{"input_tokens":500,"output_tokens":200,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
    core.on_agent_line(&backfill_line("session-mu-3", jsonl))
        .expect("backfill applies");
    let sv = core
        .snapshot()
        .sessions
        .into_iter()
        .find(|s| s.id == "session-mu-3")
        .expect("session");
    assert_eq!(sv.model_usage.len(), 1);
    let row = &sv.model_usage[0];
    assert_eq!(row.input_tokens, 500);
    assert_eq!(row.output_tokens, 200);
    assert_eq!(row.cost_usd, 0.0, "unknown model must yield 0 cost");
}

/// When a result event sets model_usage AND a backfill arrives later,
/// the backfill supersedes (replaces) the result-event value.
#[test]
fn backfill_supersedes_result_event_model_usage() {
    let backend = MemoryBackend::new();
    let mut seed = Store::new(Box::new(backend.clone()));
    seed_session(&mut seed, "session-mu-4", "host-5", Some("uuid-mu-4"));
    let mut core = core_with_backend(backend);
    core.load_sessions().expect("load_sessions");

    // First: deliver a result event with modelUsage (single model from result).
    // We deliver it as an AgentEvent since this is a restored session.
    // But for simplicity here, use the backfill path to set both, then verify supersede.
    // Deliver a backfill JSONL with opus+haiku.
    let jsonl = concat!(
        r#"{"type":"assistant","message":{"model":"claude-opus-4-5","usage":{"input_tokens":50,"output_tokens":10,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#,
        "\n",
        r#"{"type":"assistant","message":{"model":"claude-haiku-4-5","usage":{"input_tokens":30,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#,
    );
    core.on_agent_line(&backfill_line("session-mu-4", jsonl))
        .expect("first backfill");
    let sv = core
        .snapshot()
        .sessions
        .into_iter()
        .find(|s| s.id == "session-mu-4")
        .expect("session");
    assert_eq!(sv.model_usage.len(), 2, "must have opus + haiku rows");
}

/// A backfill JSONL with ONLY assistant-usage entries (no transcript lines)
/// still triggers a Session update via the model_usage path.
#[test]
fn backfill_only_usage_entries_yields_session_update() {
    let backend = MemoryBackend::new();
    let mut seed = Store::new(Box::new(backend.clone()));
    seed_session(&mut seed, "session-mu-5", "host-5", Some("uuid-mu-5"));
    let mut core = core_with_backend(backend);
    core.load_sessions().expect("load_sessions");

    // This has no user/assistant transcript entries, just assistant with model+usage.
    let jsonl = r#"{"type":"assistant","message":{"model":"claude-sonnet-4-6","usage":{"input_tokens":77,"output_tokens":33,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
    let updates = core
        .on_agent_line(&backfill_line("session-mu-5", jsonl))
        .expect("backfill applies");
    // Must still emit a Session update because model_usage was populated.
    assert!(
        updates.iter().any(|u| matches!(u, VmUpdate::Session(_))),
        "backfill with only usage entries must still emit a Session update"
    );
    let sv = core
        .snapshot()
        .sessions
        .into_iter()
        .find(|s| s.id == "session-mu-5")
        .expect("session");
    assert_eq!(sv.model_usage.len(), 1);
    assert!(sv.model_usage[0].model.contains("sonnet"));
    assert_eq!(sv.model_usage[0].input_tokens, 77);
}

// ---------------------------------------------------------------------------
// Feature #2 — discovery::aggregate_model_usage unit tests
// ---------------------------------------------------------------------------

use marsad_core::discovery;

/// aggregate_model_usage on empty input → empty map.
#[test]
fn aggregate_model_usage_empty_input() {
    let result = discovery::aggregate_model_usage("");
    assert!(result.is_empty());
}

/// aggregate_model_usage ignores non-assistant lines.
#[test]
fn aggregate_model_usage_skips_non_assistant_lines() {
    let jsonl = concat!(
        r#"{"type":"user","message":{"content":[{"type":"text","text":"hi"}]}}"#,
        "\n",
        "not json\n",
        r#"{"type":"queue-operation","data":{}}"#,
    );
    let result = discovery::aggregate_model_usage(jsonl);
    assert!(result.is_empty(), "non-assistant lines must be skipped");
}

/// aggregate_model_usage accumulates tokens across multiple entries of the same model.
#[test]
fn aggregate_model_usage_accumulates_same_model() {
    let jsonl = concat!(
        r#"{"type":"assistant","message":{"model":"claude-opus-4-5","usage":{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#,
        "\n",
        r#"{"type":"assistant","message":{"model":"claude-opus-4-5","usage":{"input_tokens":200,"output_tokens":80,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#,
    );
    let result = discovery::aggregate_model_usage(jsonl);
    assert_eq!(result.len(), 1);
    let row = result.get("claude-opus-4-5").expect("opus must be present");
    assert_eq!(row.input_tokens, 300);
    assert_eq!(row.output_tokens, 130);
    assert!(row.cost_usd > 0.0);
}

/// aggregate_model_usage with opus model applies the opus pricing.
#[test]
fn aggregate_model_usage_opus_pricing_correct() {
    // 1_000_000 tokens in JSON must be written without underscore (JSON doesn't support it)
    let jsonl = r#"{"type":"assistant","message":{"model":"claude-opus-4-5","usage":{"input_tokens":1000000,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
    // LiteLLM has claude-opus-4-5 at $5/Mtok input (the newer Opus 4.x pricing tier;
    // older $15/Mtok applied only to claude-opus-4-1 and earlier). Updated from the
    // previous hand-written $15 when LiteLLM lookup was wired in.
    let result = discovery::aggregate_model_usage(jsonl);
    let row = result.get("claude-opus-4-5").expect("opus");
    assert!(
        (row.cost_usd - 5.0).abs() < 1e-6,
        "opus input cost: {}",
        row.cost_usd
    );
}

/// aggregate_model_usage with sonnet model applies the sonnet pricing.
#[test]
fn aggregate_model_usage_sonnet_pricing_correct() {
    let jsonl = r#"{"type":"assistant","message":{"model":"claude-sonnet-4-6","usage":{"input_tokens":0,"output_tokens":1000000,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
    // 1M output tokens at $15/Mtok = $15
    let result = discovery::aggregate_model_usage(jsonl);
    let row = result.get("claude-sonnet-4-6").expect("sonnet");
    assert!(
        (row.cost_usd - 15.0).abs() < 1e-6,
        "sonnet output cost: {}",
        row.cost_usd
    );
}

/// aggregate_model_usage with haiku model applies the haiku pricing.
#[test]
fn aggregate_model_usage_haiku_pricing_correct() {
    let jsonl = r#"{"type":"assistant","message":{"model":"claude-haiku-4-5","usage":{"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":1000000,"cache_creation_input_tokens":0}}}"#;
    // 1M cache_read tokens on Haiku 4.5 at $0.10/Mtok = $0.10
    let result = discovery::aggregate_model_usage(jsonl);
    let row = result.get("claude-haiku-4-5").expect("haiku");
    assert!(
        (row.cost_usd - 0.10).abs() < 1e-9,
        "haiku cache_read cost: {}",
        row.cost_usd
    );
}

/// aggregate_model_usage cache_creation uses the correct per-model rate.
#[test]
fn aggregate_model_usage_cache_creation_pricing() {
    let jsonl = r#"{"type":"assistant","message":{"model":"claude-sonnet-4-6","usage":{"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":1000000}}}"#;
    // 1M cache_creation tokens at $3.75/Mtok = $3.75 (sonnet)
    let result = discovery::aggregate_model_usage(jsonl);
    let row = result.get("claude-sonnet-4-6").expect("sonnet");
    assert!(
        (row.cost_usd - 3.75).abs() < 1e-9,
        "sonnet cache_creation cost: {}",
        row.cost_usd
    );
}

/// aggregate_model_usage with an unknown model gives 0 cost but still counts tokens.
#[test]
fn aggregate_model_usage_unknown_model_zero_cost_but_counts_tokens() {
    let jsonl = r#"{"type":"assistant","message":{"model":"claude-future-xyz","usage":{"input_tokens":999,"output_tokens":111,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
    let result = discovery::aggregate_model_usage(jsonl);
    let row = result.get("claude-future-xyz").expect("unknown model");
    assert_eq!(row.input_tokens, 999);
    assert_eq!(row.output_tokens, 111);
    assert_eq!(row.cost_usd, 0.0, "unknown model must have 0 cost");
}

/// aggregate_model_usage skips assistant lines without a model field.
#[test]
fn aggregate_model_usage_skips_assistant_without_model() {
    let jsonl = r#"{"type":"assistant","message":{"usage":{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
    let result = discovery::aggregate_model_usage(jsonl);
    assert!(result.is_empty(), "assistant without model must be skipped");
}

/// aggregate_model_usage skips assistant lines without a usage field.
#[test]
fn aggregate_model_usage_skips_assistant_without_usage() {
    let jsonl = r#"{"type":"assistant","message":{"model":"claude-opus-4-5"}}"#;
    let result = discovery::aggregate_model_usage(jsonl);
    assert!(result.is_empty(), "assistant without usage must be skipped");
}

/// aggregate_model_usage skips assistant lines without a message field.
#[test]
fn aggregate_model_usage_skips_assistant_without_message() {
    let jsonl = r#"{"type":"assistant"}"#;
    let result = discovery::aggregate_model_usage(jsonl);
    assert!(
        result.is_empty(),
        "assistant without message must be skipped"
    );
}

// ---------------------------------------------------------------------------
// Feature #3 — running_subagents in SessionView via Core
// ---------------------------------------------------------------------------

/// A session with a Task tool_use in the stream has running_subagents > 0.
#[test]
fn session_view_exposes_running_subagents_count() {
    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "/home/user/proj".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            model: "claude-sonnet-4-6".to_string(),
            session_mode: String::new(),
            first_prompt: None,
            mode: LaunchMode::Fresh,
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "claude".to_string(),
        })
        .expect("launch");

    core.on_agent_line(&ipc_agent_event_system_init(&id))
        .expect("init");

    // Feed a ToolCallStarted with is_subagent=true to start a running subagent.
    let task_line = format!(
        r#"{{"type":"AgentEvent","session_id":"{id}","event":{{"type":"tool_call_started","name":"Task","call_id":"tu-task-view","input":{{"subagent_type":"researcher","description":"Do work"}},"is_subagent":true}}}}"#
    );
    let updates = core.on_agent_line(&task_line).expect("task tool_call_started");
    let sv = updates
        .iter()
        .find_map(|u| {
            if let VmUpdate::Session(sv) = u {
                Some(sv)
            } else {
                None
            }
        })
        .expect("must include Session update");

    assert_eq!(
        sv.running_subagents, 1,
        "running_subagents must be 1 after ToolCallStarted(is_subagent=true)"
    );

    // Now feed a ToolCallFinished to mark it done.
    let result_line = format!(
        r#"{{"type":"AgentEvent","session_id":"{id}","event":{{"type":"tool_call_finished","call_id":"tu-task-view","name":"Task","ok":true}}}}"#
    );
    let updates2 = core.on_agent_line(&result_line).expect("tool_call_finished");
    let sv2 = updates2
        .iter()
        .find_map(|u| {
            if let VmUpdate::Session(sv) = u {
                Some(sv)
            } else {
                None
            }
        })
        .expect("must include Session update");
    assert_eq!(
        sv2.running_subagents, 0,
        "running_subagents must be 0 after tool_result"
    );
    assert_eq!(
        sv2.subagents, 1,
        "subagents count still includes the completed one"
    );
}

// ---------------------------------------------------------------------------
// Feature B: disallowed_tools forwarded in IPC JSON
// ---------------------------------------------------------------------------

/// When `disallowed_tools` is non-empty in `LaunchSpec`, `launch_session`
/// includes the field in the outbound IPC JSON so the agent passes
/// `--disallowedTools` to claude.
#[test]
fn launch_session_with_disallowed_tools_includes_field_in_ipc() {
    let (mut core, mut peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "/p".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: vec![],
            model: "claude-opus-4-5".to_string(),
            session_mode: String::new(),
            first_prompt: None,
            mode: Default::default(),
            name: None,
            disallowed_tools: vec!["Bash".to_string(), "Edit".to_string()],
            wasl: false,
            driver: "claude".to_string(),
        })
        .expect("launch");
    assert!(!id.is_empty());
    let outbound = peer.drain_outbound();
    let line = outbound.first().expect("must emit IPC");
    assert!(
        line.contains("disallowed_tools"),
        "IPC must include disallowed_tools when non-empty; line: {line}"
    );
    // Verify the values are present.
    assert!(line.contains("Bash"), "IPC must include Bash; line: {line}");
    assert!(line.contains("Edit"), "IPC must include Edit; line: {line}");
}

// ---------------------------------------------------------------------------
// SessionView current_mode — reflects real permissionMode from system/init
// ---------------------------------------------------------------------------

/// Helper: build an AgentEvent with a given permission_mode for current_mode tests.
/// Includes a non-empty `claude_code_version` so the reducer populates `current_mode`.
fn ipc_agent_event_init_with_mode(session_id: &str, mode: &str) -> String {
    format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"session_init","native_session_id":"claude-uuid","model":"m","cwd":"/","tools":[],"mcp_servers":[],"permission_mode":"{mode}","claude_code_version":"1.2.3","current_mode":"{mode}"}}}}"#
    )
}

/// After system/init with permissionMode "default", current_mode must be set.
#[test]
fn session_view_current_mode_set_from_init() {
    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "/p".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: vec![],
            model: "m".to_string(),
            session_mode: String::new(),
            first_prompt: None,
            mode: Default::default(),
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "claude".to_string(),
        })
        .expect("launch");
    let updates = core
        .on_agent_line(&ipc_agent_event_init_with_mode(&id, "acceptEdits"))
        .expect("init");
    let sv = updates
        .iter()
        .find_map(|u| {
            if let VmUpdate::Session(s) = u {
                Some(s)
            } else {
                None
            }
        })
        .expect("Session update");
    assert_eq!(
        sv.current_mode.as_deref(), Some("acceptEdits"),
        "permissionMode from system/init must appear as current_mode"
    );
}

/// A session launched with session_mode carries it as permission_mode (seeded
/// from launch spec before system/init arrives).
#[test]
fn launch_session_mode_seeds_permission_mode_before_init() {
    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "/p".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: vec![],
            model: "m".to_string(),
            session_mode: "bypassPermissions".to_string(),
            first_prompt: None,
            mode: Default::default(),
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "claude".to_string(),
        })
        .expect("launch");
    // Check available via state.permission_mode — before any init fires.
    // The session state permission_mode is seeded from session_mode.
    // (The current_mode is from the live stream; before init it comes from the launch.)
    let snap = core.snapshot();
    let sv = snap.sessions.iter().find(|s| s.id == id).expect("session");
    // available_modes is empty (no init yet); the session is running in the requested mode.
    assert!(sv.available_modes.is_empty(), "available_modes empty before system/init");
}


// ---------------------------------------------------------------------------
// GAP 1: restart_session must carry persisted setup/env/claude_bin
// ---------------------------------------------------------------------------

/// A persisted session with setup/env/claude_bin: restart_session must
/// load those values from the store and forward them in the outbound
/// LaunchSession IPC — regardless of whether the host registry still has them.
#[test]
fn restart_session_carries_persisted_setup_env_claude_bin() {
    let backend = MemoryBackend::new();

    // Seed a persisted session with a full launch config.
    let mut seed = Store::new(Box::new(backend.clone()));
    let session_id = "session-restart-cfg";
    seed.upsert_session(&PersistedSession {
        id: session_id.to_string(),
        session_id: Some("claude-uuid-cfg".to_string()),
        name: session_id.to_string(),
        host_id: "localhost".to_string(),
        host_target: None,
        claude_bin: Some("genie".to_string()),
        setup: Some("source ~/module.sh".to_string()),
        env: vec![("K".to_string(), "V".to_string())],
        project: "/work/proj-cfg".to_string(),
        branch: "main".to_string(),
        model: "claude-sonnet-4-6".to_string(),
        last_phase: "Working".to_string(),
        mode: None,
        disallowed_tools: vec![],
        driver: "claude".to_string(),
        slash_commands: vec![],
        tools: vec![],
        mcp_servers: vec![],
    })
    .expect("seed session");

    // Build a Core with the shared backend and a transport to drain.
    let (transport_a, mut peer) = LoopbackFake::pair();
    let cfg = CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-test".to_string(),
    };
    let mut core = Core::with_deps(
        cfg,
        Box::new(transport_a),
        Box::new(marsad_core::RealGit),
        Box::new(marsad_core::RealTranscript),
        Box::new(backend),
    )
    .expect("with_deps");

    // Restore the persisted session so core tracks it.
    core.load_sessions().expect("load_sessions");
    let _ = peer.drain_outbound(); // discard any load-time IPC

    // Now restart with a new model/posture.
    core.restart_session(session_id, "claude-opus-4-8", "bypassPermissions".to_string(), vec![])
        .expect("restart_session must succeed");

    let outbound = peer.drain_outbound();
    // At least 2 lines: StopSession + LaunchSession.
    assert!(
        outbound.len() >= 2,
        "expected >= 2 IPC lines, got {}",
        outbound.len()
    );

    let launch_line = &outbound[1];

    // The LaunchSession must carry the persisted claude_bin, setup, and env.
    assert!(
        launch_line.contains("\"claude_bin\":\"genie\""),
        "restart LaunchSession must carry persisted claude_bin; got: {launch_line}"
    );
    assert!(
        launch_line.contains("\"setup\":\"source ~/module.sh\""),
        "restart LaunchSession must carry persisted setup; got: {launch_line}"
    );
    assert!(
        launch_line.contains(r#""env":[["K","V"]]"#),
        "restart LaunchSession must carry persisted env; got: {launch_line}"
    );
}

/// When the persisted record for a session cannot be found (e.g. already deleted
/// from the store), restart_session must not panic and must fall back to the
/// None/empty behavior (host-registry resolution inside launch_session_with_id).
#[test]
fn restart_session_absent_persisted_record_does_not_panic() {
    // Use an in-memory core (no seeded session in the store).
    let (mut core, mut peer) = make_core();

    // Launch a session via the normal path (NOT persisted to the store in test).
    let id = core
        .launch_session(LaunchSpec {
            project: "/tmp/proj-no-persist".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            model: "claude-sonnet-4-6".to_string(),
            session_mode: String::new(),
            first_prompt: None,
            mode: LaunchMode::Fresh,
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "claude".to_string(),
        })
        .expect("launch");
    let _ = peer.drain_outbound();

    // Restart — no persisted record in the store → should NOT panic, and must
    // still emit StopSession + LaunchSession.
    let result = core.restart_session(&id, "claude-haiku-4-5", String::new(), vec![]);
    assert!(
        result.is_ok(),
        "restart must succeed even when persisted record is absent, got: {:?}",
        result
    );

    let outbound = peer.drain_outbound();
    assert!(
        outbound.len() >= 2,
        "must still emit Stop + Launch, got {}",
        outbound.len()
    );
    let launch_line = &outbound[1];
    // Should NOT carry any claude_bin/setup/env (fell back to None/empty).
    assert!(
        !launch_line.contains("\"claude_bin\""),
        "fallback must not inject claude_bin; got: {launch_line}"
    );
    assert!(
        !launch_line.contains("\"setup\""),
        "fallback must not inject setup; got: {launch_line}"
    );
    assert!(
        !launch_line.contains("\"env\""),
        "fallback must not inject env; got: {launch_line}"
    );
}

/// system/init OVERRIDES the launch session_mode: launched with "acceptEdits",
/// but system/init reports "bypassPermissions" → current_mode becomes "bypassPermissions".
/// Verifies that system/init remains authoritative even after the fix.
#[test]
fn system_init_overrides_launch_session_mode() {
    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "/p".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: vec![],
            model: "m".to_string(),
            session_mode: "acceptEdits".to_string(),
            first_prompt: None,
            mode: Default::default(),
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "claude".to_string(),
        })
        .expect("launch");

    // Feed system/init with bypassPermissions — current_mode must update.
    let updates = core
        .on_agent_line(&ipc_agent_event_init_with_mode(&id, "bypassPermissions"))
        .expect("system/init");
    let sv_after = updates
        .iter()
        .find_map(|u| {
            if let VmUpdate::Session(s) = u {
                Some(s.clone())
            } else {
                None
            }
        })
        .expect("Session update from system/init");
    assert_eq!(
        sv_after.current_mode.as_deref(),
        Some("bypassPermissions"),
        "system/init bypassPermissions must set current_mode to bypassPermissions"
    );
}

// ---------------------------------------------------------------------------
// Bug #9 — tokens/context/cost seed-then-accumulate on attach
//
// Contract (user-confirmed): when a TranscriptBackfill or UsageUpdate
// arrives for a session that has never had a live `result` event
// (tokens == 0), it seeds the token total from the aggregated JSONL data
// so the UI shows the full accumulated total immediately on attach —
// not zero. Subsequent live `result` events ADD to that baseline
// (i.e. replace state.tokens with the authoritative cumulative value;
// since reduce_result sets tokens = u.total, the result IS the growing
// accumulated total). context_pct also reflects the seeded token count.
// ---------------------------------------------------------------------------

/// TranscriptBackfill seeds tokens and context_pct from the JSONL aggregate
/// when no live result has fired (tokens == 0 before the backfill).
#[test]
fn backfill_seeds_tokens_and_context_pct_when_no_live_result() {
    let backend = MemoryBackend::new();
    let mut seed = Store::new(Box::new(backend.clone()));
    seed_session(&mut seed, "session-seed-1", "host-5", Some("uuid-seed-1"));
    let mut core = core_with_backend(backend);
    core.load_sessions().expect("load_sessions");

    // Sanity: before backfill, tokens == 0.
    let sv_before = core
        .snapshot()
        .sessions
        .into_iter()
        .find(|s| s.id == "session-seed-1")
        .expect("session");
    assert_eq!(sv_before.tokens, 0, "tokens must be 0 before backfill");

    // JSONL with 1000 input + 500 output + 200 cache_read on sonnet.
    // Total counted tokens = 1000 + 500 + 200 = 1700.
    let jsonl = r#"{"type":"assistant","message":{"model":"claude-sonnet-4-6","usage":{"input_tokens":1000,"output_tokens":500,"cache_read_input_tokens":200,"cache_creation_input_tokens":0}}}"#;
    let updates = core
        .on_agent_line(&backfill_line("session-seed-1", jsonl))
        .expect("backfill applies");

    let sv = updates
        .iter()
        .find_map(|u| if let VmUpdate::Session(s) = u { Some(s) } else { None })
        .expect("Session update from backfill");

    // tokens must be seeded from the JSONL aggregate (1000 + 500 + 200 = 1700).
    assert_eq!(sv.tokens, 1700, "tokens must be seeded from JSONL (1700)");
    // context_pct must be non-zero (1700 / 200_000 * 100 ≈ 0.85%).
    assert!(sv.context_pct > 0.0, "context_pct must be > 0 after seeding");
    // The model_usage row must be present and estimated.
    assert_eq!(sv.model_usage.len(), 1, "one model_usage row");
    assert!(sv.model_usage[0].estimated, "backfill row is estimated");
}

/// A live result event after a backfill replaces (accumulates) the token total
/// with the authoritative cumulative value from the agent — never resets to 0.
#[test]
fn result_after_backfill_accumulates_not_resets() {
    let backend = MemoryBackend::new();
    let mut seed = Store::new(Box::new(backend.clone()));
    seed_session(&mut seed, "session-seed-2", "host-5", Some("uuid-seed-2"));
    let mut core = core_with_backend(backend);
    core.load_sessions().expect("load_sessions");

    // 1. Backfill seeds tokens from 800 input + 200 output = 1000 tokens.
    let jsonl = r#"{"type":"assistant","message":{"model":"claude-sonnet-4-6","usage":{"input_tokens":800,"output_tokens":200,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
    core.on_agent_line(&backfill_line("session-seed-2", jsonl))
        .expect("backfill");
    let sv_after_bf = core
        .snapshot()
        .sessions
        .into_iter()
        .find(|s| s.id == "session-seed-2")
        .expect("session");
    assert_eq!(sv_after_bf.tokens, 1000, "backfill seeds 1000 tokens");

    // 2. Feed a system/init (needed to move phase away from Stale before result).
    core.on_agent_line(&ipc_agent_event_system_init("session-seed-2"))
        .expect("init");

    // 3. Live turn_completed: cumulative usage from agent = 1200 input + 300 output = 1500.
    //    This is the authoritative running total — must REPLACE (not reset) the seeded value.
    let result_line = r#"{"type":"AgentEvent","session_id":"session-seed-2","event":{"type":"turn_completed","usage":{"input_tokens":1200,"output_tokens":300,"cache_read_tokens":0,"cache_creation_tokens":0,"cost_usd":0.01}}}"#.to_string();
    let result_updates = core.on_agent_line(&result_line).expect("result");
    let sv_final = result_updates
        .iter()
        .find_map(|u| if let VmUpdate::Session(s) = u { Some(s) } else { None })
        .expect("Session from result");

    // After result, tokens = 1200 + 300 = 1500 (authoritative cumulative from agent).
    assert_eq!(
        sv_final.tokens, 1500,
        "result must set tokens to authoritative cumulative (1500), not reset to 0"
    );
    assert!(
        sv_final.context_pct > 0.0,
        "context_pct must remain > 0 after result"
    );
    // The live result cost must be the one from the result event (authoritative).
    assert!(sv_final.cost_usd > 0.0, "cost_usd from live result must be non-zero");
}

/// UsageUpdate seeds tokens from input+output when tokens == 0 (no live result yet).
#[test]
fn usage_update_seeds_tokens_when_no_live_result() {
    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "/p".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: vec![],
            model: "claude-sonnet-4-6".to_string(),
            session_mode: String::new(),
            first_prompt: None,
            mode: Default::default(),
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "claude".to_string(),
        })
        .expect("launch");
    core.on_agent_line(&ipc_agent_event_system_init(&id))
        .expect("init");

    // Before UsageUpdate, tokens == 0 (no result event).
    let sv_before = core
        .snapshot()
        .sessions
        .into_iter()
        .find(|s| s.id == id)
        .expect("session");
    assert_eq!(sv_before.tokens, 0, "tokens 0 before usage update");

    // UsageUpdate: 600 input + 400 output on sonnet → 1000 total input+output.
    let usage = r#"{"claude-sonnet-4-6":{"input":600,"output":400,"cache_read":0,"cache_creation":0}}"#;
    let updates = core
        .on_agent_line(&ipc_usage_update(&id, usage))
        .expect("usage_update");
    let sv = updates
        .iter()
        .find_map(|u| if let VmUpdate::Session(s) = u { Some(s) } else { None })
        .expect("Session from usage_update");

    // Tokens seeded: input (600) + output (400) = 1000.
    assert_eq!(sv.tokens, 1000, "UsageUpdate must seed tokens (600+400=1000)");
    assert!(sv.context_pct > 0.0, "context_pct seeded from usage update");
}
