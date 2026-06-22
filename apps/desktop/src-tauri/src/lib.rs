//! D01 — Tauri bridge over `marsad-core`.
//!
//! Registers all bridge commands as Tauri commands, seeds the managed state
//! (live Core or MockState depending on the MARSAD_MOCK env var), and starts
//! the Tauri event loop.

pub mod bridge;
pub mod events;
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
fn resolve_socket_path() -> String {
    if let Ok(ep) = std::env::var("MARSAD_AGENT_ENDPOINT") {
        if !ep.is_empty() {
            return ep;
        }
    }
    let data_root = resolve_data_root();
    format!("{data_root}/agent.sock")
}

/// Resolve the data root directory.
fn resolve_data_root() -> String {
    if let Ok(dr) = std::env::var("MARSAD_DATA_ROOT") {
        if !dr.is_empty() {
            return dr;
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
    // Sibling binary next to the current exe.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("marsad-agent");
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

    let result = std::process::Command::new(&bin)
        .env("MARSAD_AGENT_ENDPOINT", socket_path)
        .env("MARSAD_DATA_ROOT", data_root)
        // Detach from the app's stdio so agent output doesn't pollute the desktop.
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

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

/// Connect to the agent, spawning it first if not running.
///
/// Retries with a short backoff after spawning (up to ~2 s total).
/// Returns `None` if the agent cannot be reached — caller falls back to
/// LoopbackFake so the app still launches.
fn adopt_or_spawn_agent(socket_path: &str) -> Option<marsad_transport::LocalIpc> {
    // First try: agent may already be running.
    if let Some(ipc) = try_connect(socket_path) {
        eprintln!("marsad-desktop: connected to existing agent at {socket_path}");
        return Some(ipc);
    }

    // Agent not running — try to spawn it.
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
            bridge::send_prompt,
            bridge::interrupt,
            bridge::answer_gate,
            bridge::run_slash,
            bridge::set_workspace,
            bridge::list_sessions,
            bridge::get_autostart,
            bridge::set_autostart,
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
    let core = match primary_transport {
        Some(ipc) => {
            marsad_core::Core::new(cfg, Box::new(ipc))
                .expect("Core::new should not fail")
        }
        None => {
            eprintln!("marsad-desktop: WARNING — running with LoopbackFake (no agent)");
            let (transport, _peer) = marsad_transport::LoopbackFake::pair();
            marsad_core::Core::new(cfg, Box::new(transport))
                .expect("Core::new should not fail with a loopback transport")
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

            let app_handle = app.handle().clone();
            std::thread::spawn(move || {
                run_reader_thread(app_handle, &socket_path_for_reader);
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

                // Only AgentEvent broadcasts carry session stream data. The agent
                // also sends command acks (Subscribed, SessionLaunched, …) on this
                // subscriber connection — skip those silently rather than feeding
                // them to on_agent_line (which would log an "unknown IPC type").
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                    if v.get("type").and_then(|t| t.as_str()) != Some("AgentEvent") {
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
