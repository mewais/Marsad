//! fakes.rs — drive `Core` over `LoopbackFake` end-to-end with a scripted agent.
//!
//! Simulates a full session: the test plays the role of the agent by pushing IPC
//! lines into the peer end of `LoopbackFake`, and then asserts both the emitted
//! `VmUpdate`s and the outbound commands that `Core` sent toward the agent.
//!
//! This is the integration-level test seam specified in the test plan. All assertions
//! are black-box against the public API of `marsad_core`.
//!
//! These tests WILL fail to compile until the code-agent implements `marsad_core`.

use marsad_core::{Core, CoreConfig, LaunchSpec, Posture, VmUpdate};
use marsad_gates::{approve, reject, Gate};
use marsad_transport::LoopbackFake;
use marsad_viewmodel::SlashCmd;

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

fn cfg() -> CoreConfig {
    CoreConfig {
        agent_endpoint: "loopback://fake".to_string(),
        data_root: "/tmp/marsad-fakes-test".to_string(),
    }
}

fn make_core() -> (Core, LoopbackFake) {
    let (a, b) = LoopbackFake::pair();
    let core = Core::new(cfg(), Box::new(a)).expect("Core::new");
    (core, b)
}

fn agent_init(session_id: &str) -> String {
    format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"system","subtype":"init","session_id":"{session_id}","model":"claude-opus-4-5","cwd":"/workspace","tools":["Read","Write","Bash"],"mcp_servers":[],"permissionMode":"default"}}}}"#
    )
}

fn agent_assistant_text(session_id: &str, msg_id: &str, text: &str) -> String {
    let escaped = text.replace('"', "\\\"");
    format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"assistant","message":{{"id":"{msg_id}","content":[{{"type":"text","text":"{escaped}"}}],"usage":{{"input_tokens":200,"output_tokens":50,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}}}"#
    )
}

fn agent_control_request(session_id: &str, request_id: &str, tool: &str) -> String {
    format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"control_request","request_id":"{request_id}","request":{{"subtype":"can_use_tool","tool_name":"{tool}","input":{{"command":"ls /etc"}}}}}}}}"#
    )
}

fn agent_result_success(session_id: &str, cost: f64) -> String {
    format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"result","subtype":"success","is_error":false,"total_cost_usd":{cost},"usage":{{"input_tokens":200,"output_tokens":50,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}},"num_turns":2,"permission_denials":[]}}}}"#
    )
}

fn agent_result_error(session_id: &str) -> String {
    format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"result","subtype":"error_during_execution","is_error":true,"total_cost_usd":0.01,"usage":{{"input_tokens":50,"output_tokens":10,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}},"num_turns":1,"permission_denials":[]}}}}"#
    )
}

// ---------------------------------------------------------------------------
// Script 1: Happy-path session — launch, observe full lifecycle, verify updates + outbound
// ---------------------------------------------------------------------------

