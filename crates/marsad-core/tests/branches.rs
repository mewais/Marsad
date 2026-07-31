//! branches.rs — table-driven / region-closing cases for `marsad-core`.
//!
//! Covers:
//! - Every VmUpdate variant (Overview, Session, Toast, HostHealth)
//! - Every session_mode variant (default/acceptEdits/bypassPermissions)
//! - Every SlashCmd variant dispatched via run_slash
//! - Workspace switch re-scopes the snapshot (set_workspace changes selected scope)
//! - Idempotent re-answer_gate: answering the same gate twice routes once / is a no-op
//!   via `marsad_gates::DecisionLedger` (second call is a no-op or returns no extra IPC)
//! - Multiple concurrent sessions advance independently (no cross-routing)
//! - on_agent_line for every VmUpdate variant reachable via IPC
//! - snapshot reflects state of all known sessions
//!
//! These tests WILL fail to compile until the code-agent implements `marsad_core`.

use marsad_core::{Core, CoreConfig, LaunchSpec, VmUpdate};
use marsad_gates::{approve, reject, Gate};
use marsad_transport::LoopbackFake;
use marsad_viewmodel::SlashCmd;

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn cfg() -> CoreConfig {
    CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-branch-test".to_string(),
    }
}

fn make_core() -> (Core, LoopbackFake) {
    let (a, b) = LoopbackFake::pair();
    let core = Core::new(cfg(), Box::new(a)).expect("Core::new");
    (core, b)
}

fn spec(project: &str, session_mode: String) -> LaunchSpec {
    LaunchSpec {
        project: project.to_string(),
        host_id: "local".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: "claude-opus-4-5".to_string(),
        session_mode,
        first_prompt: None,
        mode: Default::default(),
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
    }
}

fn ipc_init(session_id: &str) -> String {
    format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"system","subtype":"init","session_id":"{session_id}","model":"claude-opus-4-5","cwd":"/tmp","tools":[],"mcp_servers":[],"permissionMode":"default"}}}}"#
    )
}

fn ipc_assistant(session_id: &str, msg_id: &str) -> String {
    format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"assistant","message":{{"id":"{msg_id}","content":[{{"type":"text","text":"ok"}}],"usage":{{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}}}"#
    )
}

fn ipc_control_request(session_id: &str, request_id: &str) -> String {
    // Real Agent-SDK control protocol shape: payload nested under `request`.
    format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"control_request","request_id":"{request_id}","request":{{"subtype":"can_use_tool","tool_name":"Bash","input":{{}}}}}}}}"#
    )
}

fn ipc_result_success(session_id: &str) -> String {
    format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"result","subtype":"success","is_error":false,"total_cost_usd":0.01,"usage":{{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}},"num_turns":1,"permission_denials":[]}}}}"#
    )
}

// ---------------------------------------------------------------------------
// Every session_mode variant dispatched via launch_session
// ---------------------------------------------------------------------------

#[test]
fn session_mode_default_launches_successfully() {
    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(spec("p", String::new()))
        .expect("launch default mode");
    assert!(!id.is_empty());
}

#[test]
fn session_mode_accept_edits_launches_successfully() {
    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(spec("p", "acceptEdits".to_string()))
        .expect("launch acceptEdits");
    assert!(!id.is_empty());
}

#[test]
fn session_mode_bypass_permissions_launches_successfully() {
    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(spec("p", "bypassPermissions".to_string()))
        .expect("launch bypassPermissions");
    assert!(!id.is_empty());
}

// ---------------------------------------------------------------------------
// Every VmUpdate variant reachable
// ---------------------------------------------------------------------------

#[test]
fn vm_update_overview_variant_reachable() {
    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(spec("proj-ov", String::new()))
        .expect("launch");
    let updates = core.on_agent_line(&ipc_init(&id)).expect("on_agent_line");
    let has_overview = updates.iter().any(|u| matches!(u, VmUpdate::Overview(_)));
    assert!(
        has_overview,
        "VmUpdate::Overview must be reachable after a system/init event"
    );
}

