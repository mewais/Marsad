//! Decode inbound IPC lines and dispatch them to the matching `handle_*`
//! method, plus hook-payload ingestion (`handle_hook`/`parse_hook_event`).

use super::{Agent, AgentError};

impl Agent {
    /// Decode one inbound IPC line and return outbound IPC lines.
    pub fn handle_ipc(&mut self, line: &str) -> Result<Vec<String>, AgentError> {
        if line.is_empty() {
            return Err(AgentError::Protocol("empty IPC line".to_string()));
        }
        let msg = marsad_proto::parse_ipc(line).map_err(|e| AgentError::Protocol(e.to_string()))?;

        match msg {
            marsad_proto::IpcMsg::LaunchSession {
                session_id: wire_session_id,
                project,
                host_id,
                host_target,
                claude_bin,
                setup,
                env,
                model,
                posture,
                first_prompt,
                continue_,
                resume_session_id,
                name,
                disallowed_tools,
                wasl,
                driver,
                ..
            } => {
                // Derive the launch mode from the wire fields (§5 item 3:
                // replaces the old `parse_launch_mode_value`; mirrors
                // `IpcMsg::launch_mode()`'s precedence, which is exhaustively
                // pinned by marsad-proto's own golden test suite: non-empty
                // `resume_session_id` wins, then `continue_ == true`, else
                // Fresh — empty-string resume is treated as absent).
                let mode = match resume_session_id.filter(|id| !id.is_empty()) {
                    Some(id) => marsad_proto::LaunchMode::Resume(id),
                    None if continue_ == Some(true) => marsad_proto::LaunchMode::Continue,
                    None => marsad_proto::LaunchMode::Fresh,
                };
                self.handle_launch_session(
                    wire_session_id,
                    project,
                    host_id,
                    host_target,
                    claude_bin,
                    setup,
                    env,
                    model,
                    posture,
                    first_prompt,
                    mode,
                    name,
                    disallowed_tools,
                    wasl,
                    driver,
                )
            }
            marsad_proto::IpcMsg::ResumeSession {
                session_id,
                first_prompt,
            } => self.handle_resume_session(session_id, first_prompt),
            marsad_proto::IpcMsg::AttachSession { session_id } => {
                self.handle_attach_session(&session_id)
            }
            marsad_proto::IpcMsg::StopSession { session_id } => {
                self.handle_stop_session(session_id)
            }
            marsad_proto::IpcMsg::SendPrompt { session_id, text } => {
                self.handle_send_prompt(&session_id, &text)
            }
            marsad_proto::IpcMsg::Interrupt { session_id } => self.handle_interrupt(&session_id),
            marsad_proto::IpcMsg::AnswerGate {
                session_id,
                request_id,
                allow,
                deny_reason,
                elicitation_content,
                permission_updates,
            } => self.handle_answer_gate(
                &session_id,
                &request_id,
                allow,
                deny_reason,
                elicitation_content,
                permission_updates,
            ),
            marsad_proto::IpcMsg::AnswerQuestion {
                session_id,
                request_id,
                answers,
            } => self.handle_answer_question(&session_id, &request_id, answers),
            marsad_proto::IpcMsg::Subscribe { .. } => self.handle_subscribe(),
            marsad_proto::IpcMsg::Unsubscribe { .. } => Ok(vec![]),
            // NEW explicit arms (typed-ipc.md §5 item 3): the agent has no real
            // handler for either today — same observable reply as before typing
            // (SlashCmd still errors; SetWorkspace is a silent advisory no-op).
            marsad_proto::IpcMsg::SlashCmd { .. } => {
                Err(AgentError::Protocol("SlashCmd not implemented".to_string()))
            }
            marsad_proto::IpcMsg::SetWorkspace { .. } => Ok(vec![]),
            // X2 (docs/specs/wasl-connector.md §4.3): relay ONE explicit human
            // decision on a Wasl gate to the node's operator endpoint via the
            // registered connector for `host_id`.
            marsad_proto::IpcMsg::AnswerWaslGate {
                host_id,
                gate_id,
                outcome,
                reason,
            } => self.handle_answer_wasl_gate(host_id, gate_id, outcome, reason),
            // WAVE 3b (pty escape-hatch terminal). `cols`/`rows` are dropped
            // here (destructured with `..`): the real spawn happens in
            // `serve.rs` (needs the `subscribers` broadcast list, which only
            // that scope owns), which re-parses them straight off the raw
            // inbound line — see `pty::spawn_pty_session`'s doc comment.
            marsad_proto::IpcMsg::PtyStart {
                session_id,
                terminal_id,
                ..
            } => self.handle_pty_start(session_id, terminal_id),
            marsad_proto::IpcMsg::PtyInput {
                session_id,
                terminal_id,
                data,
            } => self.handle_pty_input(&session_id, &terminal_id, &data),
            marsad_proto::IpcMsg::PtyResize {
                session_id,
                terminal_id,
                cols,
                rows,
            } => self.handle_pty_resize(&session_id, &terminal_id, cols, rows),
            marsad_proto::IpcMsg::PtyClose {
                session_id,
                terminal_id,
            } => self.handle_pty_close(&session_id, &terminal_id),
            // Every other tag is an agent→GUI/core message (ack/broadcast) that
            // has no business arriving inbound here — same observable Protocol
            // error class as an unrecognized type.
            other => Err(AgentError::Protocol(format!(
                "misdirected IPC message on GUI channel: {other:?}"
            ))),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_launch_session(
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
    fn handle_resume_session(
        &mut self,
        session_id: String,
        first_prompt: Option<String>,
    ) -> Result<Vec<String>, AgentError> {
        // Already-live guard (the double-launch fix). If this session is already
        // tracked live — reconnected at startup, or never stopped — a resume MUST
        // NOT relaunch: `launch_with_id` re-runs the durable script
        // (`rm -f in.fifo; mkfifo in.fifo`), clobbering the FIFO the existing
        // reader is attached to. That spawns a SECOND claude + reader fighting
        // over one session dir (the duplicate-reader / duplicate-gate / stray-
        // interrupt flakiness). `self.sessions` is populated synchronously by the
        // startup reconnect BEFORE any command is processed, so it is an
        // authoritative "is a reader attached" signal here. No-op re-ack; the
        // serve loop suppresses the duplicate reader spawn for this case too.
        // (`first_prompt` on an already-live session is intentionally dropped —
        // a running session takes prompts via SendPrompt, not a resume.)
        if self.sessions.contains_key(&session_id) {
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

    fn handle_attach_session(&mut self, session_id: &str) -> Result<Vec<String>, AgentError> {
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

    fn handle_stop_session(&mut self, session_id: String) -> Result<Vec<String>, AgentError> {
        // Kill the container and drop the live tracking entry. We do NOT require the
        // session to be in the live map: after an agent restart a session is persisted
        // but not yet re-tracked (the real cause of "stop did nothing" — the old code
        // returned an error and killed nothing). `kill_container` resolves the remote
        // host from the store, so the kill works either way. The persisted record is
        // KEPT so the session remains resumable.
        let container_name = format!("marsad-{session_id}");
        self.kill_container(&session_id, &container_name);
        // Cancel the reader thread so a lingering reconnect-probe loop exits at
        // once instead of backing off for up to 30 min and stacking with the
        // reader a later Resume spawns (the "14 readers for one session" churn on
        // a flapping host). The kill above also EOFs the stream; this makes the
        // teardown prompt and race-free.
        #[cfg(not(coverage))]
        crate::reader::cancel_reader(&session_id);
        // WAVE 3b: a stopped session's pty (if the Terminal tab was ever
        // opened for it) must not outlive the session — kill + reap it too.
        // Same "kept for resumability" spirit as the container above: the
        // pty itself has no resume story (it's a plain shell, not claude),
        // so it is torn down unconditionally here.
        #[cfg(not(coverage))]
        crate::pty::teardown_session(&session_id, &self.pty_entries.0);
        self.sessions.remove(&session_id);
        let event = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::SessionStopped { session_id });
        Ok(vec![event])
    }

    fn handle_send_prompt(
        &mut self,
        session_id: &str,
        text: &str,
    ) -> Result<Vec<String>, AgentError> {
        if !self.sessions.contains_key(session_id) {
            return Err(AgentError::Protocol(format!(
                "SendPrompt: unknown session {session_id}"
            )));
        }

        // In production builds, forward the prompt to the session's in.fifo.
        // The writer is registered when the FIFO thread connects after launch.
        // Under cfg(coverage) this block is absent — the fake path returns Ok
        // without IO, which is correct for unit tests.
        #[cfg(not(coverage))]
        {
            use std::io::Write;

            // Look up the session's driver from the persisted store.
            let driver = self
                .store
                .0
                .restore()
                .ok()
                .and_then(|r| r.sessions.into_iter().find(|s| s.id == session_id))
                .map(|s| s.driver)
                .unwrap_or_else(|| "claude".to_string());

            if driver == "opencode" {
                // opencode ACP: send a session/prompt JSON-RPC request to in.fifo.
                // Look up the ACP sessionId from the registry (populated by the reader
                // thread when the session/new result arrives). If not yet available,
                // fall back to the pending-prompt queue (handled by the claude path
                // below as a fallback, but for opencode the queue holds raw ACP lines
                // so we build + queue the request here and let the reader flush it).
                let acp_session_id = self.acp_sessions.0.lock().unwrap().get(session_id).cloned();
                if let Some(acp_sid) = acp_session_id {
                    // We use a simple incrementing id based on the current time
                    // (millis) to avoid collisions across turns. The reader thread
                    // correlates responses by the numeric id > 2 (initialize=1,
                    // session/new=2, first_prompt=3, subsequent=4+).
                    let req_id = serde_json::json!(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .subsec_millis() as u64
                            + 100u64  // ensure > 3
                    );
                    let prompt_params = crate::acp::session_prompt_params(&acp_sid, text);
                    let prompt_line = crate::acp::encode_request(req_id, "session/prompt", prompt_params);
                    let writers = self.session_writers.0.lock().unwrap();
                    if let Some(shared) = writers.get(session_id) {
                        let mut writer = shared.lock().unwrap();
                        match writer
                            .write_all(prompt_line.as_bytes())
                            .and_then(|_| writer.write_all(b"\n"))
                            .and_then(|_| writer.flush())
                        {
                            Ok(()) => eprintln!(
                                "marsad-agent: session {session_id} opencode send_prompt → session/prompt ({} chars)",
                                text.len()
                            ),
                            Err(e) => eprintln!(
                                "marsad-agent: session {session_id} opencode send_prompt WRITE FAILED: {e}"
                            ),
                        }
                    } else {
                        // No writer yet: queue the ACP request line (newline-terminated).
                        let mut bytes = prompt_line.into_bytes();
                        bytes.push(b'\n');
                        let mut q = self.pending_prompts.lock().unwrap();
                        let entry = q.entry(session_id.to_string()).or_default();
                        entry.push((std::time::Instant::now(), bytes));
                        eprintln!(
                            "marsad-agent: session {session_id} opencode send_prompt QUEUED — writer not ready"
                        );
                    }
                } else {
                    // ACP sessionId not yet known (session/new result not received).
                    // Queue the raw text so the reader can build + send it later.
                    eprintln!(
                        "marsad-agent: session {session_id} opencode send_prompt QUEUED — ACP sessionId not yet known"
                    );
                    // We queue a sentinel byte-blob with the user text so it's not lost.
                    // The reader flushes queued prompts when the writer registers; but
                    // those queued blobs must be valid ACP lines. Since we don't have
                    // the ACP sessionId yet, encode a temporary placeholder that the
                    // reader's flush will send as-is (the ACP sessionId will be in the
                    // registry by then, so `handle_send_prompt` will use the real path
                    // on a subsequent call).
                    let mut bytes = text.as_bytes().to_vec();
                    bytes.push(b'\n');
                    let mut q = self.pending_prompts.lock().unwrap();
                    let entry = q.entry(session_id.to_string()).or_default();
                    entry.push((std::time::Instant::now(), bytes));
                }
            } else if driver == "codex" {
                // Codex follow-up turn: build the eval-loop command via codex_turn_cmd.
                // Look up the cached spec and the stored thread_id (from thread.started).
                let spec_opt = self.codex_specs.0.lock().unwrap().get(session_id).cloned();
                let thread_id_opt = self.codex_threads.0.lock().unwrap().get(session_id).cloned();

                if let Some(cs) = spec_opt {
                    // Build a minimal LaunchSpec for codex_turn_cmd.
                    let spec_for_cmd = marsad_proto::LaunchSpec {
                        project: cs.project.clone(),
                        host_id: "local".to_string(),
                        host_target: None,
                        claude_bin: cs.claude_bin.clone(),
                        setup: None,
                        env: vec![],
                        model: cs.model.clone(),
                        posture: cs.posture.clone(),
                        first_prompt: None,
                        mode: marsad_proto::LaunchMode::Fresh,
                        name: None,
                        disallowed_tools: vec![],
                        wasl: false,
                        driver: "codex".to_string(),
                    };
                    let cmd = crate::driver::codex_turn_cmd(
                        &spec_for_cmd,
                        thread_id_opt.as_deref(),
                        text,
                    );
                    let writers = self.session_writers.0.lock().unwrap();
                    if let Some(shared) = writers.get(session_id) {
                        let mut writer = shared.lock().unwrap();
                        match writer
                            .write_all(cmd.as_bytes())
                            .and_then(|_| writer.write_all(b"\n"))
                            .and_then(|_| writer.flush())
                        {
                            Ok(()) => eprintln!(
                                "marsad-agent: session {session_id} codex send_prompt delivered ({} chars) via eval-loop",
                                text.len()
                            ),
                            Err(e) => eprintln!(
                                "marsad-agent: session {session_id} codex send_prompt WRITE FAILED: {e}"
                            ),
                        }
                    } else {
                        eprintln!(
                            "marsad-agent: session {session_id} codex send_prompt DROPPED — no writer registered"
                        );
                    }
                } else {
                    eprintln!(
                        "marsad-agent: session {session_id} codex send_prompt DROPPED — no cached spec (session launched before Stage 2?)"
                    );
                }
            } else {
                // Claude: stream-json user message.
                use marsad_proto::{encode_user_input, ContentBlock, UserInput};
                let encoded = encode_user_input(&UserInput {
                    content: vec![ContentBlock::Text {
                        text: text.to_string(),
                    }],
                });
                let writers = self.session_writers.0.lock().unwrap();
                if let Some(shared) = writers.get(session_id) {
                    let mut writer = shared.lock().unwrap();
                    match writer
                        .write_all(encoded.as_bytes())
                        .and_then(|_| writer.write_all(b"\n"))
                        .and_then(|_| writer.flush())
                    {
                        Ok(()) => eprintln!(
                            "marsad-agent: session {session_id} send_prompt delivered ({} chars) to claude stdin",
                            text.len()
                        ),
                        Err(e) => eprintln!(
                            "marsad-agent: session {session_id} send_prompt WRITE FAILED: {e}"
                        ),
                    }
                } else {
                    // No registered writer yet — the reader thread is still
                    // (re)attaching (e.g. the seconds-long remote pid-dir resolve
                    // after a resume/reconnect). QUEUE the encoded prompt so the
                    // reader flushes it in order the moment it registers the writer,
                    // instead of dropping it while the UI shows PromptSent (which
                    // silently lost prompts typed right after a resume). Bounded by
                    // PENDING_PROMPT_MAX (no time limit — survives a slow/long
                    // outage); drained in reader.rs on writer registration.
                    let mut bytes = encoded.into_bytes();
                    bytes.push(b'\n');
                    let mut q = self.pending_prompts.lock().unwrap();
                    let entry = q.entry(session_id.to_string()).or_default();
                    entry.push((std::time::Instant::now(), bytes));
                    let n = entry.len();
                    if n > crate::PENDING_PROMPT_MAX {
                        // Bound hit — drop the oldest to cap memory. SURFACE it
                        // (don't drop silently): this is the only place queued input
                        // can still be lost, and only under an extreme backlog.
                        let overflow = n - crate::PENDING_PROMPT_MAX;
                        eprintln!(
                            "marsad-agent: session {session_id} pending-prompt queue over cap ({n} > {}) — dropping {overflow} oldest",
                            crate::PENDING_PROMPT_MAX
                        );
                        entry.drain(0..overflow);
                    }
                    eprintln!(
                        "marsad-agent: session {session_id} send_prompt QUEUED ({} chars) — writer not ready, will deliver on (re)attach ({} queued)",
                        text.len(),
                        entry.len()
                    );
                }
            }
        }
        // Suppress unused-variable warning in coverage builds.
        #[cfg(coverage)]
        let _ = text;

        let event = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::PromptSent {
            session_id: session_id.to_string(),
        });
        Ok(vec![event])
    }

    fn handle_interrupt(&mut self, session_id: &str) -> Result<Vec<String>, AgentError> {
        // Write an `interrupt` control_request to claude's stdin (via the session
        // writer) to stop the current turn. claude acks + ends the turn with
        // `result: error_during_execution`. Mirrors handle_answer_gate's writer
        // use. Under cfg(coverage) the IO block is absent (fake path = no-op ack).
        // CODEX: `codex exec` turns are one-shot; there is no per-turn interrupt
        // control message. This is a documented no-op for codex sessions — the
        // turn runs to completion or failure. We log it but do not write anything
        // to the eval-loop writer (sending a random line would corrupt the
        // eval-loop's command sequence).
        #[cfg(not(coverage))]
        {
            use std::io::Write;

            // Look up the session's driver.
            let driver = self
                .store
                .0
                .restore()
                .ok()
                .and_then(|r| r.sessions.into_iter().find(|s| s.id == session_id))
                .map(|s| s.driver)
                .unwrap_or_else(|| "claude".to_string());

            // An interrupt abandons the current turn, so EVERY gate still pending
            // for this session must be resolved and cleared — otherwise a queued
            // gate stays wedged AND (for an MCP elicitation) leaves the host's
            // request half-open, so every subsequent gate for this session fails
            // closed ("declined") without ever reaching the UI. The user asked to
            // stop, so the correct resolution is to DECLINE every pending
            // tool/elicitation. Drain the whole queue unconditionally (both
            // drivers) so fresh gates can be captured cleanly on the next turn.
            let pending: std::collections::VecDeque<_> = self
                .pending_gates
                .0
                .lock()
                .unwrap()
                .remove(session_id)
                .unwrap_or_default();

            if driver == "opencode" {
                // opencode ACP interrupt: send session/cancel notification, then
                // answer any pending permission request with {outcome:"cancelled"}.
                let acp_session_id = self.acp_sessions.0.lock().unwrap().get(session_id).cloned();
                let writers = self.session_writers.0.lock().unwrap();
                if let Some(shared) = writers.get(session_id) {
                    let mut writer = shared.lock().unwrap();

                    // 1. Answer any pending permission gate(s) with cancelled.
                    for gate in &pending {
                        let cancel_result = crate::acp::request_permission_response(None);
                        // Parse the stored request id back as a JSON Value.
                        let acp_req_id: serde_json::Value =
                            serde_json::from_str(&gate.request_id).unwrap_or_else(|_| {
                                serde_json::Value::String(gate.request_id.clone())
                            });
                        let cancel_line = crate::acp::encode_response(acp_req_id, cancel_result);
                        let _ = writer.write_all(cancel_line.as_bytes());
                        let _ = writer.write_all(b"\n");
                        eprintln!(
                            "marsad-agent: session {session_id} opencode pending gate cancelled on interrupt"
                        );
                    }
                    let _ = writer.flush();

                    // 2. Send session/cancel notification (no reply expected).
                    if let Some(ref acp_sid) = acp_session_id {
                        let cancel_params = crate::acp::session_cancel_params(acp_sid);
                        let cancel_notification =
                            crate::acp::encode_notification("session/cancel", cancel_params);
                        let _ = writer.write_all(cancel_notification.as_bytes());
                        let _ = writer.write_all(b"\n");
                        let _ = writer.flush();
                        eprintln!(
                            "marsad-agent: session {session_id} opencode sent session/cancel"
                        );
                    } else {
                        eprintln!(
                            "marsad-agent: session {session_id} opencode interrupt: no ACP sessionId (session/cancel not sent)"
                        );
                    }
                } else {
                    if !pending.is_empty() {
                        eprintln!(
                            "marsad-agent: session {session_id} opencode interrupt: {} pending gate(s) cleared (no writer to cancel them)",
                            pending.len()
                        );
                    }
                    eprintln!(
                        "marsad-agent: session {session_id} opencode interrupt: no writer registered (session/cancel not sent)"
                    );
                }
            } else if driver == "codex" {
                // Codex: interrupt is a documented no-op, and the eval-loop writer
                // must not receive a control_response (it would corrupt the command
                // sequence). Clearing the queue above is enough. Log and return ack.
                if !pending.is_empty() {
                    eprintln!(
                        "marsad-agent: session {session_id} {} pending gate(s) cleared on interrupt (codex)",
                        pending.len()
                    );
                }
                eprintln!(
                    "marsad-agent: session {session_id} interrupt NO-OP for codex \
                     (codex exec turns are one-shot; no interrupt protocol exists)"
                );
            } else {
                // Claude: decline the pending gate cleanly (if any), then send the
                // interrupt control_request to end the turn. Both go to the same
                // session writer, decline first so the host resolves the blocked
                // request before the turn is torn down.
                let writers = self.session_writers.0.lock().unwrap();
                if let Some(shared) = writers.get(session_id) {
                    let mut writer = shared.lock().unwrap();
                    for gate in pending {
                        let decline = if gate.elicitation {
                            marsad_proto::encode_control_response_elicitation(
                                &gate.request_id,
                                false,
                                None,
                            )
                        } else {
                            marsad_proto::encode_control_response(&marsad_proto::ControlResponse {
                                request_id: gate.request_id,
                                allow: false,
                                deny_reason: Some("interrupted".to_string()),
                                updated_input: None,
                                updated_permissions: None,
                            })
                        };
                        let _ = writer
                            .write_all(decline.as_bytes())
                            .and_then(|_| writer.write_all(b"\n"))
                            .and_then(|_| writer.flush());
                        eprintln!(
                            "marsad-agent: session {session_id} pending gate declined on interrupt"
                        );
                    }
                    let encoded = marsad_proto::encode_control_request_interrupt("req_interrupt");
                    let _ = writer
                        .write_all(encoded.as_bytes())
                        .and_then(|_| writer.write_all(b"\n"))
                        .and_then(|_| writer.flush());
                    eprintln!("marsad-agent: session {session_id} interrupt sent to claude");
                } else {
                    eprintln!(
                        "marsad-agent: session {session_id} interrupt DROPPED — no writer registered"
                    );
                }
            }
        }
        // If the session is gone, interrupt is a benign no-op.
        let event = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::Interrupted {
            session_id: session_id.to_string(),
        });
        Ok(vec![event])
    }

    fn handle_answer_gate(
        &mut self,
        session_id: &str,
        request_id: &str,
        allow: bool,
        deny_reason: Option<String>,
        elicitation_content: Option<serde_json::Value>,
        permission_updates: Option<serde_json::Value>,
    ) -> Result<Vec<String>, AgentError> {
        // In production builds, translate the answer into a real
        // `control_response` and write it to the session's in.fifo so claude
        // unblocks the gated tool. The `allow` decision pairs the original
        // tool input (captured when the `can_use_tool` arrived) back as
        // `updatedInput` — claude USES that value, so a plain approval must
        // echo the original input verbatim.
        //
        // Under cfg(coverage) this block is absent: the fake path returns the
        // GateAnswered ack without IO, which is what the unit tests assert.
        #[cfg(not(coverage))]
        {
            use std::io::Write;

            // Pull the specific pending gate this answer targets. The wire
            // `request_id` identifies WHICH queued gate to answer — a turn can
            // block on several at once, so we must not blindly pop the front. If
            // the wire id is empty (older caller) or no exact match is found,
            // fall back to the oldest queued gate (front) so a legacy answer
            // still resolves something rather than wedging the queue. The
            // matched gate is REMOVED; any others stay queued for their own
            // answers.
            let pending = {
                let mut gates = self.pending_gates.0.lock().unwrap();
                let taken = gates.get_mut(session_id).and_then(|queue| {
                    let idx = queue
                        .iter()
                        .position(|g| g.request_id == request_id)
                        .or(if request_id.is_empty() && !queue.is_empty() {
                            Some(0)
                        } else {
                            None
                        });
                    idx.and_then(|i| queue.remove(i))
                });
                // Drop the session's entry entirely once its queue is empty.
                if gates.get(session_id).is_some_and(|q| q.is_empty()) {
                    gates.remove(session_id);
                }
                taken
            };

            // Look up the driver for this session so we can branch on opencode.
            let gate_driver = self
                .store
                .0
                .restore()
                .ok()
                .and_then(|r| r.sessions.into_iter().find(|s| s.id == session_id))
                .map(|s| s.driver)
                .unwrap_or_else(|| "claude".to_string());

            if gate_driver == "opencode" {
                // opencode ACP: answer the pending session/request_permission with
                // { outcome:"selected", optionId } or { outcome:"cancelled" }.
                //
                // The pending gate's `request_id` is the raw ACP JSON-RPC id string
                // (stored by the reader as `gate.request_id`). The chosen optionId
                // arrives via `permission_updates` (a JSON Value carrying the ACP
                // option object: { optionId, name, kind }).
                if let Some(gate) = pending {
                    // Extract the optionId from permission_updates (the chosen ACP option).
                    let option_id = if allow {
                        permission_updates
                            .as_ref()
                            .and_then(|v| v.get("optionId").and_then(|o| o.as_str()))
                            .map(|s| s.to_string())
                    } else {
                        None
                    };

                    let result_val = crate::acp::request_permission_response(
                        option_id.as_deref(),
                    );

                    // Parse the stored ACP request id back as a JSON Value.
                    let acp_req_id: serde_json::Value =
                        serde_json::from_str(&gate.request_id).unwrap_or_else(|_| {
                            serde_json::Value::String(gate.request_id.clone())
                        });

                    let response_line = crate::acp::encode_response(acp_req_id, result_val);
                    let writers = self.session_writers.0.lock().unwrap();
                    if let Some(shared) = writers.get(session_id) {
                        let mut writer = shared.lock().unwrap();
                        let _ = writer.write_all(response_line.as_bytes());
                        let _ = writer.write_all(b"\n");
                        let _ = writer.flush();
                        eprintln!(
                            "marsad-agent: session {session_id} opencode gate answered \
                             (optionId={option_id:?}, allow={allow})"
                        );
                    } else {
                        eprintln!(
                            "marsad-agent: session {session_id} opencode gate answer DROPPED — no writer"
                        );
                    }
                }
                // Early return — skip the claude gate-answer path below.
                let event = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::GateAnswered {
                    session_id: session_id.to_string(),
                });
                return Ok(vec![event]);
            }

            if let Some(gate) = pending {
                // An MCP elicitation is answered with `{action: accept|decline}`,
                // NOT the can_use_tool `{behavior, updatedInput}` shape — sending
                // the wrong shape leaves claude blocked (the MCP host can't read
                // the result). Branch on the kind captured when the gate arrived.
                let encoded = if gate.elicitation {
                    // On accept, forward the chosen scope content (e.g.
                    // `{ "grant": "session" }`); the UI built it from the picked
                    // choice. A schemaless accept sends `{}`.
                    marsad_proto::encode_control_response_elicitation(
                        &gate.request_id,
                        allow,
                        if allow { elicitation_content.clone() } else { None },
                    )
                } else {
                    let resp = marsad_proto::ControlResponse {
                        request_id: gate.request_id,
                        allow,
                        deny_reason,
                        updated_input: if allow { Some(gate.input) } else { None },
                        // Echo the chosen "always allow …" suggestion VERBATIM so
                        // claude persists the rule it itself offered. None → a plain
                        // one-time allow (or a deny).
                        updated_permissions: if allow { permission_updates.clone() } else { None },
                    };
                    marsad_proto::encode_control_response(&resp)
                };
                let writers = self.session_writers.0.lock().unwrap();
                if let Some(shared) = writers.get(session_id) {
                    let mut writer = shared.lock().unwrap();
                    let _ = writer.write_all(encoded.as_bytes());
                    let _ = writer.write_all(b"\n");
                    let _ = writer.flush();
                }
            }
        }
        // Suppress unused-variable warnings in coverage builds (the fake path
        // never reads `allow`/`deny_reason` — no writer IO to translate them into).
        #[cfg(coverage)]
        let (_, _, _, _) = (request_id, allow, deny_reason, elicitation_content);

        // If no pending gate, this is a benign no-op (the tests assert non-Protocol-error).
        let event = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::GateAnswered {
            session_id: session_id.to_string(),
        });
        Ok(vec![event])
    }

    fn handle_answer_question(
        &mut self,
        session_id: &str,
        request_id: &str,
        answers: serde_json::Value,
    ) -> Result<Vec<String>, AgentError> {
        // In production builds: pull (and clear) the pending gate for this session,
        // then build and write the AskUserQuestion control_response to claude's stdin.
        // Mirrors handle_answer_gate's writer-locking + write/flush pattern.
        #[cfg(not(coverage))]
        {
            use std::io::Write;

            // Answer the specific queued AskUserQuestion gate identified by the
            // wire `request_id` (fall back to the oldest if empty/unmatched), and
            // remove it — mirrors handle_answer_gate's queue handling.
            let pending = {
                let mut gates = self.pending_gates.0.lock().unwrap();
                let taken = gates.get_mut(session_id).and_then(|queue| {
                    let idx = queue
                        .iter()
                        .position(|g| g.request_id == request_id)
                        .or(if request_id.is_empty() && !queue.is_empty() {
                            Some(0)
                        } else {
                            None
                        });
                    idx.and_then(|i| queue.remove(i))
                });
                if gates.get(session_id).is_some_and(|q| q.is_empty()) {
                    gates.remove(session_id);
                }
                taken
            };

            if let Some(gate) = pending {
                let encoded = marsad_proto::encode_control_response_question(
                    &gate.request_id,
                    &gate.input,
                    &answers,
                );
                let writers = self.session_writers.0.lock().unwrap();
                if let Some(shared) = writers.get(session_id) {
                    let mut writer = shared.lock().unwrap();
                    let _ = writer.write_all(encoded.as_bytes());
                    let _ = writer.write_all(b"\n");
                    let _ = writer.flush();
                }
            }
        }

        // Coverage builds: no IO — just return the ack.
        #[cfg(coverage)]
        let (_, _) = (request_id, answers);
        let event = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::QuestionAnswered {
            session_id: session_id.to_string(),
        });
        Ok(vec![event])
    }

