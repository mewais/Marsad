//! Session lifecycle IPC arms - launch, resume, attach, stop.
//!
//! Extracted from ipc.rs verbatim; impl Agent is reopened here and
//! `use super::*` brings Agent/AgentError. Handlers are pub(super) so the
//! handle_ipc dispatcher and sibling arms can call them.

use super::*;

impl Agent {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn handle_launch_session(
        &mut self,
        wire_session_id: Option<String>,
        project: String,
        host_id: Option<String>,
        host_target: Option<String>,
        claude_bin: Option<String>,
        setup: Option<String>,
        env: Vec<(String, String)>,
        model: String,
        posture: marsad_proto::Posture,
        first_prompt: Option<String>,
        mode: marsad_proto::LaunchMode,
        name: Option<String>,
        disallowed_tools: Vec<String>,
        // X4a (docs/specs/wasl-connector.md §X4a): forwarded verbatim from the
        // wire so a GUI/core sender can request the agent inject a Wasl
        // MCP-server entry at launch time (`claude_argv`'s `--mcp-config`).
        wasl: bool,
        // Stage 1 codex launch: driver discriminator.  Forwarded verbatim from
        // the wire (`IpcMsg::LaunchSession.driver`); defaults to `"claude"` on
        // old wire messages (handled by `default_driver` in marsad-proto).
        driver: String,
    ) -> Result<Vec<String>, AgentError> {
        let spec = marsad_proto::LaunchSpec {
            project,
            host_id: host_id.unwrap_or_else(|| "local".to_string()),
            model,
            posture,
            first_prompt,
            // Fresh / continue / resume-by-id, from optional IPC fields.
            mode,
            host_target,
            claude_bin,
            setup,
            env,
            name,
            disallowed_tools,
            wasl,
            driver,
        };
        // Reuse the caller's session id when supplied (Core generates it so it can
        // navigate immediately and recognise the session's events); otherwise
        // generate one (e.g. a raw client that omits it).
        let session_id = match wire_session_id.filter(|s| !s.is_empty()) {
            Some(id) => self.launch_with_id(id, spec)?,
            None => self.launch(spec)?,
        };
        let event = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::SessionLaunched { session_id });
        Ok(vec![event])
    }

    /// Resume a Marsad-tracked session by its Marsad id.
    ///
    /// Looks the session up in the persistence store, then launches a fresh
    /// container that reattaches to the underlying claude conversation:
    /// `claude --resume <claude_session_id>` when that id is known, else
    /// `claude --continue` (most recent conversation in the project dir).
    /// An optional `first_prompt` is delivered after reconnect.
    pub(super) fn handle_resume_session(
        &mut self,
        session_id: String,
        first_prompt: Option<String>,
    ) -> Result<Vec<String>, AgentError> {
        // Already-live guard (the double-launch fix). If this session has a LIVE
        // reader — reconnected at startup, or never stopped — a resume MUST NOT
        // relaunch: `launch_with_id` re-runs the durable script
        // (`rm -f in.fifo; mkfifo in.fifo`), clobbering the FIFO the existing
        // reader is attached to. That spawns a SECOND claude + reader fighting
        // over one session dir (the duplicate-reader / duplicate-gate / stray-
        // interrupt flakiness).
        //
        // The signal is `live_readers`, NOT `self.sessions`: `self.sessions` is
        // only cleared by an explicit Stop, so a session whose reader gave up on a
        // connection-drop/liveness pause stayed in `self.sessions` and this guard
        // no-op'd EVERY resume — never re-spawning a reader, stranding the next
        // prompt in the pending-prompt queue until a full restart. `live_readers`
        // is cleared when the reader gives up, so a paused session's resume falls
        // through to relaunch (re-spawning the reader, which drains the queue)
        // while a genuinely-attached session still no-ops. It is populated
        // synchronously by `launch_with_id` and the startup reconnect BEFORE any
        // command is processed, so it is authoritative here. The serve loop
        // suppresses the duplicate reader spawn for the no-op case too.
        // (`first_prompt` on an already-live session is intentionally dropped —
        // a running session takes prompts via SendPrompt, not a resume.)
        if self.live_readers.lock().unwrap().contains(&session_id) {
            eprintln!(
                "marsad-agent: resume {session_id} ignored — already live (no relaunch, no duplicate reader)"
            );
            let _ = first_prompt;
            return Ok(vec![marsad_proto::encode_ipc(
                &marsad_proto::IpcMsg::SessionResumed { session_id },
            )]);
        }

        // Look the session up in the store to recover its project + model.
        let restored = self
            .store
            .0
            .restore()
            .map_err(|e| AgentError::Lifecycle(format!("restore for resume: {e:?}")))?;
        let persisted = restored
            .sessions
            .into_iter()
            .find(|s| s.id == session_id)
            .ok_or_else(|| {
                AgentError::Protocol(format!("ResumeSession: unknown session {session_id}"))
            })?;

        // Resume by claude session id when we have one; otherwise continue the
        // most recent conversation in the project directory.
        let mode = match persisted.session_id.as_deref() {
            Some(uuid) if !uuid.is_empty() => marsad_proto::LaunchMode::Resume(uuid.to_string()),
            _ => marsad_proto::LaunchMode::Continue,
        };

        // Recover any custom name: PersistedSession.name is the marsad id when no
        // custom name was set (agent default), so treat it as None in that case.
        let persisted_name = if persisted.name != persisted.id {
            Some(persisted.name.clone())
        } else {
            None
        };
        let spec = marsad_proto::LaunchSpec {
            project: persisted.project,
            host_id: persisted.host_id,
            model: persisted.model,
            posture: persisted
                .posture
                .as_deref()
                .and_then(|s| {
                    serde_json::from_value::<marsad_proto::Posture>(serde_json::Value::String(
                        s.to_string(),
                    ))
                    .ok()
                })
                .unwrap_or(marsad_proto::Posture::Ask),
            first_prompt,
            mode,
            host_target: persisted.host_target,
            claude_bin: persisted.claude_bin,
            setup: persisted.setup,
            env: persisted.env,
            name: persisted_name,
            disallowed_tools: persisted.disallowed_tools,
            // `PersistedSession` carries no `wasl` field (out of X4's scope —
            // resume never re-attaches a Wasl MCP-server entry this cycle;
            // start a fresh `wasl:true` launch instead).
            wasl: false,
            // Stage 1: carry the persisted driver so a resume re-uses the same
            // agent binary. Defaults to "claude" for pre-Stage-1 records (the
            // `#[serde(default = "default_driver")]` on PersistedSession).
            driver: persisted.driver,
        };
        // Reuse the SAME marsad id so the resumed session re-attaches to the
        // existing UI row. Using a fresh id (the old bug) meant the relaunched
        // session's stream events were tagged with an id core never tracked, so
        // resume silently did nothing on the dashboard.
        let new_id = self.launch_with_id(session_id.clone(), spec)?;
        let event =
            marsad_proto::encode_ipc(&marsad_proto::IpcMsg::SessionResumed { session_id: new_id });
        Ok(vec![event])
    }

    pub(super) fn handle_attach_session(&mut self, session_id: &str) -> Result<Vec<String>, AgentError> {
        if !self.sessions.contains_key(session_id) {
            return Err(AgentError::Protocol(format!(
                "AttachSession: unknown session {session_id}"
            )));
        }
        let event = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::SessionAttached {
            session_id: session_id.to_string(),
        });
        Ok(vec![event])
    }

    pub(super) fn handle_stop_session(&mut self, session_id: String) -> Result<Vec<String>, AgentError> {
        // Kill the container and drop the live tracking entry. We do NOT require the
        // session to be in the live map: after an agent restart a session is persisted
        // but not yet re-tracked (the real cause of "stop did nothing" — the old code
        // returned an error and killed nothing). `kill_container` resolves the remote
        // host from the store, so the kill works either way. The persisted record is
        // KEPT so the session remains resumable.
        let container_name = format!("marsad-{session_id}");
        // Signal the reader BEFORE killing: the kill EOFs the stream, and a reader
        // that sees EOF races to emit conn_on_drop() → Reconnecting/Unstable. It
        // has no other way to tell a COMMANDED stop from an SSH drop, so the cancel
        // token IS that signal — set it first and the reader's pre-drop cancel check
        // (reader.rs) exits silently instead of painting a paused session
        // "reconnecting". Also stops a lingering reconnect-probe loop from backing
        // off for up to 30 min and stacking with a later Resume's reader (the
        // "14 readers for one session" churn on a flapping host).
        #[cfg(not(coverage))]
        crate::reader::cancel_reader(&session_id);
        self.kill_container(&session_id, &container_name);
        // WAVE 3b: a stopped session's pty (if the Terminal tab was ever
        // opened for it) must not outlive the session — kill + reap it too.
        // Same "kept for resumability" spirit as the container above: the
        // pty itself has no resume story (it's a plain shell, not claude),
        // so it is torn down unconditionally here.
        #[cfg(not(coverage))]
        crate::pty::teardown_session(&session_id, &self.pty_entries.0);
        self.sessions.remove(&session_id);
        // Clear the reader-liveness marker too: an explicit stop means no reader
        // is expected, so a later resume MUST relaunch (not no-op). The reader's
        // own give-up path also clears this, but that lags by up to a backoff
        // interval; removing it here makes a stop→resume deterministic.
        self.live_readers.lock().unwrap().remove(&session_id);
        let event = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::SessionStopped { session_id });
        Ok(vec![event])
    }
}

