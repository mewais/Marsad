//! opencode_usage.rs — UsageUpdate folds cost + context_pct into SessionState.
//!
//! Wave 1: opencode's `usage_update` ACP event becomes an `AgentEvent::UsageUpdate`
//! carrying a `Usage` (with `cost_usd`) and an already-computed `context_pct`
//! (the driver derives used/size → percent). The reducer sets `cost_usd` and
//! `context_pct` from the snapshot, replaces (never accumulates), does not change
//! phase, and survives unrelated events + a serde round-trip.

mod common;
use common::*;

use marsad_events::{reduce, Phase, SessionState};

fn init_state() -> SessionState {
    after_init(session_init("sess-usage", "claude-sonnet-4", "/tmp/work"))
}

/// An opencode-style usage snapshot: cost + a driver-computed context_pct.
fn usage_snapshot(cost_usd: f64, context_pct: f32) -> marsad_events::Event {
    ev(usage_update(usage(0, 0, 0, 0, cost_usd), Some(context_pct)))
}

#[test]
fn eu1_usage_update_sets_cost_usd() {
    let s0 = init_state();
    assert_eq!(s0.cost_usd, 0.0);
    let s1 = reduce(s0, usage_snapshot(0.42, 22.5));
    assert!((s1.cost_usd - 0.42).abs() < 0.001, "cost_usd must be 0.42, got {}", s1.cost_usd);
}

#[test]
fn eu2_usage_update_sets_context_pct() {
    let s1 = reduce(init_state(), usage_snapshot(0.42, 22.5));
    assert!(
        (s1.context_pct - 22.5).abs() < 1.0,
        "context_pct must be ~22.5%, got {}",
        s1.context_pct
    );
}

#[test]
fn eu3_second_usage_update_replaces_first() {
    let s1 = reduce(init_state(), usage_snapshot(0.10, 5.0));
    assert!((s1.cost_usd - 0.10).abs() < 0.001);
    let s2 = reduce(s1, usage_snapshot(0.42, 22.5));
    assert!((s2.cost_usd - 0.42).abs() < 0.001, "cost replaces, got {}", s2.cost_usd);
}

#[test]
fn eu4_usage_update_does_not_change_phase() {
    let s0 = init_state();
    assert_eq!(s0.phase, Phase::Working);
    let s1 = reduce(s0, usage_snapshot(0.42, 22.5));
    assert_eq!(s1.phase, Phase::Working);
    assert!((s1.cost_usd - 0.42).abs() < 0.001);
}

#[test]
fn eu5_cost_usd_survives_unrelated_event() {
    let s1 = reduce(init_state(), usage_snapshot(0.42, 22.5));
    assert!((s1.cost_usd - 0.42).abs() < 0.001);
    let s2 = reduce(s1, ev(assistant_delta("Hello")));
    assert!((s2.cost_usd - 0.42).abs() < 0.001, "cost survives, got {}", s2.cost_usd);
}

#[test]
fn eu6_zero_cost_usage_update_accepted() {
    let s1 = reduce(init_state(), usage_snapshot(0.0, 0.5));
    assert!(s1.context_pct > 0.0 && s1.context_pct < 1.0, "context_pct ~0.5%, got {}", s1.context_pct);
    assert!(s1.cost_usd.abs() < 0.001);
}

#[test]
fn eu7_session_state_round_trips_after_usage_update() {
    let s1 = reduce(init_state(), usage_snapshot(1.23, 40.0));
    let json_val = serde_json::to_value(&s1).expect("serialize");
    let s1_rt: SessionState = serde_json::from_value(json_val).expect("deserialize");
    assert!((s1_rt.cost_usd - 1.23).abs() < 0.001);
}

// ---------------------------------------------------------------------------
// A UsageUpdate without an explicit context_pct derives it from tokens.
// ---------------------------------------------------------------------------

#[test]
fn usage_update_without_context_pct_derives_from_tokens() {
    // claude's per-message usage: no context_pct, so the reducer derives it from
    // the token total against the model window (200k default → 45000 = 22.5%).
    let s1 = reduce(init_state(), ev(usage_update(usage(45000, 0, 0, 0, 0.1), None)));
    assert_eq!(s1.tokens, 45000);
    assert!((s1.context_pct - 22.5).abs() < 1.0, "derived pct ~22.5%, got {}", s1.context_pct);
}
