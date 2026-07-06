//! U4 — cost truthfulness: the "estimated" tag (docs/PLAN.md U4 + memory
//! 'marsad-full-experience').
//!
//! Owner mandate: "The self-computed litellm/hand pricing table ... remains
//! ONLY for historical transcript backfill and MUST be labeled 'estimated'
//! in the UI."
//!
//! Today NOTHING in the wire contract the UI receives (`AppSnapshot` /
//! `SessionView` / `ModelUsageRow`, all serialized to JSON for the Tauri
//! bridge — see apps/desktop/src/bridge/types.ts) carries any provenance
//! marker at all: a per-model cost figure computed by
//! `discovery::model_cost_usd` (backfill/usage-probe, an ESTIMATE) is
//! serialized identically to one copied verbatim from the claude stream's
//! `result.modelUsage` (LIVE, agent-reported, authoritative). The desktop UI
//! (Usage tab's per-model breakdown table) has no way to tell them apart, so
//! it cannot honor the "MUST be labeled 'estimated'" requirement no matter
//! how the frontend is built.
//!
//! These tests probe the JSON actually produced for the UI (not internal
//! struct fields) so they fail via a plain runtime assertion — not a compile
//! error — and so they pin the WIRE CONTRACT rather than any particular
//! internal type shape. This intentionally leaves the exact field name/type
//! open for the implementing change; the tests just require SOME boolean
//! provenance marker to exist:
//!   - a per-row `"estimated"` (or equivalent) key on each `model_usage[]`
//!     entry in the serialized `SessionView`, and
//!   - a session-level `"cost_estimated"` (or equivalent) key on the
//!     serialized `SessionView` for sessions whose displayed cost is
//!     backfill-only (no live `result` has ever been received).
//!
//! ALL tests in this file are expected RED today — this is net-new work, not
//! a partially-implemented feature.

use marsad_core::{Core, CoreConfig, LaunchMode, LaunchSpec, Posture, VmUpdate};
use marsad_transport::LoopbackFake;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn make_core() -> (Core, LoopbackFake) {
    let (transport_a, transport_b) = LoopbackFake::pair();
    let cfg = CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-u4-tag-test".to_string(),
    };
    let core = Core::new(cfg, Box::new(transport_a)).expect("Core::new should succeed");
    (core, transport_b)
}

fn launch(core: &mut Core) -> String {
    core.launch_session(LaunchSpec {
        project: "/home/user/proj".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: "claude-sonnet-4-6".to_string(),
        posture: Posture::Ask,
        first_prompt: None,
        mode: LaunchMode::Fresh,
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
    })
    .expect("launch")
}

fn init_line(session_id: &str) -> String {
    serde_json::json!({
        "type": "AgentEvent",
        "session_id": session_id,
        "event": {
            "type": "system",
            "subtype": "init",
            "session_id": session_id,
            "model": "claude-sonnet-4-6",
            "cwd": "/tmp",
            "tools": [],
            "mcp_servers": [],
            "permissionMode": "default",
        }
    })
    .to_string()
}

fn live_result_line(session_id: &str, model: &str, input: u64, output: u64, cost: f64) -> String {
    serde_json::json!({
        "type": "AgentEvent",
        "session_id": session_id,
        "event": {
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "total_cost_usd": cost,
            "usage": {
                "input_tokens": input,
                "output_tokens": output,
                "cache_read_input_tokens": 0,
                "cache_creation_input_tokens": 0,
            },
            "num_turns": 1,
            "permission_denials": [],
            "modelUsage": {
                (model): {
                    "inputTokens": input,
                    "outputTokens": output,
                    "costUSD": cost,
                }
            }
        }
    })
    .to_string()
}

fn backfill_line(session_id: &str, jsonl: &str) -> String {
    serde_json::json!({
        "type": "TranscriptBackfill",
        "session_id": session_id,
        "jsonl": jsonl,
    })
    .to_string()
}

fn usage_update_line(session_id: &str, usage_json: &str) -> String {
    format!(r#"{{"type":"UsageUpdate","session_id":"{session_id}","usage":{usage_json}}}"#)
}

fn session_json_from(updates: &[VmUpdate]) -> serde_json::Value {
    let sv = updates
        .iter()
        .find_map(|u| {
            if let VmUpdate::Session(sv) = u {
                Some(sv.clone())
            } else {
                None
            }
        })
        .expect("expected a Session update");
    serde_json::to_value(&sv).expect("SessionView must serialize")
}

fn model_row_json<'a>(sv_json: &'a serde_json::Value, model: &str) -> &'a serde_json::Value {
    sv_json["model_usage"]
        .as_array()
        .expect("model_usage must be an array")
        .iter()
        .find(|r| r["model"] == model)
        .unwrap_or_else(|| panic!("no model_usage row for {model} in {sv_json}"))
}

// ---------------------------------------------------------------------------
// Per-model row provenance
// ---------------------------------------------------------------------------