#[cfg(test)]
mod resume_liveness_tests {
    //! Regression tests for the pause/resume prompt-delivery bug: after a
    //! connection-drop/liveness pause the reader thread dies (de-registering its
    //! writer) but `self.sessions` stayed populated, so the resume guard no-op'd
    //! forever and the next prompt was stranded in `pending_prompts`. The guard
    //! now keys on reader LIVENESS (`live_readers`), not `self.sessions`.
    //!
    //! These drive the fake agent through the exact state transitions: launch
    //! marks the session reader-live; clearing `live_readers` (without touching
    //! `self.sessions`) reproduces the post-give-up state; a live entry
    //! reproduces a genuinely-attached session (the double-launch case).

    use crate::{Agent, AgentConfig, ListenSpec};
    use marsad_lifecycle::FakeContainer;
    use marsad_notify::CaptureNotifier;
    use marsad_store::{MemoryBackend, Store};

    fn agent() -> Agent {
        Agent::with_fakes(
            AgentConfig {
                listen: ListenSpec::Stdio,
                data_root: "/tmp/marsad-agent-resume-liveness-test".to_string(),
            },
            Box::new(FakeContainer::new()),
            Box::new(CaptureNotifier::new()),
            Box::new(Store::new(Box::new(MemoryBackend::new()))),
        )
        .expect("with_fakes")
    }

