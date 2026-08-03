//! marsad-events — recorded-real scenario tests, reconstructed as AgentEvent.
//!
//! Wave 1 moved raw claude/opencode stream-json PARSING into the wasit drivers.
//! `marsad-events` no longer sees NDJSON — it folds the rich `AgentEvent` the
//! driver emits. These tests therefore reconstruct the SAME session, event by
//! event, as the wasit-claude driver WOULD emit for the captured fixtures, and
//! assert the resulting `SessionState` — preserving the original fixtures' intent
//! without depending on the deleted `parse_stream_line`.
//!
//! ## qa scenario (was fixtures/streams/qa.ndjson, 3 lines)
//!   line 1 system/init   → session_id, model "claude-opus-4.8[1m]", cwd
//!   line 2 assistant      → one text delta (usage carried on the result now)
//!   line 3 result/success → total_cost_usd=0.796905, cumulative ~42463 tokens,
//!                           a 1M-context model
//!
//! The gate / plan / subagent fixtures were parse-drift regression cases for the
//! old `parse_stream_line` and are covered structurally by the direct-AgentEvent
//! gate/subagent tests in happy.rs / branches.rs.

mod common;
use common::*;

use marsad_events::{reduce, Phase, SessionState, SubagentStatus};

/// Rebuild the qa scenario's final SessionState from the driver events.
fn fold_qa() -> SessionState {
    let mut s = SessionState::initial();
    s = reduce(
        s,
        ev(session_init(
            "59215d34-0f57-4b68-9419-3bb8e27d6fab",
            "claude-opus-4.8[1m]",
            "/work/mewais/Marsad",
        )),
    );
    s = reduce(s, ev(assistant_delta("Here is the answer.")));
    // result/success: input=2, cache_creation=42448, output=13 → 42463 cumulative;
    // total_cost_usd=0.796905.
    s = reduce(s, ev(turn_success(usage(2, 13, 0, 42448, 0.796905))));
    s
}

#[test]
fn qa_phase_is_done() {
    assert_eq!(fold_qa().phase, Phase::Done);
}

#[test]
fn qa_session_id_recorded() {
    assert_eq!(
        fold_qa().session_id,
        Some("59215d34-0f57-4b68-9419-3bb8e27d6fab".to_string())
    );
}

#[test]
fn qa_model_recorded() {
    let model = fold_qa().model.expect("model must be set after init");
    assert!(model.contains("claude-opus-4"), "unexpected model string: {model}");
}

#[test]
fn qa_cwd_recorded() {
    assert_eq!(fold_qa().cwd, Some("/work/mewais/Marsad".to_string()));
}

#[test]
fn qa_cost_positive_and_accurate() {
    let state = fold_qa();
    assert!(state.cost_usd > 0.0);
    assert!(
        (state.cost_usd - 0.796905).abs() < 0.01,
        "expected cost ~0.796905, got {}",
        state.cost_usd
    );
}

#[test]
fn qa_tokens_nonzero() {
    let state = fold_qa();
    assert!(state.tokens > 0);
    // input=2 + output=13 + cache_creation=42448 = 42463.
    assert_eq!(state.tokens, 42463);
}

#[test]
fn qa_context_pct_in_range() {
    let state = fold_qa();
    assert!((0.0..=100.0).contains(&state.context_pct));
    assert!(state.context_pct > 0.0);
}

#[test]
fn qa_context_pct_small_for_1m_model() {
    // "claude-opus-4.8[1m]" → 1M window; ~42k tokens → well under 10%.
    let state = fold_qa();
    assert!(
        state.context_pct < 10.0,
        "for a 1M model with ~42k tokens, context_pct should be < 10%, got {}",
        state.context_pct
    );
}

#[test]
fn qa_no_subagents() {
    assert!(fold_qa().subagents.is_empty());
}

#[test]
fn qa_no_gate() {
    assert!(fold_qa().gate.is_none());
}

/// TurnCompleted usage is a SNAPSHOT: re-delivering the same result does not
/// double-count (Wave 1 replaced the old per-message-id dedup with authoritative
/// snapshots). This is the reconstructed version of the old dedup invariant.
#[test]
fn qa_turn_completed_snapshot_does_not_double_count() {
    let once = fold_qa();
    let again = reduce(once.clone(), ev(turn_success(usage(2, 13, 0, 42448, 0.796905))));
    assert_eq!(
        again.tokens, once.tokens,
        "re-delivering the same authoritative result must not increase the token count"
    );
    assert!((again.cost_usd - once.cost_usd).abs() < 1e-9);
}

// ---------------------------------------------------------------------------
// Gate / subagent / compaction scenarios — reconstructed structurally.
// (The old NDJSON captures probed `parse_stream_line` drift, now wasit's job.)
// ---------------------------------------------------------------------------

use marsad_gates::Gate;

#[test]
fn permission_scenario_sets_needs_and_gate() {
    let s0 = after_init(session_init("s", "claude-opus-4-5", "/tmp"));
    let s = reduce(s0, ev(permission_gate("req-p", "git push --force", "force push")));
    assert_eq!(s.phase, Phase::Needs);
    assert!(matches!(s.gate, Some(Gate::Permission { .. })));
}

#[test]
fn plan_scenario_sets_needs_and_gate() {
    let s0 = after_init(session_init("s", "claude-opus-4-5", "/tmp"));
    let s = reduce(s0, ev(plan_gate("req-plan", "## Plan\n1. do it")));
    assert_eq!(s.phase, Phase::Needs);
    assert!(matches!(s.gate, Some(Gate::Plan { .. })));
}

#[test]
fn subagent_scenario_spawn_and_complete_recorded_in_state() {
    let s0 = after_init(session_init("s", "claude-opus-4-5", "/tmp"));
    // Real environments name the subagent tool "Agent" — the driver sets
    // is_subagent, so the reducer trusts the flag (no name sniffing).
    let s1 = reduce(
        s0,
        ev(subagent_call(
            "agent-1",
            serde_json::json!({"subagent_type": "lister"}),
            None,
        )),
    );
    let s2 = reduce(s1, ev(tool_finished("agent-1", "Agent", true)));
    assert!(!s2.subagents.is_empty());
    assert!(s2.subagents.iter().any(|a| a.status == SubagentStatus::Done));
}
