//! D01 — Tauri command bridge over marsad-core.
//!
//! Each public function here is registered as a Tauri command in lib.rs.
//! The live Core is stored as Tauri managed state behind a Mutex.
//! In MARSAD_MOCK=1 mode the same state holds a MockState that serves
//! a seeded AppSnapshot (see mock.rs).

use std::sync::Mutex;
use tauri::State;

use marsad_core::{Core, CoreError, GitProbe, LaunchMode, LaunchSpec, PersistedSettings, PersistedTemplate, Posture, VmUpdate};
use marsad_gates::{Decision, DecisionId, GateDecision};
use marsad_viewmodel::{AppSnapshot, CapabilitiesView};

// ---------------------------------------------------------------------------
// AppState — unified managed state for live + mock modes
// ---------------------------------------------------------------------------

/// `Live(Core)` is much larger than `Mock` (Core owns the session map, host
/// registry, store, etc.). There is exactly one `AppState` for the whole app,
/// held in a `Mutex` — boxing the variant would buy nothing, so accept the size.
#[allow(clippy::large_enum_variant)]
pub enum AppState {
    Live(Core),
    Mock(crate::mock::MockState),
}

// Safety: Core is not Send because its inner transport is `Box<dyn Transport>`
// which is not Send by default. We wrap the whole thing in a Mutex so only one
// thread accesses it at a time; the Tauri command dispatcher calls commands on
// the same thread, so we assert Send here. The transport inside is either a
// LoopbackFake (test/startup) or a real OS transport — both are single-owner.
unsafe impl Send for AppState {}
unsafe impl Sync for AppState {}

impl AppState {
    pub fn snapshot(&self) -> AppSnapshot {
        match self {
            AppState::Live(core) => core.snapshot(),
            AppState::Mock(mock) => mock.snapshot(),
        }
    }
}

pub type ManagedState = Mutex<AppState>;

// ---------------------------------------------------------------------------
// Helper: stringify CoreError
// ---------------------------------------------------------------------------

fn map_err(e: CoreError) -> String {
    e.to_string()
}

/// Map an approval-modal outcome string to a gate [`Decision`]. Only "approved"
/// approves; every other outcome ("denied"/"dismissed"/…) rejects, carrying the
/// human's reason or — when none was given — the outcome word itself, so a
/// rejection is never a blank string.
fn wasl_outcome_to_decision(outcome: &str, reason: Option<String>) -> Decision {
    if outcome == "approved" {
        Decision::Approve
    } else {
        Decision::Reject { reason: reason.unwrap_or_else(|| outcome.to_string()) }
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command(rename_all = "snake_case")]
pub fn snapshot(state: State<'_, ManagedState>) -> Result<AppSnapshot, String> {
    let guard = state.lock().map_err(|e| e.to_string())?;
    Ok(guard.snapshot())
}

// Flat parameters (one invoke-payload key each) — matches every other command
// here and the JS `invoke("launch_session", { project, host_id, … })` call.
// A Tauri command legitimately needs all of these, hence the clippy allow.
#[tauri::command(rename_all = "snake_case")]
#[allow(clippy::too_many_arguments)]
pub fn launch_session(
    state: State<'_, ManagedState>,
    project: String,
    host_id: String,
    model: String,
    posture: String,
    first_prompt: Option<String>,
    resume_session_id: Option<String>,
    continue_recent: Option<bool>,
    name: Option<String>,
    // X4 launch-time Wasl attach: when true the agent injects --mcp-config and
    // adopt-or-spawns the host wasl-mcpd node. Optional on the wire (absent ⇒
    // false) so a launch form that omits it stays a plain, Wasl-free session.
    wasl: Option<bool>,
    // #3 — agent driver override. Absent ⇒ "claude".
    driver: Option<String>,
) -> Result<String, String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    match &mut *guard {
        AppState::Live(core) => {
            let posture_val = match posture.as_str() {
                "auto" => Posture::AutoApproveSafe,
                "full" => Posture::FullAutonomy,
                _ => Posture::Ask,
            };
            // Non-empty resume id → resume that session; else continue-recent →
            // --continue; else a fresh session.
            let mode = if let Some(id) = resume_session_id.filter(|s| !s.is_empty()) {
                LaunchMode::Resume(id)
            } else if continue_recent == Some(true) {
                LaunchMode::Continue
            } else {
                LaunchMode::Fresh
            };
            let spec = LaunchSpec {
                project,
                host_id,
                // Core resolves the remote target from host_id via its registry.
                host_target: None,
                // Core resolves claude_bin/setup/env from the host config too.
                claude_bin: None,
                setup: None,
                env: Vec::new(),
                model,
                posture: posture_val,
                first_prompt,
                mode,
                name,
                disallowed_tools: Vec::new(),
                wasl: wasl.unwrap_or(false),
                driver: driver.filter(|d| !d.is_empty()).unwrap_or_else(|| "claude".to_string()),
            };
            core.launch_session(spec).map_err(map_err)
        }
        AppState::Mock(_) => Ok("mock-session-001".to_string()),
    }
}

/// X6 (Wasl Backend) — launch a Wasl-managed session on `host_id`. The session's
/// underlying process is owned end-to-end by the Wasl runtime; this command tells
/// the agent to adopt-or-spawn the runtime node and get-or-start a
/// `WaslSessionClient` for `wasl_target_key`. Mirrors `launch_session`'s
/// lock/error-mapping shape exactly.
#[tauri::command(rename_all = "snake_case")]
pub fn launch_wasl_session(
    state: State<'_, ManagedState>,
    host_id: String,
    wasl_target_key: serde_json::Value,
    name: Option<String>,
) -> Result<String, String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    match &mut *guard {
        AppState::Live(core) => core
            .launch_wasl_session(&host_id, wasl_target_key, name)
            .map_err(map_err),
        AppState::Mock(_) => Ok("mock-wasl-session-001".to_string()),
    }
}

/// U1/U2 (docs/specs/dynamic-capabilities.md §3.6) — initial snapshot of every
/// (driver, host) capabilities entry for the `useVm` `capabilities` slice.
/// Same lock-clone-return pattern as `board()`; the bridge holds no on-disk
/// client knowledge itself (that lives agent-side, §3.1) — kill-list
/// replacement for `list_sessions`/`parse_remote_session_list`.
#[tauri::command(rename_all = "snake_case")]
pub fn capabilities(state: State<'_, ManagedState>) -> Result<Vec<CapabilitiesView>, String> {
    let guard = state.lock().map_err(|e| e.to_string())?;
    match &*guard {
        AppState::Live(core) => Ok(core.capabilities()),
        AppState::Mock(_) => Ok(Vec::new()),
    }
}

/// U1/U2 — ask the agent to (re)probe capabilities. Fire-and-forget: forwards
/// to `Core::refresh_capabilities` and returns immediately (never waits for
/// the resulting `CapabilitiesUpdate`, which arrives later as a vm-update
/// event) — the U5 responsiveness rule (§4): no probe IO on this path.
#[tauri::command(rename_all = "snake_case")]
pub fn refresh_capabilities(
    state: State<'_, ManagedState>,
    host_id: Option<String>,
    driver: Option<String>,
    project: Option<String>,
) -> Result<(), String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    match &mut *guard {
        AppState::Live(core) => {
            core.refresh_capabilities(host_id, driver, project);
            Ok(())
        }
        AppState::Mock(_) => Ok(()),
    }
}

#[tauri::command(rename_all = "snake_case")]
pub fn resume_session(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    id: String,
) -> Result<(), String> {
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core.resume_session(&id).map_err(map_err)?,
            AppState::Mock(_) => vec![],
        }
    };
    crate::events::emit_updates(&app, updates);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub fn attach_session(state: State<'_, ManagedState>, id: String) -> Result<(), String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    match &mut *guard {
        AppState::Live(core) => core.attach_session(&id).map_err(map_err),
        AppState::Mock(_) => Ok(()),
    }
}