    fn launch(agent: &mut Agent, sid: &str) {
        let line = format!(
            r#"{{"type":"LaunchSession","session_id":"{sid}","project":"/p","host_id":"local","model":"claude-opus-4-8","posture":"Ask"}}"#
        );
        agent.handle_ipc(&line).expect("launch");
    }

    fn resume(agent: &mut Agent, sid: &str) {
        let line = format!(r#"{{"type":"ResumeSession","session_id":"{sid}"}}"#);
        agent.handle_ipc(&line).expect("resume");
    }

    /// THE BUG: a session whose reader gave up (liveness cleared) but is still
    /// tracked in `self.sessions` MUST relaunch on resume — otherwise no reader
    /// is ever re-spawned and the next prompt is stranded. Asserts a fresh
    /// container spawn happens on resume. RED against the old `self.sessions`
    /// guard (which no-op'd); GREEN once the guard keys on `live_readers`.
    #[test]
    fn resume_after_reader_giveup_relaunches() {
        let mut a = agent();
        launch(&mut a, "s1");
        let after_launch = a.container_calls().len();
        // Reproduce the post-give-up state: reader dead (liveness cleared) but the
        // session is still tracked (self.sessions unchanged — exactly the bug).
        a.live_readers.lock().unwrap().remove("s1");
        assert!(
            a.sessions.contains_key("s1"),
            "precondition: session still tracked after simulated give-up"
        );
        resume(&mut a, "s1");
        let after_resume = a.container_calls().len();
        assert!(
            after_resume > after_launch,
            "resume after reader give-up MUST relaunch (spawn a container) so a \
             reader re-attaches and drains queued prompts; container_calls \
             {after_launch} -> {after_resume} (no relaunch = the stranded-prompt bug)"
        );
    }

    /// ANTI-REGRESSION (double-launch guard): a session with a LIVE reader must
    /// NOT relaunch on resume — that would spawn a duplicate claude/reader
    /// fighting over one session dir. Asserts no new container spawn.
    #[test]
    fn resume_with_live_reader_is_noop() {
        let mut a = agent();
        launch(&mut a, "s2");
        let after_launch = a.container_calls().len();
        assert!(
            a.live_readers.lock().unwrap().contains("s2"),
            "precondition: launch marks the session reader-live"
        );
        resume(&mut a, "s2");
        assert_eq!(
            a.container_calls().len(),
            after_launch,
            "resume of a session with a live reader must be a no-op (no duplicate \
             reader/claude)"
        );
    }

    /// An explicit stop clears the liveness marker, so a later resume relaunches.
    /// Also proves `handle_stop_session` removes `live_readers` (without which the
    /// guard would no-op a stopped session's resume once it keys on liveness).
    #[test]
    fn stop_clears_liveness_then_resume_relaunches() {
        let mut a = agent();
        launch(&mut a, "s3");
        let after_launch = a.container_calls().len();
        let stop = r#"{"type":"StopSession","session_id":"s3"}"#;
        a.handle_ipc(stop).expect("stop");
        assert!(
            !a.live_readers.lock().unwrap().contains("s3"),
            "stop must clear the reader-liveness marker"
        );
        resume(&mut a, "s3");
        let after_resume = a.container_calls().len();
        assert!(
            after_resume > after_launch,
            "resume after an explicit stop must relaunch; container_calls \
             {after_launch} -> {after_resume}"
        );
    }
}