/// A scripted agent drives Core through: init → working → permission gate (approved)
/// → assistant text → result/success. All outbound commands and inbound VmUpdates
/// are verified.
#[test]
fn scripted_full_happy_path_session() {
    let (mut core, mut peer) = make_core();

    // ── Step 1: GUI launches a session ────────────────────────────────────────
    let id = core
        .launch_session(LaunchSpec {
            project: "my-api".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: Some("Add a health check endpoint".to_string()),
            mode: Default::default(),
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "claude".to_string(),
        })
        .expect("launch_session");

    assert!(!id.is_empty(), "session id must be non-empty");

    // ── Assert outbound: LaunchSession IPC was sent ───────────────────────────
    let outbound_launch = peer.drain_outbound();
    assert!(!outbound_launch.is_empty(), "launch must emit outbound IPC");
    let launch_line = outbound_launch.join(" ");
    assert!(
        launch_line.contains("launch") || launch_line.contains("Launch"),
        "outbound must mention launch; got: {launch_line}"
    );

    // ── Step 2: Agent feeds system/init ──────────────────────────────────────
    let init_updates = core
        .on_agent_line(&agent_init(&id))
        .expect("on_agent_line init");
    {
        let session_ups: Vec<_> = init_updates
            .iter()
            .filter(|u| matches!(u, VmUpdate::Session(_)))
            .collect();
        assert!(
            !session_ups.is_empty(),
            "system/init must produce VmUpdate::Session"
        );
        // Check the session update has Working phase
        use marsad_events::Phase;
        for u in &session_ups {
            if let VmUpdate::Session(sv) = u {
                if sv.id == id {
                    assert_eq!(sv.state, Phase::Working, "phase must be Working after init");
                }
            }
        }
    }
    // Overview must also be updated
    assert!(
        init_updates
            .iter()
            .any(|u| matches!(u, VmUpdate::Overview(_))),
        "system/init must produce VmUpdate::Overview"
    );

    // ── Step 3: Agent feeds a permission gate (ControlRequest) ───────────────
    let ctrl_updates = core
        .on_agent_line(&agent_control_request(&id, "rq-001", "Bash"))
        .expect("ctrl");
    {
        use marsad_events::Phase;
        let needs = ctrl_updates
            .iter()
            .filter_map(|u| {
                if let VmUpdate::Session(sv) = u {
                    Some(sv)
                } else {
                    None
                }
            })
            .find(|sv| sv.id == id);
        assert!(
            needs.is_some(),
            "control_request must produce VmUpdate::Session for the session"
        );
        assert_eq!(
            needs.unwrap().state,
            Phase::Needs,
            "phase must be Needs after ControlRequest"
        );
        // gate field must be populated
        assert!(
            needs.unwrap().gate.is_some(),
            "gate must be Some after ControlRequest"
        );
    }

    // ── Step 4: GUI approves the gate ────────────────────────────────────────
    let gate = Gate::Permission {
        request_id: "req-test".to_string(),
        command: "ls /etc".to_string(),
        cwd: "/workspace".to_string(),
        risk: "low".to_string(),
        explanation: "list config files".to_string(),
        is_elicitation: false,
    };
    let decision = approve(&id, &gate);
    core.answer_gate(decision).expect("answer_gate");

    // ── Assert outbound: AnswerGate IPC was sent ─────────────────────────────
    let outbound_gate = peer.drain_outbound();
    assert!(
        !outbound_gate.is_empty(),
        "answer_gate must emit outbound IPC"
    );

    // ── Step 5: Agent sends assistant text ───────────────────────────────────
    let asst_updates = core
        .on_agent_line(&agent_assistant_text(
            &id,
            "msg-001",
            "I will add the health check.",
        ))
        .expect("asst");
    assert!(
        asst_updates
            .iter()
            .any(|u| matches!(u, VmUpdate::Session(_))),
        "assistant text must produce VmUpdate::Session"
    );

    // ── Step 6: Agent sends result/success ───────────────────────────────────
    let done_updates = core
        .on_agent_line(&agent_result_success(&id, 0.42))
        .expect("result");
    {
        use marsad_events::Phase;
        let done = done_updates
            .iter()
            .filter_map(|u| {
                if let VmUpdate::Session(sv) = u {
                    Some(sv)
                } else {
                    None
                }
            })
            .find(|sv| sv.id == id);
        assert!(
            done.is_some(),
            "result/success must produce VmUpdate::Session"
        );
        assert_eq!(
            done.unwrap().state,
            Phase::Done,
            "phase must be Done after result/success"
        );
    }

    // ── Step 7: snapshot reflects the final state ─────────────────────────────
    let snap = core.snapshot();
    let sess = snap.sessions.iter().find(|s| s.id == id);
    assert!(sess.is_some(), "snapshot must contain the session");
    use marsad_events::Phase;
    assert_eq!(
        sess.unwrap().state,
        Phase::Done,
        "snapshot session must be Done"
    );
}

// ---------------------------------------------------------------------------
// Script 2: Rejected gate — session continues or stops
// ---------------------------------------------------------------------------