#[tauri::command(rename_all = "snake_case")]
pub fn stop_session(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    id: String,
) -> Result<(), String> {
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core.stop_session(&id).map_err(map_err)?,
            AppState::Mock(_) => vec![],
        }
    };
    drop_exec_shell(&id); // kill the session's persistent `!` shell
    crate::events::emit_updates(&app, updates);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub fn remove_session(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    id: String,
) -> Result<(), String> {
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => {
                let mut u = core.remove_session(&id).map_err(map_err)?;
                // Append Workstreams + Board so the board drops the ghost card.
                u.push(VmUpdate::Workstreams(core.workstreams().to_vec()));
                u.push(VmUpdate::Board(core.board()));
                u
            }
            AppState::Mock(_) => vec![],
        }
    };
    drop_exec_shell(&id); // kill the session's persistent `!` shell
    crate::events::emit_updates(&app, updates);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub fn send_prompt(
    state: State<'_, ManagedState>,
    id: String,
    text: String,
) -> Result<(), String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    match &mut *guard {
        AppState::Live(core) => core.send_prompt(&id, &text).map_err(map_err),
        AppState::Mock(_) => Ok(()),
    }
}

#[tauri::command(rename_all = "snake_case")]
pub fn set_session_config_option(
    state: State<'_, ManagedState>,
    id: String,
    config_id: String,
    value: String,
) -> Result<(), String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    match &mut *guard {
        AppState::Live(core) => core
            .set_session_config_option(&id, &config_id, &value)
            .map_err(map_err),
        AppState::Mock(_) => Ok(()),
    }
}

#[tauri::command(rename_all = "snake_case")]
pub fn interrupt(state: State<'_, ManagedState>, id: String) -> Result<(), String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    match &mut *guard {
        AppState::Live(core) => core.interrupt(&id).map_err(map_err),
        AppState::Mock(_) => Ok(()),
    }
}

#[tauri::command(rename_all = "snake_case")]
pub fn answer_question(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    session_id: String,
    request_id: String,
    answers: serde_json::Value,
) -> Result<(), String> {
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core
                .answer_question(&session_id, &request_id, answers)
                .map_err(map_err)?,
            AppState::Mock(_) => vec![],
        }
    };
    crate::events::emit_updates(&app, updates);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub fn answer_gate(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    session_id: String,
    decision_id: String,
    allow: bool,
    deny_reason: Option<String>,
    elicitation_content: Option<serde_json::Value>,
    permission_updates: Option<serde_json::Value>,
) -> Result<(), String> {
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => {
                let decision = if allow {
                    Decision::Approve
                } else {
                    Decision::Reject { reason: deny_reason.unwrap_or_default() }
                };
                let gate_decision = GateDecision {
                    id: DecisionId(decision_id),
                    session_id,
                    decision,
                    // Only meaningful for an accepted scope-elicitation.
                    elicitation_content: if allow { elicitation_content } else { None },
                    // The chosen "always allow …" suggestion, verbatim (allow only).
                    permission_updates: if allow { permission_updates } else { None },
                };
                // Emits Session+Overview updates that clear the answered gate so
                // the card disappears immediately (see Core::answer_gate).
                core.answer_gate(gate_decision).map_err(map_err)?
            }
            AppState::Mock(_) => vec![],
        }
    };
    crate::events::emit_updates(&app, updates);
    Ok(())
}

/// X5 — relay ONE explicit human decision on a pending Wasl handoff gate.
/// `outcome` is "approved" | "denied" | "dismissed" (from the approval modal).
/// Only "approved" maps to `Approve`; everything else is a `Reject` carrying the
/// human's reason (falling back to the outcome word so denies/dismissals are
/// never a blank rejection). Wasl gates are host-scoped, not session-scoped.
#[tauri::command(rename_all = "snake_case")]
pub fn answer_wasl_gate(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    host_id: String,
    gate_id: String,
    outcome: String,
    reason: Option<String>,
) -> Result<(), String> {
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => {
                let decision = wasl_outcome_to_decision(&outcome, reason);
                core.answer_wasl_gate(&host_id, &gate_id, decision).map_err(map_err)?
            }
            AppState::Mock(_) => vec![],
        }
    };
    crate::events::emit_updates(&app, updates);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub fn run_slash(
    state: State<'_, ManagedState>,
    id: String,
    cmd: String,
) -> Result<(), String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    match &mut *guard {
        AppState::Live(core) => {
            let slash_cmd = marsad_viewmodel::parse_slash(&cmd);
            core.run_slash(&id, slash_cmd).map_err(map_err)
        }
        AppState::Mock(_) => Ok(()),
    }
}

#[tauri::command(rename_all = "snake_case")]
pub fn set_workspace(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    id: String,
) -> Result<(), String> {
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core.set_workspace(&id).map_err(map_err)?,
            AppState::Mock(mock) => {
                mock.set_active_ws(id);
                vec![]
            }
        }
    };
    crate::events::emit_updates(&app, updates);
    Ok(())
}

// ---------------------------------------------------------------------------
// Workspace commands
// ---------------------------------------------------------------------------

/// Save the current session list as a named workspace, persist, and push the
/// updated workspace list to the webview.
#[tauri::command(rename_all = "snake_case")]
pub fn save_workspace(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    name: String,
    session_ids: Vec<String>,
) -> Result<(), String> {
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core.save_workspace(&name, session_ids).map_err(map_err)?,
            AppState::Mock(_) => vec![],
        }
    };
    crate::events::emit_updates(&app, updates);
    Ok(())
}

/// Duplicate a workspace by id, persist the copy, and push the updated list.
#[tauri::command(rename_all = "snake_case")]
pub fn duplicate_workspace(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    id: String,
) -> Result<(), String> {
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core.duplicate_workspace(&id).map_err(map_err)?,
            AppState::Mock(_) => vec![],
        }
    };
    crate::events::emit_updates(&app, updates);
    Ok(())
}

/// Delete a workspace by id, remove from persistence, and push the updated list.
#[tauri::command(rename_all = "snake_case")]
pub fn delete_workspace(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    id: String,
) -> Result<(), String> {
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core.delete_workspace(&id).map_err(map_err)?,
            AppState::Mock(_) => vec![],
        }
    };
    crate::events::emit_updates(&app, updates);
    Ok(())
}

/// Export a workspace as a `.marsad.json` string. Returns the JSON directly
/// (no vm-update; the frontend triggers the file download).
#[tauri::command(rename_all = "snake_case")]
pub fn export_workspace(
    state: State<'_, ManagedState>,
    id: String,
) -> Result<String, String> {
    let guard = state.lock().map_err(|e| e.to_string())?;
    match &*guard {
        AppState::Live(core) => core.export_workspace(&id).map_err(map_err),
        AppState::Mock(_) => Ok("{}".to_string()),
    }
}

/// Import a workspace from a `.marsad.json` string, persist it, and push the
/// updated workspace list.
#[tauri::command(rename_all = "snake_case")]
pub fn import_workspace(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    json: String,
) -> Result<(), String> {
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core.import_workspace(&json).map_err(map_err)?,
            AppState::Mock(_) => vec![],
        }
    };
    crate::events::emit_updates(&app, updates);
    Ok(())
}

// ---------------------------------------------------------------------------
// Settings helpers
// ---------------------------------------------------------------------------

