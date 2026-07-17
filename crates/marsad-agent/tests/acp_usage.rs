//! TDD red-phase tests for ACP usage + cost translation in `marsad_agent::acp_translate`.
//!
//! Tests two NEW/extended functions that do NOT yet exist:
//!   - `acp_prompt_result_to_result(result: &Value) -> Value`
//!   - `acp_update_to_stream_json` handling `"usage_update"` update type.
//!
//! ALL tests MUST compile and FAIL at runtime (not compile time).
//!
//! The functions being tested do not exist yet; this file uses the same
//! import as `acp_translate_spec.rs`. If `acp_prompt_result_to_result` does not
//! exist this file will FAIL TO COMPILE — that is the expected red-phase signal
//! for the coder to scaffold the missing function signature first.
//!
//! Spec reference: docs/specs/opencode-acp.md §3, §8.
//! Wire shapes:
//!   - session/prompt RESULT carries `usage: {inputTokens, outputTokens,
//!     cachedReadTokens, cachedWriteTokens, thoughtTokens, totalTokens}`.
//!   - session/update with type="usage_update" carries
//!     `{cost:{amount,currency}, size, used}`.

use marsad_agent::acp_translate::{acp_prompt_result_to_result, acp_update_to_stream_json};
use marsad_proto::{parse_stream_line, StreamMsg};
use serde_json::{json, Value};

// ── helpers ───────────────────────────────────────────────────────────────────

const SESSION_ID: &str = "acp-usage-sess-42";
const MODEL: &str = "claude-sonnet-4";

/// Serialise `v` and parse it through `parse_stream_line`, panicking on error.
fn parse(v: &Value) -> StreamMsg {
    let s = serde_json::to_string(v).expect("Value is always serialisable");
    parse_stream_line(&s).expect("synthesised event must parse as a StreamMsg")
}

// =============================================================================
// acp_prompt_result_to_result — session/prompt result with usage
// =============================================================================

/// A session/prompt result with a full `usage` block must produce a
/// `result` stream-json Value whose usage fields map the ACP camelCase names
/// to the Marsad snake_case names:
///   inputTokens          → input_tokens
///   outputTokens         → output_tokens
///   cachedReadTokens     → cache_read_input_tokens
///   cachedWriteTokens    → cache_creation_input_tokens
#[test]
fn prompt_result_with_usage_maps_token_fields() {
    let result = json!({
        "stopReason": "completed",
        "usage": {
            "inputTokens": 1200,
            "outputTokens": 340,
            "cachedReadTokens": 900,
            "cachedWriteTokens": 50,
            "thoughtTokens": 20,
            "totalTokens": 2510
        }
    });
    let out = acp_prompt_result_to_result(&result);
    // Must be a result event
    assert_eq!(out["type"], "result", "outer type must be 'result'");
    // Usage block must be present
    let usage = &out["usage"];
    assert_ne!(usage, &Value::Null, "result must carry a usage block");
    // ACP inputTokens → input_tokens
    assert_eq!(
        usage["input_tokens"].as_u64().unwrap_or(0),
        1200,
        "input_tokens must map from inputTokens=1200"
    );
    // ACP outputTokens → output_tokens
    assert_eq!(
        usage["output_tokens"].as_u64().unwrap_or(0),
        340,
        "output_tokens must map from outputTokens=340"
    );
    // ACP cachedReadTokens → cache_read_input_tokens
    assert_eq!(
        usage["cache_read_input_tokens"].as_u64().unwrap_or(0),
        900,
        "cache_read_input_tokens must map from cachedReadTokens=900"
    );
    // ACP cachedWriteTokens → cache_creation_input_tokens
    assert_eq!(
        usage["cache_creation_input_tokens"].as_u64().unwrap_or(0),
        50,
        "cache_creation_input_tokens must map from cachedWriteTokens=50"
    );
    // Must parse as a valid result event
    assert!(
        matches!(parse(&out), StreamMsg::Result(_)),
        "must parse as StreamMsg::Result"
    );
}

