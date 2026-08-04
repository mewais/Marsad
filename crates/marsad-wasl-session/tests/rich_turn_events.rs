//! rich_turn_events.rs — black-box coverage for the six new rich informational
//! `TurnEvent` variants added from the wasit-adapter work:
//!   `usage_update`, `plan_updated`, `modes_updated`, `model_changed`,
//!   `slash_commands_updated`, `compaction`.
//!
//! Wire JSON is hand-authored from `wasl-runtime/src/lib.rs`'s `TurnEvent` serde
//! attributes (`#[serde(tag = "type", rename_all = "snake_case")]`) and the payload
//! struct fields (all `snake_case`, all `#[serde(default)]`). This matches the JSON
//! that `serde_json::to_string(&TurnEvent::UsageUpdate{..})` would produce on the
//! Wasl side — i.e. the real wire.
//!
//! Tests run through `attach()` (the only public path that consumes `turn_event`
//! frames), mirroring the pattern in `wire_translation_completeness.rs`.

mod real_support;

use marsad_wasl_session::{
    CompactionInfo, PlanEntry, RealWaslSessionProvider, SessionMode, SlashCommandInfo,
    WaslSessionClient, WaslSessionClientConfig, WaslSessionEventKind, WaslSessionProvider,
    WaslStreamItem, WaslTargetKey,
};
use real_support::{
    compact_fixture, compact_json, sse_frame, val, BufferedResponse, EventsScript,
    MockRuntimeServer,
};

// ---------------------------------------------------------------------------
// Helpers (mirror wire_translation_completeness.rs)
// ---------------------------------------------------------------------------

fn bound_client(server: &MockRuntimeServer) -> Box<dyn WaslSessionClient> {
    server.push_create(BufferedResponse::json(200, compact_fixture("target.json")));
    let provider = RealWaslSessionProvider::new(WaslSessionClientConfig {
        base_url: server.base_url(),
    });
    let (client, _snapshot) = provider
        .get_or_start(&WaslTargetKey(val(r#""ctx-rich""#)))
        .expect("create should succeed");
    client
}

/// Drive one `turn_event` payload through `attach` at SSE id 30 and return the
/// single `WaslSessionEventKind` it maps to.
fn single_event_kind_from_wire(payload: &str) -> WaslSessionEventKind {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    let mut lines = sse_frame("hello", None, &compact_fixture("hello.json"));
    lines.extend(sse_frame("turn_event", Some(30), &compact_json(payload)));
    server.push_events(EventsScript::new(lines).drop_after(true));

    let mut stream = client.attach(29);
    let mut kinds: Vec<WaslSessionEventKind> = Vec::new();
    while let Some(item) = stream.next() {
        match item {
            WaslStreamItem::Event(ev) => kinds.push(ev.kind),
            WaslStreamItem::Truncated { earliest, latest } => {
                panic!("unexpected Truncated{{{earliest},{latest}}}")
            }
        }
    }
    assert_eq!(kinds.len(), 1, "one event expected, got {kinds:?}");
    kinds.into_iter().next().unwrap()
}

// ===========================================================================
// usage_update
// wire tag: "usage_update"
// payload: { usage: UsageSnapshot, context_pct?: u8 }
// UsageSnapshot: { input_tokens, output_tokens, cache_read_tokens,
//                  cache_creation_tokens, cost_usd }
// ===========================================================================

#[test]
fn attach_maps_usage_update_with_all_token_kinds_and_context_pct() {
    // wasl-runtime: TurnEvent::UsageUpdate { usage: UsageSnapshot { ... }, context_pct: Some(42) }
    // Wire: { "type": "usage_update", "usage": { ... }, "context_pct": 42 }
    let kind = single_event_kind_from_wire(
        r#"{
            "type": "usage_update",
            "usage": {
                "input_tokens": 1000,
                "output_tokens": 500,
                "cache_read_tokens": 200,
                "cache_creation_tokens": 100,
                "cost_usd": 0.05
            },
            "context_pct": 42
        }"#,
    );
    match kind {
        WaslSessionEventKind::UsageUpdate {
            cost_usd,
            tokens,
            context_pct,
        } => {
            // tokens = input + output + cache_read + cache_creation = 1800
            assert_eq!(tokens, 1800, "tokens must sum all four token fields");
            // cost_usd is kept honestly from the snapshot
            assert!(
                (cost_usd - 0.05).abs() < 1e-9,
                "cost_usd must be forwarded from snapshot"
            );
            assert_eq!(context_pct, Some(42), "context_pct must be forwarded");
        }
        other => panic!("expected UsageUpdate, got {other:?}"),
    }
}

#[test]
fn attach_maps_usage_update_without_context_pct() {
    // context_pct is #[serde(default, skip_serializing_if = "Option::is_none")]
    // so an absent field must deserialize to None.
    let kind = single_event_kind_from_wire(
        r#"{
            "type": "usage_update",
            "usage": {
                "input_tokens": 400,
                "output_tokens": 100,
                "cost_usd": 0.01
            }
        }"#,
    );
    match kind {
        WaslSessionEventKind::UsageUpdate { context_pct, tokens, .. } => {
            assert_eq!(context_pct, None, "absent context_pct -> None");
            assert_eq!(tokens, 500, "tokens = input + output (cache fields default to 0)");
        }
        other => panic!("expected UsageUpdate, got {other:?}"),
    }
}

// ===========================================================================
// plan_updated
// wire tag: "plan_updated"
// payload: { entries: Vec<PlanEntry> }
// PlanEntry: { content: String, priority: String }
// ===========================================================================

#[test]
fn attach_maps_plan_updated_with_entries() {
    let kind = single_event_kind_from_wire(
        r#"{
            "type": "plan_updated",
            "entries": [
                { "content": "Write tests",  "priority": "high" },
                { "content": "Ship feature", "priority": "medium" }
            ]
        }"#,
    );
    match kind {
        WaslSessionEventKind::PlanUpdated { entries } => {
            assert_eq!(entries.len(), 2);
            assert_eq!(
                entries[0],
                PlanEntry {
                    content: "Write tests".to_string(),
                    priority: "high".to_string(),
                }
            );
            assert_eq!(
                entries[1],
                PlanEntry {
                    content: "Ship feature".to_string(),
                    priority: "medium".to_string(),
                }
            );
        }
        other => panic!("expected PlanUpdated, got {other:?}"),
    }
}