#[test]
fn scripted_gate_rejection_sends_deny_ipc() {
    let (mut core, mut peer) = make_core();

    let id = core
        .launch_session(LaunchSpec {
            project: "db-migration".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
            mode: Default::default(),
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "claude".to_string(),
        })
        .expect("launch");
    let _ = peer.drain_outbound();

    let _ = core.on_agent_line(&agent_init(&id)).expect("init");
    let _ = core
        .on_agent_line(&agent_control_request(&id, "rq-002", "Write"))
        .expect("ctrl");

    let gate = Gate::Permission {
        request_id: "req-test".to_string(),
        command: "DROP TABLE users".to_string(),
        cwd: "/db".to_string(),
        risk: "critical".to_string(),
        explanation: "irreversible".to_string(),
        is_elicitation: false,
    };
    let d = reject(&id, &gate, "too dangerous");
    core.answer_gate(d).expect("answer_gate reject");

    let outbound = peer.drain_outbound();
    assert!(!outbound.is_empty(), "rejected gate must emit outbound IPC");
    let combined = outbound.join(" ");
    // The outbound should include a deny signal
    assert!(
        combined.contains("deny")
            || combined.contains("Deny")
            || combined.contains("reject")
            || combined.contains("Reject")
            || combined.contains("gate")
            || combined.contains("Gate"),
        "reject gate outbound must reference deny/reject/gate; got: {combined}"
    );
}

// ---------------------------------------------------------------------------
// Script 3: Error result — phase becomes Error
// ---------------------------------------------------------------------------

#[test]
fn scripted_agent_error_result_sets_error_phase() {
    use marsad_events::Phase;

    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "failing-proj".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
            mode: Default::default(),
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "claude".to_string(),
        })
        .expect("launch");

    let _ = core.on_agent_line(&agent_init(&id)).expect("init");
    let err_updates = core
        .on_agent_line(&agent_result_error(&id))
        .expect("error result");

    let error_sess = err_updates
        .iter()
        .filter_map(|u| {
            if let VmUpdate::Session(sv) = u {
                Some(sv)
            } else {
                None
            }
        })
        .find(|sv| sv.id == id);
    assert!(
        error_sess.is_some(),
        "error result must produce a Session update"
    );
    assert_eq!(
        error_sess.unwrap().state,
        Phase::Error,
        "phase must be Error after error result"
    );
}

// ---------------------------------------------------------------------------
// Script 4: send_prompt during a live session
// ---------------------------------------------------------------------------

#[test]
fn scripted_send_prompt_during_live_session() {
    let (mut core, mut peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "interactive".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
            mode: Default::default(),
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "claude".to_string(),
        })
        .expect("launch");
    let _ = peer.drain_outbound();

    let _ = core.on_agent_line(&agent_init(&id)).expect("init");

    // GUI sends a follow-up prompt
    core.send_prompt(&id, "Now write the tests too.")
        .expect("send_prompt");
    let outbound = peer.drain_outbound();
    assert!(!outbound.is_empty(), "send_prompt must emit outbound IPC");
    let combined = outbound.join(" ");
    assert!(
        combined.contains("Now write the tests too.")
            || combined.contains("prompt")
            || combined.contains("Prompt"),
        "outbound must include the prompt; got: {combined}"
    );
}

// ---------------------------------------------------------------------------
// Script 5: interrupt during a live session
// ---------------------------------------------------------------------------

#[test]
fn scripted_interrupt_session() {
    let (mut core, mut peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "long-running".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
            mode: Default::default(),
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "claude".to_string(),
        })
        .expect("launch");
    let _ = peer.drain_outbound();

    let _ = core.on_agent_line(&agent_init(&id)).expect("init");

    // GUI interrupts
    core.interrupt(&id).expect("interrupt");
    let outbound = peer.drain_outbound();
    assert!(!outbound.is_empty(), "interrupt must emit outbound IPC");
    let combined = outbound.join(" ");
    assert!(
        combined.contains("interrupt") || combined.contains("Interrupt"),
        "outbound must reference interrupt; got: {combined}"
    );
}

// ---------------------------------------------------------------------------
// Script 6: Two concurrent sessions, no cross-routing
// ---------------------------------------------------------------------------

