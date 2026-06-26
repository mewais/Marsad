//! B02 `marsad-events` — branch coverage.
//!
//! Covers every Phase variant, the DEDUP-by-message-id invariant, context_pct
//! clamp to 100, context_window_for table (known models + fallback), every
//! HookEvent variant, and Stale/Detached preservation.
//!
//! Key invariants explicitly tested and labelled:
//!   DEDUP — folding the same assistant message id twice must NOT double-count tokens.
//!   CLAMP — context_pct must never exceed 100.0 even when tokens > context_window.

use marsad_events::{reduce, Event, HookEvent, Phase, SessionState, SubagentStatus};
use marsad_proto::parse_stream_line;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn stream(json: &str) -> Event {
    let msg = parse_stream_line(json)
        .unwrap_or_else(|e| panic!("test JSON failed to parse: {e:?}\njson: {json}"));
    Event::Stream(msg)
}

fn system_init_for_model(model: &str) -> String {
    format!(
        r#"{{"type":"system","subtype":"init","session_id":"sess-br","model":"{model}","cwd":"/tmp","tools":[],"mcp_servers":[],"permissionMode":"default"}}"#
    )
}

fn assistant_json(id: &str, input: u64, output: u64) -> String {
    format!(
        r#"{{"type":"assistant","message":{{"id":"{id}","content":[{{"type":"text","text":"x"}}],"usage":{{"input_tokens":{input},"output_tokens":{output},"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}"#
    )
}

fn result_success_json() -> &'static str {
    r#"{"type":"result","subtype":"success","is_error":false,"total_cost_usd":0.5,"usage":{"input_tokens":100,"output_tokens":20,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},"num_turns":1,"permission_denials":[]}"#
}

fn result_error_json(subtype: &str) -> String {
    format!(
        r#"{{"type":"result","subtype":"{subtype}","is_error":false,"total_cost_usd":0.01,"usage":{{"input_tokens":10,"output_tokens":2,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}},"num_turns":1,"permission_denials":[]}}"#
    )
}

fn init_with_model(model: &str) -> SessionState {
    reduce(
        SessionState::initial(),
        stream(&system_init_for_model(model)),
    )
}

// ---------------------------------------------------------------------------
// Every Phase reachable
// ---------------------------------------------------------------------------

#[test]
fn phase_starting_is_initial() {
    assert_eq!(SessionState::initial().phase, Phase::Starting);
}

#[test]
fn phase_working_reachable_via_system_init() {
    let s = init_with_model("claude-opus-4-5");
    assert_eq!(s.phase, Phase::Working);
}

#[test]
fn phase_needs_reachable_via_control_request() {
    let s0 = init_with_model("claude-opus-4-5");
    let s1 = reduce(
        s0,
        stream(
            r#"{"type":"control_request","request_id":"req-1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{}}}"#,
        ),
    );
    assert_eq!(s1.phase, Phase::Needs);
}

#[test]
fn phase_needs_reachable_via_notification_hook() {
    let s0 = init_with_model("claude-opus-4-5");
    let s1 = reduce(
        s0,
        Event::Hook(HookEvent::Notification {
            message: "Something needs attention".to_string(),
        }),
    );
    assert_eq!(s1.phase, Phase::Needs);
}

#[test]
fn phase_error_reachable_via_error_during_execution() {
    let s0 = init_with_model("claude-opus-4-5");
    let s1 = reduce(s0, stream(&result_error_json("error_during_execution")));
    assert_eq!(s1.phase, Phase::Error);
}

#[test]
fn phase_error_reachable_via_max_turns() {
    let s0 = init_with_model("claude-opus-4-5");
    let s1 = reduce(s0, stream(&result_error_json("error_max_turns")));
    assert_eq!(s1.phase, Phase::Error);
}

#[test]
fn phase_done_reachable_via_result_success() {
    let s0 = init_with_model("claude-opus-4-5");
    let s1 = reduce(s0, stream(result_success_json()));
    assert_eq!(s1.phase, Phase::Done);
}

#[test]
fn phase_done_reachable_via_stop_hook() {
    let s0 = init_with_model("claude-opus-4-5");
    let s1 = reduce(s0, Event::Hook(HookEvent::Stop));
    assert_eq!(s1.phase, Phase::Done);
}

#[test]
fn phase_stale_set_externally_and_preserved_by_hook() {
    let mut s = init_with_model("claude-opus-4-5");
    s.phase = Phase::Stale;
    // A non-superseding hook (PreCompact) should preserve Stale
    let s1 = reduce(s, Event::Hook(HookEvent::PreCompact));
    assert_eq!(s1.phase, Phase::Stale);
}

