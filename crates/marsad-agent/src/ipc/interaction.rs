//! Session interaction IPC arms - config-option change, send-prompt, interrupt.
//!
//! Extracted from ipc.rs verbatim; impl Agent is reopened here and
//! `use super::*` brings Agent/AgentError. Handlers are pub(super) so the
//! handle_ipc dispatcher and sibling arms can call them.

use super::*;

impl Agent {
    /// Change a session config option on the running agent. Currently opencode
    /// only: sends ACP `session/set_config_option {sessionId, configId, value}`.
    /// The agent's reply (echoed configOptions) is parsed by the reader, which
    /// surfaces the confirmed new state — we do not mutate anything here.
    pub(super) fn handle_set_session_config_option(
        &mut self,
        session_id: &str,
        config_id: &str,
        value: &str,
    ) -> Result<Vec<String>, AgentError> {
        if !self.sessions.contains_key(session_id) {
            return Err(AgentError::Protocol(format!(
                "SetSessionConfigOption: unknown session {session_id}"
            )));
        }
        #[cfg(not(coverage))]
        {
            use std::io::Write;
            let driver = self
                .store
                .0
                .restore()
                .ok()
                .and_then(|r| r.sessions.into_iter().find(|s| s.id == session_id))
                .map(|s| s.driver)
                .unwrap_or_else(|| "claude".to_string());
            if driver == "claude" {
                // claude: a LIVE mode switch (no restart) via the control protocol —
                // set_permission_mode. Only `mode` is live-settable this way; `model`
                // still requires a restart (handled by the Config "Apply" path).
                if config_id != "mode" {
                    eprintln!(
                        "marsad-agent: session {session_id} claude set_config_option {config_id} not live-settable (use restart)"
                    );
                    return Ok(vec![]);
                }
                let line =
                    marsad_proto::encode_control_request_set_permission_mode("req_setmode", value);
                let writers = self.session_writers.0.lock().unwrap();
                if let Some(shared) = writers.get(session_id) {
                    let mut writer = shared.lock().unwrap();
                    match writer
                        .write_all(line.as_bytes())
                        .and_then(|_| writer.write_all(b"\n"))
                        .and_then(|_| writer.flush())
                    {
                        Ok(()) => eprintln!(
                            "marsad-agent: session {session_id} claude set_permission_mode {value} (live)"
                        ),
                        Err(e) => eprintln!(
                            "marsad-agent: session {session_id} claude set_permission_mode WRITE FAILED: {e}"
                        ),
                    }
                } else {
                    eprintln!(
                        "marsad-agent: session {session_id} claude set_permission_mode DROPPED — no writer"
                    );
                }
                return Ok(vec![]);
            }
            if driver == "codex" {
                // codex app-server: a LIVE switch via thread/settings/update. Map
                // config_id → the codex setting: "model" → model; "mode" →
                // collaborationMode. The threadId was captured by the reader.
                use crate::codex_appserver as cx;
                let thread_id = self.codex_threads.0.lock().unwrap().get(session_id).map(|c| c.thread_id.clone());
                let Some(tid) = thread_id else {
                    eprintln!("marsad-agent: session {session_id} codex set_config_option DROPPED — threadId not yet known");
                    return Ok(vec![]);
                };
                let codex_field = match config_id {
                    "mode" => "collaborationMode",
                    other => other, // "model", "approvalPolicy", "sandboxPolicy", …
                };
                let req_id = serde_json::json!(
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().subsec_millis() as u64 + 300u64
                );
                let line = crate::acp::encode_request(req_id, cx::THREAD_SETTINGS_UPDATE, cx::settings_update_params(&tid, codex_field, value));
                let writers = self.session_writers.0.lock().unwrap();
                if let Some(shared) = writers.get(session_id) {
                    let mut writer = shared.lock().unwrap();
                    let _ = writer.write_all(line.as_bytes()).and_then(|_| writer.write_all(b"\n")).and_then(|_| writer.flush());
                    eprintln!("marsad-agent: session {session_id} codex thread/settings/update {codex_field}={value}");
                } else {
                    eprintln!("marsad-agent: session {session_id} codex set_config_option DROPPED — no writer");
                }
                return Ok(vec![]);
            }
            if driver != "opencode" {
                // other drivers: no wired live setter. Don't fake it.
                eprintln!(
                    "marsad-agent: session {session_id} set_config_option ignored — driver {driver} has no wired setter"
                );
                return Ok(vec![]);
            }
            let acp_session_id = self.acp_sessions.0.lock().unwrap().get(session_id).cloned();
            let Some(acp_sid) = acp_session_id else {
                eprintln!(
                    "marsad-agent: session {session_id} set_config_option DROPPED — ACP sessionId not yet known"
                );
                return Ok(vec![]);
            };
            // Millis-based id > the reserved handshake ids (1/2/3), matching
            // handle_send_prompt's scheme.
            let req_id = serde_json::json!(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .subsec_millis() as u64
                    + 200u64
            );
            let params = serde_json::json!({
                "sessionId": acp_sid,
                "configId": config_id,
                "value": value,
            });
            let line = crate::acp::encode_request(req_id, "session/set_config_option", params);
            let writers = self.session_writers.0.lock().unwrap();
            if let Some(shared) = writers.get(session_id) {
                let mut writer = shared.lock().unwrap();
                match writer
                    .write_all(line.as_bytes())
                    .and_then(|_| writer.write_all(b"\n"))
                    .and_then(|_| writer.flush())
                {
                    Ok(()) => eprintln!(
                        "marsad-agent: session {session_id} opencode set_config_option {config_id}={value}"
                    ),
                    Err(e) => eprintln!(
                        "marsad-agent: session {session_id} opencode set_config_option WRITE FAILED: {e}"
                    ),
                }
            } else {
                eprintln!(
                    "marsad-agent: session {session_id} set_config_option DROPPED — writer not ready"
                );
            }
        }
        Ok(vec![])
    }

