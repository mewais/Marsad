//! U4 — cost truthfulness (docs/PLAN.md U4 + memory 'marsad-full-experience').
//!
//! Owner mandate: agent-reported cost (`total_cost_usd` / `modelUsage` on the
//! claude stream's `result` event) is the source of truth end-to-end — it must
//! flow through `reduce()` into `SessionState` VERBATIM, never recomputed from
//! the litellm/hand pricing table (that table is `marsad-core::discovery`'s
//! `model_cost_usd`/`litellm_rate`, reserved for historical transcript
//! backfill only — see the companion `marsad-core` U4 test files).
//!
//! Verified live (per the owner mandate): a claude `result` can carry
//! `subtype:"success"` together with `is_error:true` — and the cost figure is
//! present and must surface regardless of the error flag. These tests pin
//! that contract at the events-reducer layer.
//!
//! Unlike the other U4 test files in this block, everything here is a
//! straightforward assertion over `marsad_events`' existing public API (no
//! new fields needed) — so these are expected to already be GREEN. They stay
//! in the U4 test set as regression guards: the events layer is the one place
//! that must never regress into recomputing cost from tokens.

use marsad_events::{reduce, Event, Phase, SessionState};
use marsad_proto::parse_stream_line;

fn stream(json: &str) -> Event {
    let msg = parse_stream_line(json)
        .unwrap_or_else(|e| panic!("test JSON failed to parse: {e:?}\njson: {json}"));
    Event::Stream(msg)
}

fn system_init_json() -> &'static str {
    r#"{"type":"system","subtype":"init","session_id":"sess-u4","model":"claude-sonnet-4-6","cwd":"/tmp","tools":[],"mcp_servers":[],"permissionMode":"default"}"#
}

fn after_init() -> SessionState {
    reduce(SessionState::initial(), stream(system_init_json()))
}

// ---------------------------------------------------------------------------
// is_error:true still surfaces the agent-reported cost
// ---------------------------------------------------------------------------

/// A `result` with `is_error:true` (subtype error_during_execution) must still
/// set `cost_usd` from `total_cost_usd` — an error mid-turn is not free, and
/// the owner mandate is explicit that cost is present "regardless" of
/// `is_error`.
#[test]
fn result_is_error_true_still_sets_authoritative_cost() {
    let s0 = after_init();
    let result = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"total_cost_usd":0.734,"usage":{"input_tokens":50,"output_tokens":10,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},"num_turns":1,"permission_denials":[]}"#;
    let s1 = reduce(s0, stream(result));
    assert!(
        (s1.cost_usd - 0.734).abs() < 1e-9,
        "U4: is_error:true must not suppress the agent-reported cost, got {}",
        s1.cost_usd
    );
    assert_eq!(s1.phase, Phase::Error);
}

/// The exact "verified live" scenario cited in the U4 owner mandate: a result
/// can carry `subtype:"success"` together with `is_error:true`. `is_error`
/// still wins for phase (Error), but the cost figure must surface exactly as
/// reported — not be dropped, not be zeroed, not be recomputed.
#[test]
fn result_is_error_true_with_success_subtype_still_reports_cost() {
    let s0 = after_init();
    let result = r#"{"type":"result","subtype":"success","is_error":true,"total_cost_usd":1.5,"usage":{"input_tokens":200,"output_tokens":80,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},"num_turns":3,"permission_denials":[]}"#;
    let s1 = reduce(s0, stream(result));
    assert!(
        (s1.cost_usd - 1.5).abs() < 1e-9,
        "U4: subtype:success + is_error:true must still surface total_cost_usd verbatim, got {}",
        s1.cost_usd
    );
    // is_error wins over the success subtype for phase — this is existing,
    // correct behavior (see reduce_result); pinned here so the cost
    // assertion above is checked against the exact scenario the mandate
    // describes, not a simplified stand-in.
    assert_eq!(
        s1.phase,
        Phase::Error,
        "is_error:true must win over subtype:success for phase"
    );
}

// ---------------------------------------------------------------------------
// modelUsage.costUSD flows through verbatim — never recomputed from tokens
// ---------------------------------------------------------------------------

/// A result's per-model `modelUsage[model].costUSD` must land in
/// `state.model_usage[model].cost_usd` EXACTLY as reported by the stream, even
/// when the reported figure would NOT match what the hand/litellm pricing
/// table would compute for the given token counts (e.g. a promotional
/// discount, a beta-pricing model, or simply a future rate change the local
/// pricing table hasn't caught up with yet). The events reducer must never
/// second-guess the agent's own number by recomputing it.
#[test]
fn result_model_usage_cost_is_taken_verbatim_from_stream_not_recomputed() {
    let s0 = after_init();
    // 1,000,000 input tokens on sonnet would normally price out to $3.00 via
    // the hand/litellm table (see marsad-core::discovery::litellm_rate) — but
    // the stream reports a deliberately different figure ($9.99) to prove the
    // reducer copies it verbatim rather than recomputing from tokens.
    let result = r#"{"type":"result","subtype":"success","is_error":false,"total_cost_usd":9.99,"usage":{"input_tokens":1000000,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},"num_turns":1,"permission_denials":[],"modelUsage":{"claude-sonnet-4-6":{"inputTokens":1000000,"outputTokens":0,"costUSD":9.99}}}"#;
    let s1 = reduce(s0, stream(result));
    let mu = s1
        .model_usage
        .get("claude-sonnet-4-6")
        .expect("model_usage entry for claude-sonnet-4-6 must be present");
    assert!(
        (mu.cost_usd - 9.99).abs() < 1e-9,
        "U4: modelUsage.costUSD must flow through verbatim, not be recomputed from tokens; got {}",
        mu.cost_usd
    );
    assert!(
        (s1.cost_usd - 9.99).abs() < 1e-9,
        "U4: total_cost_usd must also flow through verbatim, got {}",
        s1.cost_usd
    );
}