#[test]
fn phase_detached_set_externally_and_preserved_by_hook() {
    let mut s = init_with_model("claude-opus-4-5");
    s.phase = Phase::Detached;
    let s1 = reduce(s, Event::Hook(HookEvent::PreCompact));
    assert_eq!(s1.phase, Phase::Detached);
}

#[test]
fn phase_stale_preserved_by_unknown_stream() {
    let mut s = init_with_model("claude-opus-4-5");
    s.phase = Phase::Stale;
    let s1 = reduce(s, stream(r#"{"type":"future_msg","data":1}"#));
    assert_eq!(s1.phase, Phase::Stale);
}

#[test]
fn phase_detached_preserved_by_unknown_stream() {
    let mut s = init_with_model("claude-opus-4-5");
    s.phase = Phase::Detached;
    let s1 = reduce(s, stream(r#"{"type":"future_msg","data":1}"#));
    assert_eq!(s1.phase, Phase::Detached);
}

// ---------------------------------------------------------------------------
// DEDUP-BY-MESSAGE-ID
// Folding the same assistant message id twice must NOT double-count tokens.
// This is the critical spec invariant: "dedup assistant usage by message id".
// ---------------------------------------------------------------------------

#[test]
fn dedup_same_id_does_not_double_count() {
    // [DEDUP] Core invariant test.
    let s0 = init_with_model("claude-opus-4-5");

    let s1 = reduce(s0, stream(&assistant_json("msg-dup", 100, 20)));
    let tokens_after_first = s1.tokens;
    assert_eq!(tokens_after_first, 120, "first fold: 100+20=120");

    // Fold the SAME message id again — must NOT increase the token count.
    let s2 = reduce(s1, stream(&assistant_json("msg-dup", 100, 20)));
    assert_eq!(
        s2.tokens, tokens_after_first,
        "[DEDUP] folding msg-dup twice must not increase token count: \
         first={tokens_after_first} second={}",
        s2.tokens
    );
}

#[test]
fn dedup_different_ids_do_accumulate() {
    // [DEDUP] Different message ids MUST each contribute their tokens.
    let s0 = init_with_model("claude-opus-4-5");
    let s1 = reduce(s0, stream(&assistant_json("msg-A", 100, 20))); // 120
    let s2 = reduce(s1, stream(&assistant_json("msg-B", 200, 30))); // 230
    assert_eq!(s2.tokens, 350, "msg-A (120) + msg-B (230) = 350");
}

#[test]
fn dedup_triple_replay_still_no_double_count() {
    // [DEDUP] Even three replays of the same id stay deduped.
    let s0 = init_with_model("claude-opus-4-5");
    let s1 = reduce(s0, stream(&assistant_json("msg-trip", 50, 10))); // 60
    let s2 = reduce(s1, stream(&assistant_json("msg-trip", 50, 10))); // still 60
    let s3 = reduce(s2, stream(&assistant_json("msg-trip", 50, 10))); // still 60
    assert_eq!(
        s3.tokens, 60,
        "[DEDUP] three replays of msg-trip must stay at 60 tokens, got {}",
        s3.tokens
    );
}

// ---------------------------------------------------------------------------
// context_pct clamp to 100
// ---------------------------------------------------------------------------

#[test]
fn context_pct_computed_roughly_at_half_window() {
    let model = "claude-opus-4-5";
    let window = SessionState::context_window_for(model);
    assert!(window > 0);

    let s0 = init_with_model(model);
    let half = window / 2;
    let s1 = reduce(s0, stream(&assistant_json("msg-half", half, 0)));
    assert!(
        (s1.context_pct - 50.0).abs() < 2.0,
        "expected ~50% context at half-window, got {}",
        s1.context_pct
    );
}

#[test]
fn context_pct_clamped_to_100_on_overflow() {
    // [CLAMP] Feeding 2× the window must clamp to exactly 100.0.
    let model = "claude-opus-4-5";
    let window = SessionState::context_window_for(model);
    let s0 = init_with_model(model);
    let s1 = reduce(s0, stream(&assistant_json("msg-over", window * 2, 0)));
    assert_eq!(
        s1.context_pct, 100.0,
        "[CLAMP] context_pct must be clamped to 100.0 when tokens exceed window, got {}",
        s1.context_pct
    );
}

#[test]
fn context_pct_never_exceeds_100_after_many_messages() {
    // [CLAMP] Accumulating many large messages across different ids.
    let model = "claude-haiku-3-5";
    let window = SessionState::context_window_for(model);
    let mut s = init_with_model(model);
    for i in 0u64..5 {
        s = reduce(s, stream(&assistant_json(&format!("msg-{i}"), window, 0)));
    }
    assert!(
        s.context_pct <= 100.0,
        "[CLAMP] context_pct exceeded 100 after accumulation: {}",
        s.context_pct
    );
}

// ---------------------------------------------------------------------------
// context_window_for: known models
// ---------------------------------------------------------------------------

#[test]
fn context_window_for_known_models_positive() {
    let models = [
        "claude-opus-4-5",
        "claude-sonnet-4-5",
        "claude-haiku-3-5",
        "claude-opus-4.8[1m]",
        "claude-sonnet-4-6[1m]",
        "claude-3-opus-20240229",
        "claude-3-5-sonnet-20241022",
        "claude-3-5-haiku-20241022",
        "claude-3-haiku-20240307",
    ];
    for model in &models {
        let w = SessionState::context_window_for(model);
        assert!(w > 0, "context_window_for({model}) must be > 0, got {w}");
    }
}

#[test]
fn context_window_for_1m_suffix_returns_1_000_000() {
    // The "[1m]" suffix signals a 1M-token context window.
    // qa.ndjson modelUsage shows contextWindow=1000000 for "claude-opus-4.8[1m]".
    let w = SessionState::context_window_for("claude-opus-4.8[1m]");
    assert_eq!(
        w, 1_000_000,
        "1M-context model must return 1_000_000, got {w}"
    );
}

#[test]
fn context_window_for_unknown_model_returns_nonzero_fallback() {
    let w = SessionState::context_window_for("hypothetical-future-model-xyz-999");
    assert!(w > 0, "unknown model fallback context window must be > 0");
}

// ---------------------------------------------------------------------------
// Every HookEvent variant: no panic, expected outcome
// ---------------------------------------------------------------------------

#[test]
fn hook_all_variants_total() {
    let hooks = vec![
        Event::Hook(HookEvent::Notification {
            message: "test".to_string(),
        }),
        Event::Hook(HookEvent::Stop),
        Event::Hook(HookEvent::SubagentStop {
            name: "agent-x".to_string(),
        }),
        Event::Hook(HookEvent::PreToolUse {
            tool: "Write".to_string(),
        }),
        Event::Hook(HookEvent::PostToolUse {
            tool: "Write".to_string(),
        }),
        Event::Hook(HookEvent::SessionStart),
        Event::Hook(HookEvent::PreCompact),
    ];
    // Each hook applied to a fresh init state must not panic.
    for hook in hooks {
        let s0 = init_with_model("claude-opus-4-5");
        let _s1 = reduce(s0, hook);
    }
}

#[test]
fn hook_notification_gives_needs() {
    let s0 = init_with_model("claude-opus-4-5");
    let s1 = reduce(
        s0,
        Event::Hook(HookEvent::Notification {
            message: "confirm tool use".to_string(),
        }),
    );
    assert_eq!(s1.phase, Phase::Needs);
}

#[test]
fn hook_stop_gives_done() {
    let s0 = init_with_model("claude-opus-4-5");
    let s1 = reduce(s0, Event::Hook(HookEvent::Stop));
    assert_eq!(s1.phase, Phase::Done);
}

#[test]
fn hook_subagent_stop_records_agent() {
    let s0 = init_with_model("claude-opus-4-5");
    let s1 = reduce(
        s0,
        Event::Hook(HookEvent::SubagentStop {
            name: "search-agent".to_string(),
        }),
    );
    let agent = s1.subagents.iter().find(|a| a.name == "search-agent");
    assert!(agent.is_some(), "SubagentStop must record the subagent");
    assert_eq!(agent.unwrap().status, SubagentStatus::Done);
}

#[test]
fn hook_pre_tool_use_keeps_working() {
    let s0 = init_with_model("claude-opus-4-5");
    let s1 = reduce(
        s0,
        Event::Hook(HookEvent::PreToolUse {
            tool: "Read".to_string(),
        }),
    );
    assert_eq!(s1.phase, Phase::Working);
}

#[test]
fn hook_post_tool_use_keeps_working() {
    let s0 = init_with_model("claude-opus-4-5");
    let s1 = reduce(
        s0,
        Event::Hook(HookEvent::PostToolUse {
            tool: "Read".to_string(),
        }),
    );
    assert_eq!(s1.phase, Phase::Working);
}

#[test]
fn hook_session_start_total_from_starting() {
    let s0 = SessionState::initial();
    let s1 = reduce(s0, Event::Hook(HookEvent::SessionStart));
    let _ = s1.phase; // must not panic
}

#[test]
fn hook_pre_compact_preserves_working() {
    let s0 = init_with_model("claude-opus-4-5");
    let s1 = reduce(s0, Event::Hook(HookEvent::PreCompact));
    assert_eq!(s1.phase, Phase::Working);
}

// ---------------------------------------------------------------------------
// Stale / Detached: superseded ONLY by a real SystemInit; all other stream
// messages (Assistant, User, Result, control) are totality no-ops.
// ---------------------------------------------------------------------------

#[test]
fn stale_superseded_by_new_system_init() {
    let mut s = init_with_model("claude-opus-4-5");
    s.phase = Phase::Stale;
    let s1 = reduce(
        s,
        stream(
            r#"{"type":"system","subtype":"init","session_id":"sess-new","model":"claude-opus-4-5","cwd":"/tmp","tools":[],"mcp_servers":[],"permissionMode":"default"}"#,
        ),
    );
    assert_eq!(s1.phase, Phase::Working, "SystemInit must supersede Stale");
}

#[test]
fn detached_superseded_by_new_system_init() {
    let mut s = init_with_model("claude-opus-4-5");
    s.phase = Phase::Detached;
    let s1 = reduce(
        s,
        stream(
            r#"{"type":"system","subtype":"init","session_id":"sess-new2","model":"claude-opus-4-5","cwd":"/tmp","tools":[],"mcp_servers":[],"permissionMode":"default"}"#,
        ),
    );
    assert_eq!(
        s1.phase,
        Phase::Working,
        "SystemInit must supersede Detached"
    );
}

#[test]
fn stale_preserved_by_assistant_stream() {
    let mut s = init_with_model("claude-opus-4-5");
    s.phase = Phase::Stale;
    let s1 = reduce(s, stream(&assistant_json("msg-stale-a", 50, 10)));
    assert_eq!(s1.phase, Phase::Stale, "Assistant must not escape Stale");
}

#[test]
fn detached_preserved_by_assistant_stream() {
    let mut s = init_with_model("claude-opus-4-5");
    s.phase = Phase::Detached;
    let s1 = reduce(s, stream(&assistant_json("msg-detach-a", 50, 10)));
    assert_eq!(
        s1.phase,
        Phase::Detached,
        "Assistant must not escape Detached"
    );
}

#[test]
fn stale_preserved_by_user_stream() {
    let mut s = init_with_model("claude-opus-4-5");
    s.phase = Phase::Stale;
    let s1 = reduce(
        s,
        stream(r#"{"type":"user","message":{"content":[{"type":"text","text":"hello"}]}}"#),
    );
    assert_eq!(s1.phase, Phase::Stale, "User message must not escape Stale");
}

#[test]
fn detached_preserved_by_user_stream() {
    let mut s = init_with_model("claude-opus-4-5");
    s.phase = Phase::Detached;
    let s1 = reduce(
        s,
        stream(r#"{"type":"user","message":{"content":[{"type":"text","text":"hello"}]}}"#),
    );
    assert_eq!(
        s1.phase,
        Phase::Detached,
        "User message must not escape Detached"
    );
}

#[test]
fn stale_preserved_by_result_stream() {
    let mut s = init_with_model("claude-opus-4-5");
    s.phase = Phase::Stale;
    let s1 = reduce(s, stream(result_success_json()));
    assert_eq!(s1.phase, Phase::Stale, "Result must not escape Stale");
}

#[test]
fn detached_preserved_by_result_stream() {
    let mut s = init_with_model("claude-opus-4-5");
    s.phase = Phase::Detached;
    let s1 = reduce(s, stream(result_success_json()));
    assert_eq!(s1.phase, Phase::Detached, "Result must not escape Detached");
}

#[test]
fn stale_preserved_by_control_stream() {
    let mut s = init_with_model("claude-opus-4-5");
    s.phase = Phase::Stale;
    let s1 = reduce(
        s,
        stream(
            r#"{"type":"control_request","request_id":"req-stale","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{}}}"#,
        ),
    );
    assert_eq!(
        s1.phase,
        Phase::Stale,
        "Control request must not escape Stale"
    );
}

#[test]
fn detached_preserved_by_control_stream() {
    let mut s = init_with_model("claude-opus-4-5");
    s.phase = Phase::Detached;
    let s1 = reduce(
        s,
        stream(
            r#"{"type":"control_request","request_id":"req-detach","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{}}}"#,
        ),
    );
    assert_eq!(
        s1.phase,
        Phase::Detached,
        "Control request must not escape Detached"
    );
}

#[test]
fn stale_preserved_by_all_hooks_except_superseding() {
    // All hook events (except none — there is no superseding hook) preserve Stale.
    let hooks = vec![
        Event::Hook(HookEvent::Stop),
        Event::Hook(HookEvent::SubagentStop {
            name: "a".to_string(),
        }),
        Event::Hook(HookEvent::PreToolUse {
            tool: "Bash".to_string(),
        }),
        Event::Hook(HookEvent::PostToolUse {
            tool: "Bash".to_string(),
        }),
        Event::Hook(HookEvent::SessionStart),
        Event::Hook(HookEvent::PreCompact),
    ];
    for hook in hooks {
        let mut s = init_with_model("claude-opus-4-5");
        s.phase = Phase::Stale;
        let s1 = reduce(s, hook);
        assert_eq!(s1.phase, Phase::Stale, "hook event must preserve Stale");
    }
}

#[test]
fn detached_preserved_by_all_hooks_except_superseding() {
    // All hook events preserve Detached.
    let hooks = vec![
        Event::Hook(HookEvent::Stop),
        Event::Hook(HookEvent::SubagentStop {
            name: "b".to_string(),
        }),
        Event::Hook(HookEvent::PreToolUse {
            tool: "Write".to_string(),
        }),
        Event::Hook(HookEvent::PostToolUse {
            tool: "Write".to_string(),
        }),
        Event::Hook(HookEvent::SessionStart),
        Event::Hook(HookEvent::PreCompact),
    ];
    for hook in hooks {
        let mut s = init_with_model("claude-opus-4-5");
        s.phase = Phase::Detached;
        let s1 = reduce(s, hook);
        assert_eq!(
            s1.phase,
            Phase::Detached,
            "hook event must preserve Detached"
        );
    }
}

// ---------------------------------------------------------------------------
// Derives: PartialEq, Clone, Debug compile-time checks
// ---------------------------------------------------------------------------

#[test]
fn session_state_derives_clone_eq_debug() {
    let s = SessionState::initial();
    let s2 = s.clone();
    assert_eq!(s, s2);
    let _ = format!("{s:?}");
}

#[test]
fn phase_all_variants_derive_clone_eq_debug() {
    let phases = [
        Phase::Starting,
        Phase::Working,
        Phase::Needs,
        Phase::Error,
        Phase::Done,
        Phase::Stale,
        Phase::Detached,
    ];
    for p in &phases {
        let p2 = p.clone();
        assert_eq!(p, &p2);
        let _ = format!("{p:?}");
    }
}

#[test]
fn subagent_status_derives_clone_eq_debug() {
    let running = SubagentStatus::Running;
    let done = SubagentStatus::Done;
    assert_eq!(running.clone(), SubagentStatus::Running);
    assert_eq!(done.clone(), SubagentStatus::Done);
    assert_ne!(running, done);
    let _ = format!("{running:?} {done:?}");
}

// ---------------------------------------------------------------------------
// AskUserQuestion gate via control_request
// ---------------------------------------------------------------------------

fn system_init_json_br() -> &'static str {
    r#"{"type":"system","subtype":"init","session_id":"sess-q","model":"claude-opus-4-5","cwd":"/tmp","tools":[],"mcp_servers":[],"permissionMode":"default"}"#
}

fn ask_user_question_json(request_id: &str) -> String {
    let val = serde_json::json!({
        "type": "control_request",
        "request_id": request_id,
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "AskUserQuestion",
            "input": {
                "questions": [{
                    "question": "Which option?",
                    "header": "Choose",
                    "multiSelect": false,
                    "options": [
                        {"label": "A", "description": "First"},
                        {"label": "B", "description": "Second"}
                    ]
                }]
            }
        }
    });
    serde_json::to_string(&val).unwrap()
}

fn ask_user_question_multi_json(request_id: &str) -> String {
    let val = serde_json::json!({
        "type": "control_request",
        "request_id": request_id,
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "AskUserQuestion",
            "input": {
                "questions": [{
                    "question": "Pick all that apply?",
                    "header": "Multi",
                    "multiSelect": true,
                    "options": [
                        {"label": "X", "description": ""},
                        {"label": "Y", "description": ""}
                    ]
                }]
            }
        }
    });
    serde_json::to_string(&val).unwrap()
}

fn ask_user_question_no_questions_json(request_id: &str) -> String {
    let val = serde_json::json!({
        "type": "control_request",
        "request_id": request_id,
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "AskUserQuestion",
            "input": {}
        }
    });
    serde_json::to_string(&val).unwrap()
}

fn exit_plan_mode_json(request_id: &str) -> String {
    // Build via serde_json to avoid raw-string escaping issues with embedded newlines.
    let plan_body = "## The Plan\n\n1. Step one\n2. Step two";
    let val = serde_json::json!({
        "type": "control_request",
        "request_id": request_id,
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "ExitPlanMode",
            "input": { "plan": plan_body }
        }
    });
    serde_json::to_string(&val).unwrap()
}

fn exit_plan_mode_no_plan_json(request_id: &str) -> String {
    let val = serde_json::json!({
        "type": "control_request",
        "request_id": request_id,
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "ExitPlanMode",
            "input": {}
        }
    });
    serde_json::to_string(&val).unwrap()
}