/// A session/prompt result with stopReason "completed" must parse as
/// a successful result (is_error=false, subtype=success).
#[test]
fn prompt_result_completed_maps_to_success() {
    let result = json!({
        "stopReason": "completed",
        "usage": {
            "inputTokens": 100,
            "outputTokens": 50,
            "cachedReadTokens": 0,
            "cachedWriteTokens": 0,
            "thoughtTokens": 0,
            "totalTokens": 150
        }
    });
    let out = acp_prompt_result_to_result(&result);
    assert_eq!(out["type"], "result");
    assert_eq!(out["subtype"], "success", "completed → subtype must be 'success'");
    assert_eq!(out["is_error"], false, "completed → is_error must be false");
    assert!(matches!(parse(&out), StreamMsg::Result(_)));
}

/// A session/prompt result with stopReason "cancelled" must parse as
/// a result with interrupt semantics (is_error=false).
#[test]
fn prompt_result_cancelled_maps_to_cancel_semantics() {
    let result = json!({
        "stopReason": "cancelled",
        "usage": {
            "inputTokens": 200,
            "outputTokens": 10,
            "cachedReadTokens": 0,
            "cachedWriteTokens": 0,
            "thoughtTokens": 0,
            "totalTokens": 210
        }
    });
    let out = acp_prompt_result_to_result(&result);
    assert_eq!(out["type"], "result");
    let is_error = out["is_error"].as_bool().unwrap_or(true);
    assert!(!is_error, "cancelled → is_error must be false (calm interrupt)");
    assert!(matches!(parse(&out), StreamMsg::Result(_)));
}

/// A session/prompt result with stopReason "auth_required" must parse as
/// a result/error with a non-empty reason.
#[test]
fn prompt_result_auth_required_maps_to_error() {
    let result = json!({
        "stopReason": "auth_required",
        "usage": {
            "inputTokens": 50,
            "outputTokens": 0,
            "cachedReadTokens": 0,
            "cachedWriteTokens": 0,
            "thoughtTokens": 0,
            "totalTokens": 50
        }
    });
    let out = acp_prompt_result_to_result(&result);
    assert_eq!(out["type"], "result");
    assert_eq!(out["is_error"], true, "auth_required → is_error must be true");
    assert_eq!(out["subtype"], "error_during_execution");
    let reason = out["result"].as_str().unwrap_or("");
    assert!(!reason.is_empty(), "auth_required must carry a non-empty reason text");
    assert!(matches!(parse(&out), StreamMsg::Result(_)));
}

/// A session/prompt result with stopReason "request_limit_exceeded" must parse
/// as a result/error with a non-empty reason.
#[test]
fn prompt_result_request_limit_exceeded_maps_to_error() {
    let result = json!({
        "stopReason": "request_limit_exceeded",
        "usage": {
            "inputTokens": 5000,
            "outputTokens": 1200,
            "cachedReadTokens": 4000,
            "cachedWriteTokens": 200,
            "thoughtTokens": 100,
            "totalTokens": 10500
        }
    });
    let out = acp_prompt_result_to_result(&result);
    assert_eq!(out["type"], "result");
    assert_eq!(out["is_error"], true, "request_limit_exceeded → is_error must be true");
    assert_eq!(out["subtype"], "error_during_execution");
    let reason = out["result"].as_str().unwrap_or("");
    assert!(!reason.is_empty(), "request_limit_exceeded must carry a non-empty reason");
    assert!(matches!(parse(&out), StreamMsg::Result(_)));
}

