//! U4 — cost truthfulness (docs/PLAN.md U4 + memory 'marsad-full-experience').
//!
//! Owner mandate: agent-reported cost (claude stream `result.total_cost_usd` /
//! `result.modelUsage`) is the source of truth end-to-end. The self-computed
//! litellm/hand pricing table (`discovery::model_cost_usd` / `litellm_rate`)
//! is ONLY for historical transcript backfill (a resumed/attached session's
//! `.jsonl`, or the project-wide usage probe) and must never clobber a
//! per-model row that already carries a live, agent-reported cost.
//!
//! **The bug this file pins:** `Core::handle_transcript_backfill` and
//! `Core::handle_usage_update` (crates/marsad-core/src/lib.rs) both do
//! `record.state.model_usage.clear()` followed by inserting ONLY the rows the
//! estimated aggregation just computed. That unconditionally destroys any
//! live per-model rows a `result` event already set — including live rows
//! for models the new estimated pass doesn't even mention. Concretely: an
//! active session gets a live, verified `result.modelUsage` entry; a
//! backfill or usage probe arrives moments later (this is the NORMAL, common
//! sequence for a resumed/attached session) and the live number is silently
//! replaced by a recomputed estimate — or wiped entirely if the estimated
//! pass didn't cover that model. That is exactly the truthfulness violation
//! U4 exists to close.
//!
//! Tests marked RED fail against today's `marsad-core`. Tests marked GREEN
//! PIN already-correct behavior (regression guards) — call these out
//! explicitly so a future fix doesn't accidentally invert them.

use marsad_core::{Core, CoreConfig, LaunchMode, LaunchSpec, Posture, VmUpdate};
use marsad_transport::LoopbackFake;

// ---------------------------------------------------------------------------
// Harness (mirrors the conventions in tests/happy.rs / tests/fakes.rs)
// ---------------------------------------------------------------------------

fn make_core() -> (Core, LoopbackFake) {
    let (transport_a, transport_b) = LoopbackFake::pair();
    let cfg = CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-u4-test".to_string(),
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
    })
    .expect("launch")
}

fn init_line(session_id: &str) -> String {
    format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"system","subtype":"init","session_id":"{session_id}","model":"claude-sonnet-4-6","cwd":"/tmp","tools":[],"mcp_servers":[],"permissionMode":"default"}}}}"#
    )
}

/// A live `result` event, exactly as the claude stream reports it, with a
/// single-model `modelUsage` breakdown. `input`/`output` are deliberately
/// small so the pricing-table recomputation used in the RED tests below
/// (with vastly larger token counts) is unmistakably a different number.
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

fn session_view_from(updates: &[VmUpdate]) -> marsad_viewmodel::SessionView {
    updates
        .iter()
        .find_map(|u| {
            if let VmUpdate::Session(sv) = u {
                Some(sv.clone())
            } else {
                None
            }
        })
        .expect("expected a Session update")
}

// ---------------------------------------------------------------------------
// RED — TranscriptBackfill must not overwrite a live per-model cost
// ---------------------------------------------------------------------------

/// A model that already has a LIVE, agent-reported cost (from a `result`
/// event) must keep that exact cost after a later `TranscriptBackfill` for
/// the SAME model — even though the backfill JSONL reports vastly different
/// token counts that would recompute to a very different ESTIMATED cost via
/// the hand/litellm pricing table.
#[test]
fn live_model_usage_survives_later_transcript_backfill_for_same_model() {
    let (mut core, _peer) = make_core();
    let id = launch(&mut core);
    core.on_agent_line(&init_line(&id)).expect("init");

    // Live, agent-reported: 11 in / 155 out, $0.268 (a verified real capture
    // value used elsewhere in this test suite — see happy.rs).
    core.on_agent_line(&live_result_line(
        &id,
        "claude-sonnet-4.6[1m]",
        11,
        155,
        0.268,
    ))
    .expect("live result");

    // A later backfill recomputes the SAME model from a JSONL with wildly
    // different (much larger) token counts. Under the hand/litellm pricing
    // table this recomputes to several dollars — nowhere near $0.268 — so if
    // the row changes at all, we know the estimate clobbered the live value.
    let jsonl = r#"{"type":"assistant","message":{"model":"claude-sonnet-4.6[1m]","usage":{"input_tokens":900000,"output_tokens":900000,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
    let updates = core
        .on_agent_line(&backfill_line(&id, jsonl))
        .expect("backfill");
    let sv = session_view_from(&updates);

    let row = sv
        .model_usage
        .iter()
        .find(|r| r.model == "claude-sonnet-4.6[1m]")
        .expect("sonnet row must still be present after backfill");
    assert!(
        (row.cost_usd - 0.268).abs() < 1e-9,
        "U4 RED: a live agent-reported cost must not be overwritten by the \
         estimated backfill recomputation — expected 0.268 (live), got {} \
         (looks like the litellm/hand-table estimate)",
        row.cost_usd
    );
    assert_eq!(
        row.input_tokens, 11,
        "U4 RED: live token count must not be overwritten by the backfill's estimate"
    );
}

