//! Pure reader decision-logic (no IO) — reconnect backoff, connection-health
//! state machine, remote-reader exit classification, and the mid-turn
//! file-modifying-tool detector. Compiled in ALL build modes and unit-tested
//! (unlike the IO thread in the parent module, which is coverage-excluded).

// ── Reconnect backoff constants and helper ───────────────────────────────────

/// Initial delay before the first liveness re-probe after a connection drop (seconds).
pub const RECONNECT_INITIAL_BACKOFF_SECS: u64 = 3;

/// Maximum delay between reattach attempts (seconds).
pub const RECONNECT_MAX_BACKOFF_SECS: u64 = 30;

/// Compute the next backoff delay: double the current value, capped at
/// `RECONNECT_MAX_BACKOFF_SECS`.
///
/// Pure logic (no IO) — compiled in all build modes and unit-tested.
pub fn reconnect_backoff_next(current: u64) -> u64 {
    current.saturating_mul(2).min(RECONNECT_MAX_BACKOFF_SECS)
}

// ── Connection-health state machine (pure, unit-tested) ──────────────────────
//
// The reader emits a `ConnHealth` on real stream events. These two pure
// functions are the WHOLE decision logic — no flap counter driving state at
// attach, no verdict-string guessing. The reader just calls them and emits.

/// Map a session mode id to codex app-server `(approvalPolicy, sandbox)`.
///
/// Codex's available modes are not discoverable from the CLI, so for now every mode
/// id (including empty = backend default) maps to codex's sane default settings.
/// // TODO(modes): source codex's real modes from the app-server when wired; until
/// // then codex launches at this default and mode picking is disabled for codex.
pub fn codex_mode_settings(_mode: &str) -> (&'static str, &'static str) {
    ("on-request", "workspace-write")
}

/// Health to emit when the stream DROPS. A drop right after a bare re-attach that
/// never produced a line (`lines_seen == 0`) is a FLAP → `Unstable`; a drop from a
/// connection that was actually working → a plain `Reconnecting` blip.
pub fn conn_on_drop(lines_seen: u64) -> marsad_proto::ConnHealth {
    if lines_seen == 0 {
        marsad_proto::ConnHealth::Unstable
    } else {
        marsad_proto::ConnHealth::Reconnecting
    }
}

/// What to do when a remote reader's stream ends — decided from the reader child's
/// OWN exit signal, not a separate fixed-timeout probe (see `reconnect_action`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconnectAction {
    /// The connection dropped (transport error). The durable setsid session is
    /// presumed alive → re-attach with backoff.
    Reattach,
    /// The remote process is gone (the `tail --pid` reader exited cleanly) →
    /// finalize the session to Paused.
    Pause,
}

/// Classify why a remote reader (`ssh … tail --pid=<pid> -f out.log`) stopped, from
/// the reader child's exit code plus whether its stderr showed a connection error:
/// - a captured connection error ("Connection reset", "closed by remote host", …)
///   → the CONNECTION dropped → `Reattach`.
/// - exit code 0, no connection error → the remote command (`tail --pid`) exited
///   because the watched process DIED → `Pause`.
/// - any other/unknown exit → a transport problem, NOT proof of death → `Reattach`
///   (the durable session is presumed alive until a clean exit proves otherwise; a
///   genuinely-unreachable host keeps failing fast so the backoff escalates
///   Reconnecting → Disconnected, but it is NEVER falsely Paused).
///
/// This replaces gating re-attach on a fixed-timeout `probe_liveness`, which timed
/// out on slow hosts (ETX's SSH round-trip is 4–26s vs an 8s probe) and stranded
/// live sessions on `Disconnected`.
pub fn reconnect_action(exit_code: Option<i32>, saw_conn_error: bool) -> ReconnectAction {
    if saw_conn_error {
        return ReconnectAction::Reattach;
    }
    match exit_code {
        Some(0) => ReconnectAction::Pause,
        _ => ReconnectAction::Reattach,
    }
}

// ── assistant_has_file_modifying_tool_use ────────────────────────────────────

/// Return `true` if the given `assistant` stream event contains at least one
/// `tool_use` content block whose tool name is a known file-modifying tool.
///
/// Used by `spawn_session_reader` (#4) to decide whether to trigger a debounced
/// remote git Changes probe BEFORE the turn ends, so the Changes tab updates
/// while claude is mid-turn writing files rather than only at `result`.
///
/// Pure logic (no IO), compiled in all build modes — covered by unit tests.
pub fn assistant_has_file_modifying_tool_use(event: &serde_json::Value) -> bool {
    // File-modifying tools whose use signals a likely git-visible change.
    // claude tool names + codex app-server's (shell / apply_patch) + opencode kinds,
    // so the mid-turn Changes probe fires for every driver.
    const FILE_TOOLS: &[&str] =
        &["Bash", "Edit", "Write", "NotebookEdit", "MultiEdit", "shell", "apply_patch"];
    let content = match event
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    {
        Some(arr) => arr,
        None => return false,
    };
    content.iter().any(|block| {
        block.get("type").and_then(|t| t.as_str()) == Some("tool_use")
            && block
                .get("name")
                .and_then(|n| n.as_str())
                .map(|name| FILE_TOOLS.contains(&name))
                .unwrap_or(false)
    })
}
