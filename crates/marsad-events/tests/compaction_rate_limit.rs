//! B02 `marsad-events` — U3b: `compact_boundary` + `rate_limit_event` fold
//! into `SessionState` instead of being silently dropped by the
//! `StreamMsg::Unknown` no-op arm.
//!
//! Owner mandate (docs/UPGRADE_PLAN.md U3 + memory 'marsad-full-experience'):
//! "everything visible ... compaction indicators, rate-limit indicators.
//! Nothing silently dropped by the reducer or UI." This file is RED against
//! `marsad-events` as of this commit: `SessionState` has no `compactions`/
//! `last_compaction`/`rate_limit` field yet.

use marsad_events::{reduce, Event, Phase, SessionState};
use marsad_proto::parse_stream_line;

fn stream(json: &str) -> Event {
    let msg = parse_stream_line(json)
        .unwrap_or_else(|e| panic!("test JSON failed to parse: {e:?}\njson: {json}"));
    Event::Stream(msg)
}

fn system_init_json() -> &'static str {
    r#"{"type":"system","subtype":"init","session_id":"sess-cr","model":"claude-opus-4-5","cwd":"/tmp/work","tools":[],"mcp_servers":[],"permissionMode":"default"}"#
}

fn compact_boundary_json(trigger: &str, dropped: u64) -> String {
    format!(
        r#"{{"type":"system","subtype":"compact_boundary","session_id":"sess-cr","uuid":"u1","compact_metadata":{{"trigger":"{trigger}","pre_tokens":20000,"post_tokens":1000,"cumulative_dropped_tokens":{dropped},"duration_ms":5000}}}}"#
    )
}

fn rate_limit_event_json(status: &str) -> String {
    format!(
        r#"{{"type":"rate_limit_event","rate_limit_info":{{"status":"{status}","resetsAt":1783014000,"rateLimitType":"five_hour","overageStatus":"rejected","isUsingOverage":false}},"uuid":"u2","session_id":"sess-cr"}}"#
    )
}

fn init_state() -> SessionState {
    reduce(SessionState::initial(), stream(system_init_json()))
}

// ---------------------------------------------------------------------------
// SessionState::initial() — new fields start empty
// ---------------------------------------------------------------------------

#[test]
fn initial_state_compaction_and_rate_limit_fields_empty() {
    let s = SessionState::initial();
    assert_eq!(s.compactions, 0);
    assert!(s.last_compaction.is_none());
    assert!(s.rate_limit.is_none());
}

// ---------------------------------------------------------------------------
// compact_boundary — increments the count, records the latest metadata
// ---------------------------------------------------------------------------

#[test]
fn compact_boundary_increments_count_and_records_metadata() {
    let s = reduce(
        init_state(),
        stream(&compact_boundary_json("manual", 19000)),
    );
    assert_eq!(s.compactions, 1);
    let m = s.last_compaction.as_ref().expect("metadata recorded");
    assert_eq!(m.trigger, "manual");
    assert_eq!(m.pre_tokens, 20000);
    assert_eq!(m.post_tokens, 1000);
    assert_eq!(m.cumulative_dropped_tokens, 19000);
    assert_eq!(m.duration_ms, 5000);
}

#[test]
fn compact_boundary_does_not_reduce_context_pct_field_directly() {
    // compact_boundary is a NEW, additive signal — it must not silently
    // reach into `tokens`/`context_pct` (those stay authoritative from
    // Assistant/Result usage, per docs/specs/events.md). Folding one alone
    // (no prior usage) leaves tokens/context_pct at their zero starting point.
    let s = reduce(
        init_state(),
        stream(&compact_boundary_json("manual", 19000)),
    );
    assert_eq!(s.tokens, 0);
    assert_eq!(s.context_pct, 0.0);
}

#[test]
fn second_compact_boundary_increments_count_and_replaces_metadata() {
    let s = init_state();
    let s = reduce(s, stream(&compact_boundary_json("manual", 19000)));
    let s = reduce(s, stream(&compact_boundary_json("auto", 5000)));
    assert_eq!(s.compactions, 2, "count accumulates across compactions");
    let m = s.last_compaction.as_ref().expect("metadata recorded");
    assert_eq!(
        m.trigger, "auto",
        "last_compaction reflects the MOST RECENT compaction, not the first"
    );
    assert_eq!(m.cumulative_dropped_tokens, 5000);
}

