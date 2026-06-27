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
    name: Option<String>,
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
            };
            core.launch_session(spec).map_err(map_err)
        }
        AppState::Mock(_) => Ok("mock-session-001".to_string()),
    }
}

/// `async` is essential here: listing a REMOTE host's sessions runs an ssh/wsl
/// round-trip that can take seconds. A synchronous Tauri command runs on the main
/// thread and would FREEZE the whole UI for that time; an async command runs off
/// it, and the remote fetch is further moved to a blocking pool thread so it never
/// stalls the async runtime. The state lock is taken + dropped BEFORE any await
/// (its guard isn't held across the await).
#[tauri::command(rename_all = "snake_case")]
pub async fn list_sessions(
    state: State<'_, ManagedState>,
    project: String,
    host_id: String,
) -> Result<Vec<SessionSummary>, String> {
    // Resolve the host: a local host's transcripts live on this machine; a remote
    // host's live on the remote box and must be listed over its own connection.
    let host_target: Option<String> = {
        let guard = state.lock().map_err(|e| e.to_string())?;
        match &*guard {
            AppState::Live(core) => core
                .snapshot()
                .hosts
                .into_iter()
                .find(|h| h.id == host_id)
                .and_then(|h| if h.is_local { None } else { Some(h.target) }),
            AppState::Mock(_) => None,
        }
    }; // guard dropped here — nothing is held across the await below
    match host_target {
        None => {
            let home = std::env::var("HOME").map_err(|e| e.to_string())?;
            let projects_root = std::path::PathBuf::from(format!("{home}/.claude/projects"));
            marsad_core::discovery::list_sessions(&projects_root, &project).map_err(|e| e.to_string())
        }
        Some(target) => {
            // Run the blocking ssh/wsl fetch on the blocking pool so it doesn't
            // stall the async runtime thread.
            let raw = tauri::async_runtime::spawn_blocking(move || {
                marsad_agent::fetch_remote_session_list(&target, &project)
            })
            .await
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
            Ok(parse_remote_session_list(&raw))
        }
    }
}

/// Parse the `mtime_ms<TAB>uuid<TAB><first user line>` records produced by
/// `marsad_agent::fetch_remote_session_list` into `SessionSummary` rows (newest
/// first, as emitted). The snippet is derived from the raw user line via the same
/// `first_user_prompt` parser the local lister uses.
fn parse_remote_session_list(stdout: &str) -> Vec<SessionSummary> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let modified: u64 = parts.next()?.trim().parse().ok()?;
            let id = parts.next()?.trim().to_string();
            if id.is_empty() {
                return None;
            }
            let raw = parts.next().unwrap_or("");
            let snippet = marsad_core::discovery::first_user_prompt(raw).unwrap_or_default();
            Some(SessionSummary {
                id,
                snippet,
                modified,
            })
        })
        .collect()
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
            AppState::Live(core) => core.remove_session(&id).map_err(map_err)?,
            AppState::Mock(_) => vec![],
        }
    };
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
pub fn interrupt(state: State<'_, ManagedState>, id: String) -> Result<(), String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    match &mut *guard {
        AppState::Live(core) => core.interrupt(&id).map_err(map_err),
        AppState::Mock(_) => Ok(()),
    }
}

#[tauri::command(rename_all = "snake_case")]
pub fn answer_question(
    state: State<'_, ManagedState>,
    session_id: String,
    request_id: String,
    answers: serde_json::Value,
) -> Result<(), String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    match &mut *guard {
        AppState::Live(core) => core
            .answer_question(&session_id, &request_id, answers)
            .map_err(map_err),
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
    claude_bin: Option<String>,
    setup: Option<String>,
    env: Option<Vec<(String, String)>>,
) -> Result<(), String> {
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core
                .add_host(&label, &target, claude_bin, setup, env.unwrap_or_default())
                .map_err(map_err)?,
            AppState::Mock(_) => vec![],
        }
    };
    crate::events::emit_updates(&app, updates);
    Ok(())
}

/// Update an existing host's launch configuration (`claude_bin` / `setup` /
/// `env`), and optionally its `target`. Works for any host id INCLUDING the
/// localhost (whose target is left unchanged when `target` is `None`). Persists
/// the change and pushes the updated host list to the webview.
#[tauri::command(rename_all = "snake_case")]
pub fn update_host(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    id: String,
    claude_bin: Option<String>,
    setup: Option<String>,
    env: Option<Vec<(String, String)>>,
    target: Option<String>,
) -> Result<(), String> {
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core
                .update_host(&id, claude_bin, setup, env.unwrap_or_default(), target)
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
pub fn connect_host_interactive(target: String) -> Result<(), String> {
    launch_interactive_ssh(&target)
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
            return vec![
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
                // One-shot remote command (ssh joins the trailing tokens): confirms
                // auth + leaves the persistent master, then ssh exits.
                "echo".to_string(),
                "Marsad:".to_string(),
                "connected.".to_string(),
                "You".to_string(),
                "can".to_string(),
                "close".to_string(),
                "this".to_string(),
                "window.".to_string(),
            ];
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
#[tauri::command(rename_all = "snake_case")]
pub fn run_shell(
    app: tauri::AppHandle,
    state: State<'_, ManagedState>,
    session_id: String,
    command: String,
) -> Result<(), String> {
    // 1. Briefly lock to resolve the host target (local vs remote) AND the
    //    session's project dir, so `!cmd` runs where the session runs (e.g. `!ls`
    //    lists the project, not $HOME).
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

    // 2. Execute the command OFF the lock, in the session's project dir.
    let output = match host_target {
        Some(target) => {
            // Remote: cd into the project (best-effort) then run the command, via
            // marsad-agent's run_remote_script (login shell, quoting-safe stdin).
            // `2>&1` folds stderr into stdout — run_remote_script captures stdout
            // only, so without this an erroring/stderr-only command (e.g. a "not
            // found") would render as an empty block. Empty result → "(no output)".
            let script = format!(
                "cd {} 2>/dev/null\n{} 2>&1",
                shell_quote_cd(&project),
                command
            );
            let out = marsad_agent::run_remote_script(&target, &script).unwrap_or_default();
            if out.trim().is_empty() {
                "(no output)".to_string()
            } else {
                out
            }
        }
        None => {
            // Local execution in the project dir.
            run_shell_locally(&command, &project)
        }
    };

    // 3. Inject the result into the transcript (brief lock).
    let updates = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        match &mut *guard {
            AppState::Live(core) => core
                .inject_shell_output(&session_id, &command, &output)
                .map_err(map_err)?,
            AppState::Mock(_) => {
                // Mock: produce a canned entry so the UI looks correct in dev mode.
                vec![]
            }
        }
    }; // lock released

    crate::events::emit_updates(&app, updates);
    Ok(())
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

/// Run a shell command locally in `project` (if it exists), capturing combined
/// stdout+stderr.
///
/// Uses `sh -c` on Unix and `cmd /C` on Windows. Falls back to an error
/// message if the subprocess cannot be spawned.
fn run_shell_locally(command: &str, project: &str) -> String {
    use std::process::Command;

    #[cfg(windows)]
    let (bin, flag) = ("cmd", "/C");
    #[cfg(not(windows))]
    let (bin, flag) = ("sh", "-c");

    let mut cmd = Command::new(bin);
    cmd.arg(flag).arg(command);
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
