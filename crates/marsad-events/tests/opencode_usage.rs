//! opencode_usage.rs — events-level TDD (RED phase) tests for ACP usage/cost.
//!
//! ALL tests MUST COMPILE and FAIL at runtime (not compile time).
//!
//! Strategy: build a canonical `system/usage_update` stream-json line (the
//! contracted output shape that `acp_translate` must emit for ACP
//! `session/update{type:"usage_update"}` events), feed it into the events
//! reducer via `reduce`, then round-trip the resulting `SessionState` through
//! `serde_json::to_value` and assert on `cost_usd` and context fields.
//!
//! RED: there is no `StreamMsg::UsageUpdate` variant and no reducer arm for
//! `system/usage_update` yet. The `parse_stream_line` call below will succeed
//! (the line parses as `StreamMsg::Unknown`), but the reducer will NOT update
//! `cost_usd` or any context field — so the assertion `cost_usd == 0.42` will
//! fail at runtime, giving a clear RED.
//!
//! Implementation gate (all must be done before tests go green):
//!   1. `marsad_proto` gains a `UsageUpdate` struct + `StreamMsg::UsageUpdate`
//!      variant, parsed from `{"type":"system","subtype":"usage_update",...}`.
//!   2. `marsad_events::reduce_stream` gains a `StreamMsg::UsageUpdate` arm that
//!      updates `SessionState.cost_usd`, `SessionState.context_pct`, and/or
//!      dedicated context_used/context_size fields.
//!   3. `SessionState` may gain new fields (`context_used`, `context_size`) or
//!      reuse existing ones (`cost_usd`, `context_pct`).

use marsad_events::{reduce, Event, Phase, SessionState};
use marsad_proto::parse_stream_line;

// ---------------------------------------------------------------------------
// JSON helpers
// ---------------------------------------------------------------------------

fn stream(json: &str) -> Event {
    let msg = parse_stream_line(json)
        .unwrap_or_else(|e| panic!("test JSON failed to parse: {e:?}\njson: {json}"));
    Event::Stream(msg)
}

fn system_init_json() -> &'static str {
    r#"{"type":"system","subtype":"init","session_id":"sess-usage","model":"claude-sonnet-4","cwd":"/tmp/work","tools":[],"mcp_servers":[],"permissionMode":"default"}"#
}

/// Build a `system/usage_update` stream-json line (the canonical shape that
/// `acp_translate::acp_update_to_stream_json` must emit for ACP usage_update events).
fn usage_update_json(cost_usd: f64, context_used: u64, context_size: u64) -> String {
    format!(
        r#"{{"type":"system","subtype":"usage_update","cost_usd":{cost_usd},"context_used":{context_used},"context_size":{context_size}}}"#
    )
}

fn init_state() -> SessionState {
    reduce(SessionState::initial(), stream(system_init_json()))
}

// ---------------------------------------------------------------------------
// Helper: extract cost_usd from a serialized SessionState
// ---------------------------------------------------------------------------

/// Round-trip `state` through JSON and return `cost_usd`.
/// Panics with a clear RED message if cost_usd was not updated.
fn get_cost_usd(state: &SessionState) -> f64 {
    let json = serde_json::to_value(state).expect("SessionState must serialize");
    json.get("cost_usd")
        .and_then(|v| v.as_f64())
        .expect(
            "SessionState JSON must contain 'cost_usd' — \
             RED: usage_update reducer arm not yet implemented"
        )
}

/// Round-trip `state` through JSON and return context_pct (0..=100).
fn get_context_pct(state: &SessionState) -> f64 {
    let json = serde_json::to_value(state).expect("SessionState must serialize");
    json.get("context_pct")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0)
}

// ---------------------------------------------------------------------------
// eu1: usage_update sets cost_usd on SessionState
// ---------------------------------------------------------------------------