#[test]
fn compact_boundary_does_not_change_phase() {
    // A compaction is informational, not a HITL gate — it must not flip
    // Working into Needs (or any other phase change) on its own.
    let s = init_state();
    assert_eq!(s.phase, Phase::Working);
    let s = reduce(s, stream(&compact_boundary_json("manual", 19000)));
    assert_eq!(
        s.phase,
        Phase::Working,
        "compaction alone does not change phase"
    );
}

// ---------------------------------------------------------------------------
// rate_limit_event — records the latest status
// ---------------------------------------------------------------------------

#[test]
fn rate_limit_event_records_status() {
    let s = reduce(init_state(), stream(&rate_limit_event_json("allowed")));
    let rl = s.rate_limit.as_ref().expect("rate_limit recorded");
    assert_eq!(rl.status, "allowed");
    assert_eq!(rl.rate_limit_type, "five_hour");
    assert_eq!(rl.resets_at, 1783014000);
}

#[test]
fn second_rate_limit_event_replaces_the_first() {
    let s = init_state();
    let s = reduce(s, stream(&rate_limit_event_json("allowed")));
    let s = reduce(s, stream(&rate_limit_event_json("allowed_warning")));
    let rl = s.rate_limit.as_ref().expect("rate_limit recorded");
    assert_eq!(
        rl.status, "allowed_warning",
        "rate_limit reflects the MOST RECENT event, not the first"
    );
}

#[test]
fn rate_limit_event_does_not_change_phase() {
    let s = init_state();
    assert_eq!(s.phase, Phase::Working);
    let s = reduce(s, stream(&rate_limit_event_json("allowed")));
    assert_eq!(
        s.phase,
        Phase::Working,
        "rate_limit_event alone does not change phase"
    );
}

// ---------------------------------------------------------------------------
// Stale / Detached preservation — same totality rule as every other stream
// message (docs/specs/events.md "Stale/Detached supersede rule"): only a
// fresh SystemInit supersedes; everything else is a no-op while stale/detached.
// ---------------------------------------------------------------------------

#[test]
fn stale_preserved_by_compact_boundary_stream() {
    let mut s = init_state();
    s.phase = Phase::Stale;
    let before_compactions = s.compactions;
    let s1 = reduce(s, stream(&compact_boundary_json("manual", 19000)));
    assert_eq!(
        s1.phase,
        Phase::Stale,
        "compact_boundary must not escape Stale"
    );
    assert_eq!(
        s1.compactions, before_compactions,
        "compact_boundary must be a no-op while Stale (totality no-op, not a partial apply)"
    );
}

#[test]
fn detached_preserved_by_compact_boundary_stream() {
    let mut s = init_state();
    s.phase = Phase::Detached;
    let s1 = reduce(s, stream(&compact_boundary_json("manual", 19000)));
    assert_eq!(
        s1.phase,
        Phase::Detached,
        "compact_boundary must not escape Detached"
    );
    assert_eq!(s1.compactions, 0);
}

#[test]
fn stale_preserved_by_rate_limit_event_stream() {
    let mut s = init_state();
    s.phase = Phase::Stale;
    let s1 = reduce(s, stream(&rate_limit_event_json("allowed")));
    assert_eq!(
        s1.phase,
        Phase::Stale,
        "rate_limit_event must not escape Stale"
    );
    assert!(
        s1.rate_limit.is_none(),
        "rate_limit_event must be a no-op while Stale"
    );
}

#[test]
fn detached_preserved_by_rate_limit_event_stream() {
    let mut s = init_state();
    s.phase = Phase::Detached;
    let s1 = reduce(s, stream(&rate_limit_event_json("allowed")));
    assert_eq!(
        s1.phase,
        Phase::Detached,
        "rate_limit_event must not escape Detached"
    );
    assert!(s1.rate_limit.is_none());
}

#[test]
fn fresh_system_init_supersedes_stale_and_compaction_history_persists() {
    // A fresh SystemInit supersedes Stale (existing rule) but does NOT reset
    // compaction/rate_limit history — those are session-scoped counters, not
    // per-connection state (mirrors how `tools`/`mcp_servers` behave: a
    // reconnect's SystemInit REPLACES them, but nothing here claims
    // compactions/rate_limit are reset by it, so a naive implementation
    // clearing them on SystemInit would be a semantic surprise worth pinning).
    let s = init_state();
    let s = reduce(s, stream(&compact_boundary_json("manual", 19000)));
    assert_eq!(s.compactions, 1);
    let mut s = s;
    s.phase = Phase::Stale;
    let s = reduce(s, stream(system_init_json()));
    assert_eq!(s.phase, Phase::Working, "SystemInit supersedes Stale");
    assert_eq!(
        s.compactions, 1,
        "compaction count is session history, not cleared by a reconnect init"
    );
}
