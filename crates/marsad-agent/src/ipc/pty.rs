//! Embedded-terminal (pty) IPC arms - start, input, resize, close.
//!
//! Extracted from ipc.rs verbatim; impl Agent is reopened here and
//! `use super::*` brings Agent/AgentError. Handlers are pub(super) so the
//! handle_ipc dispatcher and sibling arms can call them.

use super::*;

impl Agent {
    /// WAVE 3b (pty escape-hatch terminal): validate the session exists.
    /// No ack (fire-and-forget, mirrors `SetWorkspace`) — the pty coming
    /// alive is observed via `PtyOutput` broadcasts. The actual spawn
    /// happens in `serve.rs` (see `pty::spawn_pty_session`'s doc comment for
    /// why it can't happen here).
    pub(super) fn handle_pty_start(
        &mut self,
        session_id: String,
        _terminal_id: String,
    ) -> Result<Vec<String>, AgentError> {
        // The terminal is a plain interactive shell that needs only the session's
        // project/host_target (resolved from the STORE in serve.rs) — NOT a live
        // claude process. So accept any session known to the store, not just one
        // currently adopted in the in-memory `sessions` map: a Stale/reconnected/
        // resumed session must still be able to open a terminal. Gating on the
        // in-memory map silently dropped PtyStart for those (black-screen bug).
        let known = self.sessions.contains_key(&session_id)
            || {
                #[cfg(not(coverage))]
                {
                    self.store
                        .0
                        .restore()
                        .map(|r| r.sessions.iter().any(|s| s.id == session_id))
                        .unwrap_or(false)
                }
                #[cfg(coverage)]
                {
                    false
                }
            };
        if !known {
            return Err(AgentError::Protocol(format!(
                "PtyStart: unknown session {session_id}"
            )));
        }
        eprintln!("marsad-agent: session {session_id} PtyStart accepted");
        Ok(vec![])
    }

    /// WAVE 3b: forward raw keystroke bytes to the session's live pty stdin.
    /// In production builds, looks up the pty registry (populated by
    /// `pty::spawn_pty_session`); under `cfg(coverage)` this is the fake
    /// no-op path (no registry exists there — see `PtyEntriesWrap`).
    pub(super) fn handle_pty_input(
        &mut self,
        session_id: &str,
        terminal_id: &str,
        data: &str,
    ) -> Result<Vec<String>, AgentError> {
        #[cfg(not(coverage))]
        {
            let key = crate::pty::pty_key(session_id, terminal_id);
            let entries = self.pty_entries.0.lock().unwrap();
            if let Some(entry_arc) = entries.get(&key) {
                let mut entry = entry_arc.lock().unwrap();
                match entry
                    .writer
                    .write_all(data.as_bytes())
                    .and_then(|_| entry.writer.flush())
                {
                    Ok(()) => {}
                    Err(e) => {
                        eprintln!("marsad-agent: session {session_id} pty input write FAILED: {e}")
                    }
                }
            } else {
                eprintln!(
                    "marsad-agent: session {session_id} PtyInput dropped — no live pty (start one first)"
                );
            }
        }
        #[cfg(coverage)]
        let (_, _) = (terminal_id, data);
        Ok(vec![])
    }

    /// WAVE 3b: resize a session's live pty viewport. Same real/coverage
    /// split as `handle_pty_input`.
    pub(super) fn handle_pty_resize(
        &mut self,
        session_id: &str,
        terminal_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<Vec<String>, AgentError> {
        #[cfg(not(coverage))]
        {
            let key = crate::pty::pty_key(session_id, terminal_id);
            let entries = self.pty_entries.0.lock().unwrap();
            if let Some(entry_arc) = entries.get(&key) {
                let entry = entry_arc.lock().unwrap();
                if let Err(e) = crate::pty::resize_pty(&entry, cols, rows) {
                    eprintln!("marsad-agent: session {session_id} pty resize FAILED: {e}");
                }
            }
        }
        #[cfg(coverage)]
        let (_, _, _) = (terminal_id, cols, rows);
        Ok(vec![])
    }

    /// v2: close (kill + reap) ONE terminal of a session. Fire-and-forget; the
    /// teardown surfaces to the UI via that terminal's `PtyExit` broadcast (the
    /// pump thread reaps and removes the entry). Idempotent.
    pub(super) fn handle_pty_close(
        &mut self,
        session_id: &str,
        terminal_id: &str,
    ) -> Result<Vec<String>, AgentError> {
        #[cfg(not(coverage))]
        {
            let key = crate::pty::pty_key(session_id, terminal_id);
            crate::pty::teardown_terminal(&key, &self.pty_entries.0);
        }
        #[cfg(coverage)]
        let _ = terminal_id;
        Ok(vec![])
    }
}