/// Feeding a `system/usage_update` stream event into the events reducer must
/// update `SessionState.cost_usd` to the delivered cost.
///
/// RED: the reducer arm does not yet exist, so `cost_usd` stays at 0.0
/// after the usage_update event — asserting 0.42 will fail.
#[test]
fn eu1_usage_update_sets_cost_usd() {
    let s0 = init_state();
    assert_eq!(s0.cost_usd, 0.0, "sanity: cost_usd starts at 0.0");

    let update_json = usage_update_json(0.42, 45000, 200000);
    let s1 = reduce(s0, stream(&update_json));

    let cost = get_cost_usd(&s1);
    assert!(
        (cost - 0.42).abs() < 0.001,
        "cost_usd must be 0.42 after usage_update, got: {cost} \
         (RED: implement usage_update reducer arm)"
    );
}

// ---------------------------------------------------------------------------
// eu2: usage_update reflects context (used/size → context_pct or dedicated fields)
// ---------------------------------------------------------------------------

/// A `system/usage_update` with `context_used`/`context_size` must drive
/// the context percentage (or dedicated fields) in `SessionState`.
///
/// The spec says used=45000, size=200000 → context_pct = 45000/200000*100 = 22.5%.
/// The reducer must either:
///   (a) set context_pct directly from used/size, OR
///   (b) set dedicated context_used/context_size fields (and the UI computes pct).
///
/// We assert on context_pct since it already exists on SessionState.
///
/// RED: the reducer arm does not yet exist.
#[test]
fn eu2_usage_update_sets_context_pct() {
    let s0 = init_state();
    let update_json = usage_update_json(0.42, 45000, 200000);
    let s1 = reduce(s0, stream(&update_json));

    let pct = get_context_pct(&s1);
    // 45000 / 200000 * 100 = 22.5%
    assert!(
        (pct - 22.5).abs() < 1.0,
        "context_pct must be ~22.5% after usage_update (45000/200000), got: {pct} \
         (RED: implement usage_update reducer arm that sets context_pct)"
    );
}

// ---------------------------------------------------------------------------
// eu3: later usage_update replaces the previous cost
// ---------------------------------------------------------------------------

/// A second `usage_update` must REPLACE `cost_usd` (not accumulate).
/// ACP emits cumulative cost — the latest value is authoritative.
///
/// RED: reducer arm absent.
#[test]
fn eu3_second_usage_update_replaces_first() {
    let s0 = init_state();

    let first_json = usage_update_json(0.10, 10000, 200000);
    let s1 = reduce(s0, stream(&first_json));
    let cost_after_first = get_cost_usd(&s1);
    assert!(
        (cost_after_first - 0.10).abs() < 0.001,
        "cost_usd after first update must be 0.10, got: {cost_after_first}"
    );

    let second_json = usage_update_json(0.42, 45000, 200000);
    let s2 = reduce(s1, stream(&second_json));
    let cost_after_second = get_cost_usd(&s2);
    assert!(
        (cost_after_second - 0.42).abs() < 0.001,
        "cost_usd after second usage_update must be 0.42 (replace), got: {cost_after_second} \
         (RED: implement usage_update reducer arm)"
    );
}

// ---------------------------------------------------------------------------
// eu4: usage_update does not change Phase
// ---------------------------------------------------------------------------

/// A `usage_update` is purely informational — it must NOT change the session
/// phase (Working → Needs, etc.).
///
/// RED: reducer arm absent; even if it were there, it would change phase.
#[test]
fn eu4_usage_update_does_not_change_phase() {
    let s0 = init_state();
    assert_eq!(s0.phase, Phase::Working, "sanity: Working after init");

    let update_json = usage_update_json(0.42, 45000, 200000);
    let s1 = reduce(s0, stream(&update_json));

    assert_eq!(
        s1.phase,
        Phase::Working,
        "usage_update must not change phase from Working (RED: implement reducer arm)"
    );

    // Also assert cost was updated (so we know it wasn't just a no-op that accidentally passed)
    let cost = get_cost_usd(&s1);
    assert!(
        (cost - 0.42).abs() < 0.001,
        "cost_usd must be 0.42 after usage_update, got: {cost}"
    );
}

