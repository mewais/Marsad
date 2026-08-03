//! marsad-events — happy path tests over `wasit_core::AgentEvent`.
//!
//! Wave 1: the reducer folds the rich `AgentEvent` model (built directly here via
//! the `common` helpers) instead of parsing raw claude stream-json. Display-field
//! extraction (a Bash gate's `$ …` command, elicitation fence parsing, question
//! parsing) now lives in the wasit drivers — Marsad receives already-shaped
//! gates — so those old parse-focused tests are gone; the gate tests here assert
//! the reducer shapes a typed `GateRequest` into a `marsad_gates::Gate` correctly.

mod common;
use common::*;

use marsad_events::{reduce, Event, HookEvent, Phase, SessionState, SubagentStatus};

// ---------------------------------------------------------------------------
// initial()
// ---------------------------------------------------------------------------

#[test]
fn initial_state_fields() {
    let s = SessionState::initial();
    assert_eq!(s.phase, Phase::Starting);
    assert_eq!(s.session_id, None);
    assert_eq!(s.model, None);
    assert_eq!(s.cwd, None);
    assert_eq!(s.tokens, 0);
    assert_eq!(s.cost_usd, 0.0);
    assert_eq!(s.context_pct, 0.0);
    assert_eq!(s.last_action, None);
    assert!(s.subagents.is_empty());
    assert!(s.gate.is_none());
    assert_eq!(s.changed_files, 0);
}

// ---------------------------------------------------------------------------
// Phase::Starting → Phase::Working via SessionInit
// ---------------------------------------------------------------------------

#[test]
fn session_init_transitions_to_working() {
    let s1 = after_init(session_init("sess-01", "claude-opus-4-5", "/tmp/work"));
    assert_eq!(s1.phase, Phase::Working);
    assert_eq!(s1.session_id, Some("sess-01".to_string()));
    assert_eq!(s1.model, Some("claude-opus-4-5".to_string()));
    assert_eq!(s1.cwd, Some("/tmp/work".to_string()));
}

// ---------------------------------------------------------------------------
// Phase::Working via assistant delta (no token accounting here anymore)
// ---------------------------------------------------------------------------

#[test]
fn assistant_delta_keeps_working() {
    let s0 = after_init(session_init("sess-01", "claude-opus-4-5", "/tmp/work"));
    let s1 = reduce(s0, ev(assistant_delta("Thinking…")));
    assert_eq!(s1.phase, Phase::Working);
    // Usage no longer arrives on assistant deltas — tokens stay 0 until a
    // UsageUpdate / TurnCompleted.
    assert_eq!(s1.tokens, 0);
}

// ---------------------------------------------------------------------------
// Phase::Needs via a permission gate
// ---------------------------------------------------------------------------

#[test]
fn permission_gate_transitions_to_needs() {
    let s0 = after_init(session_init("sess-01", "claude-opus-4-5", "/tmp/work"));
    let s1 = reduce(s0, ev(permission_gate("req-1", "ls -la", "list files")));
    assert_eq!(s1.phase, Phase::Needs);
    assert!(s1.gate.is_some());
}

// Regression (gate flash-and-vanish): a pending gate means the agent is BLOCKED.
// A subsequent assistant delta / tool call in the SAME turn must NOT flip the
// phase back to Working — keyed on `gate.is_some()`.
#[test]
fn assistant_after_gate_keeps_needs() {
    let s0 = after_init(session_init("sess-01", "claude-opus-4-5", "/tmp/work"));
    let s1 = reduce(s0, ev(permission_gate("req-1", "ls", "list")));
    assert_eq!(s1.phase, Phase::Needs);
    let s2 = reduce(s1, ev(assistant_delta("still reasoning")));
    assert_eq!(
        s2.phase,
        Phase::Needs,
        "an assistant delta must not clobber a still-open gate"
    );
    assert!(s2.gate.is_some());
}

