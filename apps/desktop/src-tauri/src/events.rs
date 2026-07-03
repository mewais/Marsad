//! D01 — Event bridge: forwards marsad_core::VmUpdate payloads to the webview.
//!
//! The webview listens to the "vm-update" Tauri event and updates the React
//! store via the useVm hook (src/bridge/useVm.ts).

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use marsad_core::VmUpdate;
use marsad_viewmodel::{BoardView, CapabilitiesView, Host, OverviewGroups, SessionView, Workspace};
use marsad_workstream::Workstream;

// ---------------------------------------------------------------------------
// Wire-format envelope emitted as "vm-update"
// ---------------------------------------------------------------------------

/// JSON-serializable envelope matching the TS VmUpdateEvent union.
///
/// `Session` is the large, common variant (full `SessionView`). These envelopes
/// are built and immediately emitted to the webview, never stored in bulk, so
/// boxing the hot-path variant would add an allocation per event for no benefit.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum VmUpdateEnvelope {
    Overview { groups: OverviewGroups },
    Session { session: SessionView },
    SessionRemoved { id: String },
    Toast { icon: String, text: String },
    HostHealth { host_id: String, status: String },
    Hosts { hosts: Vec<Host> },
    Workspaces { workspaces: Vec<Workspace> },
    Workstreams { workstreams: Vec<Workstream> },
    Board { board: BoardView },
    /// U1/U2 (dynamic-capabilities.md §3.6) — one (driver, host) capabilities
    /// snapshot; the receiving side replaces (not merges) its `capabilities`
    /// slice entry for that key.
    Capabilities { capabilities: CapabilitiesView },
}

impl From<VmUpdate> for VmUpdateEnvelope {
    fn from(u: VmUpdate) -> Self {
        match u {
            VmUpdate::Overview(groups) => VmUpdateEnvelope::Overview { groups },
            VmUpdate::Session(sv) => VmUpdateEnvelope::Session { session: sv },
            VmUpdate::SessionRemoved { id } => VmUpdateEnvelope::SessionRemoved { id },
            VmUpdate::Toast { icon, text } => VmUpdateEnvelope::Toast { icon, text },
            VmUpdate::HostHealth(payload) => {
                let status = match payload.status {
                    Some(marsad_hosts::HostStatus::Connected) => "connected".to_string(),
                    Some(marsad_hosts::HostStatus::Connecting) => "connecting".to_string(),
                    Some(marsad_hosts::HostStatus::Reconnecting) => "reconnecting".to_string(),
                    Some(marsad_hosts::HostStatus::Unreachable) => "unreachable".to_string(),
                    None => "unknown".to_string(),
                };
                VmUpdateEnvelope::HostHealth { host_id: payload.host_id, status }
            }
            VmUpdate::Hosts(hosts) => VmUpdateEnvelope::Hosts { hosts },
            VmUpdate::Workspaces(workspaces) => VmUpdateEnvelope::Workspaces { workspaces },
            VmUpdate::Workstreams(workstreams) => VmUpdateEnvelope::Workstreams { workstreams },
            VmUpdate::Board(board) => VmUpdateEnvelope::Board { board },
            VmUpdate::Capabilities(capabilities) => VmUpdateEnvelope::Capabilities { capabilities },
        }
    }
}

// ---------------------------------------------------------------------------
// Public helper
// ---------------------------------------------------------------------------

/// Emit a list of VmUpdates to the webview as "vm-update" events.
///
/// Silently ignores emit errors (e.g. webview not yet attached).
pub fn emit_updates(app: &AppHandle, updates: Vec<VmUpdate>) {
    for update in updates {
        let envelope = VmUpdateEnvelope::from(update);
        let _ = app.emit("vm-update", envelope);
    }
}
