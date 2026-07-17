//! B11 `marsad-viewmodel` — U3b: `SessionView` exposes the compaction count/
//! metadata and rate-limit status folded by `marsad-events` (U3b), so a UI
//! layer built on `SessionView` has something to render instead of the field
//! not existing at all.
//!
//! Mirrors the existing `tools`/`mcp_servers`/`slash_commands` round-trip
//! coverage style in `happy.rs` (`session_view_new_fields_populated` /
//! `session_view_new_fields_empty_by_default_in_literal`).

use marsad_events::Phase;
use marsad_proto::{CompactMetadata, RateLimitInfo};
use marsad_viewmodel::SessionView;

fn base_session_view() -> SessionView {
    SessionView {
        id: "s1".to_string(),
        name: "n".to_string(),
        host_id: "local".to_string(),
        project: "/p".to_string(),
        branch: "main".to_string(),
        model: "m".to_string(),
        state: Phase::Working,
        changed_files: 0,
        subagents: 0,
        running_subagents: 0,
        cost_usd: 0.0,
        tokens: 0,
        context_pct: 0.0,
        gate: None,
        transcript: vec![],
        changes: vec![],
        model_usage: vec![],
        cost_estimated: false,
        compactions: 0,
        last_compaction: None,
        rate_limit: None,
        tools: vec![],
        mcp_servers: vec![],
        slash_commands: vec![],
        posture: "ask".to_string(),
        subagent_list: vec![],
        shell_running: false,
        driver: "claude".to_string(),
        live: true,
        conn_health: "Connected".to_string(),
        end_reason: None,
        plan_entries: vec![],
        current_mode: None,
    }
}

// ---------------------------------------------------------------------------
// Default (never-compacted, never-rate-limited) session
// ---------------------------------------------------------------------------

#[test]
fn session_view_compaction_and_rate_limit_empty_by_default_in_literal() {
    let sv = base_session_view();
    assert_eq!(sv.compactions, 0);
    assert!(sv.last_compaction.is_none());
    assert!(sv.rate_limit.is_none());
}

// ---------------------------------------------------------------------------
// Populated (the U3b wiring's actual purpose: carry real folded values)
// ---------------------------------------------------------------------------

#[test]
fn session_view_compaction_and_rate_limit_populated_round_trip() {
    let sv = SessionView {
        compactions: 2,
        last_compaction: Some(CompactMetadata {
            trigger: "manual".to_string(),
            pre_tokens: 20000,
            post_tokens: 1000,
            cumulative_dropped_tokens: 19000,
            duration_ms: 5000,
        }),
        rate_limit: Some(RateLimitInfo {
            status: "allowed".to_string(),
            resets_at: 1783014000,
            rate_limit_type: "five_hour".to_string(),
            overage_status: "rejected".to_string(),
            is_using_overage: false,
        }),
        ..base_session_view()
    };
    let sv2 = sv.clone();
    assert_eq!(sv, sv2);

    assert_eq!(sv.compactions, 2);
    let m = sv.last_compaction.as_ref().expect("populated");
    assert_eq!(m.trigger, "manual");
    assert_eq!(m.cumulative_dropped_tokens, 19000);

    let rl = sv.rate_limit.as_ref().expect("populated");
    assert_eq!(rl.status, "allowed");
    assert_eq!(rl.rate_limit_type, "five_hour");
}

// ---------------------------------------------------------------------------
// serde defaults — a SessionView JSON blob omitting these fields (an older
// snapshot, or a hand-built minimal fixture) must not fail to deserialize.
// ---------------------------------------------------------------------------

#[test]
fn session_view_compaction_and_rate_limit_default_via_serde_when_missing() {
    let json = r#"{
        "id":"z","name":"z","host_id":"local","project":"/p","branch":"main",
        "model":"m","state":"Working","changed_files":0,"subagents":0,
        "running_subagents":0,"cost_usd":0.0,"tokens":0,"context_pct":0.0,
        "gate":null,"transcript":[],"changes":[],"model_usage":[],
        "tools":[],"mcp_servers":[],"slash_commands":[]
    }"#;
    let sv: SessionView = serde_json::from_str(json).expect("must deserialize");
    assert_eq!(sv.compactions, 0, "missing compactions must default to 0");
    assert!(
        sv.last_compaction.is_none(),
        "missing last_compaction must default to None"
    );
    assert!(
        sv.rate_limit.is_none(),
        "missing rate_limit must default to None"
    );
}

#[test]
fn session_view_compaction_and_rate_limit_deserialize_when_present() {
    let json = r#"{
        "id":"z","name":"z","host_id":"local","project":"/p","branch":"main",
        "model":"m","state":"Working","changed_files":0,"subagents":0,
        "running_subagents":0,"cost_usd":0.0,"tokens":0,"context_pct":0.0,
        "gate":null,"transcript":[],"changes":[],"model_usage":[],
        "tools":[],"mcp_servers":[],"slash_commands":[],
        "compactions":1,
        "last_compaction":{"trigger":"manual","pre_tokens":10,"post_tokens":2,"cumulative_dropped_tokens":8,"duration_ms":100},
        "rate_limit":{"status":"allowed","resetsAt":1,"rateLimitType":"five_hour","overageStatus":"","isUsingOverage":false}
    }"#;
    let sv: SessionView = serde_json::from_str(json).expect("must deserialize");
    assert_eq!(sv.compactions, 1);
    assert_eq!(sv.last_compaction.unwrap().trigger, "manual");
    assert_eq!(sv.rate_limit.unwrap().status, "allowed");
}