/// Build the sensible default `PersistedSettings` used by mock mode and when
/// no settings have been persisted yet. Mirrors `Core::default_settings()` so
/// mock and live modes return identical shapes.
fn default_settings_for_mock() -> PersistedSettings {
    use marsad_core::NotifPref;
    use std::collections::HashMap;

    let mut notif_prefs: HashMap<String, NotifPref> = HashMap::new();
    for (ev, enabled, priority, sound, group, dnd) in [
        ("gate_needs_you", true, "urgent", true, false, false),
        ("gate_error", true, "urgent", true, false, false),
        ("session_done", true, "normal", false, true, true),
        ("session_started", false, "low", false, true, true),
        ("host_unreachable", true, "urgent", true, false, false),
    ] {
        notif_prefs.insert(
            ev.to_string(),
            NotifPref {
                enabled,
                priority: priority.to_string(),
                sound,
                group,
                dnd,
            },
        );
    }
    PersistedSettings {
        notif_prefs,
        play_sound: true,
        group_alerts: true,
        do_not_disturb: false,
        background: "Gradient".to_string(),
        restore_on_launch: true,
        confirm_before_stop: true,
        default_model: "claude-sonnet-latest".to_string(),
        default_posture: "ask".to_string(),
        default_driver: "claude".to_string(),
        default_models: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Settings commands
// ---------------------------------------------------------------------------

/// Return the current persisted app settings (or sensible defaults if none saved yet).
#[tauri::command(rename_all = "snake_case")]
pub fn get_settings(state: State<'_, ManagedState>) -> Result<PersistedSettings, String> {
    let guard = state.lock().map_err(|e| e.to_string())?;
    match &*guard {
        AppState::Live(core) => Ok(core.get_settings()),
        AppState::Mock(_) => Ok(default_settings_for_mock()),
    }
}

/// Persist the app settings blob.
#[tauri::command(rename_all = "snake_case")]
pub fn set_settings(
    state: State<'_, ManagedState>,
    settings: PersistedSettings,
) -> Result<(), String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    match &mut *guard {
        AppState::Live(core) => core.set_settings(&settings).map_err(map_err),
        AppState::Mock(_) => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Autostart commands
// ---------------------------------------------------------------------------

/// Return whether Marsad is configured to launch at login.
#[tauri::command(rename_all = "snake_case")]
pub fn get_autostart(app: tauri::AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

/// Enable or disable launch-at-login for Marsad.
#[tauri::command(rename_all = "snake_case")]
pub fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    if enabled {
        app.autolaunch().enable().map_err(|e| e.to_string())
    } else {
        app.autolaunch().disable().map_err(|e| e.to_string())
    }
}

// ---------------------------------------------------------------------------
// Host commands
// ---------------------------------------------------------------------------

/// Add a remote host (`label`, `user@host`), persist it, and push the updated
/// host list to the webview. A background worker health-checks it shortly after.
#[tauri::command(rename_all = "snake_case")]
#[allow(clippy::too_many_arguments)]
pub fn add_host(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    label: String,
    target: String,
    claude_bin: Option<String>,
    setup: Option<String>,
    env: Option<Vec<(String, String)>>,
    // #1 per-driver command overrides: optional list of (driver, command) pairs.
    agent_commands: Option<Vec<(String, String)>>,
) -> Result<(), String> {
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core
                .add_host(
                    &label,
                    &target,
                    claude_bin,
                    setup,
                    env.unwrap_or_default(),
                    agent_commands.unwrap_or_default(),
                )
                .map_err(map_err)?,
            AppState::Mock(_) => vec![],
        }
    };
    crate::events::emit_updates(&app, updates);
    Ok(())
}

/// Update an existing host's launch configuration (`claude_bin` / `setup` /
/// `env` / `agent_commands`), and optionally its `target`. Works for any host id
/// INCLUDING the localhost (whose target is left unchanged when `target` is
/// `None`). Persists the change and pushes the updated host list to the webview.
#[tauri::command(rename_all = "snake_case")]
#[allow(clippy::too_many_arguments)]
pub fn update_host(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    id: String,
    claude_bin: Option<String>,
    setup: Option<String>,
    env: Option<Vec<(String, String)>>,
    target: Option<String>,
    // #1 per-driver command overrides: optional list of (driver, command) pairs.
    agent_commands: Option<Vec<(String, String)>>,
) -> Result<(), String> {
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core
                .update_host(
                    &id,
                    claude_bin,
                    setup,
                    env.unwrap_or_default(),
                    target,
                    agent_commands.unwrap_or_default(),
                )
                .map_err(map_err)?,
            AppState::Mock(_) => vec![],
        }
    };
    crate::events::emit_updates(&app, updates);
    Ok(())
}

/// Remove a host by id, delete it from persistence, and push the updated list.
#[tauri::command(rename_all = "snake_case")]
pub fn remove_host(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    id: String,
) -> Result<(), String> {
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core.remove_host(&id).map_err(map_err)?,
            AppState::Mock(_) => vec![],
        }
    };
    crate::events::emit_updates(&app, updates);
    Ok(())
}

/// List installed WSL distros as ready-to-add `wsl:<distro>` host targets.
/// Empty on non-Windows (and if WSL isn't installed). Lets the Hosts UI offer
/// one-click WSL hosts instead of the user having to know the `wsl:<distro>`
/// convention.
#[tauri::command]
pub fn list_wsl_distros() -> Result<Vec<String>, String> {
    marsad_hosts::list_wsl_distros(&marsad_hosts::RealWslRunner).map_err(|e| e.to_string())
}

/// Open a real OS terminal window running an INTERACTIVE login for `target` so
/// the user completes MFA / keyboard-interactive auth once, after which automated
/// connections reuse it (see docs/remote-hosts.md).
///
/// Two transports, selected by the target string:
/// - `ssh` (default): completes the login with NO `BatchMode`, the same
///   `ControlPath` master socket as the automated connections, which persists
///   (`ControlPersist=600`) and every later automated `ssh` rides it.
/// - `plink:<inner>` (PuTTY): runs `plink -share <inner>` (NO `-batch`). That
///   first connection becomes the connection-sharing **upstream**; later
///   automated `plink -share -batch <inner>` connections share it. This is the
///   Windows path that works without WSL, since `ssh.exe` lacks ControlMaster.
#[tauri::command(rename_all = "snake_case")]
pub fn connect_host_interactive(
    state: State<'_, ManagedState>,
    target: String,
) -> Result<(), String> {
    launch_interactive_ssh(&target)?;
    // Connect-time discovery (deliverable #4b): once the interactive login is
    // launched for a manual/MFA host, kick off a capabilities refresh for the
    // host that owns this target. It's fire-and-forget: the ControlMaster/
    // plink-share upstream is established when the user finishes auth, and the
    // agent's bounded probe rides it (or honestly reports unreachable if the
    // user hasn't finished yet — a later frontend refresh re-runs it). Never
    // blocks: a single IPC send under the lock we already hold, no probe IO.
    if let Ok(mut guard) = state.lock() {
        if let AppState::Live(core) = &mut *guard {
            let host_id = core
                .hosts_snapshot()
                .into_iter()
                .find(|h| h.target == target)
                .map(|h| h.id);
            if let Some(host_id) = host_id {
                core.refresh_capabilities(Some(host_id), None, None);
            }
        }
    }
    Ok(())
}

/// Build the interactive login argv used by `connect_host_interactive`.
///
/// `args[0]` is the binary (`ssh` or `plink`); callers pass the rest as its args.
///
/// - `plink:<inner>` → `["plink", "-share", "<inner>"]`. Deliberately NO `-batch`
///   (this is the interactive Duo login that establishes the shared upstream);
///   `-share` enables connection sharing so automated downstreams can reuse it.
/// - otherwise → an `ssh` login. Deliberately NO `BatchMode` (interactive); the
///   `ControlPath` MUST match docs/remote-hosts.md exactly so automated
///   connections reuse the master socket.
/// The one-shot remote command appended to an interactive `ssh` login so it runs,
/// confirms auth + leaves the persistent ControlMaster, then EXITS — instead of
/// dropping into an interactive remote shell that "sits at the prompt forever" and
/// never returns to Marsad. `ssh` joins these trailing tokens into the remote
/// command. Shared by the wslssh (MFA/Duo) and plain-ssh branches so both behave
/// identically (one source of truth for the confirmation message).
fn one_shot_confirm_cmd() -> [&'static str; 8] {
    ["echo", "Marsad:", "connected.", "You", "can", "close", "this", "window."]
}