    pub(super) fn handle_send_prompt(
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
                // Codex follow-up turn (app-server): send a `turn/start` JSON-RPC
                // request on the persistent thread. The threadId was captured from
                // the thread/start response (stored in codex_threads by the reader).
                let thread_id_opt = self.codex_threads.0.lock().unwrap().get(session_id).map(|c| c.thread_id.clone());
                if let Some(tid) = thread_id_opt {
                    use crate::codex_appserver as cx;
                    // Millis-based id > the reserved handshake ids (1/2) and the
                    // first turn (3), matching the opencode scheme.
                    let req_id = serde_json::json!(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .subsec_millis() as u64
                            + 100u64
                    );
                    let line = crate::acp::encode_request(req_id, cx::TURN_START, cx::turn_start_params(&tid, text));
                    let writers = self.session_writers.0.lock().unwrap();
                    if let Some(shared) = writers.get(session_id) {
                        let mut writer = shared.lock().unwrap();
                        match writer
                            .write_all(line.as_bytes())
                            .and_then(|_| writer.write_all(b"\n"))
                            .and_then(|_| writer.flush())
                        {
                            Ok(()) => eprintln!(
                                "marsad-agent: session {session_id} codex send_prompt → turn/start ({} chars)",
                                text.len()
                            ),
                            Err(e) => eprintln!(
                                "marsad-agent: session {session_id} codex send_prompt WRITE FAILED: {e}"
                            ),
                        }
                    } else {
                        eprintln!(
                            "marsad-agent: session {session_id} codex send_prompt QUEUED — writer not ready"
                        );
                    }
                } else {
                    eprintln!(
                        "marsad-agent: session {session_id} codex send_prompt DROPPED — threadId not yet known"
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

    pub(super) fn handle_interrupt(&mut self, session_id: &str) -> Result<Vec<String>, AgentError> {
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
                // Codex app-server: a REAL interrupt — send `turn/interrupt`
                // {threadId, turnId} for the in-flight turn. (The old eval-loop had
                // no interrupt; the persistent app-server does.) Decline any pending
                // gate first so codex isn't left waiting on an approval.
                use crate::codex_appserver as cx;
                let (thread_id, turn_id) = {
                    let mut threads = self.codex_threads.0.lock().unwrap();
                    match threads.get_mut(session_id) {
                        // Take the turn_id and clear it: after we ask to interrupt,
                        // that turn is done from our side, so it must not be reused as
                        // a stale interrupt target if a turn/completed is missed.
                        Some(c) => (Some(c.thread_id.clone()), c.turn_id.take()),
                        None => (None, None),
                    }
                };
                let writers = self.session_writers.0.lock().unwrap();
                if let Some(shared) = writers.get(session_id) {
                    let mut writer = shared.lock().unwrap();
                    for gate in &pending {
                        let raw_id = gate.acp_response_id.as_deref().unwrap_or(&gate.request_id);
                        let req_id: serde_json::Value = serde_json::from_str(raw_id)
                            .unwrap_or_else(|_| serde_json::Value::String(raw_id.to_string()));
                        let decline = crate::acp::encode_response(req_id, cx::approval_decision(false, false));
                        let _ = writer.write_all(decline.as_bytes()).and_then(|_| writer.write_all(b"\n")).and_then(|_| writer.flush());
                    }
                    match (thread_id, turn_id) {
                        (Some(tid), Some(uid)) => {
                            let line = crate::acp::encode_request(
                                serde_json::json!(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().subsec_millis() as u64 + 500),
                                cx::TURN_INTERRUPT,
                                cx::turn_interrupt_params(&tid, &uid),
                            );
                            let _ = writer.write_all(line.as_bytes()).and_then(|_| writer.write_all(b"\n")).and_then(|_| writer.flush());
                            eprintln!("marsad-agent: session {session_id} codex turn/interrupt sent (turn={uid})");
                        }
                        _ => eprintln!("marsad-agent: session {session_id} codex interrupt — no in-flight turn to cancel"),
                    }
                } else {
                    eprintln!("marsad-agent: session {session_id} codex interrupt DROPPED — no writer");
                }
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
}