#[test]
fn attach_maps_plan_updated_with_empty_entries() {
    // An empty plan clears the list.
    let kind = single_event_kind_from_wire(r#"{ "type": "plan_updated", "entries": [] }"#);
    match kind {
        WaslSessionEventKind::PlanUpdated { entries } => {
            assert!(entries.is_empty(), "empty entries must produce an empty Vec");
        }
        other => panic!("expected PlanUpdated, got {other:?}"),
    }
}

// ===========================================================================
// modes_updated
// wire tag: "modes_updated"
// payload: { available_modes?: Vec<AgentMode>, current_mode?: String }
// AgentMode: { id, label, description }
// ===========================================================================

#[test]
fn attach_maps_modes_updated_with_modes_and_current() {
    let kind = single_event_kind_from_wire(
        r#"{
            "type": "modes_updated",
            "available_modes": [
                { "id": "plan",  "label": "Plan",  "description": "Plan mode" },
                { "id": "build", "label": "Build", "description": "Build mode" }
            ],
            "current_mode": "plan"
        }"#,
    );
    match kind {
        WaslSessionEventKind::ModesUpdated {
            available_modes,
            current_mode,
        } => {
            assert_eq!(available_modes.len(), 2);
            assert_eq!(
                available_modes[0],
                SessionMode {
                    id: "plan".to_string(),
                    label: "Plan".to_string(),
                    description: "Plan mode".to_string(),
                }
            );
            assert_eq!(
                available_modes[1],
                SessionMode {
                    id: "build".to_string(),
                    label: "Build".to_string(),
                    description: "Build mode".to_string(),
                }
            );
            assert_eq!(current_mode, Some("plan".to_string()));
        }
        other => panic!("expected ModesUpdated, got {other:?}"),
    }
}

#[test]
fn attach_maps_modes_updated_current_mode_only() {
    // available_modes is skip_serializing_if = "Vec::is_empty" / default
    // so an absent key deserializes to an empty Vec.
    let kind = single_event_kind_from_wire(
        r#"{ "type": "modes_updated", "current_mode": "build" }"#,
    );
    match kind {
        WaslSessionEventKind::ModesUpdated {
            available_modes,
            current_mode,
        } => {
            assert!(
                available_modes.is_empty(),
                "absent available_modes -> empty Vec"
            );
            assert_eq!(current_mode, Some("build".to_string()));
        }
        other => panic!("expected ModesUpdated, got {other:?}"),
    }
}

// ===========================================================================
// model_changed
// wire tag: "model_changed"
// payload: { model: String }
// ===========================================================================

#[test]
fn attach_maps_model_changed() {
    let kind = single_event_kind_from_wire(
        r#"{ "type": "model_changed", "model": "claude-opus-4-5" }"#,
    );
    match kind {
        WaslSessionEventKind::ModelChanged { model } => {
            assert_eq!(model, "claude-opus-4-5");
        }
        other => panic!("expected ModelChanged, got {other:?}"),
    }
}

// ===========================================================================
// slash_commands_updated
// wire tag: "slash_commands_updated"
// payload: { commands: Vec<SlashCommandInfo> }
// SlashCommandInfo: { name, description, argument_hint }
// ===========================================================================