/// A session/prompt result with a MISSING `usage` field must not panic and
/// must produce a valid result event with zeros in the usage block (defensive).
#[test]
fn prompt_result_missing_usage_defaults_to_zeros() {
    let result = json!({
        "stopReason": "completed"
        // no "usage" field at all
    });
    let out = acp_prompt_result_to_result(&result);
    assert_eq!(out["type"], "result");
    let usage = &out["usage"];
    assert_ne!(usage, &Value::Null, "usage block must be present even when missing from ACP result");
    assert_eq!(
        usage["input_tokens"].as_u64().unwrap_or(999),
        0,
        "missing usage → input_tokens must default to 0"
    );
    assert_eq!(
        usage["output_tokens"].as_u64().unwrap_or(999),
        0,
        "missing usage → output_tokens must default to 0"
    );
    assert_eq!(
        usage["cache_read_input_tokens"].as_u64().unwrap_or(999),
        0,
        "missing usage → cache_read_input_tokens must default to 0"
    );
    assert_eq!(
        usage["cache_creation_input_tokens"].as_u64().unwrap_or(999),
        0,
        "missing usage → cache_creation_input_tokens must default to 0"
    );
    assert!(matches!(parse(&out), StreamMsg::Result(_)));
}

/// An ACP result with only partial usage fields (e.g. no cachedReadTokens)
/// must not panic and must zero-fill the missing sub-fields.
#[test]
fn prompt_result_partial_usage_zero_fills_missing_fields() {
    let result = json!({
        "stopReason": "completed",
        "usage": {
            "inputTokens": 500,
            "outputTokens": 100
            // cachedReadTokens, cachedWriteTokens absent
        }
    });
    let out = acp_prompt_result_to_result(&result);
    let usage = &out["usage"];
    assert_eq!(usage["input_tokens"].as_u64().unwrap_or(0), 500);
    assert_eq!(usage["output_tokens"].as_u64().unwrap_or(0), 100);
    // Missing fields must default to 0 (not panic or be absent)
    assert_eq!(
        usage["cache_read_input_tokens"].as_u64().unwrap_or(999),
        0,
        "absent cachedReadTokens must zero-fill cache_read_input_tokens"
    );
    assert_eq!(
        usage["cache_creation_input_tokens"].as_u64().unwrap_or(999),
        0,
        "absent cachedWriteTokens must zero-fill cache_creation_input_tokens"
    );
    assert!(matches!(parse(&out), StreamMsg::Result(_)));
}

/// The `thoughtTokens` field in ACP usage is informational only and must
/// NOT inflate any of the four Marsad Usage fields (it is NOT mapped to any
/// field the reducer uses for token accumulation).
#[test]
fn prompt_result_thought_tokens_not_mapped_to_standard_fields() {
    let result = json!({
        "stopReason": "completed",
        "usage": {
            "inputTokens": 100,
            "outputTokens": 50,
            "cachedReadTokens": 0,
            "cachedWriteTokens": 0,
            "thoughtTokens": 999,
            "totalTokens": 1149
        }
    });
    let out = acp_prompt_result_to_result(&result);
    let usage = &out["usage"];
    // The four standard fields must be exactly as given (thoughtTokens must NOT bleed in)
    assert_eq!(usage["input_tokens"].as_u64().unwrap_or(0), 100);
    assert_eq!(usage["output_tokens"].as_u64().unwrap_or(0), 50);
    assert_eq!(usage["cache_read_input_tokens"].as_u64().unwrap_or(0), 0);
    assert_eq!(usage["cache_creation_input_tokens"].as_u64().unwrap_or(0), 0);
    // No usage field should have the value 999 (thought tokens bleeding in)
    let s = serde_json::to_string(&usage).unwrap();
    // 999 must only appear if it was one of the four explicitly mapped fields (it isn't)
    // We check none of the four standard fields equal 999
    assert_ne!(
        usage["input_tokens"].as_u64().unwrap_or(0), 999,
        "thoughtTokens=999 must not map to input_tokens"
    );
    assert_ne!(
        usage["output_tokens"].as_u64().unwrap_or(0), 999,
        "thoughtTokens=999 must not map to output_tokens"
    );
    assert_ne!(
        usage["cache_read_input_tokens"].as_u64().unwrap_or(0), 999,
        "thoughtTokens=999 must not map to cache_read_input_tokens"
    );
    assert_ne!(
        usage["cache_creation_input_tokens"].as_u64().unwrap_or(0), 999,
        "thoughtTokens=999 must not map to cache_creation_input_tokens"
    );
    // Make the `s` variable used to avoid a compiler warning
    let _ = s;
    assert!(matches!(parse(&out), StreamMsg::Result(_)));
}

