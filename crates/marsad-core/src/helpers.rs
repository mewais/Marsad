//! Small pure helper functions used across the `Core` impl — timestamps, id
//! minting, display-name derivation, gate headlines, and posture/permission-mode
//! mappings. Extracted from `lib.rs` (behavior-identical); `use super::*` brings
//! the crate's imports (`Posture`, etc.) into scope.

use super::*;

/// Map the per-session connection health enum to the string the SessionView
/// carries to the UI ("Connected"/"Reconnecting"/"Unstable"/"Disconnected").
pub(crate) fn conn_health_str(h: marsad_proto::ConnHealth) -> String {
    match h {
        marsad_proto::ConnHealth::Connected => "Connected",
        marsad_proto::ConnHealth::Reconnecting => "Reconnecting",
        marsad_proto::ConnHealth::Unstable => "Unstable",
        marsad_proto::ConnHealth::Disconnected => "Disconnected",
    }
    .to_string()
}

/// Epoch milliseconds — the timestamp stamped on LIVE transcript entries as they
/// arrive (resumed/backfilled entries instead carry the JSONL's own time).
pub(crate) fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Simple placeholder for display; the real timestamp is the epoch seconds
    // encoded in the id for uniqueness.
    format!("{secs}")
}

pub(crate) fn new_session_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    // Include the launch time so ids are UNIQUE ACROSS RESTARTS. A pure counter
    // reset to 1 every process start, so each restart minted `session-…0001`
    // again — and the durable remote dir `~/.marsad/<id>` then collided with a
    // prior (possibly removed) session's, making old sessions appear to "come
    // back". The seconds prefix makes every launch's ids distinct.
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("session-{secs:x}-{n:x}")
}

/// Human-friendly session name: the project directory's last path component
/// (e.g. "/work/mewais/Marsad" -> "Marsad"). Falls back to the session id when
/// the project is empty or has no usable basename.
pub(crate) fn session_display_name(project: &str, id: &str) -> String {
    match project.trim_end_matches('/').rsplit('/').next() {
        Some(base) if !base.is_empty() => base.to_string(),
        _ => id.to_string(),
    }
}

/// Concise one-line summary of a pending gate, for the workstream board's
/// attention card (so a needs-you card shows WHAT is asked, not just the name).
pub(crate) fn gate_headline(gate: &marsad_gates::Gate) -> String {
    use marsad_gates::Gate;
    match gate {
        Gate::Permission { command, .. } => format!("Wants to run: {command}"),
        Gate::Plan { .. } => "Plan ready for your approval".to_string(),
        Gate::Edit { file, .. } => format!("Wants to edit {file}"),
        Gate::Question { questions, .. } => questions
            .first()
            .map(|q| q.question.clone())
            .unwrap_or_else(|| "Wants your input".to_string()),
        Gate::Handoff { summary, .. } => summary.clone(),
    }
}

/// Map a `permissionMode` string from the claude `system/init` event to the
/// canonical posture id used in `SessionView` and the Config tab.
pub(crate) fn permission_mode_to_posture(mode: Option<&str>) -> String {
    match mode {
        Some("acceptEdits") => "auto".to_string(),
        Some("bypassPermissions") => "full".to_string(),
        _ => "ask".to_string(),
    }
}

/// Map a persisted `Posture` variant name (stored in `PersistedSession.posture`)
/// back to the `permissionMode` string that `SessionState` / `make_session_view`
/// understand. Unknown → `None` (caller falls back to "ask").
pub(crate) fn posture_name_to_permission_mode(name: &str) -> Option<&'static str> {
    match name {
        "Ask" => Some("default"),
        "AutoApproveSafe" => Some("acceptEdits"),
        "FullAutonomy" => Some("bypassPermissions"),
        _ => None,
    }
}

pub(crate) fn posture_to_permission_mode(posture: &Posture) -> &'static str {
    match posture {
        Posture::Ask => "default",
        Posture::AutoApproveSafe => "autoApprove",
        Posture::FullAutonomy => "bypassPermissions",
    }
}

pub(crate) fn slash_cmd_name(cmd: &marsad_viewmodel::SlashCmd) -> &str {
    use marsad_viewmodel::SlashCmd;
    match cmd {
        SlashCmd::Clear => "clear",
        SlashCmd::Compact => "compact",
        SlashCmd::Cost => "cost",
        SlashCmd::Model => "model",
        SlashCmd::Config => "config",
        SlashCmd::Init => "init",
        SlashCmd::Review => "review",
        SlashCmd::Agents => "agents",
        SlashCmd::Resume => "resume",
        SlashCmd::Terminal => "terminal",
        SlashCmd::Help => "help",
        SlashCmd::Unknown(s) => s.as_str(),
    }
}

/// X6 (§8, v3): build the `marsad_gates::Gate::Permission` a native Wasl gate
/// maps onto. The real `WaslNativeGateDetail` is only `{summary, preview}` (no
/// `gate_id`); per-occurrence identity is the `(turn_id, seq)`-derived `gate_key`
/// (`native_gate_key`), passed in by the caller. The `command` encodes that
/// `gate_key` via `native_gate_command(gate_key, detail)` so the derived
/// `DecisionId` (which hashes `command` + `cwd`) is unique+stable per gate
/// occurrence; `explanation` carries the CLI's preview; the claude-only fields
/// are empty (a native permission prompt is a binary approve/deny with no
/// cwd/risk/elicitation/suggestions).
pub(crate) fn native_gate_permission(
    gate_key: &str,
    detail: &marsad_wasl_session::WaslNativeGateDetail,
) -> marsad_gates::Gate {
    marsad_gates::Gate::Permission {
        request_id: gate_key.to_string(),
        command: marsad_wasl_session::native_gate_command(gate_key, detail),
        cwd: String::new(),
        risk: String::new(),
        explanation: detail.preview.clone(),
        is_elicitation: false,
        elicitation_choices: Vec::new(),
        permission_suggestions: Vec::new(),
    }
}

/// X6 (§4.4, v3): peel every `WaslTurnReceipt::Duplicate { original }` layer and
/// return the underlying original receipt. A `Duplicate` is a benign resent
/// idempotency key — Wasl returns the ORIGINAL wrapped verbatim with no second
/// injection — so its disposition is exactly the original's. Any non-`Duplicate`
/// receipt is returned as-is.
pub(crate) fn unwrap_duplicate_receipt(
    receipt: &marsad_wasl_session::WaslTurnReceipt,
) -> &marsad_wasl_session::WaslTurnReceipt {
    let mut cur = receipt;
    while let marsad_wasl_session::WaslTurnReceipt::Duplicate { original } = cur {
        cur = original;
    }
    cur
}

/// X6 (§10.4): close out a Wasl turn's transcript entry exactly once. If a delta
/// already opened an assistant entry, leave it as-is (no double-close/reopen); if
/// none opened (the reply arrived only in the terminal event), append it now.
/// Marks the post-terminal guard so a stray later delta is ignored.
pub(crate) fn close_wasl_turn(record: &mut SessionRecord, reply: &str) {
    if record.wasl_open_assistant.is_none() && !reply.is_empty() {
        record
            .transcript
            .push(marsad_events::TranscriptEntry::Assistant {
                text: reply.to_string(),
                at: Some(now_ms()),
            });
    }
    record.wasl_open_assistant = None;
    record.wasl_turn_closed = true;
}