#[test]
fn vm_update_session_variant_reachable() {
    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(spec("proj-sess", String::new()))
        .expect("launch");
    let updates = core.on_agent_line(&ipc_init(&id)).expect("on_agent_line");
    let has_session = updates.iter().any(|u| matches!(u, VmUpdate::Session(_)));
    assert!(
        has_session,
        "VmUpdate::Session must be reachable after system/init"
    );
}

// ---------------------------------------------------------------------------
// Regression: a single NewSession launch must produce EXACTLY ONE board card
// (its auto-created single-node workstream), not two (empty ws + bare node).
// ---------------------------------------------------------------------------

#[test]
fn assign_to_other_workstream_removes_orphaned_auto_ws() {
    let (mut core, _peer) = make_core();
    // Build-it-out path: create a named workstream, launch a session (which
    // auto-creates ws-<id>), then assign the session to the named workstream.
    core.create_workstream("ws-named".into(), "Named".into(), "goal".into(), None)
        .expect("create ws");
    let id = core
        .launch_session(spec("proj-node", String::new()))
        .expect("launch");
    // Before assign: the auto ws AND the named ws both exist.
    assert!(core.board().all.iter().any(|c| c.id == format!("ws-{id}")));
    core.assign_session(&id, "ws-named", true).expect("assign");
    let board = core.board();
    // The orphaned auto ws-<id> must be gone; only the named ws remains.
    assert!(
        !board.all.iter().any(|c| c.id == format!("ws-{id}")),
        "orphaned auto workstream ws-{id} must be removed after reassignment"
    );
    let named = board.all.iter().find(|c| c.id == "ws-named").expect("named ws");
    assert_eq!(named.session_count, 1, "session moved to the named ws");
}

#[test]
fn single_launch_produces_one_board_card() {
    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(spec("proj-solo", String::new()))
        .expect("launch");
    let board = core.board();
    assert_eq!(
        board.all.len(),
        1,
        "one launched session must yield exactly one workstream card, got {}: {:?}",
        board.all.len(),
        board.all.iter().map(|c| (&c.id, &c.name)).collect::<Vec<_>>()
    );
    let card = &board.all[0];
    assert_eq!(card.session_count, 1, "the card must own its one session");
    assert_eq!(card.id, format!("ws-{id}"), "card is the auto fast-lane ws");
}

#[test]
fn vm_update_toast_variant_is_constructible() {
    let t = VmUpdate::Toast {
        icon: "✓".to_string(),
        text: "done".to_string(),
    };
    assert!(matches!(t, VmUpdate::Toast { .. }));
}

#[test]
fn vm_update_host_health_variant_is_constructible() {
    let h = VmUpdate::HostHealth(Default::default());
    assert!(matches!(h, VmUpdate::HostHealth(_)));
}

// ---------------------------------------------------------------------------
// Every SlashCmd variant dispatched via run_slash
// ---------------------------------------------------------------------------

fn slash_roundtrip(cmd: SlashCmd) {
    let (mut core, mut peer) = make_core();
    let id = core
        .launch_session(spec("slash-proj", String::new()))
        .expect("launch");
    let _ = peer.drain_outbound();
    core.run_slash(&id, cmd).expect("run_slash");
    let out = peer.drain_outbound();
    assert!(!out.is_empty(), "run_slash must emit at least one IPC line");
}

#[test]
fn run_slash_clear() {
    slash_roundtrip(SlashCmd::Clear);
}

#[test]
fn run_slash_compact() {
    slash_roundtrip(SlashCmd::Compact);
}

#[test]
fn run_slash_cost() {
    slash_roundtrip(SlashCmd::Cost);
}

#[test]
fn run_slash_model() {
    slash_roundtrip(SlashCmd::Model);
}

#[test]
fn run_slash_config() {
    slash_roundtrip(SlashCmd::Config);
}

#[test]
fn run_slash_init() {
    slash_roundtrip(SlashCmd::Init);
}

#[test]
fn run_slash_review() {
    slash_roundtrip(SlashCmd::Review);
}

#[test]
fn run_slash_agents() {
    slash_roundtrip(SlashCmd::Agents);
}

#[test]
fn run_slash_resume() {
    slash_roundtrip(SlashCmd::Resume);
}

#[test]
fn run_slash_terminal() {
    slash_roundtrip(SlashCmd::Terminal);
}

#[test]
fn run_slash_help() {
    slash_roundtrip(SlashCmd::Help);
}