// =============================================================================
// acp_update_to_stream_json — "usage_update" handling
// =============================================================================
//
// Per spec §3 (docs/specs/opencode-acp.md): a session/update with
// type="usage_update" carries `{cost:{amount,currency}, size, used}` where:
//   size = total context window tokens
//   used = currently used context window tokens
//
// The translation must emit a canonical stream event, e.g.:
//   { "type":"system", "subtype":"usage_update", "cost_usd":0.42,
//     "context_used":45000, "context_size":200000 }
//
// That event must parse via parse_stream_line (Unknown is acceptable
// since there is no dedicated StreamMsg variant yet — what matters is
// the VALUES are present and the events reducer can read them).

/// A `usage_update` with USD cost and size/used must produce a non-None value
/// and the serialised form must contain the cost amount.
#[test]
fn usage_update_produces_non_none_value_with_cost() {
    let update = json!({
        "type": "usage_update",
        "cost": {
            "amount": 0.42,
            "currency": "USD"
        },
        "size": 200000,
        "used": 45000
    });
    let out = acp_update_to_stream_json(&update, SESSION_ID, MODEL);
    assert!(out.is_some(), "usage_update must produce Some(Value), got None");
    let out = out.unwrap();
    let s = serde_json::to_string(&out).unwrap();
    // The cost amount must appear in the output
    assert!(
        s.contains("0.42") || s.contains("cost"),
        "usage_update output must contain cost amount 0.42 or 'cost': {s}"
    );
    // Must parse without error
    parse(&out);
}

/// The canonical usage_update output must carry `cost_usd` matching the ACP
/// `cost.amount` when currency is USD.
#[test]
fn usage_update_cost_usd_matches_acp_amount() {
    let update = json!({
        "type": "usage_update",
        "cost": {
            "amount": 1.75,
            "currency": "USD"
        },
        "size": 100000,
        "used": 30000
    });
    let out = acp_update_to_stream_json(&update, SESSION_ID, MODEL).unwrap();
    // The output must carry cost_usd = 1.75 (exact float comparison is tricky;
    // check it's present and within epsilon)
    let cost_usd = out["cost_usd"].as_f64().unwrap_or(-1.0);
    assert!(
        (cost_usd - 1.75).abs() < 0.001,
        "cost_usd must be 1.75, got: {cost_usd}"
    );
    parse(&out);
}

/// The canonical usage_update output must carry `context_used` (or equivalent)
/// matching the ACP `used` field.
#[test]
fn usage_update_context_used_matches_acp_used() {
    let update = json!({
        "type": "usage_update",
        "cost": {
            "amount": 0.10,
            "currency": "USD"
        },
        "size": 200000,
        "used": 45000
    });
    let out = acp_update_to_stream_json(&update, SESSION_ID, MODEL).unwrap();
    // The output must carry `context_used` = 45000
    let context_used = out["context_used"].as_u64().unwrap_or(0);
    assert_eq!(
        context_used, 45000,
        "context_used must match ACP used=45000, got: {context_used}"
    );
    parse(&out);
}

/// The canonical usage_update output must carry `context_size` matching the ACP
/// `size` field.
#[test]
fn usage_update_context_size_matches_acp_size() {
    let update = json!({
        "type": "usage_update",
        "cost": {
            "amount": 0.05,
            "currency": "USD"
        },
        "size": 200000,
        "used": 10000
    });
    let out = acp_update_to_stream_json(&update, SESSION_ID, MODEL).unwrap();
    let context_size = out["context_size"].as_u64().unwrap_or(0);
    assert_eq!(
        context_size, 200000,
        "context_size must match ACP size=200000, got: {context_size}"
    );
    parse(&out);
}