/// A `model_usage` row populated from a LIVE `result.modelUsage` entry must
/// be tagged as NOT estimated in the JSON the UI receives.
#[test]
fn live_model_usage_row_is_tagged_not_estimated_in_json() {
    let (mut core, _peer) = make_core();
    let id = launch(&mut core);
    core.on_agent_line(&init_line(&id)).expect("init");
    let updates = core
        .on_agent_line(&live_result_line(&id, "claude-sonnet-4-6", 11, 155, 0.268))
        .expect("live result");

    let sv_json = session_json_from(&updates);
    let row = model_row_json(&sv_json, "claude-sonnet-4-6");
    assert_eq!(
        row.get("estimated"),
        Some(&serde_json::Value::Bool(false)),
        "U4 RED: a live-reported model_usage row must serialize an \
         'estimated': false marker for the UI — row json: {row}"
    );
}

/// A `model_usage` row populated ONLY from a `TranscriptBackfill` (no live
/// `result` ever received for this session) must be tagged as estimated in
/// the JSON the UI receives — this is the litellm/hand pricing table path
/// the owner mandate requires to be labeled "estimated".
#[test]
fn backfill_only_model_usage_row_is_tagged_estimated_in_json() {
    let (mut core, _peer) = make_core();
    let id = launch(&mut core);
    core.on_agent_line(&init_line(&id)).expect("init");
    // No live result event — this session has NEVER received an
    // agent-reported cost. Its only cost data comes from the backfill.
    let jsonl = r#"{"type":"assistant","message":{"model":"claude-haiku-4-5","usage":{"input_tokens":1000,"output_tokens":500,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
    let updates = core
        .on_agent_line(&backfill_line(&id, jsonl))
        .expect("backfill");

    let sv_json = session_json_from(&updates);
    let row = model_row_json(&sv_json, "claude-haiku-4-5");
    assert_eq!(
        row.get("estimated"),
        Some(&serde_json::Value::Bool(true)),
        "U4 RED: a backfill-only model_usage row must serialize an \
         'estimated': true marker for the UI — row json: {row}"
    );
}

/// Same requirement via the `UsageUpdate` (project-wide usage probe) path.
#[test]
fn usage_update_only_model_usage_row_is_tagged_estimated_in_json() {
    let (mut core, _peer) = make_core();
    let id = launch(&mut core);
    core.on_agent_line(&init_line(&id)).expect("init");
    let usage =
        r#"{"claude-opus-4-8":{"input":1000,"output":0,"cache_read":0,"cache_creation":0}}"#;
    let updates = core
        .on_agent_line(&usage_update_line(&id, usage))
        .expect("usage_update");

    let sv_json = session_json_from(&updates);
    let row = model_row_json(&sv_json, "claude-opus-4-8");
    assert_eq!(
        row.get("estimated"),
        Some(&serde_json::Value::Bool(true)),
        "U4 RED: a usage-probe-only model_usage row must serialize an \
         'estimated': true marker for the UI — row json: {row}"
    );
}

// ---------------------------------------------------------------------------
// Session-level provenance (drives the Usage tab's top Cost tile)
// ---------------------------------------------------------------------------

/// A session that has NEVER received a live `result` event, and whose only
/// cost figure comes from a transcript backfill, must expose a session-level
/// marker so the Usage tab's top "Cost" tile can render a visible
/// estimated/"~" affordance instead of presenting a recomputed number as
/// plain fact.
#[test]
fn session_with_only_backfill_cost_is_tagged_cost_estimated_in_json() {
    let (mut core, _peer) = make_core();
    let id = launch(&mut core);
    core.on_agent_line(&init_line(&id)).expect("init");
    let jsonl = r#"{"type":"assistant","message":{"model":"claude-haiku-4-5","usage":{"input_tokens":1000,"output_tokens":500,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
    let updates = core
        .on_agent_line(&backfill_line(&id, jsonl))
        .expect("backfill");

    let sv_json = session_json_from(&updates);
    assert_eq!(
        sv_json.get("cost_estimated"),
        Some(&serde_json::Value::Bool(true)),
        "U4 RED: a session whose only cost data is backfill-derived must \
         serialize a session-level 'cost_estimated': true marker — session json: {sv_json}"
    );
}

/// A session with a LIVE agent-reported result must expose that same
/// session-level marker as `false` — the Usage tab's Cost tile must render
/// plainly, with no estimate marker, once real agent cost has been reported.
#[test]
fn session_with_live_result_is_not_tagged_cost_estimated_in_json() {
    let (mut core, _peer) = make_core();
    let id = launch(&mut core);
    core.on_agent_line(&init_line(&id)).expect("init");
    let updates = core
        .on_agent_line(&live_result_line(&id, "claude-sonnet-4-6", 11, 155, 0.268))
        .expect("live result");

    let sv_json = session_json_from(&updates);
    assert_eq!(
        sv_json.get("cost_estimated"),
        Some(&serde_json::Value::Bool(false)),
        "U4 RED: a session with a live agent-reported result must serialize \
         'cost_estimated': false (or the marker must default to false) — \
         session json: {sv_json}"
    );
}