/// Same defect, via the `UsageUpdate` (project-wide usage probe) path instead
/// of `TranscriptBackfill` — `handle_usage_update` has the identical
/// unconditional `model_usage.clear()` + reinsert bug.
#[test]
fn live_model_usage_survives_later_usage_update_for_same_model() {
    let (mut core, _peer) = make_core();
    let id = launch(&mut core);
    core.on_agent_line(&init_line(&id)).expect("init");

    core.on_agent_line(&live_result_line(&id, "claude-opus-4-8", 5, 2, 0.001))
        .expect("live result");

    // The usage probe reports the SAME model with far larger token counts —
    // would recompute to a very different estimated cost.
    let usage =
        r#"{"claude-opus-4-8":{"input":900000,"output":900000,"cache_read":0,"cache_creation":0}}"#;
    let updates = core
        .on_agent_line(&usage_update_line(&id, usage))
        .expect("usage_update");
    let sv = session_view_from(&updates);

    let row = sv
        .model_usage
        .iter()
        .find(|r| r.model == "claude-opus-4-8")
        .expect("opus row must still be present after usage_update");
    assert!(
        (row.cost_usd - 0.001).abs() < 1e-9,
        "U4 RED: a live agent-reported cost must not be overwritten by the \
         estimated usage-probe recomputation — expected 0.001 (live), got {}",
        row.cost_usd
    );
}

/// The overwrite bug is destructive, not just inaccurate: a backfill JSONL
/// that doesn't even mention a model with an existing LIVE row wipes that
/// row out entirely (today's `model_usage.clear()` runs unconditionally
/// before the estimated rows are inserted). A live sonnet row must survive a
/// backfill that only reports opus usage — ending with BOTH rows present.
#[test]
fn estimated_backfill_must_not_delete_live_row_for_a_different_model() {
    let (mut core, _peer) = make_core();
    let id = launch(&mut core);
    core.on_agent_line(&init_line(&id)).expect("init");

    core.on_agent_line(&live_result_line(&id, "claude-sonnet-4-6", 11, 155, 0.268))
        .expect("live result");

    // Backfill JSONL mentions ONLY opus — no sonnet lines at all.
    let jsonl = r#"{"type":"assistant","message":{"model":"claude-opus-4-5","usage":{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
    let updates = core
        .on_agent_line(&backfill_line(&id, jsonl))
        .expect("backfill");
    let sv = session_view_from(&updates);

    assert!(
        sv.model_usage
            .iter()
            .any(|r| r.model == "claude-sonnet-4-6"),
        "U4 RED: the live sonnet row must survive a backfill that only covers \
         a different model (opus) — got rows: {:?}",
        sv.model_usage.iter().map(|r| &r.model).collect::<Vec<_>>()
    );
    assert!(
        sv.model_usage.iter().any(|r| r.model.contains("opus")),
        "the new estimated opus row must still be added"
    );
}

// ---------------------------------------------------------------------------
// GREEN — already-correct behavior, pinned as regression guards
// ---------------------------------------------------------------------------

/// `SessionView.cost_usd` (the top-level authoritative session cost) is only
/// ever set by a live `result` event (see `marsad_events::reduce_result`) —
/// neither `handle_transcript_backfill` nor `handle_usage_update` touch it.
/// This is ALREADY correct; pinned here as a regression guard so a future
/// change doesn't accidentally wire the top-level cost to the estimated
/// per-model sum.
#[test]
fn session_cost_usd_top_level_survives_backfill() {
    let (mut core, _peer) = make_core();
    let id = launch(&mut core);
    core.on_agent_line(&init_line(&id)).expect("init");
    core.on_agent_line(&live_result_line(&id, "claude-sonnet-4-6", 11, 155, 0.268))
        .expect("live result");

    let jsonl = r#"{"type":"assistant","message":{"model":"claude-sonnet-4-6","usage":{"input_tokens":900000,"output_tokens":900000,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
    let updates = core
        .on_agent_line(&backfill_line(&id, jsonl))
        .expect("backfill");
    let sv = session_view_from(&updates);

    assert!(
        (sv.cost_usd - 0.268).abs() < 1e-9,
        "GREEN pin: top-level cost_usd must remain the live agent-reported \
         value across a backfill, got {}",
        sv.cost_usd
    );
}

/// End-to-end (events → core → SessionView) confirmation of the mandate's
/// "verified live" scenario: a result with `subtype:"success"` AND
/// `is_error:true` still surfaces its cost all the way to the view the UI
/// consumes.
#[test]
fn result_is_error_true_with_success_subtype_cost_flows_to_session_view() {
    let (mut core, _peer) = make_core();
    let id = launch(&mut core);
    core.on_agent_line(&init_line(&id)).expect("init");

    let line = format!(
        r#"{{"type":"AgentEvent","session_id":"{id}","event":{{"type":"result","subtype":"success","is_error":true,"total_cost_usd":2.5,"usage":{{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}},"num_turns":1,"permission_denials":[]}}}}"#
    );
    let updates = core.on_agent_line(&line).expect("result");
    let sv = session_view_from(&updates);

    assert!(
        (sv.cost_usd - 2.5).abs() < 1e-9,
        "GREEN pin: is_error:true + subtype:success must still surface cost \
         through to SessionView, got {}",
        sv.cost_usd
    );
}
