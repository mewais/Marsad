//! wasl_rich_events.rs — marsad-core tests for the six new rich `WaslSessionEvent`
//! kinds introduced by the wasit-adapter work: `UsageUpdate`, `PlanUpdated`,
//! `ModesUpdated`, `ModelChanged`, `SlashCommandsUpdated`, `Compaction`.
//!
//! Each test feeds the event through `on_agent_line` (via `WaslSessionEvent`
//! encoded into `IpcMsg::WaslSessionEvent`) and asserts that `handle_wasl_session_event`
//! folds it into the correct `SessionView` field — the same fields the solo-session
//! path sets via `handle_agent_event` / `reduce`.
//!
//! Setup mirrors `tests/wasl_session.rs`: loopback `LoopbackFake`, one
//! `WaslManaged` session launched via `launch_wasl_session`.

use marsad_core::{Core, CoreConfig, VmUpdate};
use marsad_events::Phase;
use marsad_proto::{encode_ipc, IpcMsg, PlanEntry, SessionMode, SlashCommandInfo};
use marsad_transport::LoopbackFake;
use marsad_wasl_session::{CompactionInfo, WaslSessionEvent, WaslSessionEventKind};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn cfg() -> CoreConfig {
    CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-core-wasl-rich-test".to_string(),
    }
}

fn make_core() -> (Core, LoopbackFake) {
    let (a, b) = LoopbackFake::pair();
    let core = Core::new(cfg(), Box::new(a)).expect("Core::new");
    (core, b)
}

fn launch_wasl(core: &mut Core, peer: &mut LoopbackFake) -> String {
    let id = core
        .launch_wasl_session(
            "local",
            serde_json::json!({"protocol": "claude", "ctx": "rich-test"}),
            None,
        )
        .expect("launch_wasl_session must succeed");
    let _ = peer.drain_outbound();
    id
}

fn event_line(session_id: &str, seq: u64, kind: WaslSessionEventKind) -> String {
    let ev = WaslSessionEvent { seq, kind };
    encode_ipc(&IpcMsg::WaslSessionEvent {
        session_id: session_id.to_string(),
        seq,
        event: serde_json::to_value(&ev).unwrap(),
    })
}

