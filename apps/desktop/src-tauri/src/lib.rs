//! Marsad desktop: a thin Tauri v2 shell over the local Marsad daemon (the hub).
//!
//! * On start: connect to the local daemon with the supervised `marsad-client`, which
//!   starts `marsad-agent serve --hub` on demand (readiness line). The daemon binary is the
//!   bundled `marsad-agent` next to the app binary (`$MARSAD_AGENT_BIN` overrides it);
//!   binaries for other targets come from `$MARSAD_AGENT_BIN_DIR`, which the daemon reads;
//!   unset, the daemon the app starts gets `<resources>/agents` (where the release bundles
//!   ship `agents/<target-triple>/marsad-agent[.exe]`).
//!   `$MARSAD_DATA` overrides the data directory, `$MARSAD_DAEMON_ARGS` (whitespace-separated)
//!   adds `serve` options — both for development and tests.
//! * The webview gets exactly the generic bridges of [`bridge`]: `rpc`, `rpc_subscribe`,
//!   `rpc_unsubscribe`, `connection_state` (+ the `connection` event), `reconnect_daemon`. No per-feature
//!   commands; `desktop.*` methods of `rpc` are the native extras (open/reveal/pick folder,
//!   save an exported diagnostics file). The one more command, [`diag::diag_log`], is how the
//!   webview writes to the app's diagnostics log.
//! * Diagnostics ([`diag`]): the app log `<data>/diag/app.ndjson`, installed first thing.
//! * Native: OS notifications from the fleet and the stored preferences ([`watch`]), a tray
//!   item with the attention count ([`tray`]), single instance, window state persistence.

#![forbid(unsafe_code)]

pub mod bridge;
pub mod connection;
pub mod diag;
pub mod native;
pub mod notify;
pub mod smoke;
pub mod surface;
pub mod tray;
pub mod watch;

use std::path::PathBuf;

use marsad_client::{ClientConfig, MarsadClient};
use tauri::{Manager, RunEvent, WindowEvent};

/// Where the daemon finds `marsad-agent` builds for other targets: `$MARSAD_AGENT_BIN_DIR`
/// when set (the daemon reads it itself, so nothing is added), else the bundle's
/// `<resources>/agents`.
fn agent_bin_dir_env(
    from_env: Option<std::ffi::OsString>,
    resource_dir: Option<PathBuf>,
) -> Option<(String, String)> {
    if from_env.is_some_and(|v| !v.is_empty()) {
        return None;
    }
    // Tauri's resource dir is a `\\?\` verbatim path on Windows; pass the plain spelling on.
    let dir = wasit_process::plain_path(&resource_dir?.join("agents"));
    Some((
        "MARSAD_AGENT_BIN_DIR".into(),
        dir.to_string_lossy().into_owned(),
    ))
}

/// The data root: `$MARSAD_DATA`, else the platform's default.
fn data_root() -> PathBuf {
    std::env::var_os("MARSAD_DATA")
        .map(PathBuf::from)
        .or_else(default_data_dir)
        .unwrap_or_else(|| PathBuf::from(".marsad"))
}

fn client_config(resource_dir: Option<PathBuf>) -> ClientConfig {
    let mut cfg = ClientConfig::new(
        data_root(),
        wasit_control::Hello::new(
            concat!("marsad-desktop/", env!("CARGO_PKG_VERSION")),
            "marsad-desktop",
            "gui",
        ),
    );
    if let Ok(args) = std::env::var("MARSAD_DAEMON_ARGS") {
        cfg.serve_args = args.split_whitespace().map(String::from).collect();
    }
    cfg.daemon_env.extend(agent_bin_dir_env(
        std::env::var_os("MARSAD_AGENT_BIN_DIR"),
        resource_dir,
    ));
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
    // First: the app log (and the panic hook), as soon as the data layout is known, so that
    // everything after it, a failed start included, is on record.
    let exports = match marsad_proto::layout::Layout::resolve(data_root(), None) {
        Ok(layout) => {
            diag::start(layout.diag());
            Some(layout.diag().join("exports"))
        }
        Err(e) => {
            // The daemon link fails for the same reason and says so in the window.
            eprintln!("marsad-desktop: no diagnostics: {e}");
            None
        }
    };
    diag::init_tracing();
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
        .manage(diag::ExportsDir(exports))
        .setup(|app| {
            let handle = app.handle().clone();
            // `connect` returns at once; it needs the runtime to spawn its supervisor.
            let cfg = client_config(app.path().resource_dir().ok());
            let client = tauri::async_runtime::block_on(async { MarsadClient::connect(cfg) });
            app.manage(bridge::Shell::new(client, handle.clone()));
            if let Err(e) = tray::create(&handle) {
                // Linux without an AppIndicator host has none; the app runs without it.
                diag::failed("tray_create", &e);
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
            bridge::reconnect_daemon,
            diag::diag_log,
            smoke::smoke_report,
        ])
        .build(tauri::generate_context!());
    match app {
        Ok(app) => app.run(|_app, ev| {
            if let RunEvent::Exit = ev {
                // The daemon and its sessions keep running.
                diag::emit(marsad_proto::diag::AppEvent::Exit {});
                diag::finish();
            }
        }),
        Err(e) => {
            eprintln!("marsad-desktop: cannot start: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_bin_dir_defaults_to_the_bundled_agents() {
        let res = PathBuf::from("res");
        assert_eq!(
            agent_bin_dir_env(None, Some(res.clone())),
            Some((
                "MARSAD_AGENT_BIN_DIR".into(),
                res.join("agents").to_string_lossy().into_owned()
            ))
        );
        // An explicit setting wins (the daemon inherits it); an empty one does not count.
        assert_eq!(
            agent_bin_dir_env(Some("/opt/agents".into()), Some(res.clone())),
            None
        );
        assert!(agent_bin_dir_env(Some("".into()), Some(res)).is_some());
        assert_eq!(agent_bin_dir_env(None, None), None);
    }
}
