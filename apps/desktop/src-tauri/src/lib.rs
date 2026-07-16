//! D01 — Tauri bridge over `marsad-core`.
//!
//! Registers all bridge commands as Tauri commands, seeds the managed state
//! (live Core or MockState depending on the MARSAD_MOCK env var), and starts
//! the Tauri event loop.

pub mod bridge;
pub mod events;
pub mod git;
pub mod mock;

use std::sync::Mutex;

use bridge::{AppState, ManagedState};

// ---------------------------------------------------------------------------
// Helpers for live mode
// ---------------------------------------------------------------------------

/// Resolve the agent socket path.
///
/// Priority:
///   1. $MARSAD_AGENT_ENDPOINT
///   2. <data_root>/agent.sock  where data_root = $MARSAD_DATA_ROOT or ~/.local/share/marsad
pub(crate) fn resolve_socket_path() -> String {
    if let Ok(ep) = std::env::var("MARSAD_AGENT_ENDPOINT") {
        if !ep.is_empty() {
            return ep;
        }
    }
    let data_root = resolve_data_root();
    format!("{data_root}/agent.sock")
}

/// Resolve the data root directory.
/// Append a diagnostic line to `<data_root>/desktop.log` (the GUI process has no
/// visible stderr on Windows, so eprintln is invisible). Best-effort; never fails.
pub(crate) fn diag_log(msg: &str) {
    let dr = resolve_data_root();
    let path = std::path::Path::new(&dr).join("desktop.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        use std::io::Write;
        let _ = writeln!(f, "{msg}");
    }
    eprintln!("{msg}");
}

fn resolve_data_root() -> String {
    if let Ok(dr) = std::env::var("MARSAD_DATA_ROOT") {
        if !dr.is_empty() {
            return dr;
        }
    }
    // Windows: %LOCALAPPDATA%\Marsad (HOME is usually unset on Windows).
    #[cfg(windows)]
    {
        if let Ok(la) = std::env::var("LOCALAPPDATA") {
            if !la.is_empty() {
                return format!("{la}\\Marsad");
            }
        }
    }
    std::env::var("HOME")
        .map(|h| format!("{h}/.local/share/marsad"))
        .unwrap_or_else(|_| "/var/lib/marsad".to_string())
}

/// Ensure the parent directory of the socket path exists.
fn ensure_socket_parent(socket_path: &str) {
    if let Some(parent) = std::path::Path::new(socket_path).parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
}