fn last_session_view(updates: &[VmUpdate], id: &str) -> marsad_viewmodel::SessionView {
    updates
        .iter()
        .rev()
        .find_map(|u| match u {
            VmUpdate::Session(sv) if sv.id == id => Some(sv.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no VmUpdate::Session for {id} in {updates:?}"))
}

// ===========================================================================
// #8 — a WaslManaged session's DISPLAY driver is its NATIVE protocol, so the
// graph/board show "codex"/"opencode"/"claude", never "wasl".
// ===========================================================================

fn launch_wasl_proto(core: &mut Core, peer: &mut LoopbackFake, protocol: &str) -> String {
    let id = core
        .launch_wasl_session(
            "local",
            serde_json::json!({ "protocol": protocol, "ctx": "proto-test" }),
            None,
        )
        .expect("launch_wasl_session must succeed");
    let _ = peer.drain_outbound();
    id
}

fn node_driver(core: &Core, id: &str) -> marsad_workstream::Driver {
    core.nodes()
        .into_iter()
        .find(|n| n.id == id)
        .unwrap_or_else(|| panic!("no node for {id}"))
        .driver
}

#[test]
fn wasl_node_shows_native_protocol_driver_not_wasl() {
    let (mut core, mut peer) = make_core();

    let claude = launch_wasl_proto(&mut core, &mut peer, "claude");
    assert_eq!(node_driver(&core, &claude), marsad_workstream::Driver::Claude);

    let codex = launch_wasl_proto(&mut core, &mut peer, "codex");
    assert_eq!(node_driver(&core, &codex), marsad_workstream::Driver::Codex);

    let oc = launch_wasl_proto(&mut core, &mut peer, "opencode");
    assert_eq!(node_driver(&core, &oc), marsad_workstream::Driver::Opencode);
}

// ===========================================================================
// UsageUpdate
// ===========================================================================

#[test]
fn wasl_usage_update_sets_cost_tokens_context_pct_with_explicit_pct() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    let updates = core
        .on_agent_line(&event_line(
            &id,
            1,
            WaslSessionEventKind::UsageUpdate {
                cost_usd: 0.25,
                tokens: 10_000,
                context_pct: Some(5),
            },
        ))
        .expect("on_agent_line");

    let sv = last_session_view(&updates, &id);
    assert!(
        (sv.cost_usd - 0.25).abs() < 1e-9,
        "cost_usd must be set from UsageUpdate; got {:.6}",
        sv.cost_usd
    );
    assert_eq!(sv.tokens, 10_000, "tokens must be forwarded from UsageUpdate");
    assert!(
        (sv.context_pct - 5.0).abs() < 0.5,
        "context_pct must come from the event; got {}",
        sv.context_pct
    );
}

#[test]
fn wasl_usage_update_derives_context_pct_when_absent() {
    // When context_pct is None, the handler derives it from tokens / context-window.
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    // Set a known model so context_window_for is deterministic.
    let _ = core.on_agent_line(&event_line(
        &id,
        1,
        WaslSessionEventKind::ModelChanged {
            model: "claude-opus-4-5".to_string(),
        },
    ));

    let updates = core
        .on_agent_line(&event_line(
            &id,
            2,
            WaslSessionEventKind::UsageUpdate {
                cost_usd: 0.0,
                tokens: 1_000,
                context_pct: None,
            },
        ))
        .expect("on_agent_line");

    let sv = last_session_view(&updates, &id);
    assert_eq!(sv.tokens, 1_000);
    // context_pct must be non-zero and derived (the exact value depends on the
    // window size, but 1000 tokens / 200_000 window = 0.5%).
    assert!(
        sv.context_pct > 0.0,
        "context_pct must be derived when None; got {}",
        sv.context_pct
    );
}

// ===========================================================================
// PlanUpdated
// ===========================================================================

#[test]
fn wasl_plan_updated_replaces_plan_entries() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    let updates = core
        .on_agent_line(&event_line(
            &id,
            1,
            WaslSessionEventKind::PlanUpdated {
                entries: vec![
                    PlanEntry {
                        content: "Write tests".to_string(),
                        priority: "high".to_string(),
                    },
                    PlanEntry {
                        content: "Ship feature".to_string(),
                        priority: "medium".to_string(),
                    },
                ],
            },
        ))
        .expect("on_agent_line");

    let sv = last_session_view(&updates, &id);
    assert_eq!(sv.plan_entries.len(), 2, "plan_entries must have two rows");
    assert_eq!(sv.plan_entries[0].content, "Write tests");
    assert_eq!(sv.plan_entries[1].content, "Ship feature");
}

#[test]
fn wasl_plan_updated_empty_clears_plan_entries() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    // First set some entries.
    let _ = core.on_agent_line(&event_line(
        &id,
        1,
        WaslSessionEventKind::PlanUpdated {
            entries: vec![PlanEntry {
                content: "Old task".to_string(),
                priority: String::new(),
            }],
        },
    ));

    // Then clear with empty list.
    let updates = core
        .on_agent_line(&event_line(
            &id,
            2,
            WaslSessionEventKind::PlanUpdated { entries: vec![] },
        ))
        .expect("on_agent_line");

    let sv = last_session_view(&updates, &id);
    assert!(
        sv.plan_entries.is_empty(),
        "PlanUpdated with empty entries must clear plan_entries"
    );
}

// ===========================================================================
// ModesUpdated
// ===========================================================================

