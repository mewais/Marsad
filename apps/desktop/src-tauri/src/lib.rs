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

pub fn run() {
    // Check MARSAD_MOCK at startup to decide live vs mock mode.
    let is_mock = std::env::var("MARSAD_MOCK").map(|v| v == "1").unwrap_or(false);

    let managed: ManagedState = if is_mock {
        Mutex::new(AppState::Mock(mock::seeded_mock()))
    } else {
        // Live mode: build a Core backed by a LoopbackFake transport so the
        // app starts without panicking before a real agent connects.
        let cfg = marsad_core::CoreConfig {
            agent_endpoint: std::env::var("MARSAD_AGENT_ENDPOINT")
                .unwrap_or_else(|_| "localhost:9999".to_string()),
            data_root: std::env::var("MARSAD_DATA_ROOT")
                .unwrap_or_else(|_| "/tmp/marsad".to_string()),
        };
        let (transport, _peer) = marsad_transport::LoopbackFake::pair();
        let core = marsad_core::Core::new(cfg, Box::new(transport))
            .expect("Core::new should not fail with a loopback transport");
        Mutex::new(AppState::Live(core))
    };

    tauri::Builder::default()
        .manage(managed)
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running Tauri application");
}