/// Try to locate the `marsad-agent` binary.
///
/// Search order:
///   1. $MARSAD_AGENT_BIN (explicit override)
///   2. Next to the current exe: <exe_dir>/marsad-agent
///   3. None — caller falls back to LoopbackFake
fn find_agent_bin() -> Option<std::path::PathBuf> {
    // Override via env var.
    if let Ok(bin) = std::env::var("MARSAD_AGENT_BIN") {
        if !bin.is_empty() {
            let p = std::path::PathBuf::from(&bin);
            if p.is_file() {
                return Some(p);
            }
            eprintln!("marsad-desktop: MARSAD_AGENT_BIN={bin} not found; trying fallbacks");
        }
    }
    // Sibling binary next to the current exe (".exe" on Windows, "" elsewhere).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(format!("marsad-agent{}", std::env::consts::EXE_SUFFIX));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Try to connect a `LocalIpc` to `socket_path`. Returns the connected
/// transport on success.
fn try_connect(socket_path: &str) -> Option<marsad_transport::LocalIpc> {
    use marsad_transport::Transport;
    let mut ipc = marsad_transport::LocalIpc::new(socket_path);
    match ipc.connect() {
        Ok(()) => Some(ipc),
        Err(_) => None,
    }
}

/// Spawn the agent daemon as a detached child process.
///
/// Passes the socket path and data root via environment variables so the
/// child resolves them the same way it always does.
fn spawn_agent(socket_path: &str, data_root: &str) {
    let bin = match find_agent_bin() {
        Some(b) => b,
        None => {
            eprintln!("marsad-desktop: marsad-agent binary not found — cannot spawn agent");
            return;
        }
    };

    eprintln!("marsad-desktop: spawning agent: {}", bin.display());

    // Route the detached agent's stdout+stderr to <data_root>/agent.log so remote
    // launch/spawn failures (ssh/wsl/tmux errors) are visible for diagnosis instead
    // of vanishing into /dev/null. Falls back to null if the file can't be opened.
    let _ = std::fs::create_dir_all(data_root);
    let log_path = std::path::Path::new(data_root).join("agent.log");
    let open_log = || {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map(std::process::Stdio::from)
            .unwrap_or_else(|_| std::process::Stdio::null())
    };

    let mut cmd = std::process::Command::new(&bin);
    cmd.env("MARSAD_AGENT_ENDPOINT", socket_path)
        .env("MARSAD_DATA_ROOT", data_root)
        .stdin(std::process::Stdio::null())
        .stdout(open_log())
        .stderr(open_log());
    hide_console(&mut cmd); // no console flash on Windows
    let result = cmd.spawn();

    match result {
        Ok(_child) => {
            // Intentionally drop the Child handle — we want the agent to run
            // independently.  The OS will keep it alive (it's a true daemon).
            eprintln!("marsad-desktop: agent spawned");
        }
        Err(e) => {
            eprintln!("marsad-desktop: failed to spawn agent: {e}");
        }
    }
}

/// Adopt a running agent if one is reachable, else spawn the shipped one.
///
/// We do NOT kill-then-spawn on every launch. The NSIS installer's
/// `NSIS_HOOK_PREINSTALL` already `taskkill`s the old agent before writing
/// files, so any agent still running at launch is guaranteed to be the current
/// build — killing it on every launch was needless churn (and tore down the
/// durable daemon that live sessions depend on). So: if we can already connect
/// to a running agent, adopt it; otherwise spawn the one shipped with this
/// build and retry.
///
/// Retries with a short backoff after spawning (up to ~2 s total).
/// Returns `None` if the agent cannot be reached — caller falls back to
/// LoopbackFake so the app still launches.
fn adopt_or_spawn_agent(socket_path: &str) -> Option<marsad_transport::LocalIpc> {
    // Adopt a pre-existing agent if one is already listening on the socket.
    if let Some(ipc) = try_connect(socket_path) {
        eprintln!("marsad-desktop: adopted running agent at {socket_path}");
        return Some(ipc);
    }

    // No agent running — spawn the one shipped with this build.
    let data_root = resolve_data_root();
    spawn_agent(socket_path, &data_root);

    // Retry loop: up to 20 × 100 ms = 2 s.
    let backoff = std::time::Duration::from_millis(100);
    for attempt in 1..=20 {
        std::thread::sleep(backoff);
        if let Some(ipc) = try_connect(socket_path) {
            eprintln!(
                "marsad-desktop: connected to agent at {socket_path} (attempt {attempt})"
            );
            return Some(ipc);
        }
    }

    eprintln!(
        "marsad-desktop: could not connect to agent at {socket_path} after 2 s — \
         falling back to LoopbackFake"
    );
    None
}

// ---------------------------------------------------------------------------
// Common builder configuration (plugins, tray, window events, commands)
// ---------------------------------------------------------------------------

/// Apply plugins, the tray icon, close-to-tray behaviour, and command
/// registration to a `Builder`.  Both the mock and live paths call this so
/// the two builder chains stay in sync.
fn configure(builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> {
    builder
        // ── Plugins ────────────────────────────────────────────────────────
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_notification::init())
        // ── Close-to-tray: hide the window instead of quitting ─────────────
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        // ── Invoke handler — both mock and live commands ────────────────────
        .invoke_handler(tauri::generate_handler![
            bridge::snapshot,
            bridge::launch_session,
            bridge::resume_session,
            bridge::attach_session,
            bridge::stop_session,
            bridge::remove_session,
            bridge::send_prompt,
            bridge::interrupt,
            bridge::answer_gate,
            bridge::answer_wasl_gate,
            bridge::answer_question,
            bridge::run_slash,
            bridge::set_workspace,
            bridge::capabilities,
            bridge::refresh_capabilities,
            bridge::get_settings,
            bridge::set_settings,
            bridge::get_autostart,
            bridge::set_autostart,
            bridge::add_host,
            bridge::update_host,
            bridge::remove_host,
            bridge::connect_host_interactive,
            bridge::list_wsl_distros,
            bridge::save_workspace,
            bridge::duplicate_workspace,
            bridge::delete_workspace,
            bridge::export_workspace,
            bridge::import_workspace,
            bridge::commit,
            bridge::restart_session,
            bridge::save_template,
            bridge::list_templates,
            bridge::run_shell,
            bridge::get_file_diff,
            bridge::create_workstream,
            bridge::delete_workstream,
            bridge::pause_workstream,
            bridge::resume_workstream,
            bridge::add_connection,
            bridge::remove_connection,
            bridge::create_profile,
            bridge::delete_profile,
            bridge::assign_session,
            bridge::set_active_profile,
            bridge::board,
            bridge::graph,
            bridge::cost_breakdown,
            bridge::pty_start,
            bridge::pty_input,
            bridge::pty_resize,
            bridge::pty_close,
        ])
}

// ---------------------------------------------------------------------------
// Tray icon setup (called inside .setup() for both modes)
// ---------------------------------------------------------------------------

/// Build and register the system-tray icon.
///
/// Menu:
///   • "Show Marsad" — shows and focuses the main window.
///   • "Quit"        — exits the application.
///
/// Left-clicking the tray icon also shows/focuses the main window.
fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::TrayIconBuilder;
    use tauri::Manager;

    let show_item = MenuItem::with_id(app, "show", "Show Marsad", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_item, &quit_item])?;

    TrayIconBuilder::new()
        .icon(app.default_window_icon().cloned().expect("app icon not found"))
        .menu(&menu)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "quit" => app.exit(0),
            "show" => {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.set_focus();
                }
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.set_focus();
                }
            }
        })
        .build(app)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// run()