fn interactive_ssh_args(target: &str) -> Vec<String> {
    if let Some(inner) = target.strip_prefix("plink:") {
        return vec![
            "plink".to_string(),
            "-share".to_string(),
            inner.to_string(),
        ];
    }
    // WSL-routed SSH: authenticate INSIDE the distro (`wsl ssh …`) with
    // ControlMaster so completing the Duo/MFA prompt here establishes the master
    // socket that the agent's automated `wsl ssh` connections then reuse.
    //
    // Crucially we DON'T open an interactive shell — `ssh <host>` with no command
    // sits at a remote prompt forever ("doesn't go back to Marsad, stays in the
    // terminal"). Instead we run a ONE-SHOT command: Duo prompts during auth, the
    // master is established, the command prints a "done" message, and ssh EXITS.
    // The master persists via ControlPersist=600, so later connections reuse it
    // with no re-auth. The cmd /K window stays open showing the message until the
    // user closes it. No -T/BatchMode: keyboard-interactive (Duo) needs prompting.
    if let Some(rest) = target.strip_prefix("wslssh:") {
        let mut parts = rest.splitn(2, ':');
        let distro = parts.next().unwrap_or("");
        let inner = parts.next().unwrap_or("");
        if !distro.is_empty() && !inner.is_empty() {
            let mut args = vec![
                "wsl.exe".to_string(),
                "-d".to_string(),
                distro.to_string(),
                "ssh".to_string(),
                "-o".to_string(),
                "ControlMaster=auto".to_string(),
                "-o".to_string(),
                "ControlPath=~/.ssh/marsad-cm-%r@%h:%p".to_string(),
                "-o".to_string(),
                "ControlPersist=600".to_string(),
                "-o".to_string(),
                "ServerAliveInterval=30".to_string(),
                inner.to_string(),
            ];
            args.extend(one_shot_confirm_cmd().iter().map(|s| s.to_string()));
            return args;
        }
    }
    let mut args = vec!["ssh".to_string()];
    // SSH connection multiplexing (ControlMaster/ControlPath/ControlPersist) lets an
    // interactive login's master socket be reused by later headless connections — but
    // it relies on Unix-domain sockets that **Windows OpenSSH does not support**, where
    // it errors `getsockname failed: Not a socket`. So enable it only on Unix; on
    // Windows the interactive login is a plain `ssh <target>`.
    #[cfg(unix)]
    {
        args.extend([
            "-o".to_string(),
            "ControlMaster=auto".to_string(),
            "-o".to_string(),
            "ControlPath=~/.ssh/marsad-cm-%r@%h:%p".to_string(),
            "-o".to_string(),
            "ControlPersist=600".to_string(),
        ]);
    }
    args.extend([
        "-o".to_string(),
        "ServerAliveInterval=30".to_string(),
        target.to_string(),
    ]);
    // One-shot remote command — IDENTICAL rationale to the wslssh path above: a
    // bare `ssh <target>` opens an interactive shell that sits at the remote prompt
    // forever ("connects but never goes back to Marsad"). Appending a command makes
    // ssh authenticate (prompting for a password/MFA if the host needs it),
    // establish the persistent ControlMaster (on Unix), print a confirmation, then
    // EXIT. Key-based hosts (e.g. ETX) just skip the prompt.
    args.extend(one_shot_confirm_cmd().iter().map(|s| s.to_string()));
    args
}

/// Resolve `plink.exe` to a concrete path: `PATH` first, then the standard PuTTY
/// install dirs (modern PuTTY preferred over an Xming-bundled fallback). Falls
/// back to the bare `"plink"` sentinel if none is found. Mirrors the agent's
/// `resolve_remote_bin` so the interactive upstream and automated downstream use
/// the same binary.
#[cfg(target_os = "windows")]
fn resolve_plink_bin() -> String {
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(';').filter(|d| !d.is_empty()) {
            let cand = std::path::Path::new(dir).join("plink.exe");
            if cand.exists() {
                return cand.to_string_lossy().into_owned();
            }
        }
    }
    for cand in [
        r"C:\Program Files\PuTTY\plink.exe",
        r"C:\Program Files (x86)\PuTTY\plink.exe",
        r"C:\Program Files (x86)\Xming\plink.exe",
    ] {
        if std::path::Path::new(cand).exists() {
            return cand.to_string();
        }
    }
    "plink".to_string()
}

/// Spawn a visible OS terminal emulator running the interactive ssh login.
/// Factored per-OS so each branch only compiles on its target.
#[cfg(target_os = "windows")]
fn launch_interactive_ssh(target: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    // Open a NEW visible console running `cmd /K ssh <args…>`:
    // - CREATE_NEW_CONSOLE (0x10) guarantees a fresh, visible console window from a
    //   GUI (windowless) process — more reliable than `cmd /C start`, which depended
    //   on a parent console and was the "no terminal appears" bug.
    // - `cmd /K` keeps the window open after ssh exits so a failed/re-prompting Duo
    //   login stays visible, and gives ssh a real console (PTY) for the prompt.
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    // args[0] is the binary (`ssh` or `plink`); the rest are its arguments.
    // Resolve `plink` to a concrete path (PATH / PuTTY install dirs) so a PuTTY
    // install that isn't on PATH still opens the interactive login window. This
    // MUST match the agent's resolution so the interactive upstream and the
    // automated downstream use the same plink.
    let mut args = interactive_ssh_args(target);
    if args.first().map(String::as_str) == Some("plink") {
        args[0] = resolve_plink_bin();
    }
    let mut cmd = Command::new("cmd.exe");
    if target.starts_with("wslssh:") {
        // wslssh authenticates with a one-shot command and EXITS (the ControlMaster
        // persists), so the window has nothing to hold open. `cmd /C <login> & timeout`
        // runs the login, shows the Duo prompt + result for a few seconds, then the
        // window AUTO-CLOSES so the user lands back in Marsad instead of a leftover
        // prompt. (/K would keep the dead window open — the reported issue.)
        cmd.arg("/C");
        for a in &args {
            cmd.arg(a);
        }
        cmd.arg("&").arg("timeout").arg("/t").arg("6");
    } else {
        // Native ssh opens a real interactive shell; plink's window IS the persistent
        // sharing upstream (closing it drops the connection). Keep it open with /K.
        cmd.arg("/K");
        for a in &args {
            cmd.arg(a);
        }
    }
    cmd.creation_flags(CREATE_NEW_CONSOLE);
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to open terminal: {e}"))
}

/// macOS: drive Terminal.app via `osascript` so the ssh command runs in a real
/// terminal window the user can type into.
#[cfg(target_os = "macos")]
fn launch_interactive_ssh(target: &str) -> Result<(), String> {
    use std::process::Command;
    // Quote each arg for the AppleScript `do script` shell string.
    let cmdline = interactive_ssh_args(target)
        .iter()
        .map(|a| format!("'{}'", a.replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join(" ");
    let script = format!(
        "tell application \"Terminal\"\nactivate\ndo script \"{}\"\nend tell",
        cmdline.replace('\\', "\\\\").replace('"', "\\\"")
    );
    Command::new("osascript")
        .arg("-e")
        .arg(script)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to open Terminal: {e}"))
}

/// Linux: try common terminal emulators in order, using the first that exists.
#[cfg(target_os = "linux")]
fn launch_interactive_ssh(target: &str) -> Result<(), String> {
    use std::process::Command;
    let ssh = interactive_ssh_args(target);
    // (binary, flag-that-precedes-the-command-argv)
    let candidates: [(&str, &str); 4] = [
        ("x-terminal-emulator", "-e"),
        ("gnome-terminal", "--"),
        ("konsole", "-e"),
        ("xterm", "-e"),
    ];
    for (bin, flag) in candidates {
        // Probe existence via PATH lookup.
        let found = Command::new("sh")
            .arg("-c")
            .arg(format!("command -v {bin} >/dev/null 2>&1"))
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !found {
            continue;
        }
        let mut cmd = Command::new(bin);
        cmd.arg(flag);
        for a in &ssh {
            cmd.arg(a);
        }
        return cmd
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("failed to launch {bin}: {e}"));
    }
    Err(
        "no terminal emulator found (tried x-terminal-emulator, gnome-terminal, konsole, xterm)"
            .to_string(),
    )
}

// ---------------------------------------------------------------------------
// Commit command
// ---------------------------------------------------------------------------

/// Stage `files` and create a git commit in the session's project directory.
///
/// The git subprocess runs WITHOUT holding the Core mutex — only the brief
/// project-dir lookup and the post-commit changes update lock it.
#[tauri::command(rename_all = "snake_case")]
pub fn commit(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    session_id: String,
    message: String,
    files: Vec<String>,
) -> Result<(), String> {
    // Mock mode: no filesystem to touch.
    let is_mock = {
        let guard = state.lock().map_err(|e| e.to_string())?;
        matches!(&*guard, AppState::Mock(_))
    };
    if is_mock {
        return Ok(());
    }

    // 1. Briefly lock to resolve the project directory from the snapshot.
    let project = {
        let guard = state.lock().map_err(|e| e.to_string())?;
        match &*guard {
            AppState::Live(core) => {
                let snap = core.snapshot();
                snap.sessions
                    .iter()
                    .find(|s| s.id == session_id)
                    .map(|s| s.project.clone())
                    .ok_or_else(|| format!("unknown session: {session_id}"))?
            }
            AppState::Mock(_) => unreachable!("checked above"),
        }
    }; // lock released

    // 2. Run git add + commit OFF the lock.
    crate::git::commit(&project, &message, &files)?;

    // 3. Re-probe the working tree OFF the lock.
    let fresh = marsad_core::RealGit.changes(&project);

    // 4. Briefly lock again to update the session's change list.
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core.set_session_changes(&session_id, fresh),
            AppState::Mock(_) => vec![],
        }
    }; // lock released

    crate::events::emit_updates(&app, updates);
    Ok(())
}

