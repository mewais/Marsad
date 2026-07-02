//! B02 `marsad-events` — recorded-real fixture tests.
//!
//! Folds real captured NDJSON streams line-by-line via `reduce` and asserts
//! the final `SessionState` matches known expected values.
//!
//! ## qa.ndjson (captured — 3 lines)
//!   line 1: system/init  → session_id, model "claude-opus-4.8[1m]", cwd
//!   line 2: assistant    → id "msg_vrtx_019BtVq34GVCztT9N466pLQd", input=2,
//!                          cache_creation=42448, output=1
//!   line 3: result/success → total_cost_usd=0.796905, output=13, contextWindow=1_000_000
//!
//! ## gate / subagent / compaction fixtures
//! Not yet captured. Tests are #[ignore]-tagged; see fixtures/README.md.

use marsad_events::{reduce, Event, Phase, SessionState, SubagentStatus};
use marsad_proto::parse_stream_line;

// ---------------------------------------------------------------------------
// Helper: fold an NDJSON string into a fresh SessionState
// ---------------------------------------------------------------------------

fn fold_ndjson(ndjson: &str) -> SessionState {
    fold_ndjson_into(SessionState::initial(), ndjson)
}

fn fold_ndjson_into(initial: SessionState, ndjson: &str) -> SessionState {
    let mut state = initial;
    for (i, line) in ndjson.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let msg = parse_stream_line(line)
            .unwrap_or_else(|e| panic!("fixture line {i} failed to parse: {e:?}\nline: {line}"));
        state = reduce(state, Event::Stream(msg));
    }
    state
}

// ---------------------------------------------------------------------------
// qa.ndjson — plain Q&A: init → assistant → result/success
// ---------------------------------------------------------------------------

const QA_NDJSON: &str = include_str!("../../../fixtures/streams/qa.ndjson");

#[test]
fn fixture_qa_phase_is_done() {
    let state = fold_ndjson(QA_NDJSON);
    assert_eq!(
        state.phase,
        Phase::Done,
        "qa fixture must end with Phase::Done"
    );
}

#[test]
fn fixture_qa_session_id_recorded() {
    let state = fold_ndjson(QA_NDJSON);
    assert_eq!(
        state.session_id,
        Some("59215d34-0f57-4b68-9419-3bb8e27d6fab".to_string()),
        "session_id must match the SystemInit line"
    );
}

#[test]
fn fixture_qa_model_recorded() {
    let state = fold_ndjson(QA_NDJSON);
    let model = state.model.expect("model must be set after SystemInit");
    // System init line uses "claude-opus-4.8[1m]"
    assert!(
        model.contains("claude-opus-4"),
        "unexpected model string: {model}"
    );
}

#[test]
fn fixture_qa_cwd_recorded() {
    let state = fold_ndjson(QA_NDJSON);
    assert_eq!(
        state.cwd,
        Some("/work/mewais/Marsad".to_string()),
        "cwd must match the SystemInit line"
    );
}

#[test]
fn fixture_qa_cost_positive_and_accurate() {
    let state = fold_ndjson(QA_NDJSON);
    assert!(
        state.cost_usd > 0.0,
        "cost_usd must be positive, got {}",
        state.cost_usd
    );
    // Result line: total_cost_usd = 0.796905. Accept within 1%.
    assert!(
        (state.cost_usd - 0.796905).abs() < 0.01,
        "expected cost ~0.796905, got {}",
        state.cost_usd
    );
}

#[test]
fn fixture_qa_tokens_nonzero() {
    let state = fold_ndjson(QA_NDJSON);
    assert!(
        state.tokens > 0,
        "tokens must be nonzero after folding qa fixture, got {}",
        state.tokens
    );
    // Result usage: input=2, cache_creation=42448, output=13.
    // At minimum output tokens (13) must be counted; full sum is 42463.
    assert!(
        state.tokens >= 13,
        "tokens must be at least 13 (output tokens), got {}",
        state.tokens
    );
}

#[test]
fn fixture_qa_context_pct_in_range() {
    let state = fold_ndjson(QA_NDJSON);
    assert!(
        (0.0..=100.0).contains(&state.context_pct),
        "context_pct out of [0,100]: {}",
        state.context_pct
    );
    assert!(
        state.context_pct > 0.0,
        "context_pct must be > 0 after real usage, got {}",
        state.context_pct
    );
}

