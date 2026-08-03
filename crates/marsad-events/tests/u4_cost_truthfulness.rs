//! U4 — cost truthfulness (docs/PLAN.md U4 + memory 'marsad-full-experience').
//!
//! Agent-reported cost (`usage.cost_usd` and `per_model[*].cost_usd` on the
//! `TurnCompleted` event) is the source of truth end-to-end — it must flow
//! through `reduce()` into `SessionState` VERBATIM, never recomputed from a
//! pricing table. These pin that contract at the events-reducer layer over the
//! Wave 1 `AgentEvent` model.
//!
//! In the old stream model an error turn carried `is_error:true`; in the wasit
//! model it carries `end_reason: ExecutionError`. Either way the cost figure
//! must surface regardless of the error classification.

mod common;
use common::*;

use marsad_events::{reduce, Phase, SessionState};
use std::collections::HashMap;
use wasit_core::{EndReason as WEndReason, ModelUsage};

fn after() -> SessionState {
    after_init(session_init("sess-u4", "claude-sonnet-4-6", "/tmp"))
}

// ---------------------------------------------------------------------------
// An error turn still surfaces the agent-reported cost
// ---------------------------------------------------------------------------

#[test]
fn execution_error_still_sets_authoritative_cost() {
    let s1 = reduce(after(), ev(turn_end(WEndReason::ExecutionError, usage(50, 10, 0, 0, 0.734))));
    assert!(
        (s1.cost_usd - 0.734).abs() < 1e-9,
        "U4: an execution error must not suppress the agent-reported cost, got {}",
        s1.cost_usd
    );
    assert_eq!(s1.phase, Phase::Error);
}

#[test]
fn error_turn_with_large_cost_reports_verbatim() {
    let s1 = reduce(after(), ev(turn_end(WEndReason::ExecutionError, usage(200, 80, 0, 0, 1.5))));
    assert!(
        (s1.cost_usd - 1.5).abs() < 1e-9,
        "U4: error turn must surface cost verbatim, got {}",
        s1.cost_usd
    );
    assert_eq!(s1.phase, Phase::Error, "an execution error turn is Error");
}

// ---------------------------------------------------------------------------
// per_model cost flows through verbatim — never recomputed from tokens
// ---------------------------------------------------------------------------

#[test]
fn per_model_cost_is_taken_verbatim_not_recomputed() {
    // 1,000,000 input tokens on sonnet would price to ~$3.00 via a pricing table,
    // but the driver reports $9.99 — the reducer must copy it verbatim.
    let mut per_model = HashMap::new();
    per_model.insert(
        "claude-sonnet-4-6".to_string(),
        ModelUsage {
            input_tokens: 1_000_000,
            output_tokens: 0,
            cost_usd: 9.99,
            estimated: false,
        },
    );
    let s1 = reduce(
        after(),
        ev(turn_success_per_model(usage(1_000_000, 0, 0, 0, 9.99), per_model)),
    );
    let mu = s1
        .model_usage
        .get("claude-sonnet-4-6")
        .expect("model_usage entry must be present");
    assert!(
        (mu.cost_usd - 9.99).abs() < 1e-9,
        "U4: per_model cost must flow through verbatim, got {}",
        mu.cost_usd
    );
    assert!(
        (s1.cost_usd - 9.99).abs() < 1e-9,
        "U4: total cost must also flow through verbatim, got {}",
        s1.cost_usd
    );
}