// ---------------------------------------------------------------------------
// Session config commands
// ---------------------------------------------------------------------------

/// Restart a session with a new model, posture, and disallowed tools.
///
/// Sends `StopSession` to kill the current claude process, then relaunches it
/// with the SAME marsad session id (so the UI row is preserved) and resumes
/// the conversation via `--resume <claude-uuid>` (or `--continue` if no UUID
/// is known yet). Any tools in `disallowed_tools` are passed to claude via
/// `--disallowedTools`.
#[tauri::command(rename_all = "snake_case")]
pub fn restart_session(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    id: String,
    model: String,
    posture: String,
    disallowed_tools: Option<Vec<String>>,
) -> Result<(), String> {
    let posture_val = match posture.as_str() {
        "auto" => Posture::AutoApproveSafe,
        "full" => Posture::FullAutonomy,
        _ => Posture::Ask,
    };
    let disallowed = disallowed_tools.unwrap_or_default();
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core.restart_session(&id, &model, posture_val, disallowed).map_err(map_err)?,
            AppState::Mock(_) => vec![],
        }
    };
    crate::events::emit_updates(&app, updates);
    Ok(())
}

/// Persist a named config template (model + posture).
#[tauri::command(rename_all = "snake_case")]
pub fn save_template(
    state: State<'_, ManagedState>,
    name: String,
    model: String,
    posture: String,
) -> Result<(), String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    match &mut *guard {
        AppState::Live(core) => core.save_template(&name, &model, &posture).map_err(map_err),
        AppState::Mock(_) => Ok(()),
    }
}

/// Return all persisted config templates.
#[tauri::command(rename_all = "snake_case")]
pub fn list_templates(state: State<'_, ManagedState>) -> Result<Vec<PersistedTemplate>, String> {
    let guard = state.lock().map_err(|e| e.to_string())?;
    match &*guard {
        AppState::Live(core) => core.list_templates().map_err(map_err),
        AppState::Mock(_) => Ok(vec![]),
    }
}

// ---------------------------------------------------------------------------
// Shell command execution (! prefix)
// ---------------------------------------------------------------------------

/// Run a shell command on the session's host (local or remote) and inject the
/// result into the session's transcript as a `User("! <cmd>")` + `Tool(shell)`
/// pair.  The transcript update is emitted to the webview via the usual
/// `VmUpdate::Session` channel so the Activity tab refreshes immediately.
///
/// Execution is done OFF the Core mutex (same discipline as git/SSH probes):
///   • local host → `sh -c` (Unix) / `cmd /C` (Windows)
///   • remote host → `marsad_agent::run_remote_script` (ssh/wsl/plink)
///
/// Stdout and stderr are both captured and combined for display.
///
/// In mock mode, a canned output is injected so the UI renders something
/// visually correct even without a real agent.
/// `async` is essential: a `!` command can be long-running, and a SYNCHRONOUS
/// Tauri command runs on the webview's main thread — it would FREEZE the whole
/// UI until the command finished, then dump the `!` line + output at once. This
/// async version instead: (1) injects the `! <cmd>` line + a "shell running"
/// flag immediately so it shows the instant the user hits enter (with a working
/// indicator), (2) runs the command on the blocking pool (off the async runtime
/// and off the main thread), (3) injects the output when it completes. The state
/// lock is taken + dropped around each step, never held across the await.
/// Process-wide registry of persistent per-session `!` shells, keyed by session
/// id. Each shell sources the host `setup` once and stays alive for the session,
/// so subsequent `!` commands skip the setup cost. Dropping an entry kills its
/// child (see `ExecShell`'s Drop).
type ExecShellRegistry = std::sync::Mutex<
    std::collections::HashMap<String, std::sync::Arc<std::sync::Mutex<marsad_agent::exec_shell::ExecShell>>>,
>;

fn exec_shells() -> &'static ExecShellRegistry {
    static R: std::sync::OnceLock<ExecShellRegistry> = std::sync::OnceLock::new();
    R.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Run `command` through the session's persistent shell. Returns `Some(output)`
/// on success, or `None` to signal the caller should use the one-shot fallback
/// (shell couldn't spawn, or died — in which case the dead entry is dropped so a
/// later `!` respawns it fresh).
fn exec_via_persistent_shell(
    session_id: &str,
    host_target: Option<&str>,
    project: &str,
    setup: Option<&str>,
    env: &[(String, String)],
    command: &str,
) -> Option<String> {
    // Get-or-spawn this session's shell (spawn under the lock so two racing `!`s
    // don't create two shells).
    let shell = {
        let mut reg = exec_shells().lock().ok()?;
        if let Some(existing) = reg.get(session_id) {
            existing.clone()
        } else {
            match marsad_agent::exec_shell::ExecShell::spawn(host_target, project, setup, env) {
                Ok(s) => {
                    let arc = std::sync::Arc::new(std::sync::Mutex::new(s));
                    reg.insert(session_id.to_string(), arc.clone());
                    arc
                }
                Err(e) => {
                    crate::diag_log(&format!(
                        "marsad-desktop: exec shell spawn failed for {session_id}: {e} — one-shot fallback"
                    ));
                    return None;
                }
            }
        }
    };

    // Run (per-session serialized by this shell's own lock). On error the shell
    // has died — drop it so the next `!` respawns, and fall back for THIS call.
    let result = {
        let mut sh = shell.lock().ok()?;
        sh.run(command)
    };
    match result {
        Ok(out) => Some(out),
        Err(e) => {
            crate::diag_log(&format!(
                "marsad-desktop: exec shell died for {session_id}: {e} — one-shot fallback"
            ));
            exec_shells().lock().ok()?.remove(session_id);
            None
        }
    }
}

/// Kill + forget a session's persistent `!` shell (on stop/delete). Best-effort.
pub(crate) fn drop_exec_shell(session_id: &str) {
    if let Ok(mut reg) = exec_shells().lock() {
        reg.remove(session_id); // Drop kills the child
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn run_shell(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    session_id: String,
    command: String,
) -> Result<(), String> {
    // 1. Briefly lock to resolve host target/project/setup/env AND inject the
    //    `! <command>` line + mark the shell running — so it renders immediately.
    #[allow(clippy::type_complexity)]
    let (host_target, project, host_setup, host_env): (Option<String>, String, Option<String>, Vec<(String, String)>) = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => {
                let snap = core.snapshot();
                let sv = snap
                    .sessions
                    .iter()
                    .find(|s| s.id == session_id)
                    .ok_or_else(|| format!("unknown session: {session_id}"))?;
                let project = sv.project.clone();
                let host_id = sv.host_id.clone();
                let host = snap.hosts.into_iter().find(|h| h.id == host_id);
                let (target, setup, env) = match host {
                    Some(h) => {
                        let target = if h.is_local { None } else { Some(h.target) };
                        (target, h.setup, h.env)
                    }
                    None => (None, None, Vec::new()),
                };
                // Inject the command line + working flag NOW (before executing).
                let updates = core
                    .inject_shell_command(&session_id, &command)
                    .map_err(map_err)?;
                crate::events::emit_updates(&app, updates);
                (target, project, setup, env)
            }
            AppState::Mock(_) => (None, String::new(), None, Vec::new()),
        }
    }; // lock released before the await below

    // 2. Execute the command on the BLOCKING POOL so the UI never freezes.
    //    Prefer the session's PERSISTENT exec shell (setup sourced once → a bare
    //    `ls` is instant even on a slow-setup host, and cwd/exports persist across
    //    `!`s). Fall back to the one-shot path if the shell can't spawn or dies.
    let cmd_for_exec = command.clone();
    let sid_for_exec = session_id.clone();
    let output = tauri::async_runtime::spawn_blocking(move || {
        if let Some(out) = exec_via_persistent_shell(
            &sid_for_exec,
            host_target.as_deref(),
            &project,
            host_setup.as_deref(),
            &host_env,
            &cmd_for_exec,
        ) {
            return if out.trim().is_empty() { "(no output)".to_string() } else { out };
        }
        // Fallback: one-shot (old behavior) — re-sources setup each call.
        match host_target {
            Some(target) => {
                let script =
                    build_remote_shell_script(&host_setup, &host_env, &project, &cmd_for_exec);
                // `run_remote_script` returns None when it couldn't run the command
                // at all (connection down/slow). Distinguish that from a command
                // that ran and produced nothing — never silently swallow a command
                // as "(no output)" when it never actually executed.
                match marsad_agent::run_remote_script(&target, &script) {
                    Some(out) if !out.trim().is_empty() => out,
                    Some(_) => "(no output)".to_string(),
                    None => "⚠ connection lost — command not run. Try again once reconnected.".to_string(),
                }
            }
            None => run_shell_locally_with_env(&cmd_for_exec, &project, &host_setup, &host_env),
        }
    })
    .await
    .unwrap_or_else(|e| format!("(shell task failed: {e})"));

    // 3. Inject the output (the `! <cmd>` line was already injected in step 1).
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core
                .inject_shell_result(&session_id, &output)
                .map_err(map_err)?,
            AppState::Mock(_) => vec![],
        }
    };
    crate::events::emit_updates(&app, updates);
    Ok(())
}