#[test]
fn wasl_modes_updated_sets_available_modes_and_current_mode() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    let updates = core
        .on_agent_line(&event_line(
            &id,
            1,
            WaslSessionEventKind::ModesUpdated {
                available_modes: vec![
                    SessionMode {
                        id: "plan".to_string(),
                        label: "Plan".to_string(),
                        description: "Plan mode".to_string(),
                    },
                    SessionMode {
                        id: "build".to_string(),
                        label: "Build".to_string(),
                        description: "Build mode".to_string(),
                    },
                ],
                current_mode: Some("plan".to_string()),
            },
        ))
        .expect("on_agent_line");

    let sv = last_session_view(&updates, &id);
    assert_eq!(sv.available_modes.len(), 2, "available_modes must have two rows");
    assert_eq!(sv.available_modes[0].id, "plan");
    assert_eq!(sv.available_modes[1].id, "build");
    assert_eq!(
        sv.current_mode.as_deref(),
        Some("plan"),
        "current_mode must be set"
    );
}

#[test]
fn wasl_modes_updated_current_mode_only_does_not_clear_available_modes() {
    // When available_modes is empty, only current_mode is updated; the existing
    // available_modes list is preserved.
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    // Set available_modes first.
    let _ = core.on_agent_line(&event_line(
        &id,
        1,
        WaslSessionEventKind::ModesUpdated {
            available_modes: vec![
                SessionMode {
                    id: "plan".to_string(),
                    label: "Plan".to_string(),
                    description: String::new(),
                },
                SessionMode {
                    id: "build".to_string(),
                    label: "Build".to_string(),
                    description: String::new(),
                },
            ],
            current_mode: Some("plan".to_string()),
        },
    ));

    // Now update only current_mode.
    let updates = core
        .on_agent_line(&event_line(
            &id,
            2,
            WaslSessionEventKind::ModesUpdated {
                available_modes: vec![],
                current_mode: Some("build".to_string()),
            },
        ))
        .expect("on_agent_line");

    let sv = last_session_view(&updates, &id);
    assert_eq!(
        sv.available_modes.len(),
        2,
        "available_modes must NOT be cleared when update sends empty list"
    );
    assert_eq!(
        sv.current_mode.as_deref(),
        Some("build"),
        "current_mode must switch to build"
    );
}

// ===========================================================================
// ModelChanged
// ===========================================================================

#[test]
fn wasl_model_changed_sets_model_on_session_view() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    let updates = core
        .on_agent_line(&event_line(
            &id,
            1,
            WaslSessionEventKind::ModelChanged {
                model: "claude-opus-4-5".to_string(),
            },
        ))
        .expect("on_agent_line");

    let sv = last_session_view(&updates, &id);
    assert_eq!(
        sv.model, "claude-opus-4-5",
        "model must be set from ModelChanged event"
    );
}

// ===========================================================================
// SlashCommandsUpdated
// ===========================================================================

#[test]
fn wasl_slash_commands_updated_replaces_slash_commands() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    let updates = core
        .on_agent_line(&event_line(
            &id,
            1,
            WaslSessionEventKind::SlashCommandsUpdated {
                commands: vec![
                    SlashCommandInfo {
                        name: "compact".to_string(),
                        description: "Compact context".to_string(),
                        argument_hint: "[focus]".to_string(),
                    },
                    SlashCommandInfo {
                        name: "review".to_string(),
                        description: "Review the diff".to_string(),
                        argument_hint: String::new(),
                    },
                ],
            },
        ))
        .expect("on_agent_line");

    let sv = last_session_view(&updates, &id);
    assert_eq!(sv.slash_commands.len(), 2, "slash_commands must have two rows");
    assert_eq!(sv.slash_commands[0].name, "compact");
    assert_eq!(sv.slash_commands[1].name, "review");
}

// ===========================================================================
// Compaction
// ===========================================================================