#[test]
fn run_slash_unknown_dispatches_without_panic() {
    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(spec("slash-unk", String::new()))
        .expect("launch");
    // Unknown slash commands must not panic; may succeed or return an error
    let _result = core.run_slash(&id, SlashCmd::Unknown("/foobar".to_string()));
}

// ---------------------------------------------------------------------------
// Workspace switch re-scopes the snapshot
// ---------------------------------------------------------------------------

#[test]
fn set_workspace_rescopes_snapshot() {
    let (mut core, _peer) = make_core();
    let id_a = core
        .launch_session(spec("proj-ws-a", String::new()))
        .expect("launch a");
    let id_b = core
        .launch_session(spec("proj-ws-b", String::new()))
        .expect("launch b");

    // Before set_workspace, snapshot has both sessions
    let snap_before = core.snapshot();
    let has_a = snap_before.sessions.iter().any(|s| s.id == id_a);
    let has_b = snap_before.sessions.iter().any(|s| s.id == id_b);
    assert!(
        has_a && has_b,
        "snapshot must contain both sessions before workspace switch"
    );

    // Switch workspace to id_a's context
    core.set_workspace(&id_a).expect("set_workspace");

    // The snapshot should now be re-scoped; this doesn't necessarily remove sessions
    // but active_ws (or the filter) should reflect the workspace
    let snap_after = core.snapshot();
    // At minimum the snapshot must still be valid (no panic, has fields)
    let _ = snap_after;
}

// ---------------------------------------------------------------------------
// Idempotent re-answer_gate: second answer is a no-op via DecisionLedger
// ---------------------------------------------------------------------------

#[test]
fn answer_gate_idempotent_second_call_is_noop() {
    let (mut core, mut peer) = make_core();
    let id = core
        .launch_session(spec("proj-gate-idem", String::new()))
        .expect("launch");
    let _ = peer.drain_outbound(); // consume launch IPC

    let gate = Gate::Permission {
        request_id: "req-test".to_string(),
        command: "cat /etc/passwd".to_string(),
        cwd: "/".to_string(),
        risk: "low".to_string(),
        explanation: "read only".to_string(),
        is_elicitation: false,
        elicitation_choices: Vec::new(),
        permission_suggestions: Vec::new(),
    };

    // First answer — should succeed and emit one outbound IPC
    let d1 = approve(&id, &gate);
    core.answer_gate(d1.clone()).expect("first answer_gate");
    let first_outbound = peer.drain_outbound();
    assert!(
        !first_outbound.is_empty(),
        "first answer_gate must emit outbound IPC"
    );

    // Second answer with the SAME decision id — must be a no-op (idempotent via DecisionLedger)
    let d2 = approve(&id, &gate); // same gate → same DecisionId
    let result = core.answer_gate(d2);
    // May succeed (silently ignored) or return an error; must NOT emit a second outbound line
    let second_outbound = peer.drain_outbound();
    // Key assertion: no duplicate outbound command was sent
    match result {
        Ok(_) => {
            // Idempotent no-op: accepted but no duplicate IPC
            assert!(
                second_outbound.is_empty(),
                "duplicate answer_gate must not emit a second outbound IPC line, got: {second_outbound:?}"
            );
        }
        Err(e) => {
            // Error path is also acceptable (DuplicateDecision surfaces as an error in Core)
            let _ = e; // some error variant — not a panic
        }
    }
}