/// Compute the side-by-side diff rows for a single changed file in the session's
/// project directory (local or remote host).
///
/// Resolves the session's host + project exactly like [`run_shell`], then runs a
/// small POSIX script that tries, in order: `git diff HEAD -- <path>` (staged or
/// unstaged change to a tracked file), `git diff -- <path>` (worktree change),
/// and finally `git diff --no-index -- /dev/null <path>` (an untracked file, so
/// the whole file shows as additions). The captured unified patch is parsed into
/// `DiffLine`s and folded into `SideRow`s for the UI.
///
/// Returns an empty vec when there is no diff (e.g. the file is unchanged or the
/// path doesn't exist).
#[tauri::command(rename_all = "snake_case")]
pub fn get_file_diff(
    state: State<'_, ManagedState>,
    session_id: String,
    path: String,
) -> Result<Vec<marsad_diff::SideRow>, String> {
    // 1. Resolve host target (local vs remote) + project dir under a brief lock.
    let (host_target, project): (Option<String>, String) = {
        let guard = state.lock().map_err(|e| e.to_string())?;
        match &*guard {
            AppState::Live(core) => {
                let snap = core.snapshot();
                let sv = snap
                    .sessions
                    .iter()
                    .find(|s| s.id == session_id)
                    .ok_or_else(|| format!("unknown session: {session_id}"))?;
                let project = sv.project.clone();
                let host_id = sv.host_id.clone();
                let target = snap
                    .hosts
                    .into_iter()
                    .find(|h| h.id == host_id)
                    .and_then(|h| if h.is_local { None } else { Some(h.target) });
                (target, project)
            }
            AppState::Mock(_) => (None, String::new()),
        }
    }; // lock released

    // 2. Build the diff script (tracked, then worktree, then untracked).
    //    Single-quote the path so spaces/specials are safe inside the script.
    let qpath = format!("'{}'", path.replace('\'', r"'\''"));
    let script = format!(
        "cd {cd} 2>/dev/null\n\
         d=$(git diff HEAD -- {p} 2>/dev/null)\n\
         [ -z \"$d\" ] && d=$(git diff -- {p} 2>/dev/null)\n\
         [ -z \"$d\" ] && d=$(git diff --no-index -- /dev/null {p} 2>/dev/null)\n\
         printf '%s' \"$d\"",
        cd = shell_quote_cd(&project),
        p = qpath,
    );

    // 3. Execute OFF the lock.
    let out = match host_target {
        Some(target) => marsad_agent::run_remote_script(&target, &script).unwrap_or_default(),
        None => run_diff_locally(&script, &project),
    };

    // 4. Parse the unified patch into side-by-side rows.
    let lines = marsad_diff::parse_unified(&out);
    Ok(marsad_diff::split_side_by_side(&lines))
}

/// Run a POSIX diff `script` locally in `project` (if it exists), returning the
/// captured stdout. Returns stdout verbatim (no "(no output)" substitution) so
/// an empty diff stays empty.
fn run_diff_locally(script: &str, project: &str) -> String {
    use std::process::Command;

    #[cfg(windows)]
    let (bin, flag) = ("cmd", "/C");
    #[cfg(not(windows))]
    let (bin, flag) = ("sh", "-c");

    let mut cmd = Command::new(bin);
    cmd.arg(flag).arg(script);
    if !project.is_empty() && std::path::Path::new(project).is_dir() {
        cmd.current_dir(project);
    }

    match cmd.output() {
        Ok(out) => String::from_utf8_lossy(&out.stdout).into_owned(),
        Err(_) => String::new(),
    }
}

/// Shell-quote a project path for use after `cd` in a remote POSIX script.
/// Leaves `~` / `~/...` unquoted so the remote shell expands them; single-quotes
/// anything else (safe for spaces). Empty → `.` (stay in the default dir).
fn shell_quote_cd(project: &str) -> String {
    if project.is_empty() {
        ".".to_string()
    } else if project == "~" || project.starts_with("~/") {
        project.to_string()
    } else {
        format!("'{}'", project.replace('\'', r"'\''"))
    }
}

/// POSIX single-quote a VALUE string for use in `export K='V'` lines inside a
/// shell script.  Only values are quoted; keys are always valid identifiers and
/// must NOT be quoted (POSIX sh does not allow quoted variable names in export).
/// The remote target is always a POSIX `sh`, so this quoting is platform-safe.
fn shell_quote_value(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Build the POSIX shell script that `run_shell` sends to the remote host.
///
/// Prepends the host's `setup` command (if any) and `export K=V` lines for
/// each `env` pair BEFORE the `cd` + user command, so `!cmd` runs in the
/// SAME environment that the session's claude process gets.
fn build_remote_shell_script(
    setup: &Option<String>,
    env: &[(String, String)],
    project: &str,
    command: &str,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(s) = setup.as_deref() {
        if !s.trim().is_empty() {
            parts.push(s.to_string());
        }
    }
    for (k, v) in env {
        // Key is always a valid identifier — no quoting needed; value is quoted.
        parts.push(format!("export {}={}", k, shell_quote_value(v)));
    }
    parts.push(format!("cd {} 2>/dev/null", shell_quote_cd(project)));
    parts.push(format!("{command} 2>&1"));
    parts.join("\n")
}

/// Run a shell command locally in `project` (if it exists), capturing combined
/// stdout+stderr.  Prepends `setup` and `env` exports so the command runs in
/// the SAME environment as the session's claude process.
///
/// Uses `sh -c` on Unix and `cmd /C` on Windows (on Windows, setup/env are
/// best-effort since the setup is typically a POSIX command). Falls back to an
/// error message if the subprocess cannot be spawned.
// On Windows the POSIX setup/env preamble is intentionally skipped (the setup
// command is a POSIX shell expression that wouldn't run under cmd), so those
// params are unused on that target only.
#[cfg_attr(windows, allow(unused_variables))]
fn run_shell_locally_with_env(
    command: &str,
    project: &str,
    setup: &Option<String>,
    env: &[(String, String)],
) -> String {
    use std::process::Command;

    // Build a composite script: prepend setup + exports, then the command.
    // On Windows we stay with `cmd /C` and skip the POSIX preamble (the setup
    // command is typically a POSIX shell expression that wouldn't run under cmd).
    #[cfg(windows)]
    let full_command = command.to_string();
    #[cfg(not(windows))]
    let full_command = {
        let mut parts: Vec<String> = Vec::new();
        if let Some(s) = setup.as_deref() {
            if !s.trim().is_empty() {
                parts.push(s.to_string());
            }
        }
        for (k, v) in env {
            // Key is always a valid identifier — no quoting needed; value is quoted.
            parts.push(format!("export {}={}", k, shell_quote_value(v)));
        }
        parts.push(command.to_string());
        parts.join("\n")
    };

    #[cfg(windows)]
    let (bin, flag) = ("cmd", "/C");
    #[cfg(not(windows))]
    let (bin, flag) = ("sh", "-c");

    let mut cmd = Command::new(bin);
    cmd.arg(flag).arg(&full_command);
    // Run in the session's project dir when it's a real existing path.
    if !project.is_empty() && std::path::Path::new(project).is_dir() {
        cmd.current_dir(project);
    }

    match cmd.output() {
        Ok(out) => {
            let mut combined = String::new();
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !stdout.is_empty() {
                combined.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !combined.is_empty() {
                    combined.push('\n');
                }
                combined.push_str(&stderr);
            }
            if combined.is_empty() {
                if out.status.success() {
                    "(no output)".to_string()
                } else {
                    format!("(exit {})", out.status.code().unwrap_or(-1))
                }
            } else {
                combined
            }
        }
        Err(e) => format!("failed to run command: {e}"),
    }
}



// ---------------------------------------------------------------------------
// Workstream / connection / profile / board / graph commands
//
// The B12 workstream layer over marsad-core. Mutations return the refreshed
// board (and related updates) which we push to the webview via "vm-update";
// `board`/`graph` are pure reads. Invoke names + payload keys mirror
// apps/desktop/src/bridge/api.ts exactly. In Mock mode these are inert (the
// JS layer uses its own mockWorkstreams when not running under Tauri).
// ---------------------------------------------------------------------------

/// Create a workstream, persist, and push the refreshed board to the webview.
#[tauri::command(rename_all = "snake_case")]
pub fn create_workstream(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    id: String,
    name: String,
    goal: String,
    profile: Option<String>,
) -> Result<(), String> {
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core
                .create_workstream(id, name, goal, profile)
                .map_err(map_err)?,
            AppState::Mock(_) => vec![],
        }
    };
    crate::events::emit_updates(&app, updates);
    Ok(())
}

