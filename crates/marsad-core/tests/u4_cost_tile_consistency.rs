//! u4_cost_tile_consistency.rs — regression coverage for review finding #4
//! (cost_usd and cost_estimated can contradict in the UI).
//!
//! `SessionView.cost_usd` was a raw pass-through of `SessionState.cost_usd`,
//! which is set ONLY by a live `result` event (`marsad_events::reduce_result`).
//! `cost_estimated` is derived independently from `model_usage` provenance
//! (`make_session_view`). So a backfill-only session (no live result) got
//! `cost_estimated: true` while `cost_usd: 0.0`, even though its
//! `model_usage` rows carry real, nonzero per-model estimated dollars. The
//! Usage tab then rendered a "~ estimated $0.00" HEADLINE above a
//! model-breakdown table showing nonzero estimated dollars — a direct
//! contradiction — and the "Cost today" aggregate tile (which filters on
//! `cost_usd != 0`) silently excluded such sessions.
//!
//! Fix: when the session cost is estimated, `SessionView.cost_usd` reflects
//! the summed per-model estimated cost, so the headline equals the sum of
//! the breakdown rows below it. A LIVE session's authoritative
//! agent-reported cost is never overridden.

use marsad_core::{Core, CoreConfig, LaunchMode, LaunchSpec, VmUpdate};
use marsad_transport::LoopbackFake;

fn make_core() -> (Core, LoopbackFake) {
    let (transport_a, transport_b) = LoopbackFake::pair();
    let cfg = CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-u4-cost-tile-test".to_string(),
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
        session_mode: String::new(),
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
    format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"session_init","native_session_id":"{session_id}","model":"claude-sonnet-4-6","cwd":"/tmp","tools":[],"mcp_servers":[],"permission_mode":"default"}}}}"#
    )
}

fn live_result_line(session_id: &str, model: &str, input: u64, output: u64, cost: f64) -> String {
    let mut per_model = serde_json::Map::new();
    per_model.insert(model.to_string(), serde_json::json!({
        "input_tokens": input,
        "output_tokens": output,
        "cost_usd": cost,
    }));
    serde_json::json!({
        "type": "AgentEvent",
        "session_id": session_id,
        "event": {
            "type": "turn_completed",
            "usage": {
                "input_tokens": input,
                "output_tokens": output,
                "cache_read_tokens": 0,
                "cache_creation_tokens": 0,
                "cost_usd": cost,
            },
            "per_model": per_model,
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

/// The core assertion: for an estimated-only session, the headline
/// `cost_usd` (what the Usage tab's top tile and the "Cost today" aggregate
/// read) must EQUAL the sum of the model-breakdown rows' `cost_usd` (what
/// the table below renders) — never a stale 0.0.
#[test]
fn estimated_only_session_headline_cost_equals_sum_of_model_breakdown() {
    let (mut core, _peer) = make_core();
    let id = launch(&mut core);
    core.on_agent_line(&init_line(&id)).expect("init");

    // Two distinct models, backfill-only (all estimated), each with real
    // token counts → nonzero estimated per-model cost.
    let jsonl = concat!(
        r#"{"type":"assistant","message":{"model":"claude-sonnet-4-6","usage":{"input_tokens":1000000,"output_tokens":500000,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#,
        "\n",
        r#"{"type":"assistant","message":{"model":"claude-haiku-4-5","usage":{"input_tokens":200000,"output_tokens":100000,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#,
    );
    let updates = core
        .on_agent_line(&backfill_line(&id, jsonl))
        .expect("backfill");
    let sv = session_view_from(&updates);

    assert!(
        sv.cost_estimated,
        "sanity: a backfill-only session must be cost_estimated"
    );
    assert!(
        !sv.model_usage.is_empty(),
        "sanity: the backfill must have produced model_usage rows"
    );

    let breakdown_sum: f64 = sv.model_usage.iter().map(|r| r.cost_usd).sum();
    assert!(
        breakdown_sum > 0.0,
        "sanity: the estimated model breakdown must sum to a nonzero cost, got {breakdown_sum}"
    );

    assert!(
        (sv.cost_usd - breakdown_sum).abs() < 1e-9,
        "the estimated session headline cost_usd ({}) must equal the sum of its \
         model-breakdown rows ({breakdown_sum}) — the tile and the table must agree",
        sv.cost_usd
    );
    // And specifically it must NOT be the old stale 0.0.
    assert!(
        sv.cost_usd > 0.0,
        "an estimated-only session with real model_usage must report a nonzero \
         headline cost so the 'Cost today' aggregate (which filters cost_usd != 0) \
         includes it, got {}",
        sv.cost_usd
    );
}

/// A LIVE session's authoritative agent-reported `cost_usd` must never be
/// overridden by a per-model estimated sum. Here the live `result` reports
/// `$9.99` for a token count whose pricing-table recompute would be a very
/// different number — the headline must stay `$9.99`.
#[test]
fn live_session_headline_cost_is_the_agent_reported_value_not_an_estimate() {
    let (mut core, _peer) = make_core();
    let id = launch(&mut core);
    core.on_agent_line(&init_line(&id)).expect("init");
    let updates = core
        .on_agent_line(&live_result_line(
            &id,
            "claude-sonnet-4-6",
            1_000_000,
            1_000_000,
            9.99,
        ))
        .expect("live result");
    let sv = session_view_from(&updates);

    assert!(
        !sv.cost_estimated,
        "sanity: a live-result session must not be cost_estimated"
    );
    assert!(
        (sv.cost_usd - 9.99).abs() < 1e-9,
        "a live session's headline cost_usd must be the agent-reported total \
         (9.99), never a table recomputation, got {}",
        sv.cost_usd
    );
}

/// An empty session (no cost data at all) is neither estimated nor costed:
/// headline stays 0.0, cost_estimated stays false. (Exercises the
/// non-estimated branch with an empty model_usage — the fall-through.)
#[test]
fn empty_session_has_zero_headline_cost_and_is_not_estimated() {
    let (mut core, _peer) = make_core();
    let id = launch(&mut core);
    let updates = core.on_agent_line(&init_line(&id)).expect("init");
    let sv = session_view_from(&updates);

    assert!(!sv.cost_estimated);
    assert_eq!(sv.cost_usd, 0.0);
    assert!(sv.model_usage.is_empty());
}