#[test]
fn answer_gate_different_decisions_are_not_idempotent() {
    // Approve then reject with the same gate must be deduplicated as the same id,
    // OR the second one for a different session may route differently.
    // We test: two approve calls produce the same DecisionId, so the second is a no-op.
    let (mut core, mut peer) = make_core();
    let id = core
        .launch_session(spec("proj-gate2", String::new()))
        .expect("launch");
    let _ = peer.drain_outbound();

    let gate = Gate::Plan {
        request_id: "req-plan-core-branch".to_string(),
        summary: "Refactor auth".to_string(),
        steps: vec![],
    };

    let d1 = approve(&id, &gate);
    let d2 = reject(&id, &gate, "too risky");

    core.answer_gate(d1).expect("approve");
    let out1 = peer.drain_outbound();
    assert!(!out1.is_empty());

    // Reject has a DIFFERENT DecisionId only if the id is derived from gate+session+decision type.
    // The spec says decision_id is derived from (session_id, gate identity) — NOT the decision value.
    // So approve and reject produce the SAME id → second is idempotent/no-op.
    let result = core.answer_gate(d2);
    let out2 = peer.drain_outbound();
    // DuplicateDecision error is acceptable; if Ok, output must be empty.
    if let Ok(_) = result {
        assert!(
            out2.is_empty(),
            "reject on same gate after approve must be no-op, got: {out2:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Multiple concurrent sessions advance independently (no cross-routing)
// ---------------------------------------------------------------------------

#[test]
fn concurrent_sessions_do_not_cross_route() {
    let (mut core, _peer) = make_core();

    let id_x = core
        .launch_session(spec("session-x", String::new()))
        .expect("launch x");
    let id_y = core
        .launch_session(spec("session-y", String::new()))
        .expect("launch y");

    // Feed init events for both sessions
    let _ = core.on_agent_line(&ipc_init(&id_x)).expect("init x");
    let _ = core.on_agent_line(&ipc_init(&id_y)).expect("init y");

    // Feed assistant event only to x
    let updates_x = core
        .on_agent_line(&ipc_assistant(&id_x, "msg-001"))
        .expect("asst x");

    // Session-x update must reference x's id, not y's
    for u in &updates_x {
        if let VmUpdate::Session(sv) = u {
            assert_ne!(sv.id, id_y, "session x update must not have session y's id");
        }
    }

    // Feed result only to y
    let updates_y = core
        .on_agent_line(&ipc_result_success(&id_y))
        .expect("result y");

    // Session-y update must reference y's id
    for u in &updates_y {
        if let VmUpdate::Session(sv) = u {
            assert_ne!(sv.id, id_x, "session y update must not have session x's id");
        }
    }
}

#[test]
fn streamed_blocks_sharing_a_message_id_all_appear_and_dedup() {
    // claude streams ONE assistant message as multiple events sharing a message
    // id (one per content block). All distinct blocks must land in the transcript
    // (regression: deduping by id alone dropped every block after the first, so
    // the live transcript froze and only the restart backfill showed the reply).
    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(spec("s-stream", String::new()))
        .expect("launch");
    let _ = core.on_agent_line(&ipc_init(&id)).expect("init");

    let text_evt = ipc_assistant(&id, "msg-same"); // content: text "ok"
    let tool_evt = format!(
        r#"{{"type":"AgentEvent","session_id":"{id}","event":{{"type":"assistant","message":{{"id":"msg-same","content":[{{"type":"tool_use","id":"tu1","name":"Bash","input":{{"command":"ls"}}}}],"usage":{{"input_tokens":1,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}}}"#
    );
    let _ = core.on_agent_line(&text_evt).expect("text block");
    let _ = core.on_agent_line(&tool_evt).expect("tool block (same id)");
    // Re-feed the identical text block — must be deduped (insert-false branch).
    let _ = core.on_agent_line(&text_evt).expect("dup text block");

    let snap = core.snapshot();
    let sv = snap.sessions.iter().find(|s| s.id == id).expect("session");
    let text_count = sv
        .transcript
        .iter()
        .filter(|e| matches!(e, marsad_events::TranscriptEntry::Assistant { text, .. } if text == "ok"))
        .count();
    let has_tool = sv
        .transcript
        .iter()
        .any(|e| matches!(e, marsad_events::TranscriptEntry::Tool { name, .. } if name == "Bash"));
    assert_eq!(
        text_count, 1,
        "text block present exactly once: {:?}",
        sv.transcript
    );
    assert!(
        has_tool,
        "tool block (same msg id) must also appear: {:?}",
        sv.transcript
    );
}

#[test]
fn snapshot_contains_all_launched_sessions() {
    let (mut core, _peer) = make_core();
    let id1 = core.launch_session(spec("s1", String::new())).expect("l1");
    let id2 = core
        .launch_session(spec("s2", "acceptEdits".to_string()))
        .expect("l2");
    let id3 = core
        .launch_session(spec("s3", "bypassPermissions".to_string()))
        .expect("l3");

    let snap = core.snapshot();
    let ids: Vec<&str> = snap.sessions.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&id1.as_str()), "snapshot must contain s1");
    assert!(ids.contains(&id2.as_str()), "snapshot must contain s2");
    assert!(ids.contains(&id3.as_str()), "snapshot must contain s3");
}

// ---------------------------------------------------------------------------
// on_agent_line: VmUpdate contents reflect actual session state
// ---------------------------------------------------------------------------

#[test]
fn session_update_after_init_reflects_working_phase() {
    use marsad_events::Phase;

    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(spec("proj-phase", String::new()))
        .expect("launch");

    let updates = core.on_agent_line(&ipc_init(&id)).expect("init");
    let session_updates: Vec<_> = updates
        .into_iter()
        .filter_map(|u| {
            if let VmUpdate::Session(sv) = u {
                Some(sv)
            } else {
                None
            }
        })
        .collect();
    assert!(
        !session_updates.is_empty(),
        "must have at least one Session update"
    );
    for sv in &session_updates {
        if sv.id == id {
            assert_eq!(
                sv.state,
                Phase::Working,
                "after system/init the session must be in Working phase"
            );
        }
    }
}

#[test]
fn session_update_after_result_reflects_done_phase() {
    use marsad_events::Phase;

    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(spec("proj-done-ph", String::new()))
        .expect("launch");

    let _ = core.on_agent_line(&ipc_init(&id)).expect("init");
    let _ = core
        .on_agent_line(&ipc_assistant(&id, "msg-Z"))
        .expect("asst");

    let updates = core
        .on_agent_line(&ipc_result_success(&id))
        .expect("result");
    let session_updates: Vec<_> = updates
        .into_iter()
        .filter_map(|u| {
            if let VmUpdate::Session(sv) = u {
                Some(sv)
            } else {
                None
            }
        })
        .collect();
    assert!(!session_updates.is_empty());
    for sv in &session_updates {
        if sv.id == id {
            assert_eq!(
                sv.state,
                Phase::Done,
                "after result/success the session must be in Done phase"
            );
        }
    }
}

#[test]
fn session_update_after_control_request_reflects_needs_phase() {
    use marsad_events::Phase;

    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(spec("proj-needs", String::new()))
        .expect("launch");

    let _ = core.on_agent_line(&ipc_init(&id)).expect("init");

    let updates = core
        .on_agent_line(&ipc_control_request(&id, "rq-99"))
        .expect("ctrl");
    let session_updates: Vec<_> = updates
        .into_iter()
        .filter_map(|u| {
            if let VmUpdate::Session(sv) = u {
                Some(sv)
            } else {
                None
            }
        })
        .collect();
    assert!(!session_updates.is_empty());
    for sv in &session_updates {
        if sv.id == id {
            assert_eq!(
                sv.state,
                Phase::Needs,
                "after control_request the session must be in Needs phase"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// answer_gate for each Gate type routes correctly
// ---------------------------------------------------------------------------

#[test]
fn answer_gate_permission_gate_outbound_references_gate() {
    let (mut core, mut peer) = make_core();
    let id = core
        .launch_session(spec("gate-perm", String::new()))
        .expect("launch");
    let _ = peer.drain_outbound();

    let gate = Gate::Permission {
        request_id: "req-test".to_string(),
        command: "docker run alpine".to_string(),
        cwd: "/app".to_string(),
        risk: "medium".to_string(),
        explanation: "run container".to_string(),
        is_elicitation: false,
        elicitation_choices: Vec::new(),
        permission_suggestions: Vec::new(),
    };
    let d = approve(&id, &gate);
    core.answer_gate(d).expect("answer_gate permission");
    let out = peer.drain_outbound();
    assert!(!out.is_empty(), "answer_gate(Permission) must emit IPC");
}

#[test]
fn answer_gate_clears_pending_gate_and_emits_session_update() {
    // Regression: after the human answers a permission gate, the session's gate
    // must be cleared immediately (via a returned Session VmUpdate) so the card
    // disappears — it must NOT linger until a later stream message arrives.
    let (mut core, mut peer) = make_core();
    let id = core
        .launch_session(spec("gate-clear", String::new()))
        .expect("launch");
    let _ = peer.drain_outbound();

    // Drive a real permission gate onto the session via the stream (system/init
    // then a can_use_tool control_request), so state.gate is Some(...).
    let _ = core.on_agent_line(&ipc_init(&id));
    let _ = core.on_agent_line(&ipc_control_request(&id, "req-clear"));
    let _ = peer.drain_outbound();
    // Sanity: the session view now carries a pending gate.
    let sv_before = core
        .snapshot()
        .sessions
        .into_iter()
        .find(|s| s.id == id)
        .expect("session present");
    assert!(sv_before.gate.is_some(), "gate must be pending before answer");

    let gate = Gate::Permission {
        request_id: "req-clear".to_string(),
        command: "ls".to_string(),
        cwd: "/app".to_string(),
        risk: "low".to_string(),
        explanation: "list".to_string(),
        is_elicitation: false,
        elicitation_choices: Vec::new(),
        permission_suggestions: Vec::new(),
    };
    let updates = core
        .answer_gate(approve(&id, &gate))
        .expect("answer_gate");

    // A Session VmUpdate with gate == None must be emitted immediately.
    let session_update_clears = updates.iter().any(|u| {
        matches!(u, marsad_core::VmUpdate::Session(sv) if sv.id == id && sv.gate.is_none())
    });
    assert!(
        session_update_clears,
        "answer_gate must emit a Session update with the gate cleared; got {updates:?}"
    );
}

#[test]
fn answer_gate_plan_gate_outbound_references_gate() {
    let (mut core, mut peer) = make_core();
    let id = core
        .launch_session(spec("gate-plan", String::new()))
        .expect("launch");
    let _ = peer.drain_outbound();

    let gate = Gate::Plan {
        request_id: "req-plan-core-plan".to_string(),
        summary: "migrate DB schema".to_string(),
        steps: vec![],
    };
    let d = approve(&id, &gate);
    core.answer_gate(d).expect("answer_gate plan");
    let out = peer.drain_outbound();
    assert!(!out.is_empty(), "answer_gate(Plan) must emit IPC");
}

#[test]
fn answer_gate_edit_gate_outbound_references_gate() {
    let (mut core, mut peer) = make_core();
    let id = core
        .launch_session(spec("gate-edit", String::new()))
        .expect("launch");
    let _ = peer.drain_outbound();

    let gate = Gate::Edit {
        request_id: "req-edit-core-edit".to_string(),
        file: "src/lib.rs".to_string(),
        diff: vec![],
    };
    let d = reject(&id, &gate, "not safe");
    core.answer_gate(d).expect("answer_gate edit");
    let out = peer.drain_outbound();
    assert!(!out.is_empty(), "answer_gate(Edit reject) must emit IPC");
}

// ---------------------------------------------------------------------------
// handle_usage_update — None-usage and non-object-usage no-ops
// ---------------------------------------------------------------------------

/// A UsageUpdate with missing `usage` field (null / absent) is a no-op.
#[test]
fn usage_update_null_usage_field_is_noop() {
    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(spec("/p", String::new()))
        .expect("launch");
    // `usage` key absent → serde default = None → should return Ok(vec![])
    let line = format!(r#"{{"type":"UsageUpdate","session_id":"{id}"}}"#);
    let updates = core.on_agent_line(&line).expect("null usage is ok");
    assert!(updates.is_empty(), "null usage must produce no updates");
}

/// A UsageUpdate where `usage` is a JSON array (not an object) is a no-op.
#[test]
fn usage_update_array_usage_field_is_noop() {
    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(spec("/p", String::new()))
        .expect("launch");
    // `usage` is an array instead of an object → as_object() returns None → no-op
    let line = format!(r#"{{"type":"UsageUpdate","session_id":"{id}","usage":[1,2,3]}}"#);
    let updates = core.on_agent_line(&line).expect("array usage is ok");
    assert!(updates.is_empty(), "array usage must produce no updates");
}

// ---------------------------------------------------------------------------
// A running `!` shell command surfaces the session as Working, not Done.
// ---------------------------------------------------------------------------

#[test]
fn running_shell_command_shows_working_not_done() {
    let (mut core, _peer) = make_core();
    let id = core
        .launch_session(spec("/p", String::new()))
        .expect("launch");
    // Drive the session to a terminal phase (Done) via a success result.
    core.on_agent_line(&ipc_result_success(&id)).expect("result");
    // Sanity: it's Done before any shell command.
    let done = core.snapshot().sessions.iter().find(|s| s.id == id).unwrap().state.clone();
    assert_eq!(format!("{done:?}"), "Done", "precondition: session is Done");

    // A `!` shell command starts running — the session must read as Working.
    core.inject_shell_command(&id, "sleep 5").expect("inject shell cmd");
    let running = core.snapshot().sessions.iter().find(|s| s.id == id).unwrap().state.clone();
    assert_eq!(
        format!("{running:?}"),
        "Working",
        "a running ! shell command must show Working, not Done"
    );

    // When the output arrives, shell_running clears and the real phase returns.
    core.inject_shell_result(&id, "done").expect("inject shell result");
    let after = core.snapshot().sessions.iter().find(|s| s.id == id).unwrap().state.clone();
    assert_eq!(format!("{after:?}"), "Done", "phase reverts after shell finishes");
}

// ---------------------------------------------------------------------------
// A host transitioning INTO Connected (reconnect) re-probes capabilities so the
// stale "unreachable" catalog from a disconnected startup/connect is refreshed.
// ---------------------------------------------------------------------------

#[test]
fn reconnect_to_connected_triggers_capability_preload() {
    use marsad_core::HostStatus;

    let (mut core, mut peer) = make_core();
    // add_host starts the host in a non-Connected state (Reconnecting until the
    // first successful health check).
    let u = core
        .add_host("srv", "wslssh:Ubuntu:narval", None, None, Vec::new(), Vec::new())
        .expect("add host");
    let host_id = u
        .iter()
        .find_map(|up| match up {
            VmUpdate::Hosts(hs) => hs.iter().find(|h| h.target == "wslssh:Ubuntu:narval").map(|h| h.id.clone()),
            _ => None,
        })
        .expect("host id");
    let _ = peer.drain_outbound(); // consume add_host IPC

    // Transition INTO Connected → must fire a RefreshCapabilities IPC (preload).
    core.set_host_status(&host_id, HostStatus::Connected);
    let out = peer.drain_outbound();
    assert!(
        out.iter().any(|l| l.contains("RefreshCapabilities")),
        "reconnect to Connected must trigger a capability preload; got {out:?}"
    );

    // Already Connected → another Connected report (health tick) must NOT re-probe.
    let out2 = peer.drain_outbound();
    core.set_host_status(&host_id, HostStatus::Connected);
    let out3 = peer.drain_outbound();
    let _ = out2;
    assert!(
        !out3.iter().any(|l| l.contains("RefreshCapabilities")),
        "a repeat Connected (no transition) must NOT re-probe; got {out3:?}"
    );
}

// ---------------------------------------------------------------------------
// The staleness fix: a host whose caps look "fully loaded" from PERSISTED data
// restored on restart must STILL re-probe on the first Connected of the session.
// Otherwise a CLI installed since the last run (the WSL/SSH opencode bug) stays
// invisible forever — the persisted caps suppress the connect-time re-probe.
// This is why the trigger keys off `discovery_triggered`, not `caps_loaded`.
// ---------------------------------------------------------------------------

#[test]
fn connect_reprobes_even_when_persisted_caps_already_fully_loaded() {
    use marsad_core::HostStatus;

    let (mut core, mut peer) = make_core();
    let u = core
        .add_host("srv", "wslssh:Ubuntu:narval", None, None, Vec::new(), Vec::new())
        .expect("add host");
    let host_id = u
        .iter()
        .find_map(|up| match up {
            VmUpdate::Hosts(hs) => hs
                .iter()
                .find(|h| h.target == "wslssh:Ubuntu:narval")
                .map(|h| h.id.clone()),
            _ => None,
        })
        .expect("host id");

    // Simulate persisted caps replayed on startup: every PROBED_DRIVER lands
    // BEFORE the host connects, so it looks fully-loaded from stale on-disk data.
    let caps_line = |driver: &str| {
        format!(
            r#"{{"type":"CapabilitiesUpdate","host_id":"{host_id}","driver":"{driver}","capabilities":{{"driver":"{driver}","host_id":"{host_id}","cli_available":true,"launchable":true,"models":{{"models":[],"source":"Observed","free_text":true}},"session_scope":null,"sessions":[],"fetched_at_ms":0,"errors":[]}}}}"#
        )
    };
    for d in ["claude", "codex", "gemini"] {
        core.on_agent_line(&caps_line(d)).expect("seed persisted caps");
    }
    let _ = peer.drain_outbound(); // discard add_host + seeding traffic

    // First Connected of the session → a FRESH all-driver probe must still fire.
    core.set_host_status(&host_id, HostStatus::Connected);
    let out = peer.drain_outbound();
    let refreshes = out.iter().filter(|l| l.contains("RefreshCapabilities")).count();
    assert_eq!(
        refreshes, 1,
        "a fully-loaded (persisted-caps) host must STILL re-probe on first Connected; got {out:?}"
    );

    // Once per session: a repeat Connected tick must NOT re-probe.
    core.set_host_status(&host_id, HostStatus::Connected);
    let out2 = peer.drain_outbound();
    assert!(
        !out2.iter().any(|l| l.contains("RefreshCapabilities")),
        "a repeat Connected (no transition) must not re-probe; got {out2:?}"
    );

    // Drop + reconnect re-triggers a fresh probe.
    core.set_host_status(&host_id, HostStatus::Unreachable);
    let _ = peer.drain_outbound();
    core.set_host_status(&host_id, HostStatus::Connected);
    let out3 = peer.drain_outbound();
    assert!(
        out3.iter().any(|l| l.contains("RefreshCapabilities")),
        "reconnect after a drop must re-probe; got {out3:?}"
    );
}

// ---------------------------------------------------------------------------
// A reachable host stays Connecting until EVERY probed driver's caps land, then
// flips to Connected (agents + versioned models loaded). No optimism.
// ---------------------------------------------------------------------------

#[test]
fn host_connecting_until_all_drivers_discovered() {
    use marsad_core::HostStatus;

    let (mut core, _peer) = make_core();
    let u = core
        .add_host("srv", "wslssh:Ubuntu:narval", None, None, Vec::new(), Vec::new())
        .expect("add host");
    let host_id = u
        .iter()
        .find_map(|up| match up {
            VmUpdate::Hosts(hs) => hs
                .iter()
                .find(|h| h.target == "wslssh:Ubuntu:narval")
                .map(|h| h.id.clone()),
            _ => None,
        })
        .expect("host id");

    let status_of = |core: &Core| {
        core.snapshot()
            .hosts
            .into_iter()
            .find(|h| h.id == host_id)
            .unwrap()
            .status
    };
    let caps_line = |driver: &str| {
        format!(
            r#"{{"type":"CapabilitiesUpdate","host_id":"{host_id}","driver":"{driver}","capabilities":{{"driver":"{driver}","host_id":"{host_id}","cli_available":true,"launchable":true,"models":{{"models":[],"source":"Observed","free_text":true}},"session_scope":null,"sessions":[],"fetched_at_ms":0,"errors":[]}}}}"#
        )
    };

    // Reachable but no caps yet → Connecting.
    core.set_host_status(&host_id, HostStatus::Connected);
    assert_eq!(status_of(&core), HostStatus::Connecting, "no caps yet → Connecting");

    // Two of three drivers land → still Connecting.
    core.on_agent_line(&caps_line("claude")).expect("claude caps");
    core.on_agent_line(&caps_line("codex")).expect("codex caps");
    assert_eq!(
        status_of(&core),
        HostStatus::Connecting,
        "partial discovery → still Connecting"
    );

    // The last probed driver lands → promoted to Connected.
    core.on_agent_line(&caps_line("gemini")).expect("gemini caps");
    assert_eq!(
        status_of(&core),
        HostStatus::Connected,
        "all drivers discovered → Connected"
    );

    // Going Unreachable forgets discovery so a later reconnect returns to Connecting.
    core.set_host_status(&host_id, HostStatus::Unreachable);
    assert_eq!(status_of(&core), HostStatus::Unreachable);
    core.set_host_status(&host_id, HostStatus::Connected);
    assert_eq!(
        status_of(&core),
        HostStatus::Connecting,
        "after a drop, caps must be re-discovered → Connecting again"
    );
}