#[test]
fn fixture_qa_context_pct_small_for_1m_model() {
    // "claude-opus-4.8[1m]" has a 1M context window.
    // Total usage ~42k tokens → context_pct should be well under 10%.
    let state = fold_ndjson(QA_NDJSON);
    assert!(
        state.context_pct < 10.0,
        "for a 1M model with ~42k tokens, context_pct should be < 10%, got {}",
        state.context_pct
    );
}

#[test]
fn fixture_qa_no_subagents() {
    let state = fold_ndjson(QA_NDJSON);
    assert!(
        state.subagents.is_empty(),
        "qa fixture has no subagents, got {:?}",
        state.subagents
    );
}

#[test]
fn fixture_qa_no_gate() {
    let state = fold_ndjson(QA_NDJSON);
    assert!(
        state.gate.is_none(),
        "qa fixture has no gate, got {:?}",
        state.gate
    );
}

#[test]
fn fixture_qa_all_lines_parse_clean() {
    // Every line in the real fixture must parse without error.
    for (i, line) in QA_NDJSON.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        parse_stream_line(line)
            .unwrap_or_else(|e| panic!("qa.ndjson line {i} failed to parse: {e:?}\nline: {line}"));
    }
}

/// Dedup across replay: fold the stream twice (simulating a re-delivered assistant
/// message with id "msg_vrtx_019BtVq34GVCztT9N466pLQd"). The assistant usage must
/// not double-count.
///
/// [DEDUP] This is the recorded-real version of the dedup invariant.
#[test]
fn fixture_qa_dedup_on_assistant_replay() {
    let state_once = fold_ndjson(QA_NDJSON);

    // Re-fold only the assistant line (line index 1) into state_once.
    let assistant_line = QA_NDJSON
        .lines()
        .nth(1)
        .expect("qa.ndjson must have at least 2 lines")
        .trim();
    assert!(
        assistant_line.contains("assistant"),
        "line 1 must be the assistant message"
    );

    let msg = parse_stream_line(assistant_line).expect("assistant line must parse");
    let state_replayed = reduce(state_once.clone(), Event::Stream(msg));

    // After re-delivering the same assistant message id, tokens must NOT increase.
    // [DEDUP] The impl must track seen message ids and skip re-counting.
    assert_eq!(
        state_replayed.tokens, state_once.tokens,
        "[DEDUP] re-delivering assistant msg_vrtx_019BtVq34GVCztT9N466pLQd \
         must not increase token count: before={} after={}",
        state_once.tokens, state_replayed.tokens
    );
}

// ---------------------------------------------------------------------------
// Gate / subagent / compaction fixtures — CAPTURED (real `claude` 2.1.198 runs).
//
// KNOWN REAL-CONTRACT MISMATCHES (see fixtures/README.md "Known gaps" section):
// these fixtures are genuine unmodified captures, but the real claude 2.1.198
// stream-json shape has drifted from what `marsad_proto::parse_stream_line`
// expects in three ways that make `fold_ndjson` panic partway through most of
// these fixtures (fold_ndjson panics on the first unparseable line, same as
// the qa fixture's dedup test does on a parse failure):
//   1. assistant content blocks include a `{"type":"thinking",...}` block
//      (this account has `alwaysThinkingEnabled: true`) — `ContentBlock` has
//      no `Thinking` variant, so any assistant message that reasoned first
//      fails with SchemaMismatch.
//   2. `parse_stream_line` dispatches EVERY `"type":"system"` line straight to
//      `SystemInit`, but real streams emit many other `system` subtypes
//      (thinking_tokens, status, task_started/progress/updated/notification,
//      compact_boundary, hook_started/hook_response) that lack SystemInit's
//      required fields (model/cwd/tools/permissionMode) — each fails with
//      SchemaMismatch("missing field `model`").
//   3. after `/compact`, the CLI emits `user` messages whose `content` is a
//      bare string (the compaction summary / `<local-command-stdout>`), not
//      the `Vec<ContentBlock>` `UserMessage.content` requires — SchemaMismatch
//      ("invalid type: string ... expected a sequence").
// These tests assert exactly the documented expectations (see comments below,
// carried over from the TODO stubs) against the real captures. Per the
// capture task's contract, a genuine red test here — caused by real captured
// data outrunning the current parser, NOT by a bug in the test or a doctored
// fixture — is the correct, honest outcome. Do not weaken these assertions to
// dodge the failure; fix `marsad-proto` instead (out of scope for this task).
// ---------------------------------------------------------------------------

use marsad_gates::Gate;