/// Delete a workstream (kills all member sessions, cascades connections), then refresh.
#[tauri::command(rename_all = "snake_case")]
pub fn delete_workstream(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    id: String,
) -> Result<(), String> {
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core.delete_workstream(&id).map_err(map_err)?,
            AppState::Mock(_) => vec![],
        }
    };
    crate::events::emit_updates(&app, updates);
    Ok(())
}

/// Pause (stop) all sessions in a workstream. Each session's process is killed
/// (best-effort StopSession IPC) and its phase flips to Stale; they remain
/// resumable. Returns session + overview updates so the UI refreshes immediately.
#[tauri::command(rename_all = "snake_case")]
pub fn pause_workstream(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    id: String,
) -> Result<(), String> {
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core.pause_workstream(&id).map_err(map_err)?,
            AppState::Mock(_) => vec![],
        }
    };
    crate::events::emit_updates(&app, updates);
    Ok(())
}

/// Resume all sessions in a workstream (ResumeSession IPC for each; phases flip
/// to Starting). Returns session + overview updates so the UI refreshes immediately.
#[tauri::command(rename_all = "snake_case")]
pub fn resume_workstream(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    id: String,
) -> Result<(), String> {
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core.resume_workstream(&id).map_err(map_err)?,
            AppState::Mock(_) => vec![],
        }
    };
    crate::events::emit_updates(&app, updates);
    Ok(())
}

/// Add a connection between two sessions in a workstream, then refresh the board.
// A Tauri command legitimately needs all of these invoke-payload keys.
#[tauri::command(rename_all = "snake_case")]
#[allow(clippy::too_many_arguments)]
pub fn add_connection(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    id: String,
    ws: String,
    from: String,
    to: String,
    kind: marsad_workstream::ConnKind,
    bi: bool,
) -> Result<(), String> {
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core
                .add_connection(id, ws, from, to, kind, bi)
                .map_err(map_err)?,
            AppState::Mock(_) => vec![],
        }
    };
    crate::events::emit_updates(&app, updates);
    Ok(())
}

/// Remove a connection, then refresh the board.
#[tauri::command(rename_all = "snake_case")]
pub fn remove_connection(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    id: String,
) -> Result<(), String> {
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core.remove_connection(&id).map_err(map_err)?,
            AppState::Mock(_) => vec![],
        }
    };
    crate::events::emit_updates(&app, updates);
    Ok(())
}

/// Create a profile, then refresh the board.
#[tauri::command(rename_all = "snake_case")]
pub fn create_profile(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    id: String,
    name: String,
) -> Result<(), String> {
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core.create_profile(id, name).map_err(map_err)?,
            AppState::Mock(_) => vec![],
        }
    };
    crate::events::emit_updates(&app, updates);
    Ok(())
}

/// Delete a profile (unassigning its workstreams), then refresh the board.
#[tauri::command(rename_all = "snake_case")]
pub fn delete_profile(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    id: String,
) -> Result<(), String> {
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core.delete_profile(&id).map_err(map_err)?,
            AppState::Mock(_) => vec![],
        }
    };
    crate::events::emit_updates(&app, updates);
    Ok(())
}

/// Assign a session to a workstream (optionally as its lead), then refresh the board.
#[tauri::command(rename_all = "snake_case")]
pub fn assign_session(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    session_id: String,
    ws_id: String,
    lead: bool,
) -> Result<(), String> {
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core
                .assign_session(&session_id, &ws_id, lead)
                .map_err(map_err)?,
            AppState::Mock(_) => vec![],
        }
    };
    crate::events::emit_updates(&app, updates);
    Ok(())
}

/// Set (or clear) the active profile board filter, then refresh the board.
#[tauri::command(rename_all = "snake_case")]
pub fn set_active_profile(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    profile: Option<String>,
) -> Result<(), String> {
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core.set_active_profile(profile),
            AppState::Mock(_) => vec![],
        }
    };
    crate::events::emit_updates(&app, updates);
    Ok(())
}

/// Read the Level-1 workstreams board (profile-filtered by core's active profile).
#[tauri::command(rename_all = "snake_case")]
pub fn board(state: State<'_, ManagedState>) -> Result<marsad_viewmodel::BoardView, String> {
    let guard = state.lock().map_err(|e| e.to_string())?;
    match &*guard {
        AppState::Live(core) => Ok(core.board()),
        AppState::Mock(_) => Ok(marsad_viewmodel::board_view(&[], &[], &[], &[], &[], None)),
    }
}

/// Read the Level-2 dependency graph for one workstream (None if unknown).
#[tauri::command(rename_all = "snake_case")]
pub fn graph(
    state: State<'_, ManagedState>,
    ws_id: String,
) -> Result<Option<marsad_viewmodel::GraphView>, String> {
    let guard = state.lock().map_err(|e| e.to_string())?;
    match &*guard {
        AppState::Live(core) => Ok(core.graph(&ws_id)),
        AppState::Mock(_) => Ok(None),
    }
}

/// Read the Usage tab's cost breakdown ("By host" / "By node") for one
/// workstream (None if `ws_id` is unknown). BUILT-NOT-VERIFIED (Wave 3a):
/// mirrors `board`/`graph` exactly (lock-clone-return, no IO) against
/// `Core::cost_breakdown`'s already-tested signature
/// (crates/marsad-core/src/lib.rs) — but, like the rest of src-tauri, this
/// has never compiled/run here (no webkit2gtk). See docs/GAPS.md.
#[tauri::command(rename_all = "snake_case")]
pub fn cost_breakdown(
    state: State<'_, ManagedState>,
    ws_id: String,
) -> Result<Option<marsad_viewmodel::WorkstreamCostBreakdown>, String> {
    let guard = state.lock().map_err(|e| e.to_string())?;
    match &*guard {
        AppState::Live(core) => Ok(core.cost_breakdown(&ws_id)),
        AppState::Mock(_) => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// WAVE 3b — pty escape-hatch terminal (Terminal tab). BUILT-NOT-VERIFIED:
// like the rest of src-tauri, this has never compiled/run here (no
// webkit2gtk). See docs/GAPS.md.
//
// Deliberately NO `pty_stop` command: the agent tears a session's pty down
// automatically on `StopSession` (`crates/marsad-agent/src/ipc.rs`'s
// `handle_stop_session` calls `pty::teardown_session`) — a user merely
// navigating away from the Terminal tab does NOT kill the shell, mirroring
// tmux detach/reattach semantics (consistent with the rest of Marsad's
// "sessions survive GUI exit" philosophy). The frontend just stops
// listening for `"pty"` events on unmount; the real pty keeps running
// server-side until the session itself stops.
// ---------------------------------------------------------------------------

/// Send one `IpcMsg` line directly to the agent over a fresh, one-shot
/// connection.
///
/// NOT routed through `Core` (which has no pty-aware methods — marsad-core
/// was deliberately not touched by this wave: raw terminal bytes have no
/// session/business meaning for it to reduce) and NOT reusing the reader
/// thread's long-lived subscriber connection (`run_reader_thread` in
/// `lib.rs` owns that one exclusively for its blocking `recv_line` loop; a
/// second writer on the same socket handle would race it). A dedicated
/// ephemeral connection per call is the simplest correct option: the
/// agent's disconnect handling is tolerant BY DESIGN ("Client disconnect
/// must NOT kill sessions: dropping a core connection only removes a
/// subscriber", `docs/specs/agent.md`), so sending one line and dropping the
/// connection is a well-supported pattern — it mirrors how the reader
/// thread itself opens its OWN separate `LocalIpc` connection just to send
/// one `Subscribe` line at startup.
///
/// Best-effort: connect/send failures are logged, never surfaced as a
/// command error — a dropped keystroke/resize is far less disruptive to the
/// user than an error toast on every keypress. (A persistent, reused
/// connection would cut the per-call connect overhead; deferred as a
/// follow-up — see `docs/GAPS.md` — to keep this BUILT-NOT-VERIFIED surface
/// as simple/low-risk as possible.)
fn send_pty_ipc(msg: &marsad_proto::IpcMsg, _hold: bool) {
    // Enqueue onto the single persistent pty sender (ordered, non-blocking). The
    // OLD approach — a fresh ephemeral connection per keystroke, dropped
    // immediately for PtyInput — raced the agent's read on close and SILENTLY
    // DROPPED keystrokes (and per-connection connect overhead added lag). A
    // single long-lived connection with a dedicated writer thread delivers every
    // line in order with a real flush and no close-race. A companion drain thread
    // reads + discards the agent's broadcasts so its per-client buffer never fills.
    let _ = pty_sender().send(marsad_proto::encode_ipc(msg));
}

/// The process-wide ordered pty-command sender. Lazily spins up one background
/// thread owning a persistent agent connection; every `pty_*` command pushes its
/// encoded line here. Serialized → keystrokes never arrive out of order.
fn pty_sender() -> &'static std::sync::mpsc::Sender<String> {
    use std::sync::OnceLock;
    static TX: OnceLock<std::sync::mpsc::Sender<String>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || pty_sender_loop(rx));
        tx
    })
}