// ---------------------------------------------------------------------------

pub fn run() {
    // Check MARSAD_MOCK at startup to decide live vs mock mode.
    let is_mock = std::env::var("MARSAD_MOCK").map(|v| v == "1").unwrap_or(false);

    if is_mock {
        // ── Mock mode ──────────────────────────────────────────────────────
        let managed: ManagedState = Mutex::new(AppState::Mock(mock::seeded_mock()));
        configure(tauri::Builder::default())
            .manage(managed)
            .setup(|app| {
                setup_tray(app)?;
                Ok(())
            })
            .run(tauri::generate_context!())
            .expect("error while running Tauri application");
        return;
    }

    // ── Live mode ──────────────────────────────────────────────────────────
    let socket_path = resolve_socket_path();
    let data_root = resolve_data_root();
    ensure_socket_parent(&socket_path);

    eprintln!("marsad-desktop: live mode — socket={socket_path}  data_root={data_root}");

    // Build the CoreConfig.
    let cfg = marsad_core::CoreConfig {
        agent_endpoint: socket_path.clone(),
        data_root: data_root.clone(),
    };

    // Try to connect to (or spawn) the agent.
    let primary_transport = adopt_or_spawn_agent(&socket_path);

    // Build the primary Core transport — use real LocalIpc if connected,
    // otherwise fall back to LoopbackFake so the app still launches.
    // Use BackgroundGit so the per-event git-changes probe never blocks the Core
    // mutex on a subprocess (which would freeze the whole UI intermittently).
    let core = match primary_transport {
        Some(ipc) => marsad_core::Core::with_git(
            cfg,
            Box::new(ipc),
            Box::new(git::BackgroundGit::new()),
        )
        .expect("Core::with_git should not fail"),
        None => {
            eprintln!("marsad-desktop: WARNING — running with LoopbackFake (no agent)");
            let (transport, _peer) = marsad_transport::LoopbackFake::pair();
            marsad_core::Core::with_git(cfg, Box::new(transport), Box::new(git::BackgroundGit::new()))
                .expect("Core::with_git should not fail with a loopback transport")
        }
    };

    let managed: ManagedState = Mutex::new(AppState::Live(core));

    // ── Build the Tauri app and spawn the event reader thread ──────────────
    //
    // The reader thread uses approach (a): its OWN second LocalIpc connection
    // to the same socket.  This cleanly separates SEND (primary Core transport)
    // from RECEIVE (dedicated reader connection).
    //
    // Because Core is !Send (non-Send transport inside), we cannot move Core
    // into another thread.  Instead the reader thread:
    //   1. Opens its own LocalIpc to the same socket and sends Subscribe.
    //   2. Loops: reads one line, then briefly locks the shared Mutex<AppState>
    //      to call core.on_agent_line(line), releases the lock, then emits
    //      any VmUpdates to the webview.
    //
    // AppState has `unsafe impl Send + Sync`, so Arc<Mutex<AppState>> (accessed
    // via app.state::<ManagedState>()) is safely accessible from the thread.

    let socket_path_for_reader = socket_path.clone();

    configure(tauri::Builder::default())
        .manage(managed)
        .setup(move |app| {
            setup_tray(app)?;

            // Load persisted hosts, workspaces, and sessions so the dashboard
            // shows them immediately (the initial `snapshot` call then includes
            // them). Sessions load LAST so host labels resolve, and so a full
            // app exit + relaunch restores the session list instead of coming
            // up empty.
            {
                use tauri::Manager;
                let (host_updates, ws_updates, session_updates, workstream_updates) =
                    match app.state::<ManagedState>().lock() {
                        Ok(mut guard) => match &mut *guard {
                            AppState::Live(core) => {
                                let hosts = core.load_hosts().unwrap_or_default();
                                let ws = core.load_workspaces().unwrap_or_default();
                                let sessions_res = core.load_sessions();
                                if let Err(e) = &sessions_res {
                                    diag_log(&format!("marsad-desktop: STARTUP load_sessions ERROR: {e:?}"));
                                }
                                let sessions = sessions_res.unwrap_or_default();
                                diag_log(&format!(
                                    "marsad-desktop: STARTUP load_sessions -> {} update(s); core snapshot has {} session(s)",
                                    sessions.len(),
                                    core.snapshot().sessions.len()
                                ));
                                // Rebuild the workstreams board AFTER sessions load:
                                // the redesigned dashboard is workstream-centric, and
                                // load_workstreams auto-assigns every restored session
                                // (with no membership) into a single-node workstream —
                                // so a full app exit + relaunch shows the restored
                                // (Stale/resumable) sessions on the Board instead of
                                // coming up empty. Must run after load_sessions so the
                                // restored sessions exist when memberships are built.
                                let workstreams = core.load_workstreams().unwrap_or_default();
                                {
                                    let b = core.board();
                                    diag_log(&format!(
                                        "marsad-desktop: STARTUP load_workstreams -> {} update(s); board has {} attention + {} all card(s); snapshot {} session(s)",
                                        workstreams.len(),
                                        b.attention.len(),
                                        b.all.len(),
                                        core.snapshot().sessions.len()
                                    ));
                                }
                                // Connect-time discovery (deliverable #4a): TRULY
                                // populate capabilities for every known host at
                                // startup — non-blocking fire-and-forget sends; the
                                // agent probes on background threads and broadcasts
                                // `CapabilitiesUpdate`s that the reader thread picks
                                // up. Done under the same lock as load_hosts so the
                                // registry is populated first.
                                core.refresh_capabilities_all_hosts();
                                (hosts, ws, sessions, workstreams)
                            }
                            AppState::Mock(_) => (vec![], vec![], vec![], vec![]),
                        },
                        Err(_) => (vec![], vec![], vec![], vec![]),
                    };
                events::emit_updates(&app.handle().clone(), host_updates);
                events::emit_updates(&app.handle().clone(), ws_updates);
                events::emit_updates(&app.handle().clone(), session_updates);
                events::emit_updates(&app.handle().clone(), workstream_updates);
            }

            let app_handle = app.handle().clone();
            std::thread::spawn(move || {
                run_reader_thread(app_handle, &socket_path_for_reader);
            });

            // Background SSH health-check worker — runs `ssh … echo ping` OFF the
            // Core lock so the UI never blocks on it (same discipline as git).
            let health_handle = app.handle().clone();
            std::thread::spawn(move || {
                run_host_health_worker(health_handle);
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Tauri application");
}

// ---------------------------------------------------------------------------
// Reader thread — 2nd connection, dedicated to receiving agent broadcasts
// ---------------------------------------------------------------------------

/// Body of the background reader thread.
///
/// Opens its own `LocalIpc` connection, sends `Subscribe`, then loops
/// reading agent event lines and forwarding them to the webview via
/// `emit_updates`.
///
/// Keeps track of the last-seen `Phase` for each session so OS notifications
/// are only fired when a session *transitions* into `Needs` or `Error`.
///
/// A recv error (agent gone, socket closed) causes the loop to exit
/// silently — the app continues running.
fn run_reader_thread(app: tauri::AppHandle, socket_path: &str) {
    use marsad_core::VmUpdate;
    use marsad_events::Phase;
    use marsad_transport::Transport;
    use std::collections::HashMap;
    use tauri::Manager;
    use tauri_plugin_notification::NotificationExt;

    // Try to open a 2nd connection for receiving broadcasts.
    let mut reader_ipc = marsad_transport::LocalIpc::new(socket_path);
    if let Err(e) = reader_ipc.connect() {
        eprintln!("marsad-desktop: reader thread: failed to connect to agent: {e}");
        return;
    }

    // Send Subscribe so the agent adds this connection to its broadcast list.
    // IpcMsg::Subscribe requires a session_id field; the agent ignores its value
    // and registers the connection for all broadcasts.
    let subscribe_line = serde_json::json!({
        "type": "Subscribe",
        "session_id": ""
    })
    .to_string();

    if let Err(e) = reader_ipc.send_line(&subscribe_line) {
        eprintln!("marsad-desktop: reader thread: failed to send Subscribe: {e}");
        return;
    }

    eprintln!("marsad-desktop: reader thread: subscribed to agent at {socket_path}");

    // Track the last-seen phase per session to detect transitions.
    // `None` means we haven't observed this session yet.
    let mut last_phase: HashMap<String, Phase> = HashMap::new();

    // Read agent lines and forward them.
    loop {
        match reader_ipc.recv_line() {
            Ok(Some(line)) => {
                // Skip empty lines silently.
                if line.trim().is_empty() {
                    continue;
                }

                // AgentEvent broadcasts carry session stream data; TranscriptBackfill
                // carries a resumed remote session's history; ChangesUpdate carries a
                // remote git-status probe; UsageUpdate carries the project-wide
                // per-model token aggregation. All go to core. The agent also sends
                // command acks (Subscribed, SessionLaunched, …) on this subscriber
                // connection — skip those silently rather than feeding them to
                // on_agent_line (which would log an "unknown IPC type").
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                    let ty = v.get("type").and_then(|t| t.as_str());

                    // WAVE 3b (pty escape-hatch terminal): `PtyOutput`/`PtyExit`
                    // are agent-originated but have no `Core::on_agent_line` arm
                    // (marsad-core was deliberately not touched by this wave —
                    // raw terminal bytes have no session/business meaning for it
                    // to reduce). Intercept them here — BEFORE the allow-list
                    // check below would `continue`-skip them as an unrecognized
                    // type — and forward straight to the webview on the
                    // dedicated `"pty"` event (see `events::emit_pty_update`'s
                    // doc comment for why not `"vm-update"`).
                    if ty == Some("PtyOutput") {
                        if let (Some(session_id), Some(data)) = (
                            v.get("session_id").and_then(|s| s.as_str()),
                            v.get("data").and_then(|s| s.as_str()),
                        ) {
                            events::emit_pty_update(
                                &app,
                                events::VmUpdateEnvelope::PtyOutput {
                                    session_id: session_id.to_string(),
                                    terminal_id: v
                                        .get("terminal_id")
                                        .and_then(|s| s.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    data: data.to_string(),
                                },
                            );
                        }
                        continue;
                    }
                    if ty == Some("PtyExit") {
                        if let (Some(session_id), Some(code)) = (
                            v.get("session_id").and_then(|s| s.as_str()),
                            v.get("code").and_then(|c| c.as_i64()),
                        ) {
                            events::emit_pty_update(
                                &app,
                                events::VmUpdateEnvelope::PtyExit {
                                    session_id: session_id.to_string(),
                                    terminal_id: v
                                        .get("terminal_id")
                                        .and_then(|s| s.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    code: code as i32,
                                },
                            );
                        }
                        continue;
                    }

                    // Forward every agent→core broadcast that `Core::on_agent_line`
                    // reduces into a VmUpdate. CapabilitiesUpdate (model/session
                    // discovery, incl. the subscribe-replay) and the Wasl gate/
                    // observation broadcasts were previously DROPPED here — which
                    // silently starved the model picker ("No models observed yet")
                    // and the Wasl surfaces. SessionLiveness (Resumed↔Paused truth,
                    // incl. the subscribe-replay) was ALSO dropped here — which left
                    // a session that died on its host stuck showing Working/Resumed
                    // and silently queuing prompts into a dead session. Acks
                    // (Subscribed/SessionLaunched/CapabilitiesRefreshQueued/…) and
                    // pty (handled above) stay skipped.
                    if ty != Some("AgentEvent")
                        && ty != Some("TranscriptBackfill")
                        && ty != Some("ChangesUpdate")
                        && ty != Some("UsageUpdate")
                        && ty != Some("CapabilitiesUpdate")
                        && ty != Some("SessionLiveness")
                        && ty != Some("SessionConn")
                        && ty != Some("WaslGateRequested")
                        && ty != Some("WaslGateDecided")
                        && ty != Some("WaslObservation")
                    {
                        continue;
                    }
                }

                // Lock the shared Core to process the inbound line.
                let updates = {
                    let state_ref = app.state::<ManagedState>();
                    let mut guard = match state_ref.lock() {
                        Ok(g) => g,
                        Err(e) => {
                            eprintln!("marsad-desktop: reader thread: lock poisoned: {e}");
                            return;
                        }
                    };
                    match &mut *guard {
                        AppState::Live(core) => match core.on_agent_line(&line) {
                            Ok(updates) => updates,
                            Err(e) => {
                                // Protocol errors (unknown session, unknown type, etc.)
                                // are non-fatal — log and continue.
                                eprintln!(
                                    "marsad-desktop: reader thread: on_agent_line error: {e}"
                                );
                                vec![]
                            }
                        },
                        AppState::Mock(_) => vec![],
                    }
                }; // lock released here

                // Fire OS notifications for sessions that transition to Needs/Error.
                for update in &updates {
                    if let VmUpdate::Session(sv) = update {
                        let new_phase = &sv.state;
                        let session_id = sv.id.clone();
                        let session_name = sv.name.clone();

                        let prev = last_phase.get(&session_id);

                        let should_notify = match new_phase {
                            Phase::Needs | Phase::Error => {
                                // Only notify on a genuine TRANSITION into a
                                // needs-attention state (seen before in a
                                // different phase). The first time we observe a
                                // session we just record its baseline — avoids a
                                // burst of notifications every time the app
                                // (re)connects to already-pending sessions.
                                prev.is_some() && prev != Some(new_phase)
                            }
                            _ => false,
                        };

                        // Update the tracked phase unconditionally.
                        last_phase.insert(session_id.clone(), new_phase.clone());

                        if should_notify {
                            let body = match new_phase {
                                Phase::Needs => {
                                    format!("{session_name} needs your input")
                                }
                                Phase::Error => {
                                    format!("{session_name} encountered an error")
                                }
                                _ => unreachable!(),
                            };
                            let _ = app
                                .notification()
                                .builder()
                                .title("Marsad")
                                .body(&body)
                                .show();
                        }
                    }
                }

                // DIAG (#1 vanishing sessions): log any Board update the reader
                // forwards post-startup — a later empty/partial board would clobber
                // the correct startup board in the frontend.
                for update in &updates {
                    if let VmUpdate::Board(b) = update {
                        diag_log(&format!(
                            "marsad-desktop: reader Board update -> {} attention + {} all card(s) (from line: {})",
                            b.attention.len(),
                            b.all.len(),
                            line.chars().take(80).collect::<String>()
                        ));
                    }
                }

                events::emit_updates(&app, updates);
            }
            Ok(None) => {
                // Clean EOF — agent closed the connection.
                eprintln!("marsad-desktop: reader thread: agent closed the connection");
                break;
            }
            Err(e) => {
                eprintln!("marsad-desktop: reader thread: recv error: {e}");
                break;
            }
        }
    }

    eprintln!("marsad-desktop: reader thread: exiting");
}

// ---------------------------------------------------------------------------
// Host health-check worker — runs `ssh` OFF the Core lock
// ---------------------------------------------------------------------------

/// Periodically probes each non-local host over SSH and reports the result back
/// to Core. The `ssh` subprocess runs WITHOUT holding the Core mutex (only the
/// brief snapshot read + status write are locked), so checks never freeze the UI.
fn run_host_health_worker(app: tauri::AppHandle) {
    use tauri::Manager;

    // Snapshot the (id, target) of every remote host, locking only briefly.
    let remote_hosts = |app: &tauri::AppHandle| -> Vec<(String, String)> {
        match app.state::<ManagedState>().lock() {
            Ok(guard) => match &*guard {
                AppState::Live(core) => core
                    .snapshot()
                    .hosts
                    .into_iter()
                    .filter(|h| !h.is_local)
                    .map(|h| (h.id, h.target))
                    .collect(),
                AppState::Mock(_) => Vec::new(),
            },
            Err(_) => Vec::new(),
        }
    };

    // Give startup a moment (load_hosts + window) before the first sweep.
    std::thread::sleep(std::time::Duration::from_secs(2));
    loop {
        for (id, target) in remote_hosts(&app) {
            let status = host_health(&target); // subprocess — NO lock held here
            let updates = match app.state::<ManagedState>().lock() {
                Ok(mut guard) => match &mut *guard {
                    AppState::Live(core) => core.set_host_status(&id, status),
                    AppState::Mock(_) => vec![],
                },
                Err(_) => vec![],
            };
            events::emit_updates(&app, updates);
        }
        std::thread::sleep(std::time::Duration::from_secs(15));
    }
}

/// One SSH reachability probe. `ssh exit 0` → Connected; ssh ran but failed
/// (host down / auth / timeout) → Unreachable; couldn't spawn ssh → Reconnecting.
/// Suppress the console window that a spawned subprocess would otherwise flash on
/// Windows (e.g. the recurring `ssh` health probes). No-op on other platforms.
fn hide_console(cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

fn ssh_health(target: &str) -> marsad_core::HostStatus {
    use marsad_core::HostStatus;
    let mut cmd = std::process::Command::new("ssh");
    cmd.args([
        // `-T` disables PTY allocation. On Windows `ssh.exe` otherwise spins up a
        // ConPTY/conhost which flashes a console window every probe — CREATE_NO_WINDOW
        // alone does NOT suppress that. `-T` keeps the recurring health probe silent.
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=5",
        target,
        "echo ping",
    ])
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null());
    hide_console(&mut cmd);
    match cmd.status() {
        Ok(s) if s.success() => HostStatus::Connected,
        Ok(_) => HostStatus::Unreachable,
        Err(_) => HostStatus::Reconnecting,
    }
}

/// Parse the running-distro names out of `wsl.exe --list --running --quiet`
/// stdout. Pure (no IO) so it's unit-testable. wsl.exe emits UTF-16LE by
/// default (older builds) or UTF-8 when `WSL_UTF8=1` is honored; for ASCII
/// distro names both decode correctly if we drop the UTF-16 NUL padding, the
/// BOM bytes, and CRs before splitting on lines. Blank lines are ignored.
fn wsl_running_distro_names(raw: &[u8]) -> Vec<String> {
    let cleaned: Vec<u8> = raw
        .iter()
        .copied()
        .filter(|&b| b != 0x00 && b != 0xFF && b != 0xFE && b != b'\r')
        .collect();
    String::from_utf8_lossy(&cleaned)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Whether a WSL distro is currently RUNNING, WITHOUT launching it.
///
/// `wsl.exe --list --running --quiet` lists only already-running distros and
/// never auto-starts one. This is the crux of the fix: the old probe
/// `wsl.exe -d <distro> -- true` RESURRECTS a shut-down distro (WSL auto-starts
/// the VM on any `-d` command), so it could never observe a `wsl --shutdown`
/// and always read Connected. `None` means wsl.exe couldn't be run at all (not
/// Windows / not installed / `--list` failed) — the caller degrades to
/// Reconnecting rather than a false Unreachable.
fn wsl_distro_running(distro: &str) -> Option<bool> {
    let mut cmd = std::process::Command::new("wsl.exe");
    cmd.args(["--list", "--running", "--quiet"])
        // Ask newer wsl.exe to emit UTF-8; older builds ignore it and emit
        // UTF-16LE, which wsl_running_distro_names still decodes for ASCII names.
        .env("WSL_UTF8", "1")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    hide_console(&mut cmd);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let target = distro.trim();
    Some(
        wsl_running_distro_names(&out.stdout)
            .iter()
            .any(|name| name.eq_ignore_ascii_case(target)),
    )
}

/// One WSL reachability probe. WSL hosts are NOT ssh targets, so they must be
/// probed via `wsl.exe`, not `ssh`. Liveness is decided by whether the distro is
/// RUNNING (`--list --running`) — NOT by running a command inside it, which would
/// resurrect a `wsl --shutdown`-ed distro and mask the outage. Off Windows
/// `wsl.exe` is absent → Reconnecting.
fn wsl_health(distro: &str) -> marsad_core::HostStatus {
    use marsad_core::HostStatus;
    match wsl_distro_running(distro) {
        Some(true) => HostStatus::Connected,
        // Distro is stopped (e.g. `wsl --shutdown`, host reboot) — report it down
        // instead of silently relaunching it behind the user's back.
        Some(false) => HostStatus::Unreachable,
        // Couldn't run wsl.exe at all (not Windows / not installed).
        None => HostStatus::Reconnecting,
    }
}

/// One plink reachability probe: `plink -share -batch <inner> true`. PuTTY hosts
/// reach the SSH box via connection sharing — once the user has done the
/// interactive `plink -share <inner>` login (the sharing upstream), this `-share
/// -batch` downstream rides that upstream with no re-auth and reads Connected.
/// `-batch` disables interactive prompts so the probe fails cleanly (rather than
/// hanging) when no upstream exists yet. Probing plink hosts with `ssh.exe` would
/// always read Unreachable, so they must go through `plink`.
fn plink_health(inner: &str) -> marsad_core::HostStatus {
    use marsad_core::HostStatus;
    let mut cmd = std::process::Command::new("plink");
    cmd.args(["-share", "-batch", inner, "true"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    hide_console(&mut cmd);
    match cmd.status() {
        Ok(s) if s.success() => HostStatus::Connected,
        Ok(_) => HostStatus::Unreachable,
        // plink not installed / not on PATH → surface as Reconnecting (same as a
        // missing ssh/wsl binary), so the UI shows the host isn't reachable yet.
        Err(_) => HostStatus::Reconnecting,
    }
}

/// Probe a host's reachability, dispatching by target type: `wsl:<distro>` → WSL,
/// `plink:<inner>` → PuTTY plink connection sharing, everything else
/// (`user@host`, bare alias) → SSH.
fn host_health(target: &str) -> marsad_core::HostStatus {
    // `wslssh:<distro>:<target>` must be probed via `wsl ssh` (with the SAME
    // ControlMaster path the agent uses), NOT native ssh — otherwise it probes a
    // bogus literal host and never reads Connected even after the Duo login. Check
    // this BEFORE the `wsl:` prefix (note: `wslssh:` does not start with `wsl:`).
    if let Some(rest) = target.strip_prefix("wslssh:") {
        let mut parts = rest.splitn(2, ':');
        let distro = parts.next().unwrap_or("");
        let inner = parts.next().unwrap_or("");
        if !distro.is_empty() && !inner.is_empty() {
            return wslssh_health(distro, inner);
        }
        return marsad_core::HostStatus::Unreachable;
    }
    if let Some(distro) = target.strip_prefix("wsl:") {
        return wsl_health(distro);
    }
    if let Some(inner) = target.strip_prefix("plink:") {
        return plink_health(inner);
    }
    ssh_health(target)
}

/// Probe a WSL-routed SSH host: `wsl -d <distro> ssh -o BatchMode … <target> true`,
/// reusing the agent's ControlMaster path. Before the interactive Duo login there's
/// no master and BatchMode fails → Unreachable; once the user has logged in, the
/// shared master makes this succeed → Connected (same pattern as plink_health).
fn wslssh_health(distro: &str, target: &str) -> marsad_core::HostStatus {
    use marsad_core::HostStatus;
    // The ssh-through-wsl probe below would itself resurrect a shut-down distro
    // (any `wsl -d <distro> …` auto-starts the VM), masking a `wsl --shutdown`.
    // So first check the distro is genuinely RUNNING, without launching it. If
    // it's down, the whole wslssh route is unreachable regardless of the inner
    // ssh target.
    match wsl_distro_running(distro) {
        Some(false) => return HostStatus::Unreachable,
        None => return HostStatus::Reconnecting,
        Some(true) => {}
    }
    let mut cmd = std::process::Command::new("wsl.exe");
    cmd.args([
        "-d",
        distro,
        "ssh",
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=5",
        "-o",
        "ControlMaster=auto",
        "-o",
        "ControlPath=~/.ssh/marsad-cm-%r@%h:%p",
        target,
        "true",
    ])
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null());
    hide_console(&mut cmd);
    match cmd.status() {
        Ok(s) if s.success() => HostStatus::Connected,
        Ok(_) => HostStatus::Unreachable,
        Err(_) => HostStatus::Reconnecting,
    }
}

#[cfg(test)]
mod wsl_health_tests {
    use super::wsl_running_distro_names;

    #[test]
    fn parses_utf8_list() {
        // `WSL_UTF8=1` output: plain UTF-8, one distro per line.
        let raw = b"Ubuntu\nDebian\n";
        assert_eq!(wsl_running_distro_names(raw), vec!["Ubuntu", "Debian"]);
    }

    #[test]
    fn parses_utf16le_list_with_bom_and_cr() {
        // Legacy wsl.exe output: UTF-16LE with a BOM and CRLF line endings.
        // "Ubuntu\r\n" as UTF-16LE, prefixed with the 0xFF 0xFE BOM.
        let mut raw: Vec<u8> = vec![0xFF, 0xFE];
        for ch in "Ubuntu\r\n".chars() {
            raw.push(ch as u8);
            raw.push(0x00);
        }
        assert_eq!(wsl_running_distro_names(&raw), vec!["Ubuntu"]);
    }

    #[test]
    fn empty_when_no_distros_running() {
        // After `wsl --shutdown`, `--list --running` prints nothing (or blanks).
        assert!(wsl_running_distro_names(b"").is_empty());
        assert!(wsl_running_distro_names(b"\r\n\r\n").is_empty());
        // UTF-16LE of just a trailing newline decodes to no names either.
        assert!(wsl_running_distro_names(&[0xFF, 0xFE, 0x0A, 0x00]).is_empty());
    }
}