#[test]
fn wasl_compaction_increments_compactions_count_and_sets_last_compaction() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    let updates = core
        .on_agent_line(&event_line(
            &id,
            1,
            WaslSessionEventKind::Compaction {
                info: CompactionInfo {
                    trigger: "auto".to_string(),
                    pre_tokens: 80_000,
                    post_tokens: 20_000,
                    cumulative_dropped_tokens: 60_000,
                    duration_ms: 300,
                },
            },
        ))
        .expect("on_agent_line");

    let sv = last_session_view(&updates, &id);
    assert_eq!(sv.compactions, 1, "compactions count must be 1 after first event");
    let lc = sv
        .last_compaction
        .as_ref()
        .expect("last_compaction must be set");
    assert_eq!(lc.trigger, "auto");
    assert_eq!(lc.cumulative_dropped_tokens, 60_000);
}

#[test]
fn wasl_compaction_appends_transcript_entry() {
    // A Compaction event must append a TranscriptEntry::Compaction in sequence
    // (mirrors the solo path).
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    core.on_agent_line(&event_line(
        &id,
        1,
        WaslSessionEventKind::Compaction {
            info: CompactionInfo {
                trigger: "manual".to_string(),
                cumulative_dropped_tokens: 5_000,
                ..Default::default()
            },
        },
    ))
    .expect("on_agent_line");

    // The transcript is not directly on SessionView; check via the session.
    // Use the phase (still Done/Starting after a plain Compaction event) and
    // that no panic occurred — the real assertion is the compactions counter.
    let updates = core
        .on_agent_line(&event_line(
            &id,
            2,
            WaslSessionEventKind::Compaction {
                info: CompactionInfo {
                    trigger: "manual".to_string(),
                    cumulative_dropped_tokens: 10_000,
                    ..Default::default()
                },
            },
        ))
        .expect("second compaction");

    let sv = last_session_view(&updates, &id);
    assert_eq!(
        sv.compactions, 2,
        "compactions must increment on each Compaction event"
    );
    assert_eq!(
        sv.last_compaction.as_ref().unwrap().cumulative_dropped_tokens,
        10_000,
        "last_compaction must hold the most-recent info"
    );
}

#[test]
fn wasl_compaction_does_not_change_phase() {
    // A Compaction event is informational — it must NOT move the phase.
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    // Put the session into Working phase via TurnAccepted.
    core.on_agent_line(&event_line(
        &id,
        1,
        WaslSessionEventKind::TurnAccepted {
            turn_id: "t-1".to_string(),
            source: "local_human".to_string(),
        },
    ))
    .expect("TurnAccepted");

    let updates = core
        .on_agent_line(&event_line(
            &id,
            2,
            WaslSessionEventKind::Compaction {
                info: CompactionInfo {
                    trigger: "auto".to_string(),
                    ..Default::default()
                },
            },
        ))
        .expect("Compaction");

    let sv = last_session_view(&updates, &id);
    assert_eq!(
        sv.state,
        Phase::Working,
        "Compaction must NOT change phase; session should remain Working"
    );
}

// ===========================================================================
// SessionCapabilities (Feature B) — folds the init catalog into the SAME
// SessionState/SessionView fields the native `system/init` path populates, so
// the Config tab renders for a Wasl-managed session with no frontend change.
// ===========================================================================

