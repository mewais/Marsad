//! D01 — Tauri command bridge over marsad-core.
//!
//! Each public function here is registered as a Tauri command in lib.rs.
//! The live Core is stored as Tauri managed state behind a Mutex.
//! In MARSAD_MOCK=1 mode the same state holds a MockState that serves
//! a seeded AppSnapshot (see mock.rs).

use std::sync::Mutex;
use tauri::State;

use marsad_core::{Core, CoreError, GitProbe, LaunchMode, LaunchSpec, PersistedSettings, PersistedTemplate, Posture, SessionSummary};
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
                // Core resolves the remote target from host_id via its registry.
                host_target: None,
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
        default_model: "claude-sonnet-4-6".to_string(),
        default_posture: "ask".to_string(),
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

/// List installed WSL distros as ready-to-add `wsl:<distro>` host targets.
/// Empty on non-Windows (and if WSL isn't installed). Lets the Hosts UI offer
/// one-click WSL hosts instead of the user having to know the `wsl:<distro>`
/// convention.
#[tauri::command]
pub fn list_wsl_distros() -> Result<Vec<String>, String> {
    marsad_hosts::list_wsl_distros(&marsad_hosts::RealWslRunner).map_err(|e| e.to_string())
}

/// Open a real OS terminal window running an INTERACTIVE `ssh` login for
/// `target`, using the SAME ControlPath as the automated connections (see
/// docs/remote-hosts.md) so the resulting master socket is reused headlessly.
///
/// This is the MFA / keyboard-interactive escape hatch: the user completes the
/// login once in a visible terminal (NO `BatchMode`), the master persists
/// (`ControlPersist=600`), and every later automated `ssh` rides it.
#[tauri::command(rename_all = "snake_case")]
pub fn connect_host_interactive(target: String) -> Result<(), String> {
    launch_interactive_ssh(&target)
}

/// Build the interactive `ssh` argv used by `connect_host_interactive`.
///
/// Deliberately NO `BatchMode` (this is the interactive login). The
/// `ControlPath` MUST match docs/remote-hosts.md exactly so automated
/// connections reuse the master socket.
fn interactive_ssh_args(target: &str) -> Vec<String> {
    vec![
        "ssh".to_string(),
        "-o".to_string(),
        "ControlMaster=auto".to_string(),
        "-o".to_string(),
        "ControlPath=~/.ssh/marsad-cm-%r@%h:%p".to_string(),
        "-o".to_string(),
        "ControlPersist=600".to_string(),
        "-o".to_string(),
        "ServerAliveInterval=30".to_string(),
        target.to_string(),
    ]
}

/// Spawn a visible OS terminal emulator running the interactive ssh login.
/// Factored per-OS so each branch only compiles on its target.
#[cfg(target_os = "windows")]
fn launch_interactive_ssh(target: &str) -> Result<(), String> {
    use std::process::Command;
    // Open a new VISIBLE console that STAYS open and shows ssh's output/prompts.
    //   cmd /C start "Marsad login" cmd /K ssh <args…>
    // - `start "title"` opens a separate window (the first quoted token is the
    //   window title, not a program — required so the real program isn't mistaken
    //   for a title).
    // - `cmd /K` keeps that window open after ssh exits, so a failed/again-prompting
    //   Duo login is visible instead of the window flashing shut (the "shows
    //   nothing" bug). It also gives ssh a real console → a PTY for the prompt.
    // We deliberately do NOT set CREATE_NO_WINDOW here — this window is intentional.
    let mut cmd = Command::new("cmd.exe");
    cmd.arg("/C")
        .arg("start")
        .arg("Marsad login")
        .arg("cmd")
        .arg("/K")
        .arg("ssh");
    // interactive_ssh_args[0] is "ssh"; pass the rest as ssh's arguments.
    for a in interactive_ssh_args(target).into_iter().skip(1) {
        cmd.arg(a);
    }
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

/// Restart a session with a new model and posture.
///
/// Sends `StopSession` to kill the current claude process, then relaunches it
/// with the SAME marsad session id (so the UI row is preserved) and resumes
/// the conversation via `--resume <claude-uuid>` (or `--continue` if no UUID
/// is known yet).
#[tauri::command(rename_all = "snake_case")]
pub fn restart_session(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    id: String,
    model: String,
    posture: String,
) -> Result<(), String> {
    let posture_val = match posture.as_str() {
        "auto" => Posture::AutoApproveSafe,
        "full" => Posture::FullAutonomy,
        _ => Posture::Ask,
    };
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core.restart_session(&id, &model, posture_val).map_err(map_err)?,
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