#[test]
fn scripted_two_concurrent_sessions_no_cross_routing() {
    use marsad_events::Phase;

    let (mut core, _peer) = make_core();

    let id_alpha = core
        .launch_session(LaunchSpec {
            project: "alpha".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
            mode: Default::default(),
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "claude".to_string(),
        })
        .expect("launch alpha");

    let id_beta = core
        .launch_session(LaunchSpec {
            project: "beta".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            model: "claude-sonnet-4-5".to_string(),
            posture: Posture::AutoApproveSafe,
            first_prompt: None,
            mode: Default::default(),
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "claude".to_string(),
        })
        .expect("launch beta");

    // Both inits
    let _ = core
        .on_agent_line(&agent_init(&id_alpha))
        .expect("init alpha");
    let _ = core
        .on_agent_line(&agent_init(&id_beta))
        .expect("init beta");

    // Alpha gets a result, beta stays in Working
    let alpha_done_updates = core
        .on_agent_line(&agent_result_success(&id_alpha, 0.10))
        .expect("result alpha");

    // Only alpha should be in Done
    for u in &alpha_done_updates {
        if let VmUpdate::Session(sv) = u {
            if sv.id == id_alpha {
                assert_eq!(sv.state, Phase::Done, "alpha must be Done");
            }
            if sv.id == id_beta {
                // beta should still be Working (or unchanged)
                assert_ne!(
                    sv.state,
                    Phase::Done,
                    "beta must not be marked Done when only alpha finishes"
                );
            }
        }
    }

    // Verify snapshot: alpha=Done, beta=Working
    let snap = core.snapshot();
    let alpha_snap = snap
        .sessions
        .iter()
        .find(|s| s.id == id_alpha)
        .expect("alpha in snap");
    let beta_snap = snap
        .sessions
        .iter()
        .find(|s| s.id == id_beta)
        .expect("beta in snap");
    assert_eq!(alpha_snap.state, Phase::Done, "snapshot: alpha Done");
    assert_eq!(beta_snap.state, Phase::Working, "snapshot: beta Working");
}

// ---------------------------------------------------------------------------
// Script 7: SlashCmd dispatched, outbound verified
// ---------------------------------------------------------------------------

#[test]
fn scripted_slash_compact_dispatches_ipc() {
    let (mut core, mut peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "slash-test".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
            mode: Default::default(),
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "claude".to_string(),
        })
        .expect("launch");
    let _ = peer.drain_outbound();
    let _ = core.on_agent_line(&agent_init(&id)).expect("init");
    let _ = peer.drain_outbound();

    core.run_slash(&id, SlashCmd::Compact)
        .expect("run /compact");
    let outbound = peer.drain_outbound();
    assert!(!outbound.is_empty(), "/compact must emit outbound IPC");
    let combined = outbound.join(" ");
    assert!(
        combined.contains("compact") || combined.contains("Compact") || combined.contains("slash"),
        "/compact outbound must reference compact; got: {combined}"
    );
}

// ---------------------------------------------------------------------------
// Script 8: Idempotent answer_gate end-to-end
// ---------------------------------------------------------------------------