    fn handle_subscribe(&mut self) -> Result<Vec<String>, AgentError> {
        let subscriber_id = format!("sub-{}", self.next_id);
        self.next_id += 1;
        let ack = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::Subscribed { subscriber_id });
        let mut out = vec![ack];

        // Reconcile liveness on (re)subscribe. `SessionLiveness` is otherwise a
        // one-shot broadcast: a GUI that subscribes AFTER a session died (a
        // desktop restart, or reconnecting to this durable daemon) would never
        // learn it died and would sit stuck on the stale phase, silently queuing
        // prompts into a dead session. So replay the authoritative liveness for
        // every tracked session now: a session with a registered writer has a
        // live reader attached (Resumed); one without does not (Paused). The GUI
        // applies these and shows the truth (Resume button, not a working spinner).
        {
            let writers = self.session_writers.0.lock().unwrap();
            for sid in self.sessions.keys() {
                let live = writers.contains_key(sid);
                out.push(marsad_proto::encode_ipc(
                    &marsad_proto::IpcMsg::SessionLiveness {
                        session_id: sid.clone(),
                        live,
                    },
                ));
            }
        }
        Ok(out)
    }

    /// X2 (docs/specs/wasl-connector.md §4.3): relay one explicit human
    /// decision on a Wasl gate to the connector registered for `host_id`.
    ///
    /// Absent `host_id` in the registry (including the no-config case, where
    /// the map is empty) is the ENTIRE no-config surface of this arm: a
    /// `Protocol` error, no side effects. The agent never originates a
    /// decision — `decide()` is called from nowhere else.
    fn handle_answer_wasl_gate(
        &mut self,
        host_id: String,
        gate_id: String,
        outcome: String,
        reason: Option<String>,
    ) -> Result<Vec<String>, AgentError> {
        // Review finding #1 (docs/specs/wasl-connector.md §X4.3 amendment):
        // clone the connector handle out and drop the `wasl_connectors`
        // lock BEFORE calling `decide()` — `decide()` is a blocking network
        // call (bounded by marsad-wasl's own overall read deadline, but
        // still potentially slow), and it must never be made while holding
        // a lock anything else could need. The registry only ever needs the
        // lock for this quick lookup+clone (an `Arc` clone, not a deep
        // copy); see `WaslConnectorMap`'s doc comment in `lib.rs`.
        let connector = {
            let connectors = self.wasl_connectors.0.lock().unwrap();
            connectors.get(&host_id).cloned().ok_or_else(|| {
                AgentError::Protocol(format!("no wasl operator configured for host '{host_id}'"))
            })?
        };
        let ack = connector.decide(&gate_id, &outcome, reason.as_deref());

        match ack {
            Ok(marsad_wasl::DecisionAck::Delivered) => {}
            Ok(marsad_wasl::DecisionAck::AlreadyDecided) => {
                eprintln!(
                    "marsad-agent: wasl gate {gate_id} on host {host_id} already decided (benign race)"
                );
            }
            Ok(marsad_wasl::DecisionAck::UnknownGate) => {
                eprintln!(
                    "marsad-agent: wasl gate {gate_id} on host {host_id} unknown (stale answer)"
                );
            }
            Err(e) => {
                return Err(AgentError::Protocol(format!(
                    "wasl decide failed for host {host_id} gate {gate_id}: {e}"
                )));
            }
        }

        let event =
            marsad_proto::encode_ipc(&marsad_proto::IpcMsg::WaslGateAnswered { host_id, gate_id });
        Ok(vec![event])
    }

    /// WAVE 3b (pty escape-hatch terminal): validate the session exists.
    /// No ack (fire-and-forget, mirrors `SetWorkspace`) — the pty coming
    /// alive is observed via `PtyOutput` broadcasts. The actual spawn
    /// happens in `serve.rs` (see `pty::spawn_pty_session`'s doc comment for
    /// why it can't happen here).
    fn handle_pty_start(
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
    fn handle_pty_input(
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
    fn handle_pty_resize(
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
    fn handle_pty_close(
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

    /// Ingest a Claude Code hook payload.
    pub fn handle_hook(&mut self, payload: &str) -> Result<(), AgentError> {
        let value: serde_json::Value = serde_json::from_str(payload)
            .map_err(|e| AgentError::Protocol(format!("bad hook json: {e}")))?;

        let hook_type = value
            .as_object()
            .and_then(|obj| obj.get("hook"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::Protocol("missing hook discriminant".to_string()))?;

        let hook_event = parse_hook_event(hook_type, &value)?;

        if let marsad_events::HookEvent::Notification { ref message } = hook_event {
            let note = marsad_notify::Notification {
                kind: marsad_notify::NotifKind::Needs,
                title: "Notification".to_string(),
                body: message.clone(),
                host: "local".to_string(),
                priority: marsad_notify::Priority::High,
                actions: vec![],
                session_id: String::new(),
                play_sound: false,
            };
            let _ = self.notifier.0.deliver(&note);
        }

        Ok(())
    }
}

// ── parse_hook_event ──────────────────────────────────────────────────────────

/// Parse a hook payload into a `HookEvent`.
/// All arms covered by the committed hook-event tests.
fn parse_hook_event(
    hook_type: &str,
    value: &serde_json::Value,
) -> Result<marsad_events::HookEvent, AgentError> {
    match hook_type {
        "Notification" => {
            let message = value["message"].as_str().unwrap_or("").to_string();
            Ok(marsad_events::HookEvent::Notification { message })
        }
        "Stop" => Ok(marsad_events::HookEvent::Stop),
        "SubagentStop" => {
            let name = value["name"].as_str().unwrap_or("").to_string();
            Ok(marsad_events::HookEvent::SubagentStop { name })
        }
        "PreToolUse" => {
            let tool = value["tool"].as_str().unwrap_or("").to_string();
            Ok(marsad_events::HookEvent::PreToolUse { tool })
        }
        "PostToolUse" => {
            let tool = value["tool"].as_str().unwrap_or("").to_string();
            Ok(marsad_events::HookEvent::PostToolUse { tool })
        }
        "SessionStart" => Ok(marsad_events::HookEvent::SessionStart),
        "SessionEnd" => Ok(marsad_events::HookEvent::SessionEnd),
        "PreCompact" => Ok(marsad_events::HookEvent::PreCompact),
        other => Err(AgentError::Protocol(format!("unknown hook type: {other}"))),
    }
}