// Regression: a turn-level `TurnCompleted` is the agent's, not the human's
// answer, so it must not mark the session Done out from under a pending gate.
// But cost/tokens from the result are still recorded.
#[test]
fn turn_completed_after_gate_keeps_needs_but_records_cost() {
    let s0 = after_init(session_init("sess-01", "claude-opus-4-5", "/tmp/work"));
    let s1 = reduce(s0, ev(permission_gate("req-1", "ls", "list")));
    assert_eq!(s1.phase, Phase::Needs);
    let s2 = reduce(s1, ev(turn_success(usage(100, 20, 0, 0, 1.0))));
    assert_eq!(s2.phase, Phase::Needs, "a result must not clobber an open gate");
    assert!(s2.gate.is_some());
    assert_eq!(s2.cost_usd, 1.0);
}

// ---------------------------------------------------------------------------
// The reducer shapes a typed Permission GateRequest into a marsad_gates::Gate,
// passing command/explanation VERBATIM (wasit computed them) and falling back to
// the session cwd when the request carries none.
// ---------------------------------------------------------------------------

#[test]
fn permission_gate_carries_command_and_explanation_verbatim() {
    let s0 = after_init(session_init("sess-01", "claude-opus-4-5", "/home/user/project"));
    let s1 = reduce(
        s0,
        ev(permission_gate("req-1", "rm -rf /tmp/x", "Delete the temp dir")),
    );
    match s1.gate.as_ref().expect("gate present") {
        marsad_gates::Gate::Permission {
            command,
            explanation,
            cwd,
            ..
        } => {
            assert_eq!(command, "rm -rf /tmp/x");
            assert_eq!(explanation, "Delete the temp dir");
            // cwd was empty on the request → falls back to the session cwd.
            assert_eq!(cwd, "/home/user/project");
        }
        other => panic!("expected Permission gate, got {other:?}"),
    }
}

#[test]
fn permission_gate_uses_request_cwd_when_present() {
    let s0 = after_init(session_init("sess-01", "claude-opus-4-5", "/session/cwd"));
    let s1 = reduce(
        s0,
        ev(permission_gate_full(
            "req-2",
            "Read",
            "Read /etc/hosts",
            "/gate/cwd",
            "",
            false,
            vec![],
            vec![],
        )),
    );
    match s1.gate.as_ref().expect("gate present") {
        marsad_gates::Gate::Permission { cwd, .. } => assert_eq!(cwd, "/gate/cwd"),
        other => panic!("expected Permission gate, got {other:?}"),
    }
}

#[test]
fn elicitation_gate_sets_is_elicitation_true() {
    let s0 = after_init(session_init("sess-01", "claude-opus-4-5", "/tmp/work"));
    let s1 = reduce(
        s0,
        ev(permission_gate_full(
            "eli-1",
            "mcp__hares__run",
            "",
            "",
            "Approve?",
            true,
            vec![],
            vec![],
        )),
    );
    match s1.gate.as_ref().expect("gate present") {
        marsad_gates::Gate::Permission { is_elicitation, .. } => {
            assert!(*is_elicitation, "elicitation gate must have is_elicitation==true");
        }
        other => panic!("expected Permission gate, got {other:?}"),
    }
}

#[test]
fn can_use_tool_gate_sets_is_elicitation_false() {
    let s0 = after_init(session_init("sess-01", "claude-opus-4-5", "/tmp/work"));
    let s1 = reduce(s0, ev(permission_gate("req-tool", "ls", "list")));
    match s1.gate.as_ref().expect("gate present") {
        marsad_gates::Gate::Permission { is_elicitation, .. } => {
            assert!(!is_elicitation, "can_use_tool gate must have is_elicitation==false");
        }
        other => panic!("expected Permission gate, got {other:?}"),
    }
}

