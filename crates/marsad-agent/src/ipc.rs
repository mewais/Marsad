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
                allow,
                deny_reason,
                ..
            } => self.handle_answer_gate(&session_id, allow, deny_reason),
            marsad_proto::IpcMsg::AnswerQuestion {
                session_id,
                answers,
                ..
            } => self.handle_answer_question(&session_id, answers),
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
            use marsad_proto::{encode_user_input, ContentBlock, UserInput};
            use std::io::Write;
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
                // No registered writer — the prompt is dropped on the floor and
                // the UI shows nothing. Surface it instead of failing silently.
                eprintln!(
                    "marsad-agent: session {session_id} send_prompt DROPPED — no writer registered (session not connected?)"
                );
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
        #[cfg(not(coverage))]
        {
            use std::io::Write;
            let encoded = marsad_proto::encode_control_request_interrupt("req_interrupt");
            let writers = self.session_writers.0.lock().unwrap();
            if let Some(shared) = writers.get(session_id) {
                let mut writer = shared.lock().unwrap();
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
        // If the session is gone, interrupt is a benign no-op.
        let event = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::Interrupted {
            session_id: session_id.to_string(),
        });
        Ok(vec![event])
    }

    fn handle_answer_gate(
        &mut self,
        session_id: &str,
        allow: bool,
        deny_reason: Option<String>,
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

            // Pull (and clear) the pending gate for this session.
            let pending = {
                let mut gates = self.pending_gates.0.lock().unwrap();
                gates.remove(session_id)
            };

            if let Some(gate) = pending {
                // An MCP elicitation is answered with `{action: accept|decline}`,
                // NOT the can_use_tool `{behavior, updatedInput}` shape — sending
                // the wrong shape leaves claude blocked (the MCP host can't read
                // the result). Branch on the kind captured when the gate arrived.
                let encoded = if gate.elicitation {
                    marsad_proto::encode_control_response_elicitation(&gate.request_id, allow)
                } else {
                    let resp = marsad_proto::ControlResponse {
                        request_id: gate.request_id,
                        allow,
                        deny_reason,
                        updated_input: if allow { Some(gate.input) } else { None },
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
        let (_, _) = (allow, deny_reason);

        // If no pending gate, this is a benign no-op (the tests assert non-Protocol-error).
        let event = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::GateAnswered {
            session_id: session_id.to_string(),
        });
        Ok(vec![event])
    }

    fn handle_answer_question(
        &mut self,
        session_id: &str,
        answers: serde_json::Value,
    ) -> Result<Vec<String>, AgentError> {
        // In production builds: pull (and clear) the pending gate for this session,
        // then build and write the AskUserQuestion control_response to claude's stdin.
        // Mirrors handle_answer_gate's writer-locking + write/flush pattern.
        #[cfg(not(coverage))]
        {
            use std::io::Write;

            let pending = {
                let mut gates = self.pending_gates.0.lock().unwrap();
                gates.remove(session_id)
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
        let _ = answers;
        let event = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::QuestionAnswered {
            session_id: session_id.to_string(),
        });
        Ok(vec![event])
    }

    fn handle_subscribe(&mut self) -> Result<Vec<String>, AgentError> {
        let subscriber_id = format!("sub-{}", self.next_id);
        self.next_id += 1;
        let ack = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::Subscribed { subscriber_id });
        Ok(vec![ack])
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
        let ack = {
            let connectors = self.wasl_connectors.0.lock().unwrap();
            let connector = connectors.get(&host_id).ok_or_else(|| {
                AgentError::Protocol(format!("no wasl operator configured for host '{host_id}'"))
            })?;
            connector.decide(&gate_id, &outcome, reason.as_deref())
        };

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