#[test]
fn scripted_idempotent_answer_gate_end_to_end() {
    let (mut core, mut peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "idem-gate-e2e".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
            mode: Default::default(),
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "claude".to_string(),
        })
        .expect("launch");
    let _ = peer.drain_outbound();

    let _ = core.on_agent_line(&agent_init(&id)).expect("init");
    let _ = core
        .on_agent_line(&agent_control_request(&id, "rq-idem", "Bash"))
        .expect("ctrl");

    let gate = Gate::Permission {
        request_id: "req-test".to_string(),
        command: "make build".to_string(),
        cwd: "/project".to_string(),
        risk: "low".to_string(),
        explanation: "build the project".to_string(),
        is_elicitation: false,
    };

    // First approval — must emit exactly one IPC line
    let d1 = approve(&id, &gate);
    core.answer_gate(d1).expect("first answer");
    let first_out = peer.drain_outbound();
    assert!(
        !first_out.is_empty(),
        "first answer_gate must emit outbound IPC"
    );
    let first_count = first_out.len();

    // Second approval with the SAME gate (same DecisionId) — must not emit another IPC
    let d2 = approve(&id, &gate);
    let second_result = core.answer_gate(d2);
    let second_out = peer.drain_outbound();

    match second_result {
        Ok(_) => {
            assert_eq!(
                second_out.len(),
                0,
                "idempotent second answer must not emit additional IPC (first emitted {first_count} line(s))"
            );
        }
        Err(_) => {
            // Error is an acceptable idempotency signal
            assert_eq!(
                second_out.len(),
                0,
                "even on error, duplicate answer must not emit IPC"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Script 9: Full session with stop
// ---------------------------------------------------------------------------

#[test]
fn scripted_stop_session_after_launch() {
    let (mut core, mut peer) = make_core();
    let id = core
        .launch_session(LaunchSpec {
            project: "stoppable".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
            mode: Default::default(),
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "claude".to_string(),
        })
        .expect("launch");
    let _ = peer.drain_outbound();

    let _ = core.on_agent_line(&agent_init(&id)).expect("init");
    let _ = peer.drain_outbound();

    core.stop_session(&id).expect("stop_session");
    let outbound = peer.drain_outbound();
    assert!(!outbound.is_empty(), "stop_session must emit outbound IPC");
    let combined = outbound.join(" ");
    assert!(
        combined.contains("stop") || combined.contains("Stop"),
        "stop outbound must reference stop; got: {combined}"
    );
}

// ---------------------------------------------------------------------------
// Script 10: resume then attach — outbound IPC ordering
// ---------------------------------------------------------------------------

#[test]
fn scripted_resume_then_attach() {
    let (mut core, mut peer) = make_core();

    // Launch so id is known
    let id = core
        .launch_session(LaunchSpec {
            project: "re-attach".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
            mode: Default::default(),
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "claude".to_string(),
        })
        .expect("launch");
    let _ = peer.drain_outbound();

    core.resume_session(&id).expect("resume");
    let resume_out = peer.drain_outbound();
    assert!(!resume_out.is_empty(), "resume must emit IPC");

    core.attach_session(&id).expect("attach");
    let attach_out = peer.drain_outbound();
    assert!(!attach_out.is_empty(), "attach must emit IPC");

    // Verify ordering: resume came first, then attach
    // (already confirmed by drain order above)
    let resume_combined = resume_out.join(" ");
    let attach_combined = attach_out.join(" ");
    assert!(
        resume_combined.contains("resume") || resume_combined.contains("Resume"),
        "resume IPC must mention resume; got: {resume_combined}"
    );
    assert!(
        attach_combined.contains("attach") || attach_combined.contains("Attach"),
        "attach IPC must mention attach; got: {attach_combined}"
    );
}

// ---------------------------------------------------------------------------
// Feature A: SessionView exposes tools/mcp_servers/slash_commands from system/init
// ---------------------------------------------------------------------------

/// After a system/init AgentEvent the SessionView must expose tools, mcp_servers,
/// and slash_commands populated from the event.
#[test]
fn session_view_exposes_tools_mcp_slash_after_init() {
    let (mut core, mut peer) = make_core();

    // Launch, drain the outbound LaunchSession IPC.
    let id = core
        .launch_session(LaunchSpec {
            project: "/work/proj".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: vec![],
            model: "claude-opus-4-5".to_string(),
            posture: Posture::Ask,
            first_prompt: None,
            mode: Default::default(),
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "claude".to_string(),
        })
        .expect("launch");
    let _ = peer.drain_outbound();

    // Deliver a rich system/init with tools, mcp_servers, and slash_commands.
    let init_line = format!(
        r#"{{"type":"AgentEvent","session_id":"{id}","event":{{"type":"system","subtype":"init","session_id":"claude-uuid","model":"claude-opus-4-5","cwd":"/work/proj","tools":["Read","Bash","Edit"],"mcp_servers":[{{"name":"hares","status":"failed"}},{{"name":"hub","status":"connected"}}],"permissionMode":"default","slash_commands":[{{"name":"/clear","description":"Clear conversation history","argumentHint":""}},{{"name":"/model","description":"Switch model","argumentHint":"model-name"}}]}}}}"#
    );
    let updates = core.on_agent_line(&init_line).expect("on_agent_line");

    // Find the Session VmUpdate.
    let sv = updates
        .iter()
        .find_map(|u| match u {
            VmUpdate::Session(sv) => Some(sv.clone()),
            _ => None,
        })
        .expect("Session VmUpdate must be emitted");

    assert_eq!(sv.tools, vec!["Read", "Bash", "Edit"]);
    assert_eq!(sv.mcp_servers.len(), 2);
    assert_eq!(sv.mcp_servers[0].name, "hares");
    assert_eq!(sv.mcp_servers[0].status, "failed");
    assert_eq!(sv.mcp_servers[1].name, "hub");
    assert_eq!(sv.slash_commands.len(), 2);
    assert_eq!(sv.slash_commands[0].name, "/clear");
    assert_eq!(sv.slash_commands[1].argument_hint, "model-name");
}