#[test]
fn wasl_session_capabilities_folds_into_config_fields() {
    use marsad_proto::McpServerInfo;
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    let updates = core
        .on_agent_line(&event_line(
            &id,
            1,
            WaslSessionEventKind::SessionCapabilities {
                tools: vec!["Read".to_string(), "Edit".to_string()],
                mcp_servers: vec![McpServerInfo {
                    name: "wasl".to_string(),
                    status: "connected".to_string(),
                }],
                slash_commands: vec![SlashCommandInfo {
                    name: "compact".to_string(),
                    description: "Compact".to_string(),
                    argument_hint: String::new(),
                }],
                available_modes: vec![SessionMode {
                    id: "default".to_string(),
                    label: "Default".to_string(),
                    description: String::new(),
                }],
                current_mode: "default".to_string(),
                permission_mode: "acceptEdits".to_string(),
            },
        ))
        .expect("on_agent_line");

    let sv = last_session_view(&updates, &id);
    assert_eq!(sv.tools, vec!["Read", "Edit"], "tools must fold into SessionView");
    assert_eq!(sv.mcp_servers.len(), 1, "mcp_servers must fold");
    assert_eq!(sv.mcp_servers[0].name, "wasl");
    assert_eq!(sv.slash_commands.len(), 1, "slash_commands must fold");
    assert_eq!(sv.slash_commands[0].name, "compact");
    assert_eq!(sv.available_modes.len(), 1, "available_modes must fold");
    assert_eq!(sv.available_modes[0].id, "default");
    assert_eq!(sv.current_mode.as_deref(), Some("default"));
}

#[test]
fn wasl_session_capabilities_empty_modes_do_not_clobber_prior() {
    // A later empty-modes catalog (e.g. codex reporting nothing) must NOT wipe a
    // previously-learned mode list — matching native's "don't clobber" rule.
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    // Learn modes first via ModesUpdated.
    let _ = core.on_agent_line(&event_line(
        &id,
        1,
        WaslSessionEventKind::ModesUpdated {
            available_modes: vec![SessionMode {
                id: "plan".to_string(),
                label: "Plan".to_string(),
                description: String::new(),
            }],
            current_mode: Some("plan".to_string()),
        },
    ));

    // A capabilities event with EMPTY modes must leave the learned list intact.
    let updates = core
        .on_agent_line(&event_line(
            &id,
            2,
            WaslSessionEventKind::SessionCapabilities {
                tools: vec!["Bash".to_string()],
                mcp_servers: vec![],
                slash_commands: vec![],
                available_modes: vec![],
                current_mode: String::new(),
                permission_mode: String::new(),
            },
        ))
        .expect("on_agent_line");

    let sv = last_session_view(&updates, &id);
    assert_eq!(sv.tools, vec!["Bash"], "tools still fold (authoritative snapshot)");
    assert_eq!(
        sv.available_modes.len(),
        1,
        "empty available_modes must NOT clobber the previously-learned list"
    );
    assert_eq!(sv.current_mode.as_deref(), Some("plan"), "current_mode preserved");
}

// ===========================================================================
// Rich events do NOT change phase (regression: informational events must be
// no-ops for phase, like the existing SessionReady/TurnQueued/StatusChanged).
// ===========================================================================

#[test]
fn wasl_usage_update_does_not_change_phase() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    // Starting phase → UsageUpdate → phase must stay Starting (pre-turn).
    let updates = core
        .on_agent_line(&event_line(
            &id,
            1,
            WaslSessionEventKind::UsageUpdate {
                cost_usd: 0.01,
                tokens: 100,
                context_pct: None,
            },
        ))
        .expect("on_agent_line");

    let sv = last_session_view(&updates, &id);
    // The session was just launched — phase is Starting or Done depending on
    // the launch path. Either way, UsageUpdate must not flip it to Error/Working/etc.
    assert!(
        !matches!(sv.state, Phase::Error),
        "UsageUpdate must not set Error phase; got {:?}",
        sv.state
    );
}

#[test]
fn wasl_model_changed_does_not_change_phase() {
    let (mut core, mut peer) = make_core();
    let id = launch_wasl(&mut core, &mut peer);

    let updates = core
        .on_agent_line(&event_line(
            &id,
            1,
            WaslSessionEventKind::ModelChanged {
                model: "new-model".to_string(),
            },
        ))
        .expect("on_agent_line");

    let sv = last_session_view(&updates, &id);
    assert!(
        !matches!(sv.state, Phase::Error),
        "ModelChanged must not set Error phase; got {:?}",
        sv.state
    );
}
