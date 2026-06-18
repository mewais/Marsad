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

use marsad_events::{reduce, Event, Phase, SessionState};
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
        let msg = parse_stream_line(line).unwrap_or_else(|e| {
            panic!("fixture line {i} failed to parse: {e:?}\nline: {line}")
        });
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
    assert_eq!(state.phase, Phase::Done, "qa fixture must end with Phase::Done");
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
        parse_stream_line(line).unwrap_or_else(|e| {
            panic!("qa.ndjson line {i} failed to parse: {e:?}\nline: {line}")
        });
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
// Gate / subagent / compaction fixtures — TODO (not yet captured)
// Remove #[ignore] when the files are committed. See fixtures/README.md.
// ---------------------------------------------------------------------------

/// NOTE: fixture not yet captured — fixtures/streams/permission.ndjson: TODO(fixture).
/// When captured: assert phase=Needs, gate.is_some() after folding.
#[test]
#[ignore = "TODO(fixture): fixtures/streams/permission.ndjson not captured yet — \
            see fixtures/README.md for capture instructions"]
fn fixture_permission_gate_sets_needs_and_gate() {
    // Uncomment when fixture is available:
    // const NDJSON: &str = include_str!("../../../fixtures/streams/permission.ndjson");
    // let state = fold_ndjson(NDJSON);
    // assert_eq!(state.phase, Phase::Needs);
    // assert!(state.gate.is_some());
    todo!("remove #[ignore] when fixture captured")
}

/// NOTE: fixture not yet captured — fixtures/streams/plan.ndjson: TODO(fixture).
#[test]
#[ignore = "TODO(fixture): fixtures/streams/plan.ndjson not captured yet — \
            see fixtures/README.md"]
fn fixture_plan_gate_sets_needs_and_gate() {
    todo!("remove #[ignore] when fixture captured")
}

/// NOTE: fixture not yet captured — fixtures/streams/edit.ndjson: TODO(fixture).
#[test]
#[ignore = "TODO(fixture): fixtures/streams/edit.ndjson not captured yet — \
            see fixtures/README.md"]
fn fixture_edit_gate_sets_needs_and_gate() {
    todo!("remove #[ignore] when fixture captured")
}

/// NOTE: fixture not yet captured — fixtures/streams/subagent.ndjson: TODO(fixture).
/// When captured: assert subagents non-empty, at least one with status=Done.
#[test]
#[ignore = "TODO(fixture): fixtures/streams/subagent.ndjson not captured yet — \
            see fixtures/README.md"]
fn fixture_subagent_spawn_recorded_in_state() {
    // const NDJSON: &str = include_str!("../../../fixtures/streams/subagent.ndjson");
    // let state = fold_ndjson(NDJSON);
    // assert!(!state.subagents.is_empty());
    // assert!(state.subagents.iter().any(|a| a.status == SubagentStatus::Done));
    todo!("remove #[ignore] when fixture captured")
}

/// NOTE: fixture not yet captured — fixtures/streams/compaction.ndjson: TODO(fixture).
/// When captured: PreCompact hook should be present; assert context_pct behaviour post-compaction.
#[test]
#[ignore = "TODO(fixture): fixtures/streams/compaction.ndjson not captured yet — \
            see fixtures/README.md"]
fn fixture_compaction_context_resets() {
    todo!("remove #[ignore] when fixture captured")
}