#[test]
fn elicitation_choices_pass_through() {
    use wasit_core::ElicitationChoice;
    let s0 = after_init(session_init("sess-01", "claude-opus-4-5", "/tmp/work"));
    let choices = vec![
        ElicitationChoice {
            key: "grant".to_string(),
            value: "session".to_string(),
            label: "This session".to_string(),
        },
        ElicitationChoice {
            key: "grant".to_string(),
            value: "once".to_string(),
            label: "Just once".to_string(),
        },
    ];
    let s1 = reduce(
        s0,
        ev(permission_gate_full(
            "eli-c",
            "mcp__x",
            "",
            "",
            "Grant?",
            true,
            choices.clone(),
            vec![],
        )),
    );
    match s1.gate.as_ref().expect("gate present") {
        marsad_gates::Gate::Permission {
            elicitation_choices,
            ..
        } => {
            assert_eq!(elicitation_choices.len(), 2);
            assert_eq!(elicitation_choices[0].value, "session");
            assert_eq!(elicitation_choices[1].value, "once");
        }
        other => panic!("expected Permission gate, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Phase::Done via TurnCompleted/Success
// ---------------------------------------------------------------------------

#[test]
fn turn_success_transitions_to_done() {
    let s0 = after_init(session_init("sess-01", "claude-opus-4-5", "/tmp/work"));
    let s1 = reduce(s0, ev(assistant_delta("done")));
    let s2 = reduce(s1, ev(turn_success(usage(100, 20, 0, 0, 1.23))));
    assert_eq!(s2.phase, Phase::Done);
    assert!(s2.cost_usd > 0.0);
    // Cumulative tokens from the authoritative TurnCompleted usage.
    assert_eq!(s2.tokens, 120);
}

// ---------------------------------------------------------------------------
// Full lifecycle: Starting → Working → Needs → Working (gate answered) → Done
// ---------------------------------------------------------------------------

#[test]
fn full_nominal_lifecycle() {
    let mut s = SessionState::initial();
    assert_eq!(s.phase, Phase::Starting);

    s = reduce(s, ev(session_init("sess-01", "claude-opus-4-5", "/tmp/work")));
    assert_eq!(s.phase, Phase::Working);

    s = reduce(s, ev(assistant_delta("working")));
    assert_eq!(s.phase, Phase::Working);

    s = reduce(s, ev(permission_gate("req-1", "ls", "list")));
    assert_eq!(s.phase, Phase::Needs);

    // The human answers via the optimistic clear (control channel, not a stream
    // event — the old `user` turn no longer clears gates).
    s.clear_gate_on_answer();
    assert_eq!(s.phase, Phase::Working);
    assert!(s.gate.is_none());

    s = reduce(s, ev(assistant_delta("more work")));
    assert_eq!(s.phase, Phase::Working);

    s = reduce(s, ev(turn_success(usage(500, 130, 0, 0, 2.50))));
    assert_eq!(s.phase, Phase::Done);
    assert!(s.cost_usd > 0.0);
}

// ---------------------------------------------------------------------------
// Usage snapshot: UsageUpdate replaces (not accumulates) the token total
// ---------------------------------------------------------------------------

#[test]
fn usage_update_is_a_snapshot_not_accumulation() {
    let s0 = after_init(session_init("sess-01", "claude-opus-4-5", "/tmp/work"));
    let s1 = reduce(s0, ev(usage_update(usage(100, 10, 0, 0, 0.1), None)));
    assert_eq!(s1.tokens, 110);
    // A later snapshot REPLACES — it does not add to the previous 110.
    let s2 = reduce(s1, ev(usage_update(usage(200, 20, 0, 0, 0.2), None)));
    assert_eq!(s2.tokens, 220);
    assert!((s2.cost_usd - 0.2).abs() < 1e-9);
}

// ---------------------------------------------------------------------------
// cost_usd updated from TurnCompleted
// ---------------------------------------------------------------------------

#[test]
fn cost_updated_from_turn_completed() {
    let s0 = after_init(session_init("sess-01", "claude-opus-4-5", "/tmp/work"));
    let s1 = reduce(s0, ev(turn_success(usage(100, 10, 0, 0, 0.42))));
    assert!(
        (s1.cost_usd - 0.42).abs() < 1e-9,
        "expected cost 0.42, got {}",
        s1.cost_usd
    );
}

// ---------------------------------------------------------------------------
// last_action updated by ToolCallStarted
// ---------------------------------------------------------------------------

#[test]
fn last_action_set_by_tool_call() {
    let s0 = after_init(session_init("sess-01", "claude-opus-4-5", "/tmp/work"));
    let s1 = reduce(
        s0,
        ev(tool_call(
            "Read",
            "tu-1",
            serde_json::json!({"path": "/etc/passwd"}),
        )),
    );
    let action = s1.last_action.expect("last_action must be set after a tool call");
    assert!(action.contains("Read"), "expected 'Read' in last_action, got: {action}");
}

// ---------------------------------------------------------------------------
// Subagent: SubagentStop hook records subagent as Done
// ---------------------------------------------------------------------------

#[test]
fn subagent_stop_hook_marks_done() {
    let s0 = after_init(session_init("sess-01", "claude-opus-4-5", "/tmp/work"));
    let s1 = reduce(
        s0,
        Event::Hook(HookEvent::SubagentStop {
            name: "worker-1".to_string(),
        }),
    );
    let agent = s1.subagents.iter().find(|a| a.name == "worker-1");
    assert!(agent.is_some(), "SubagentStop must record the subagent");
    assert_eq!(agent.unwrap().status, SubagentStatus::Done);
}

// ---------------------------------------------------------------------------
// Hooks
// ---------------------------------------------------------------------------

#[test]
fn hook_session_start_is_total() {
    let s1 = reduce(SessionState::initial(), Event::Hook(HookEvent::SessionStart));
    let _ = s1; // must not panic
}

#[test]
fn hook_stop_transitions_to_done() {
    let s0 = after_init(session_init("sess-01", "claude-opus-4-5", "/tmp/work"));
    let s1 = reduce(s0, Event::Hook(HookEvent::Stop));
    assert_eq!(s1.phase, Phase::Done);
}

#[test]
fn hook_notification_transitions_to_needs() {
    let s0 = after_init(session_init("sess-01", "claude-opus-4-5", "/tmp/work"));
    let s1 = reduce(
        s0,
        Event::Hook(HookEvent::Notification {
            message: "Attention needed".to_string(),
        }),
    );
    assert_eq!(s1.phase, Phase::Needs);
}

#[test]
fn hook_pre_post_tool_use_keeps_working() {
    let s0 = after_init(session_init("sess-01", "claude-opus-4-5", "/tmp/work"));
    let s1 = reduce(
        s0,
        Event::Hook(HookEvent::PreToolUse {
            tool: "Bash".to_string(),
        }),
    );
    assert_eq!(s1.phase, Phase::Working);
    let s2 = reduce(
        s1,
        Event::Hook(HookEvent::PostToolUse {
            tool: "Bash".to_string(),
        }),
    );
    assert_eq!(s2.phase, Phase::Working);
}

#[test]
fn hook_pre_compact_is_total_and_working() {
    let s0 = after_init(session_init("sess-01", "claude-opus-4-5", "/tmp/work"));
    let s1 = reduce(s0, Event::Hook(HookEvent::PreCompact));
    assert_eq!(s1.phase, Phase::Working);
}

// ---------------------------------------------------------------------------
// model_usage — populated from TurnCompleted per_model (replace)
// ---------------------------------------------------------------------------

#[test]
fn turn_completed_with_per_model_populates_state() {
    use std::collections::HashMap;
    use wasit_core::ModelUsage;
    let s0 = after_init(session_init("sess-01", "claude-opus-4-5", "/tmp/work"));
    let mut per_model = HashMap::new();
    per_model.insert(
        "claude-sonnet-4.6[1m]".to_string(),
        ModelUsage {
            input_tokens: 11,
            output_tokens: 155,
            cost_usd: 0.268,
            estimated: false,
        },
    );
    let s1 = reduce(s0, ev(turn_success_per_model(usage(11, 155, 0, 0, 0.268), per_model)));
    assert!(!s1.model_usage.is_empty(), "model_usage must be populated");
    let mu = s1.model_usage.get("claude-sonnet-4.6[1m]").expect("key must exist");
    assert_eq!(mu.input_tokens, 11);
    assert_eq!(mu.output_tokens, 155);
    assert!((mu.cost_usd - 0.268).abs() < 1e-9);
}

// ---------------------------------------------------------------------------
// tools / mcp_servers / slash_commands — populated from SessionInit
// ---------------------------------------------------------------------------

#[test]
fn session_init_populates_tools_mcp_servers_slash_commands() {
    use marsad_proto::{McpServerInfo, SlashCommandInfo};
    let init = session_init_full(
        "s1",
        "m",
        "/",
        vec!["Read".into(), "Bash".into(), "Edit".into()],
        vec![
            McpServerInfo {
                name: "hares".to_string(),
                status: "failed".to_string(),
            },
            McpServerInfo {
                name: "hub".to_string(),
                status: "connected".to_string(),
            },
        ],
        vec![
            SlashCommandInfo {
                name: "/clear".to_string(),
                description: "Clear conversation history".to_string(),
                argument_hint: "".to_string(),
            },
            SlashCommandInfo {
                name: "/model".to_string(),
                description: "Switch model".to_string(),
                argument_hint: "model-name".to_string(),
            },
        ],
    );
    let s1 = after_init(init);
    assert_eq!(s1.tools, vec!["Read", "Bash", "Edit"]);
    assert_eq!(s1.mcp_servers.len(), 2);
    assert_eq!(s1.mcp_servers[0].name, "hares");
    assert_eq!(s1.mcp_servers[0].status, "failed");
    assert_eq!(s1.mcp_servers[1].name, "hub");
    assert_eq!(s1.slash_commands.len(), 2);
    assert_eq!(s1.slash_commands[0].name, "/clear");
    assert_eq!(s1.slash_commands[1].name, "/model");
    assert_eq!(s1.slash_commands[1].argument_hint, "model-name");
}

#[test]
fn initial_state_new_fields_empty() {
    let s = SessionState::initial();
    assert!(s.tools.is_empty());
    assert!(s.mcp_servers.is_empty());
    assert!(s.slash_commands.is_empty());
    assert!(s.permission_mode.is_none());
}

// ---------------------------------------------------------------------------
// permission_mode — stored from SessionInit
// ---------------------------------------------------------------------------

#[test]
fn session_init_stores_permission_mode_default() {
    let s = after_init(session_init_mode("s1", "m", "/", "default"));
    assert_eq!(s.permission_mode, Some("default".to_string()));
}

#[test]
fn session_init_stores_permission_mode_accept_edits() {
    let s = after_init(session_init_mode("s1", "m", "/", "acceptEdits"));
    assert_eq!(s.permission_mode, Some("acceptEdits".to_string()));
}

#[test]
fn session_init_stores_permission_mode_bypass() {
    let s = after_init(session_init_mode("s1", "m", "/", "bypassPermissions"));
    assert_eq!(s.permission_mode, Some("bypassPermissions".to_string()));
}

// ---------------------------------------------------------------------------
// Subagents via ToolCallStarted(is_subagent) / ToolCallFinished
// ---------------------------------------------------------------------------

#[test]
fn subagent_call_marks_subagent_running() {
    let s0 = after_init(session_init("sess-01", "claude-opus-4-5", "/tmp/work"));
    let s1 = reduce(
        s0,
        ev(subagent_call(
            "tu-task-1",
            serde_json::json!({"subagent_type": "researcher", "description": "Do work"}),
            None,
        )),
    );
    assert_eq!(s1.subagents.len(), 1);
    assert_eq!(s1.subagents[0].name, "researcher");
    assert_eq!(s1.subagents[0].status, SubagentStatus::Running);
}

#[test]
fn tool_finished_marks_subagent_done() {
    let s0 = after_init(session_init("sess-01", "claude-opus-4-5", "/tmp/work"));
    let s1 = reduce(
        s0,
        ev(subagent_call(
            "tu-task-2",
            serde_json::json!({"subagent_type": "coder"}),
            None,
        )),
    );
    assert_eq!(s1.subagents[0].status, SubagentStatus::Running);
    let s2 = reduce(s1, ev(tool_finished("tu-task-2", "Task", true)));
    assert_eq!(s2.subagents[0].status, SubagentStatus::Done);
}

#[test]
fn tool_finished_unknown_id_is_noop() {
    let s0 = after_init(session_init("sess-01", "claude-opus-4-5", "/tmp/work"));
    let s1 = reduce(s0, ev(tool_finished("unknown-tu-id", "Task", true)));
    assert!(s1.subagents.is_empty());
}

#[test]
fn subagent_call_uses_description_fallback() {
    let s0 = after_init(session_init("sess-01", "claude-opus-4-5", "/tmp/work"));
    let s1 = reduce(
        s0,
        ev(subagent_call(
            "tu-fb",
            serde_json::json!({"description": "analyze codebase"}),
            None,
        )),
    );
    assert_eq!(s1.subagents[0].name, "analyze codebase");
    assert_eq!(s1.subagents[0].status, SubagentStatus::Running);
}

#[test]
fn subagent_call_uses_default_name_when_input_empty() {
    let s0 = after_init(session_init("sess-01", "claude-opus-4-5", "/tmp/work"));
    let s1 = reduce(s0, ev(subagent_call("tu-def", serde_json::json!({}), None)));
    assert_eq!(s1.subagents[0].name, "subagent");
}

#[test]
fn subagent_carries_model_override() {
    let s0 = after_init(session_init("sess-01", "claude-opus-4-5", "/tmp/work"));
    let s1 = reduce(
        s0,
        ev(subagent_call(
            "tu-m",
            serde_json::json!({"subagent_type": "worker"}),
            Some("claude-haiku".to_string()),
        )),
    );
    assert_eq!(s1.subagents[0].model.as_deref(), Some("claude-haiku"));
}

#[test]
fn two_subagents_both_resolve_to_done() {
    let s0 = after_init(session_init("sess-01", "claude-opus-4-5", "/tmp/work"));
    let s1 = reduce(
        s0,
        ev(subagent_call(
            "tu-a",
            serde_json::json!({"subagent_type": "worker-a"}),
            None,
        )),
    );
    let s2 = reduce(
        s1,
        ev(subagent_call(
            "tu-b",
            serde_json::json!({"subagent_type": "worker-b"}),
            None,
        )),
    );
    assert_eq!(s2.subagents.len(), 2);
    assert_eq!(s2.subagents[0].status, SubagentStatus::Running);
    assert_eq!(s2.subagents[1].status, SubagentStatus::Running);

    let s3 = reduce(s2, ev(tool_finished("tu-a", "Task", true)));
    assert_eq!(s3.subagents[0].status, SubagentStatus::Done);
    assert_eq!(s3.subagents[1].status, SubagentStatus::Running);

    let s4 = reduce(s3, ev(tool_finished("tu-b", "Task", true)));
    assert_eq!(s4.subagents[0].status, SubagentStatus::Done);
    assert_eq!(s4.subagents[1].status, SubagentStatus::Done);
}

#[test]
fn subagent_stop_hook_still_records_done_alongside_call_tracking() {
    let s0 = after_init(session_init("sess-01", "claude-opus-4-5", "/tmp/work"));
    let s1 = reduce(
        s0,
        ev(subagent_call(
            "tu-t4",
            serde_json::json!({"subagent_type": "helper"}),
            None,
        )),
    );
    let s2 = reduce(
        s1,
        Event::Hook(HookEvent::SubagentStop {
            name: "external-agent".to_string(),
        }),
    );
    assert_eq!(s2.subagents.len(), 2);
    assert_eq!(s2.subagents[1].status, SubagentStatus::Done);
    assert_eq!(s2.subagents[1].name, "external-agent");
    assert_eq!(s2.subagents[0].status, SubagentStatus::Running);
}
