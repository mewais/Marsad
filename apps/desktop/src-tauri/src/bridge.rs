//! D01 — Tauri command bridge over marsad-core.
//!
//! Each public function here is registered as a Tauri command in lib.rs.
//! The live Core is stored as Tauri managed state behind a Mutex.
//! In MARSAD_MOCK=1 mode the same state holds a MockState that serves
//! a seeded AppSnapshot (see mock.rs).

use std::sync::Mutex;
use tauri::State;

use marsad_core::{Core, CoreError, LaunchMode, LaunchSpec, Posture, SessionSummary};
use marsad_gates::{Decision, DecisionId, GateDecision};
use marsad_viewmodel::AppSnapshot;

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
                model,
                posture: posture_val,
                first_prompt,
                mode,
            };
            core.launch_session(spec).map_err(map_err)
        }
        AppState::Mock(_) => Ok("mock-session-001".to_string()),
    }
}

#[tauri::command(rename_all = "snake_case")]
pub fn list_sessions(project: String) -> Result<Vec<SessionSummary>, String> {
    let home = std::env::var("HOME").map_err(|e| e.to_string())?;
    let projects_root = std::path::PathBuf::from(format!("{home}/.claude/projects"));
    marsad_core::discovery::list_sessions(&projects_root, &project)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub fn resume_session(state: State<'_, ManagedState>, id: String) -> Result<(), String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    match &mut *guard {
        AppState::Live(core) => core.resume_session(&id).map_err(map_err),
        AppState::Mock(_) => Ok(()),
    }
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
pub fn stop_session(state: State<'_, ManagedState>, id: String) -> Result<(), String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    match &mut *guard {
        AppState::Live(core) => core.stop_session(&id).map_err(map_err),
        AppState::Mock(_) => Ok(()),
    }
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
pub fn interrupt(state: State<'_, ManagedState>, id: String) -> Result<(), String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    match &mut *guard {
        AppState::Live(core) => core.interrupt(&id).map_err(map_err),
        AppState::Mock(_) => Ok(()),
    }
}

#[tauri::command(rename_all = "snake_case")]
pub fn answer_gate(
    state: State<'_, ManagedState>,
    session_id: String,
    decision_id: String,
    allow: bool,
    deny_reason: Option<String>,
) -> Result<(), String> {
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
            };
            core.answer_gate(gate_decision).map_err(map_err)
        }
        AppState::Mock(_) => Ok(()),
    }
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
pub fn set_workspace(state: State<'_, ManagedState>, id: String) -> Result<(), String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    match &mut *guard {
        AppState::Live(core) => core.set_workspace(&id).map_err(map_err),
        AppState::Mock(mock) => {
            mock.set_active_ws(id);
            Ok(())
        }
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
pub fn add_host(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    label: String,
    target: String,
) -> Result<(), String> {
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core.add_host(&label, &target).map_err(map_err)?,
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