#[test]
fn ask_user_question_single_select_builds_question_gate() {
    let s0 = reduce(SessionState::initial(), stream(system_init_json_br()));
    let s1 = reduce(s0, stream(&ask_user_question_json("req-aq-1")));
    assert_eq!(s1.phase, Phase::Needs);
    match s1.gate.as_ref().expect("gate must be set") {
        marsad_gates::Gate::Question {
            request_id,
            questions,
        } => {
            assert_eq!(request_id, "req-aq-1");
            assert_eq!(questions.len(), 1);
            assert_eq!(questions[0].question, "Which option?");
            assert_eq!(questions[0].header, "Choose");
            assert!(!questions[0].multi_select);
            assert_eq!(questions[0].options.len(), 2);
            assert_eq!(questions[0].options[0].label, "A");
            assert_eq!(questions[0].options[0].description, "First");
        }
        other => panic!("expected Question gate, got {other:?}"),
    }
}

#[test]
fn ask_user_question_multi_select_builds_question_gate() {
    let s0 = reduce(SessionState::initial(), stream(system_init_json_br()));
    let s1 = reduce(s0, stream(&ask_user_question_multi_json("req-aq-2")));
    assert_eq!(s1.phase, Phase::Needs);
    match s1.gate.as_ref().expect("gate must be set") {
        marsad_gates::Gate::Question { questions, .. } => {
            assert!(questions[0].multi_select);
        }
        other => panic!("expected Question gate, got {other:?}"),
    }
}