// ---------------------------------------------------------------------------
// eu5: unrelated events do not change cost_usd set by usage_update
// ---------------------------------------------------------------------------

/// After a usage_update, feeding an unrelated stream event (e.g. an assistant
/// message) must NOT clear or reset cost_usd.
///
/// RED: reducer arm absent.
#[test]
fn eu5_cost_usd_survives_unrelated_event() {
    let s0 = init_state();
    let update_json = usage_update_json(0.42, 45000, 200000);
    let s1 = reduce(s0, stream(&update_json));

    // Cost must be set (the RED assertion above)
    let cost_before = get_cost_usd(&s1);
    assert!(
        (cost_before - 0.42).abs() < 0.001,
        "cost_usd must be 0.42 after usage_update, got: {cost_before}"
    );

    // Feed an assistant message (unrelated)
    let assistant_json = r#"{"type":"assistant","message":{"id":"msg-after-usage","content":[{"type":"text","text":"Hello"}],"usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
    let s2 = reduce(s1, stream(assistant_json));

    let cost_after = get_cost_usd(&s2);
    assert!(
        (cost_after - 0.42).abs() < 0.001,
        "cost_usd must still be 0.42 after unrelated assistant event, got: {cost_after} \
         (RED: implement usage_update reducer arm that sets cost_usd)"
    );
}

// ---------------------------------------------------------------------------
// eu6: usage_update with zero cost is valid (cost may be 0 early in a session)
// ---------------------------------------------------------------------------

/// A `usage_update` with cost 0.0 must be accepted and set cost_usd to 0.0.
/// This covers the early-session state before any tokens are billed.
///
/// RED: reducer arm absent.
#[test]
fn eu6_zero_cost_usage_update_accepted() {
    let s0 = init_state();
    let update_json = usage_update_json(0.0, 1000, 200000);
    let s1 = reduce(s0, stream(&update_json));

    // Must not panic; cost_usd must be explicitly 0.0 (not unset/defaulted)
    // We verify by checking context_pct was set (proves the reducer ran)
    let pct = get_context_pct(&s1);
    // 1000 / 200000 * 100 = 0.5%
    assert!(
        pct > 0.0 && pct < 1.0,
        "context_pct must be ~0.5% after usage_update with used=1000/size=200000, got: {pct} \
         (RED: reducer arm must run even for zero cost)"
    );
    // cost_usd must be 0
    let cost = get_cost_usd(&s1);
    assert!(
        cost.abs() < 0.001,
        "cost_usd must be 0.0 after zero-cost usage_update, got: {cost}"
    );
}

// ---------------------------------------------------------------------------
// eu7: usage_update round-trips through serde cleanly (no panic on serialization)
// ---------------------------------------------------------------------------

/// The `SessionState` after a `usage_update` must round-trip through
/// `serde_json::to_value` and back without panicking or losing cost_usd.
///
/// RED: reducer arm absent, so cost_usd stays 0 and the assert at eu1 fires first.
#[test]
fn eu7_session_state_round_trips_after_usage_update() {
    let s0 = init_state();
    let update_json = usage_update_json(1.23, 80000, 200000);
    let s1 = reduce(s0, stream(&update_json));

    // Serialize + deserialize
    let json_val = serde_json::to_value(&s1).expect("SessionState must serialize");
    let s1_rt: SessionState = serde_json::from_value(json_val).expect("SessionState must deserialize");

    let cost = get_cost_usd(&s1_rt);
    assert!(
        (cost - 1.23).abs() < 0.001,
        "cost_usd must survive a serde round-trip: expected 1.23, got {cost} \
         (RED: implement usage_update reducer arm)"
    );
}
