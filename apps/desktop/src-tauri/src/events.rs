//! D01 — Event bridge: forwards marsad_core::VmUpdate payloads to the webview.
//!
//! The webview listens to the "vm-update" Tauri event and updates the React
//! store via the useVm hook (src/bridge/useVm.ts).

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use marsad_core::{VmUpdate, WaslGateView};
use marsad_viewmodel::{BoardView, CapabilitiesView, Host, OverviewGroups, SessionView, Workspace};
use marsad_wasl::AuditEntry;
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
    /// X3 (wasl-connector.md §5.2) — full pending Wasl-gate list; the receiver
    /// replaces its `waslGates` slice wholesale (Hosts-style semantics).
    WaslGates { gates: Vec<WaslGateView> },
    /// One Wasl handoff gate resolved — the receiver drops it from the pending
    /// list. `outcome`/`decided_by` are verbatim from the operator wire.
    WaslGateDecided {
        host_id: String,
        gate_id: String,
        outcome: String,
        decided_by: String,
        reason: Option<String>,
    },
    /// One Wasl observation-feed entry — appended to the activity surface.
    WaslObservation { host_id: String, entry: AuditEntry },
    /// WAVE 3b (pty escape-hatch terminal, `docs/design/DESIGN_HANDOFF.md`
    /// "Terminal tab"): a chunk of raw pty output. `data` is base64 —
    /// mirrors `marsad_proto::IpcMsg::PtyOutput`'s wire shape verbatim (see
    /// that type's doc comment for why plain text isn't safe here). NOT
    /// constructed via `From<VmUpdate>` (there is no `VmUpdate::Pty*` —
    /// pty bytes have no session/business meaning for `marsad-core` to
    /// reduce) and NOT emitted on the `"vm-update"` channel — see
    /// `emit_pty_update`'s doc comment for why this is a separate event.
    PtyOutput { session_id: String, data: String },
    /// The pty's child process exited. `code` is the process exit code
    /// (`-1` when the pty could not even be spawned).
    PtyExit { session_id: String, code: i32 },
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
            VmUpdate::WaslGates(gates) => VmUpdateEnvelope::WaslGates { gates },
            VmUpdate::WaslGateDecided { host_id, gate_id, outcome, decided_by, reason } => {
                VmUpdateEnvelope::WaslGateDecided { host_id, gate_id, outcome, decided_by, reason }
            }
            VmUpdate::WaslObservation { host_id, entry } => {
                VmUpdateEnvelope::WaslObservation { host_id, entry }
            }
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

/// WAVE 3b: emit a `PtyOutput`/`PtyExit` envelope on a SEPARATE `"pty"` Tauri
/// event, not `"vm-update"`.
///
/// This is deliberately NOT folded into `emit_updates`/`"vm-update"`: raw
/// terminal bytes have no session/business meaning for `marsad-core` to
/// reduce into a `VmUpdate` (unlike every other envelope arm above, which
/// mirrors a real `VmUpdate` variant), and mixing high-frequency keystroke
/// echo traffic into the same event the `useVm` reducer parses for
/// Board/Session/Workstream state would be both a layering violation and
/// unnecessary reducer churn. `run_reader_thread` (`lib.rs`) intercepts raw
/// `PtyOutput`/`PtyExit` agent lines BEFORE they would otherwise reach
/// `Core::on_agent_line` (which has no arm for either — `marsad-core` was
/// deliberately not touched by this wave) and calls this directly instead.
///
/// Silently ignores emit errors (e.g. webview not yet attached), same
/// disposition as `emit_updates`.
pub fn emit_pty_update(app: &AppHandle, envelope: VmUpdateEnvelope) {
    let _ = app.emit("pty", envelope);
}

#[cfg(test)]
mod tests {
    use super::*;

    // These assert the exact `kind` tags and field names the JS reducer matches
    // on (apps/desktop/src/bridge/useVm.ts VM_UPDATE arms). Adding a VmUpdate
    // variant without an envelope arm fails to compile; renaming a field here
    // silently breaks the UI — so pin the wire shape.

    #[test]
    fn wasl_gates_envelope_wire_shape() {
        let gate = WaslGateView {
            host_id: "narval".into(),
            gate_id: "g1".into(),
            kind: "consult".into(),
            from: "sessB".into(),
            summary: "asks to consult".into(),
            preview: "…".into(),
            requested_at: "t0".into(),
            deadline: "t1".into(),
        };
        let env = VmUpdateEnvelope::from(VmUpdate::WaslGates(vec![gate]));
        let v = serde_json::to_value(&env).unwrap();
        assert_eq!(v["kind"], "wasl_gates");
        assert_eq!(v["gates"][0]["host_id"], "narval");
        assert_eq!(v["gates"][0]["gate_id"], "g1");
        assert_eq!(v["gates"][0]["deadline"], "t1");
    }

    #[test]
    fn wasl_gate_decided_envelope_wire_shape() {
        let env = VmUpdateEnvelope::from(VmUpdate::WaslGateDecided {
            host_id: "narval".into(),
            gate_id: "g1".into(),
            outcome: "approved".into(),
            decided_by: "human".into(),
            reason: None,
        });
        let v = serde_json::to_value(&env).unwrap();
        assert_eq!(v["kind"], "wasl_gate_decided");
        assert_eq!(v["host_id"], "narval");
        assert_eq!(v["gate_id"], "g1");
        assert_eq!(v["outcome"], "approved");
        assert_eq!(v["decided_by"], "human");
    }

    #[test]
    fn wasl_observation_envelope_wire_shape() {
        let entry = AuditEntry {
            seq: 7,
            kind: "consult".into(),
            actor: "sessA".into(),
            targets: vec!["sessB".into()],
            summary: "consulted".into(),
            decision: None,
        };
        let env = VmUpdateEnvelope::from(VmUpdate::WaslObservation {
            host_id: "narval".into(),
            entry,
        });
        let v = serde_json::to_value(&env).unwrap();
        assert_eq!(v["kind"], "wasl_observation");
        assert_eq!(v["host_id"], "narval");
        assert_eq!(v["entry"]["seq"], 7);
        assert_eq!(v["entry"]["actor"], "sessA");
    }

    // WAVE 3b — pty envelope arms are constructed directly (no `VmUpdate`
    // counterpart to convert from), so these pin the `kind`/field wire shape
    // the same way the `From<VmUpdate>`-backed tests above do.

    #[test]
    fn pty_output_envelope_wire_shape() {
        let env = VmUpdateEnvelope::PtyOutput {
            session_id: "s-1".into(),
            data: "bHMgLWxhCg==".into(),
        };
        let v = serde_json::to_value(&env).unwrap();
        assert_eq!(v["kind"], "pty_output");
        assert_eq!(v["session_id"], "s-1");
        assert_eq!(v["data"], "bHMgLWxhCg==");
    }

    #[test]
    fn pty_exit_envelope_wire_shape() {
        let env = VmUpdateEnvelope::PtyExit {
            session_id: "s-1".into(),
            code: 0,
        };
        let v = serde_json::to_value(&env).unwrap();
        assert_eq!(v["kind"], "pty_exit");
        assert_eq!(v["session_id"], "s-1");
        assert_eq!(v["code"], 0);
    }
}