#[test]
fn ask_user_question_empty_input_yields_empty_questions() {
    let s0 = reduce(SessionState::initial(), stream(system_init_json_br()));
    let s1 = reduce(
        s0,
        stream(&ask_user_question_no_questions_json("req-aq-3")),
    );
    assert_eq!(s1.phase, Phase::Needs);
    match s1.gate.as_ref().expect("gate must be set") {
        marsad_gates::Gate::Question { questions, .. } => {
            assert!(questions.is_empty());
        }
        other => panic!("expected Question gate, got {other:?}"),
    }
}

#[test]
fn exit_plan_mode_builds_plan_gate_with_markdown() {
    let s0 = reduce(SessionState::initial(), stream(system_init_json_br()));
    let s1 = reduce(s0, stream(&exit_plan_mode_json("req-ep-1")));
    assert_eq!(s1.phase, Phase::Needs);
    match s1.gate.as_ref().expect("gate must be set") {
        marsad_gates::Gate::Plan { summary, steps } => {
            assert!(summary.contains("Step one"), "summary: {summary}");
            assert!(steps.is_empty());
        }
        other => panic!("expected Plan gate, got {other:?}"),
    }
}

#[test]
fn exit_plan_mode_no_plan_field_yields_empty_summary() {
    let s0 = reduce(SessionState::initial(), stream(system_init_json_br()));
    let s1 = reduce(s0, stream(&exit_plan_mode_no_plan_json("req-ep-2")));
    assert_eq!(s1.phase, Phase::Needs);
    match s1.gate.as_ref().expect("gate must be set") {
        marsad_gates::Gate::Plan { summary, steps } => {
            assert!(summary.is_empty());
            assert!(steps.is_empty());
        }
        other => panic!("expected Plan gate, got {other:?}"),
    }
}