/// When the currency is NOT USD, the cost must be omitted or set to 0
/// (non-USD costs are not meaningful to the reducer).
#[test]
fn usage_update_non_usd_currency_omits_or_zeros_cost() {
    let update = json!({
        "type": "usage_update",
        "cost": {
            "amount": 99.99,
            "currency": "EUR"   // non-USD
        },
        "size": 200000,
        "used": 50000
    });
    let out = acp_update_to_stream_json(&update, SESSION_ID, MODEL).unwrap();
    // cost_usd must be absent or 0 for non-USD
    let cost_usd = out["cost_usd"].as_f64().unwrap_or(0.0);
    assert!(
        cost_usd.abs() < 0.001,
        "non-USD cost must result in cost_usd=0 or absent, got: {cost_usd}"
    );
    parse(&out);
}

/// When the `cost` field is entirely absent, the event must still be emitted
/// (size/used are still useful) with cost_usd=0 or absent.
#[test]
fn usage_update_missing_cost_field_tolerated() {
    let update = json!({
        "type": "usage_update",
        "size": 200000,
        "used": 30000
        // no "cost" field
    });
    let out = acp_update_to_stream_json(&update, SESSION_ID, MODEL);
    // Must still produce Some value (size/used are useful even without cost)
    assert!(out.is_some(), "usage_update with missing cost must still produce Some(Value)");
    let out = out.unwrap();
    let cost_usd = out["cost_usd"].as_f64().unwrap_or(0.0);
    assert!(
        cost_usd.abs() < 0.001,
        "missing cost → cost_usd must be 0 or absent, got: {cost_usd}"
    );
    // size/used must still be present
    let context_size = out["context_size"].as_u64().unwrap_or(0);
    let context_used = out["context_used"].as_u64().unwrap_or(0);
    assert_eq!(context_size, 200000, "context_size must still be 200000");
    assert_eq!(context_used, 30000, "context_used must still be 30000");
    parse(&out);
}

/// When both `size` and `used` are absent, the event must still be emitted
/// (cost alone is still useful).
#[test]
fn usage_update_missing_size_and_used_tolerated() {
    let update = json!({
        "type": "usage_update",
        "cost": {
            "amount": 0.25,
            "currency": "USD"
        }
        // no "size" or "used"
    });
    let out = acp_update_to_stream_json(&update, SESSION_ID, MODEL);
    // Must still produce Some value (cost is useful even without context)
    assert!(out.is_some(), "usage_update with missing size/used must still produce Some(Value)");
    let out = out.unwrap();
    let cost_usd = out["cost_usd"].as_f64().unwrap_or(-1.0);
    assert!(
        (cost_usd - 0.25).abs() < 0.001,
        "cost_usd must be 0.25 even without size/used, got: {cost_usd}"
    );
    parse(&out);
}

/// The usage_update output type and subtype must be canonical system event shape
/// (so the events reducer can find it via pattern match).
#[test]
fn usage_update_emits_canonical_system_subtype() {
    let update = json!({
        "type": "usage_update",
        "cost": { "amount": 0.42, "currency": "USD" },
        "size": 200000,
        "used": 45000
    });
    let out = acp_update_to_stream_json(&update, SESSION_ID, MODEL).unwrap();
    // Must be a system-type event (so reduce can handle it)
    assert_eq!(
        out["type"].as_str().unwrap_or(""),
        "system",
        "usage_update canonical output must have type='system'"
    );
    assert_eq!(
        out["subtype"].as_str().unwrap_or(""),
        "usage_update",
        "usage_update canonical output must have subtype='usage_update'"
    );
    parse(&out);
}