fn pty_sender_loop(rx: std::sync::mpsc::Receiver<String>) {
    use marsad_transport::Transport;
    use std::io::Write;

    let mut writer: Option<Box<dyn Write + Send>> = None;
    for line in rx {
        // (Re)connect if we have no live writer.
        if writer.is_none() {
            let mut ipc = marsad_transport::LocalIpc::new(&crate::resolve_socket_path());
            if ipc.connect().is_err() {
                crate::diag_log("marsad-desktop: pty sender: connect failed");
                continue; // drop this one line; next keystroke retries
            }
            match ipc.into_rw() {
                Some((w, mut reader)) => {
                    // Drain + discard the agent's broadcasts so this persistent
                    // connection's server-side buffer never fills.
                    std::thread::spawn(move || {
                        use std::io::BufRead;
                        let mut buf = String::new();
                        loop {
                            buf.clear();
                            match reader.read_line(&mut buf) {
                                Ok(0) | Err(_) => break,
                                Ok(_) => {}
                            }
                        }
                    });
                    writer = Some(w);
                }
                None => continue,
            }
        }
        if let Some(w) = writer.as_mut() {
            let framed = format!("{line}\n");
            if w.write_all(framed.as_bytes()).and_then(|_| w.flush()).is_err() {
                // Broken pipe — drop the writer so the next line reconnects.
                writer = None;
            }
        }
    }
}

/// Start (or, if already live, no-op re-attach to) a session's interactive
/// pty. No return payload — the pty coming alive is observed via `PtyOutput`
/// events on the `"pty"` channel (see `events::emit_pty_update`).
#[tauri::command(rename_all = "snake_case")]
pub fn pty_start(
    state: State<'_, ManagedState>,
    session_id: String,
    terminal_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    crate::diag_log(&format!(
        "marsad-desktop: pty_start command invoked sid={session_id} term={terminal_id} {cols}x{rows}"
    ));
    let guard = state.lock().map_err(|e| e.to_string())?;
    if let AppState::Live(_) = &*guard {
        send_pty_ipc(
            &marsad_proto::IpcMsg::PtyStart {
                session_id,
                terminal_id,
                cols,
                rows,
            },
            true, // hold: PtyStart fires once; keep the socket open so it lands
        );
    }
    Ok(())
}

/// Forward raw keystroke bytes to a session's live pty stdin.
#[tauri::command(rename_all = "snake_case")]
pub fn pty_input(
    state: State<'_, ManagedState>,
    session_id: String,
    terminal_id: String,
    data: String,
) -> Result<(), String> {
    let guard = state.lock().map_err(|e| e.to_string())?;
    if let AppState::Live(_) = &*guard {
        send_pty_ipc(
            &marsad_proto::IpcMsg::PtyInput {
                session_id,
                terminal_id,
                data,
            },
            false,
        );
    }
    Ok(())
}

/// Resize a session's live pty viewport (e.g. `xterm-addon-fit` recomputed
/// cols/rows after the window/pane resized).
#[tauri::command(rename_all = "snake_case")]
pub fn pty_resize(
    state: State<'_, ManagedState>,
    session_id: String,
    terminal_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let guard = state.lock().map_err(|e| e.to_string())?;
    if let AppState::Live(_) = &*guard {
        send_pty_ipc(
            &marsad_proto::IpcMsg::PtyResize {
                session_id,
                terminal_id,
                cols,
                rows,
            },
            false,
        );
    }
    Ok(())
}

/// v2: close (kill) ONE terminal of a session.
#[tauri::command(rename_all = "snake_case")]
pub fn pty_close(
    state: State<'_, ManagedState>,
    session_id: String,
    terminal_id: String,
) -> Result<(), String> {
    let guard = state.lock().map_err(|e| e.to_string())?;
    if let AppState::Live(_) = &*guard {
        send_pty_ipc(
            &marsad_proto::IpcMsg::PtyClose {
                session_id,
                terminal_id,
            },
            true, // hold: fires rarely; keep the socket open so it lands
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interactive_ssh_plain_target_runs_one_shot_and_exits() {
        // A bare `ssh <target>` opens an interactive shell that never returns
        // ("connects but never goes back to Marsad"). The interactive connect MUST
        // append a one-shot command so ssh authenticates, establishes the master,
        // then EXITS — same as the wslssh/MFA path.
        let args = interactive_ssh_args("etx");
        assert_eq!(args[0], "ssh");
        assert!(args.contains(&"etx".to_string()), "target present");
        // The last tokens are the one-shot remote command (ssh joins them).
        assert_eq!(args.last().map(String::as_str), Some("window."));
        assert!(args.contains(&"echo".to_string()), "one-shot echo present so ssh exits");
        // Sanity: `echo` comes AFTER the target (it's the remote command, not an -o).
        let echo_i = args.iter().position(|a| a == "echo").unwrap();
        let tgt_i = args.iter().position(|a| a == "etx").unwrap();
        assert!(echo_i > tgt_i, "remote command follows the target");
    }

    #[test]
    fn interactive_ssh_wslssh_target_still_one_shot() {
        let args = interactive_ssh_args("wslssh:Ubuntu:narval");
        assert_eq!(args[0], "wsl.exe");
        assert!(args.contains(&"echo".to_string()), "wslssh one-shot preserved");
        assert_eq!(args.last().map(String::as_str), Some("window."));
    }

    #[test]
    fn wasl_outcome_approved_maps_to_approve() {
        assert_eq!(
            wasl_outcome_to_decision("approved", None),
            Decision::Approve
        );
        // A reason on an approval is ignored — approval carries no reason.
        assert_eq!(
            wasl_outcome_to_decision("approved", Some("whatever".into())),
            Decision::Approve
        );
    }

    #[test]
    fn wasl_outcome_denied_carries_reason() {
        assert_eq!(
            wasl_outcome_to_decision("denied", Some("not now".into())),
            Decision::Reject { reason: "not now".into() }
        );
    }

    #[test]
    fn wasl_outcome_without_reason_falls_back_to_outcome_word() {
        // A deny/dismiss with no typed reason must never be a blank rejection.
        assert_eq!(
            wasl_outcome_to_decision("dismissed", None),
            Decision::Reject { reason: "dismissed".into() }
        );
        assert_eq!(
            wasl_outcome_to_decision("denied", None),
            Decision::Reject { reason: "denied".into() }
        );
    }
}
