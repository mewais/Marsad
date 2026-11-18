//! Marsad desktop: a thin Tauri v2 shell over the local Marsad daemon (the hub).
//!
//! * On start: connect to the local daemon with the supervised `marsad-client`, which
//!   starts `marsad-agent serve --hub` on demand (readiness line). The daemon binary is the
//!   bundled `marsad-agent` next to the app binary (`$MARSAD_AGENT_BIN` overrides it);
//!   binaries for other targets come from `$MARSAD_AGENT_BIN_DIR`, which the daemon reads.
//!   `$MARSAD_DATA` overrides the data directory, `$MARSAD_DAEMON_ARGS` (whitespace-separated)
//!   adds `serve` options — both for development and tests.
//! * The webview gets exactly the generic bridges of [`bridge`]: `rpc`, `rpc_subscribe`,
//!   `rpc_unsubscribe`, `connection_state` (+ the `connection` event). No per-feature
//!   commands; `desktop.*` methods of `rpc` are the native extras (open/reveal/pick folder).
//! * Native: OS notifications from the fleet and the stored preferences ([`watch`]), a tray
//!   item with the attention count ([`tray`]), single instance, window state persistence.

#![forbid(unsafe_code)]

pub mod bridge;
pub mod connection;
pub mod native;
pub mod notify;
pub mod smoke;
pub mod tray;
pub mod watch;

use std::path::PathBuf;

use marsad_client::{ClientConfig, MarsadClient};
use tauri::{Manager, RunEvent, WindowEvent};

fn client_config() -> ClientConfig {
    let data = std::env::var_os("MARSAD_DATA")
        .map(PathBuf::from)
        .or_else(default_data_dir)
        .unwrap_or_else(|| PathBuf::from(".marsad"));
    let mut cfg = ClientConfig::new(
        data,
        wasit_control::Hello::new(
            concat!("marsad-desktop/", env!("CARGO_PKG_VERSION")),
            "marsad-desktop",
            "gui",
        ),
    );
    if let Ok(args) = std::env::var("MARSAD_DAEMON_ARGS") {
        cfg.serve_args = args.split_whitespace().map(String::from).collect();
    }
    cfg
}

fn default_data_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Marsad"))
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".marsad"))
    }
}

pub fn run() {
    // Only fails if a subscriber is already set.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("MARSAD_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();
    let app = tauri::Builder::default()
        // First: a second launch focuses the running app instead.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            watch::bring_forward(app);
        }))
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(watch::FocusTarget::default())
        .setup(|app| {
            let handle = app.handle().clone();
            // `connect` returns at once; it needs the runtime to spawn its supervisor.
            let client =
                tauri::async_runtime::block_on(async { MarsadClient::connect(client_config()) });
            app.manage(bridge::Shell::new(client, handle.clone()));
            if let Err(e) = tray::create(&handle) {
                tracing::warn!(error = %e, "no tray icon on this system");
            }
            tauri::async_runtime::spawn(watch::run(handle));
            Ok(())
        })
        .on_page_load(|webview, payload| {
            if payload.event() == tauri::webview::PageLoadEvent::Finished {
                smoke::on_page_load(webview);
            }
        })
        .on_window_event(|window, ev| {
            if let WindowEvent::Focused(true) = ev {
                watch::navigate_pending(window.app_handle());
            }
        })
        .invoke_handler(tauri::generate_handler![
            bridge::rpc,
            bridge::rpc_subscribe,
            bridge::rpc_unsubscribe,
            bridge::connection_state,
            smoke::smoke_report,
        ])
        .build(tauri::generate_context!());
    match app {
        Ok(app) => app.run(|_app, ev| {
            if let RunEvent::Exit = ev {
                tracing::info!("exiting (the daemon and its sessions keep running)");
            }
        }),
        Err(e) => {
            eprintln!("marsad-desktop: cannot start: {e}");
            std::process::exit(1);
        }
    }
}
