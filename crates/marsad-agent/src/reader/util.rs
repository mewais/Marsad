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
/// OWN exit signal (see `reconnect_action`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconnectAction {
    /// The connection dropped (transport error). The durable setsid session is
    /// presumed alive → re-attach with backoff.
    Reattach,
    /// The remote process is gone (the `tail --pid` reader exited cleanly) →
    /// finalize the session to Paused.
    Pause,
    /// AMBIGUOUS: `tail --pid` exited 0 with no captured transport error. On a
    /// clean local link that means the watched process died — but on WSL2/ssh a
    /// collapsed transport (`Cannot send after transport endpoint shutdown`) also
    /// exits `tail` 0 while claude is still alive. Don't guess: the caller must
    /// run an AUTHORITATIVE `remote_liveness_script` probe (`kill -0 claude.pid`)
    /// and map Dead→Pause, Live/Unreachable→Reattach. This is what stops a
    /// transport blip from being falsely Paused (which triggers a relaunch churn
    /// that can truncate a bridge-backed transcript).
    Probe,
}

/// Classify why a remote reader (`ssh … tail --pid=<pid> -f out.log`) stopped, from
/// the reader child's exit code plus whether its stderr showed a connection error:
/// - a captured connection error ("Connection reset", "closed by remote host", …)
///   → the CONNECTION dropped → `Reattach`.
/// - exit code 0, no connection error → AMBIGUOUS → `Probe`. `tail --pid` returns 0
///   both when the watched process dies AND when the ssh transport collapses
///   cleanly (WSL2's `Cannot send after transport endpoint shutdown` exits `tail` 0
///   without landing in the stderr conn-error set). Blindly Pausing here falsely
///   pauses a LIVE session → a relaunch churn that can truncate a bridge-backed
///   transcript, so the caller must resolve it with an authoritative
///   `remote_liveness_script` probe instead (Dead→Pause, Live/Unreachable→Reattach).
/// - any other/unknown exit → a transport problem, NOT proof of death → `Reattach`
///   (the durable session is presumed alive until a probe proves otherwise; a
///   genuinely-unreachable host keeps failing fast so the backoff escalates
///   Reconnecting → Disconnected, but it is NEVER falsely Paused).
///
/// The exit code no longer decides death on its own — it gates WHETHER to probe.
/// This keeps the authoritative check off the hot path (only the ambiguous clean
/// exit probes) while fixing the false-pause the exit-code-only heuristic caused.
pub fn reconnect_action(exit_code: Option<i32>, saw_conn_error: bool) -> ReconnectAction {
    if saw_conn_error {
        return ReconnectAction::Reattach;
    }
    match exit_code {
        Some(0) => ReconnectAction::Probe,
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

/// Return `true` if the given `AgentEvent` is a `ToolCallStarted` for a known
/// file-modifying tool.
///
/// The wasit migration made the reader operate over typed `AgentEvent`s rather
/// than raw claude stream-json Values, so the mid-turn Changes-probe trigger
/// (#4) now inspects the parser's `ToolCallStarted` events. Same tool set as
/// `assistant_has_file_modifying_tool_use` (claude names + codex `shell`/
/// `apply_patch` + opencode kinds), so the probe fires for every driver.
///
/// Pure logic (no IO), compiled in all build modes — covered by unit tests.
pub fn agent_event_is_file_modifying_tool(ev: &wasit_core::AgentEvent) -> bool {
    const FILE_TOOLS: &[&str] = &[
        "Bash",
        "Edit",
        "Write",
        "NotebookEdit",
        "MultiEdit",
        "shell",
        "apply_patch",
    ];
    match ev {
        wasit_core::AgentEvent::ToolCallStarted { name, .. } => {
            FILE_TOOLS.contains(&name.as_str())
        }
        _ => false,
    }
}

#[cfg(test)]
mod agent_event_file_tool_tests {
    use super::agent_event_is_file_modifying_tool;
    use wasit_core::AgentEvent;

    fn tool_started(name: &str) -> AgentEvent {
        AgentEvent::ToolCallStarted {
            name: name.to_string(),
            call_id: "c1".to_string(),
            input: serde_json::Value::Null,
            is_subagent: false,
            subagent_model: None,
        }
    }

    #[test]
    fn bash_is_file_modifying() {
        assert!(agent_event_is_file_modifying_tool(&tool_started("Bash")));
    }

    #[test]
    fn codex_shell_and_apply_patch_are_file_modifying() {
        assert!(agent_event_is_file_modifying_tool(&tool_started("shell")));
        assert!(agent_event_is_file_modifying_tool(&tool_started(
            "apply_patch"
        )));
    }

    #[test]
    fn read_is_not_file_modifying() {
        assert!(!agent_event_is_file_modifying_tool(&tool_started("Read")));
    }

    #[test]
    fn non_tool_events_are_not_file_modifying() {
        assert!(!agent_event_is_file_modifying_tool(
            &AgentEvent::AssistantDelta {
                text: "hi".to_string(),
                message_id: None,
            }
        ));
        assert!(!agent_event_is_file_modifying_tool(&AgentEvent::TurnStarted {
            native_turn_id: None,
        }));
    }
}
