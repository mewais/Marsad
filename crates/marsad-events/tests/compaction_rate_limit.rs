//! marsad-events — Compaction + RateLimitNotice fold into SessionState.
//!
//! These informational events bump the compaction count / record the latest
//! rate-limit status without touching phase, tokens, or context_pct. Same
//! Stale/Detached totality rule as every other non-init event.

mod common;
use common::*;

use marsad_events::{reduce, Phase, SessionState};
use wasit_core::{AgentEvent, CompactionInfo, RateLimitInfo};

fn init_state() -> SessionState {
    after_init(session_init("sess-cr", "claude-opus-4-5", "/tmp/work"))
}

fn compaction(trigger: &str, dropped: u64) -> AgentEvent {
    AgentEvent::Compaction {
        info: CompactionInfo {
            trigger: trigger.to_string(),
            pre_tokens: 20000,
            post_tokens: 1000,
            cumulative_dropped_tokens: dropped,
            duration_ms: 5000,
        },
    }
}

fn rate_limit(status: &str) -> AgentEvent {
    AgentEvent::RateLimitNotice {
        info: RateLimitInfo {
            status: status.to_string(),
            resets_at: 1783014000,
            rate_limit_type: "five_hour".to_string(),
            overage_status: "rejected".to_string(),
            is_using_overage: false,
        },
    }
}

// ---------------------------------------------------------------------------
// initial() — new fields start empty
// ---------------------------------------------------------------------------

#[test]
fn initial_state_compaction_and_rate_limit_fields_empty() {
    let s = SessionState::initial();
    assert_eq!(s.compactions, 0);
    assert!(s.last_compaction.is_none());
    assert!(s.rate_limit.is_none());
}

// ---------------------------------------------------------------------------
// Compaction — increments the count, records the latest metadata
// ---------------------------------------------------------------------------

#[test]
fn compaction_increments_count_and_records_metadata() {
    let s = reduce(init_state(), ev(compaction("manual", 19000)));
    assert_eq!(s.compactions, 1);
    let m = s.last_compaction.as_ref().expect("metadata recorded");
    assert_eq!(m.trigger, "manual");
    assert_eq!(m.pre_tokens, 20000);
    assert_eq!(m.post_tokens, 1000);
    assert_eq!(m.cumulative_dropped_tokens, 19000);
    assert_eq!(m.duration_ms, 5000);
}

#[test]
fn compaction_does_not_touch_tokens_or_context_pct() {
    let s = reduce(init_state(), ev(compaction("manual", 19000)));
    assert_eq!(s.tokens, 0);
    assert_eq!(s.context_pct, 0.0);
}

#[test]
fn second_compaction_increments_count_and_replaces_metadata() {
    let s = init_state();
    let s = reduce(s, ev(compaction("manual", 19000)));
    let s = reduce(s, ev(compaction("auto", 5000)));
    assert_eq!(s.compactions, 2);
    let m = s.last_compaction.as_ref().expect("metadata recorded");
    assert_eq!(m.trigger, "auto");
    assert_eq!(m.cumulative_dropped_tokens, 5000);
}

#[test]
fn compaction_does_not_change_phase() {
    let s = init_state();
    assert_eq!(s.phase, Phase::Working);
    let s = reduce(s, ev(compaction("manual", 19000)));
    assert_eq!(s.phase, Phase::Working);
}

// ---------------------------------------------------------------------------
// RateLimitNotice — records the latest status
// ---------------------------------------------------------------------------

#[test]
fn rate_limit_records_status() {
    let s = reduce(init_state(), ev(rate_limit("allowed")));
    let rl = s.rate_limit.as_ref().expect("rate_limit recorded");
    assert_eq!(rl.status, "allowed");
    assert_eq!(rl.rate_limit_type, "five_hour");
    assert_eq!(rl.resets_at, 1783014000);
}

#[test]
fn second_rate_limit_replaces_the_first() {
    let s = init_state();
    let s = reduce(s, ev(rate_limit("allowed")));
    let s = reduce(s, ev(rate_limit("allowed_warning")));
    let rl = s.rate_limit.as_ref().expect("rate_limit recorded");
    assert_eq!(rl.status, "allowed_warning");
}

#[test]
fn rate_limit_does_not_change_phase() {
    let s = init_state();
    assert_eq!(s.phase, Phase::Working);
    let s = reduce(s, ev(rate_limit("allowed")));
    assert_eq!(s.phase, Phase::Working);
}

// ---------------------------------------------------------------------------
// Stale / Detached preservation
// ---------------------------------------------------------------------------

#[test]
fn stale_preserved_by_compaction() {
    let mut s = init_state();
    s.phase = Phase::Stale;
    let before = s.compactions;
    let s1 = reduce(s, ev(compaction("manual", 19000)));
    assert_eq!(s1.phase, Phase::Stale);
    assert_eq!(s1.compactions, before, "compaction is a no-op while Stale");
}

#[test]
fn detached_preserved_by_compaction() {
    let mut s = init_state();
    s.phase = Phase::Detached;
    let s1 = reduce(s, ev(compaction("manual", 19000)));
    assert_eq!(s1.phase, Phase::Detached);
    assert_eq!(s1.compactions, 0);
}

#[test]
fn stale_preserved_by_rate_limit() {
    let mut s = init_state();
    s.phase = Phase::Stale;
    let s1 = reduce(s, ev(rate_limit("allowed")));
    assert_eq!(s1.phase, Phase::Stale);
    assert!(s1.rate_limit.is_none(), "rate_limit is a no-op while Stale");
}

#[test]
fn detached_preserved_by_rate_limit() {
    let mut s = init_state();
    s.phase = Phase::Detached;
    let s1 = reduce(s, ev(rate_limit("allowed")));
    assert_eq!(s1.phase, Phase::Detached);
    assert!(s1.rate_limit.is_none());
}

#[test]
fn fresh_session_init_supersedes_stale_and_compaction_history_persists() {
    let s = init_state();
    let s = reduce(s, ev(compaction("manual", 19000)));
    assert_eq!(s.compactions, 1);
    let mut s = s;
    s.phase = Phase::Stale;
    let s = reduce(s, ev(session_init("sess-cr", "claude-opus-4-5", "/tmp/work")));
    assert_eq!(s.phase, Phase::Working, "SessionInit supersedes Stale");
    assert_eq!(
        s.compactions, 1,
        "compaction count is session history, not cleared by a reconnect init"
    );
}