/// fixtures/streams/permission.ndjson — a real `git push --force` Bash
/// permission gate (`can_use_tool`), captured against claude 2.1.198.
///
/// EXPECTED TO PANIC today: `fold_ndjson` hits a `system/thinking_tokens`
/// line (mismatch #2 above) before it ever reaches the `control_request`
/// line, so this currently fails inside `fold_ndjson`, not at the asserts.
#[test]
fn fixture_permission_gate_sets_needs_and_gate() {
    const NDJSON: &str = include_str!("../../../fixtures/streams/permission.ndjson");
    let state = fold_ndjson(NDJSON);
    assert_eq!(state.phase, Phase::Needs);
    assert!(state.gate.is_some());
    assert!(matches!(state.gate, Some(Gate::Permission { .. })));
}

/// fixtures/streams/plan.ndjson — a real plan-mode `ExitPlanMode` gate.
///
/// EXPECTED TO PANIC today: same `system/thinking_tokens` / `system/status`
/// parse failure (mismatch #2) short-circuits `fold_ndjson` before the
/// `ExitPlanMode` control_request line is folded.
#[test]
fn fixture_plan_gate_sets_needs_and_gate() {
    const NDJSON: &str = include_str!("../../../fixtures/streams/plan.ndjson");
    let state = fold_ndjson(NDJSON);
    assert_eq!(state.phase, Phase::Needs);
    assert!(state.gate.is_some());
    assert!(matches!(state.gate, Some(Gate::Plan { .. })));
}

/// fixtures/streams/edit.ndjson — a real `Edit` tool permission gate on a
/// scratch NOTES.txt file.
///
/// EXPECTED TO PANIC today: the first assistant message includes a
/// `thinking` block (mismatch #1), so `fold_ndjson` fails before the `Edit`
/// control_request line. Note: `reduce_control_request` has no special case
/// for the `Edit` tool name — it falls through to the generic `Gate::Permission`
/// branch (there is no code path that ever constructs `marsad_gates::Gate::Edit`
/// today), so the expectation below is `Gate::Permission`, not `Gate::Edit`.
#[test]
fn fixture_edit_gate_sets_needs_and_gate() {
    const NDJSON: &str = include_str!("../../../fixtures/streams/edit.ndjson");
    let state = fold_ndjson(NDJSON);
    assert_eq!(state.phase, Phase::Needs);
    assert!(state.gate.is_some());
    assert!(matches!(state.gate, Some(Gate::Permission { .. })));
}

/// fixtures/streams/subagent.ndjson — a real subagent spawn + completion
/// (list two files, summarize).
///
/// EXPECTED TO PANIC today (for TWO independent real reasons):
///   a) mismatch #1/#2 (thinking block / system-subtype dispatch) fail
///      `fold_ndjson` before the subagent's tool_use line is even reached.
///   b) even if folding got that far: the real tool_use block that spawns the
///      subagent is named `"Agent"` in this environment's tool set (input
///      carries `subagent_type`), NOT `"Task"`. `reduce_assistant` only
///      special-cases `name == "Task"` (marsad-events/src/lib.rs), so a real
///      `"Agent"` tool_use would never populate `state.subagents` — the
///      asserts below would fail (not panic) even with (a) fixed.
#[test]
fn fixture_subagent_spawn_recorded_in_state() {
    const NDJSON: &str = include_str!("../../../fixtures/streams/subagent.ndjson");
    let state = fold_ndjson(NDJSON);
    assert!(!state.subagents.is_empty());
    assert!(state
        .subagents
        .iter()
        .any(|a| a.status == SubagentStatus::Done));
}

/// fixtures/streams/compaction.ndjson — a real successful `/compact` run
/// (four short Q&A turns, then `/compact`; `compact_result: "success"`).
///
/// EXPECTED TO PANIC today: mismatch #2 (`system/status`, `system/compact_boundary`)
/// and mismatch #3 (`user` message with string `content` after compaction) both
/// occur in this fixture; `fold_ndjson` fails on the first of these
/// (`system/status`) well before the final `result` line.
#[test]
fn fixture_compaction_context_resets() {
    const NDJSON: &str = include_str!("../../../fixtures/streams/compaction.ndjson");
    let state = fold_ndjson(NDJSON);
    // Documented expectation: the session ends Done (last `result` is
    // success) with a sane, in-range context_pct after compaction folded in.
    assert_eq!(state.phase, Phase::Done);
    assert!((0.0..=100.0).contains(&state.context_pct));
}
