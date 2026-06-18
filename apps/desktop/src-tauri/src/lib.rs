//! D01 — Tauri bridge over `marsad-core`.
//!
//! TODO(D01 code-agent): expose Tauri commands (launch/resume/attach/stop/
//! send_prompt/answer_gate/set_workspace/host+notif+settings ops) and an event
//! bridge that forwards `marsad_core::VmUpdate`s to the web UI. Wire window/tray/
//! notification integration. See apps/desktop/AGENTS.md and docs/specs/core.md.

pub fn run() {
    // tauri::Builder::default()...run(...) goes here.
}