#[test]
fn attach_maps_slash_commands_updated_with_commands() {
    let kind = single_event_kind_from_wire(
        r#"{
            "type": "slash_commands_updated",
            "commands": [
                {
                    "name": "compact",
                    "description": "Compact the context",
                    "argument_hint": "[focus]"
                },
                {
                    "name": "review",
                    "description": "Review the diff"
                }
            ]
        }"#,
    );
    match kind {
        WaslSessionEventKind::SlashCommandsUpdated { commands } => {
            assert_eq!(commands.len(), 2);
            assert_eq!(
                commands[0],
                SlashCommandInfo {
                    name: "compact".to_string(),
                    description: "Compact the context".to_string(),
                    argument_hint: "[focus]".to_string(),
                }
            );
            // argument_hint defaults to empty string when absent on the wire.
            assert_eq!(
                commands[1],
                SlashCommandInfo {
                    name: "review".to_string(),
                    description: "Review the diff".to_string(),
                    argument_hint: String::new(),
                }
            );
        }
        other => panic!("expected SlashCommandsUpdated, got {other:?}"),
    }
}

// ===========================================================================
// compaction
// wire tag: "compaction"
// payload: { info: CompactionInfo }
// CompactionInfo: { trigger, pre_tokens, post_tokens,
//                   cumulative_dropped_tokens, duration_ms }
// ===========================================================================

#[test]
fn attach_maps_compaction_with_full_info() {
    let kind = single_event_kind_from_wire(
        r#"{
            "type": "compaction",
            "info": {
                "trigger":                   "auto",
                "pre_tokens":                80000,
                "post_tokens":               20000,
                "cumulative_dropped_tokens": 60000,
                "duration_ms":               350
            }
        }"#,
    );
    match kind {
        WaslSessionEventKind::Compaction { info } => {
            assert_eq!(
                info,
                CompactionInfo {
                    trigger: "auto".to_string(),
                    pre_tokens: 80000,
                    post_tokens: 20000,
                    cumulative_dropped_tokens: 60000,
                    duration_ms: 350,
                }
            );
        }
        other => panic!("expected Compaction, got {other:?}"),
    }
}

#[test]
fn attach_maps_compaction_with_defaults() {
    // All CompactionInfo fields are `#[serde(default)]` — an info object with
    // only `trigger` set must still parse, leaving numeric fields as 0.
    let kind = single_event_kind_from_wire(
        r#"{ "type": "compaction", "info": { "trigger": "manual" } }"#,
    );
    match kind {
        WaslSessionEventKind::Compaction { info } => {
            assert_eq!(info.trigger, "manual");
            assert_eq!(info.pre_tokens, 0);
            assert_eq!(info.cumulative_dropped_tokens, 0);
        }
        other => panic!("expected Compaction, got {other:?}"),
    }
}

// ===========================================================================
// thinking_delta — deliberately NOT surfaced; must still be tolerantly dropped
// (no Unknown or parse error for a turn_event the new variants don't handle).
// These two tests exercise the tolerant-skip path (turn_event_from_json returns
// None for unrecognised types). Under llvm-cov, real.rs replaces the `?`-skip
// with an expect() to keep the happy path covered under instrument mode — so
// these tests would panic. Mark them #[cfg(not(coverage))] (same pattern as the
// existing real_*.rs tests that exercise the None path). They run under
// `cargo test` and `cargo clippy` but are excluded from `cargo llvm-cov`.
// ===========================================================================

#[cfg(not(coverage))]
#[test]
fn attach_tolerantly_drops_thinking_delta() {
    // `thinking_delta` is present on the Wasl wire but Marsad deliberately
    // drops it (like the solo-session path). An attach() call whose only
    // event is a thinking_delta must yield zero items rather than panicking
    // or producing an Unknown.
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    let mut lines = sse_frame("hello", None, &compact_fixture("hello.json"));
    lines.extend(sse_frame(
        "turn_event",
        Some(31),
        &compact_json(r#"{ "type": "thinking_delta", "text": "hmm..." }"#),
    ));
    server.push_events(EventsScript::new(lines).drop_after(true));

    let mut stream = client.attach(30);
    let mut items: Vec<WaslStreamItem> = Vec::new();
    while let Some(it) = stream.next() {
        items.push(it);
    }
    assert!(
        items.is_empty(),
        "thinking_delta must be tolerantly dropped; got {items:?}"
    );
}

// ===========================================================================
// unknown future variant — forward-compat tolerant skip (existing behaviour
// preserved with new variants in the enum).
// ===========================================================================

#[cfg(not(coverage))]
#[test]
fn attach_still_tolerantly_drops_unknown_turn_event_type() {
    // After adding new variants, the existing tolerant-skip path for an
    // unrecognized `type` must still work. A hypothetical future "cost_snapshot"
    // must be dropped silently.
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    let mut lines = sse_frame("hello", None, &compact_fixture("hello.json"));
    lines.extend(sse_frame(
        "turn_event",
        Some(32),
        &compact_json(r#"{ "type": "cost_snapshot", "value": 99 }"#),
    ));
    server.push_events(EventsScript::new(lines).drop_after(true));

    let mut stream = client.attach(31);
    let mut items: Vec<WaslStreamItem> = Vec::new();
    while let Some(it) = stream.next() {
        items.push(it);
    }
    assert!(
        items.is_empty(),
        "unknown turn_event type must be tolerantly dropped; got {items:?}"
    );
}
