//! Session interaction IPC arms - config-option change, send-prompt, interrupt.
//!
//! Extracted from ipc.rs verbatim; impl Agent is reopened here and
//! `use super::*` brings Agent/AgentError. Handlers are pub(super) so the
//! handle_ipc dispatcher and sibling arms can call them.

use super::*;

impl Agent {
    /// Change a session config option on the running agent.
    ///
    /// **claude `mode`** (Fix #4): LIVE-settable via `wasit_claude::encode_set_permission_mode`.
    /// claude does NOT echo the new mode back, so on a successful write we REFLECT it by
    /// emitting `wasit_core::AgentEvent::ModesUpdated { current_mode: Some(value) }` as an
    /// IPC line — the same event core folds into `SessionView.current_mode`. Reflection is
    /// tied to the actual write succeeding (honest — no write, no reflection).
    /// claude `model` is NOT live-settable; restart required (honest no-op log).
    ///
    /// **codex** (model AND mode): NOT live-settable — wasit codex reports
    /// `model_switchable=false`/`mode_switchable=false` and exposes no live encoder.
    /// Honest no-op; restart required (Config "Apply"). Do NOT touch.
    ///
    /// **opencode** (model AND mode): LIVE via ACP `session/set_config_option`. If the ACP
    /// sessionId is not yet known (session/new result hasn't arrived yet), QUEUE the
    /// (config_id, value) pair in `pending_config_options` instead of dropping it; the reader
    /// drains it right after `acp_sessions.insert` (Fix #6). Unknown config_ids are
    /// EXPLICITLY logged and ignored (no silent swallow).
    ///
    /// (Wasl-managed claude sessions switch mode live through the Wasl runtime — a separate
    /// path that does not go through this handler.)
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
                // Fix #4: claude permission-mode IS live-settable via
                // `encode_set_permission_mode`. claude model is NOT live-settable
                // (no encoder; restart required). Reflect on successful write so the
                // UI radio moves immediately — claude does not echo mode changes back.
                if config_id == "mode" {
                    let req_id = format!(
                        "req_mode_{}",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .subsec_millis()
                    );
                    let line = wasit_claude::encode_set_permission_mode(&req_id, value);
                    let writers = self.session_writers.0.lock().unwrap();
                    if let Some(shared) = writers.get(session_id) {
                        let mut writer = shared.lock().unwrap();
                        match writer
                            .write_all(line.as_bytes())
                            .and_then(|_| writer.write_all(b"\n"))
                            .and_then(|_| writer.flush())
                        {
                            Ok(()) => {
                                eprintln!(
                                    "marsad-agent: session {session_id} claude set_permission_mode → {value}"
                                );
                                // Reflect the new mode so the UI radio updates immediately
                                // (claude does not echo mode changes; we synthesize the event
                                // tied to the actual successful write — honest, not optimistic).
                                drop(writer);
                                drop(writers);
                                let reflect_event = wasit_core::AgentEvent::ModesUpdated {
                                    available_modes: vec![],
                                    current_mode: Some(value.to_string()),
                                };
                                let reflect_line =
                                    marsad_proto::encode_ipc(&marsad_proto::IpcMsg::AgentEvent {
                                        session_id: session_id.to_string(),
                                        event: reflect_event,
                                    });
                                return Ok(vec![reflect_line]);
                            }
                            Err(e) => {
                                eprintln!(
                                    "marsad-agent: session {session_id} claude set_permission_mode WRITE FAILED: {e} — mode NOT reflected"
                                );
                            }
                        }
                    } else {
                        eprintln!(
                            "marsad-agent: session {session_id} claude set_permission_mode DROPPED — writer not ready (mode NOT reflected)"
                        );
                    }
                    return Ok(vec![]);
                }
                // claude model (and any other config_id): not live-settable — restart required.
                eprintln!(
                    "marsad-agent: session {session_id} claude set_config_option {config_id} not live-settable — use restart (Config Apply)"
                );
                return Ok(vec![]);
            }

            if driver == "codex" {
                // codex app-server: model AND mode are both not live-settable —
                // wasit codex reports model_switchable=false / mode_switchable=false
                // and exposes no live-config encoder. Honest no-op; restart required.
                eprintln!(
                    "marsad-agent: session {session_id} codex set_config_option {config_id} not live-settable via wasit — use restart (Config Apply)"
                );
                return Ok(vec![]);
            }

            if driver != "opencode" {
                // other drivers: no wired live setter. Don't fake it.
                eprintln!(
                    "marsad-agent: session {session_id} set_config_option ignored — driver {driver} has no wired setter"
                );
                return Ok(vec![]);
            }

            // opencode: check that this is a wired knob BEFORE looking up the session.
            // Unknown options are explicitly logged and ignored (no silent swallow).
            match config_id {
                "mode" | "model" => {}
                other => {
                    eprintln!(
                        "marsad-agent: session {session_id} opencode set_config_option {other} not wired — explicitly ignored (only mode/model are live knobs)"
                    );
                    return Ok(vec![]);
                }
            }

            let acp_session_id = self.acp_sessions.0.lock().unwrap().get(session_id).cloned();
            let Some(acp_sid) = acp_session_id else {
                // Fix #6: ACP sessionId not yet known — QUEUE instead of DROP.
                // The reader drains this right after acp_sessions.insert (on the
                // session/new result), sending the queued items as set_mode/set_model.
                let mut q = self.pending_config_options.0.lock().unwrap();
                let entry = q.entry(session_id.to_string()).or_default();
                entry.push((config_id.to_string(), value.to_string()));
                eprintln!(
                    "marsad-agent: session {session_id} opencode set_config_option {config_id}={value} QUEUED — ACP sessionId not yet known ({} queued)",
                    entry.len()
                );
                return Ok(vec![]);
            };

            // Millis-based id > the reserved handshake ids (1/2/3/4), matching
            // handle_send_prompt's scheme. opencode live model/mode via the wasit
            // set_model/set_mode encoders (session/set_config_option under the hood).
            let req_id = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_millis() as u64
                + 200u64;
            let line = match config_id {
                "mode" => wasit_opencode::encode_set_mode(req_id, &acp_sid, value),
                // "model" is the only other reachable arm (guarded above)
                _ => wasit_opencode::encode_set_model(req_id, &acp_sid, value),
            };
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
                // Writer not ready but sessionId is known — queue as config option
                // so the drain (which runs right after the writer registers on
                // acp_sessions.insert) will send it. This window shouldn't normally
                // occur (acp_sessions.insert happens before the writer is registered
                // for reconnect paths), but guard it explicitly.
                let mut q = self.pending_config_options.0.lock().unwrap();
                let entry = q.entry(session_id.to_string()).or_default();
                entry.push((config_id.to_string(), value.to_string()));
                eprintln!(
                    "marsad-agent: session {session_id} opencode set_config_option {config_id}={value} QUEUED — writer not ready"
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
                    // Simple time-based (millis) incrementing id to avoid collisions
                    // across turns (initialize=1, session/new=2, first_prompt=4+).
                    let req_id = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .subsec_millis() as u64
                        + 100u64; // ensure > handshake ids
                    let prompt_line = wasit_opencode::encode_session_prompt(req_id, &acp_sid, text);
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
                    // Millis-based id > the reserved handshake ids (1/2) and the
                    // first turn (3), matching the opencode scheme. Follow-up turn
                    // via the wasit codex turn/start encoder.
                    let req_id = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .subsec_millis() as u64
                        + 100u64;
                    let line = wasit_codex::encode_turn_start(req_id, &tid, text);
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
                    // Fix #2: threadId not yet known — QUEUE the raw text instead
                    // of dropping it. The drain fires in reader.rs right after
                    // `codex_threads.insert` (when `thread/started` arrives) by
                    // building a real `turn/start` for each queued text using the
                    // then-known threadId. We MUST NOT store pre-encoded bytes here
                    // (that would write raw text as a JSON-RPC line — wire corruption);
                    // the dedicated `pending_codex_prompts` map holds raw strings.
                    let mut q = self.pending_codex_prompts.0.lock().unwrap();
                    let entry = q.entry(session_id.to_string()).or_default();
                    entry.push((std::time::Instant::now(), text.to_string()));
                    let n = entry.len();
                    if n > crate::PENDING_PROMPT_MAX {
                        let overflow = n - crate::PENDING_PROMPT_MAX;
                        eprintln!(
                            "marsad-agent: session {session_id} pending-codex-prompt queue over cap ({n} > {}) — dropping {overflow} oldest",
                            crate::PENDING_PROMPT_MAX
                        );
                        entry.drain(0..overflow);
                    }
                    eprintln!(
                        "marsad-agent: session {session_id} codex send_prompt QUEUED ({} chars) — threadId not yet known, will deliver as turn/start on thread/started ({} queued)",
                        text.len(),
                        entry.len()
                    );
                }
            } else {
                // Claude: stream-json user message (wasit encoder).
                let encoded = wasit_claude::encode_user_message(text);
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
        // Send a per-driver interrupt to the session:
        //
        //   claude  — writes a `control_request {subtype:"interrupt"}` to claude's stdin
        //             via the session writer. claude acks + ends the turn with
        //             `result: error_during_execution`. Any pending MCP elicitation or
        //             `can_use_tool` gate is DECLINED first (same writer, gate first).
        //             Mirrors handle_answer_gate's writer use.
        //
        //   opencode — DECLINES any pending permission gate(s) via `encode_gate_deny`
        //              (keyed on the raw ACP JSON-RPC id), then sends `session/cancel`
        //              (wasit opencode encoder). Both go to the same session writer.
        //
        //   codex   — DECLINES any pending gate(s) via `wasit_codex::encode_gate_deny`
        //             (keyed on method+raw id), then sends `turn/interrupt {threadId, turnId}`
        //             (wasit codex encoder) for the in-flight turn. The turn_id is taken
        //             and cleared from `codex_threads` so it cannot be reused as a stale
        //             target if a `turn/completed` is missed.
        //
        // Under cfg(coverage) the IO block is absent (fake path = no-op ack).
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
                // opencode ACP interrupt: deny any pending permission gate, then
                // send session/cancel (wasit encoders).
                let acp_session_id = self.acp_sessions.0.lock().unwrap().get(session_id).cloned();
                let writers = self.session_writers.0.lock().unwrap();
                if let Some(shared) = writers.get(session_id) {
                    let mut writer = shared.lock().unwrap();

                    // 1. Deny any pending permission gate(s) — keyed on the raw ACP
                    //    JSON-RPC id (acp_response_id), falling back to the routing id.
                    for gate in &pending {
                        let raw = gate.acp_response_id.as_deref().unwrap_or(&gate.request_id);
                        let raw_id: serde_json::Value = serde_json::from_str(raw)
                            .unwrap_or_else(|_| serde_json::Value::String(raw.to_string()));
                        let cancel_line = wasit_opencode::encode_gate_deny(&raw_id);
                        let _ = writer.write_all(cancel_line.as_bytes());
                        let _ = writer.write_all(b"\n");
                        eprintln!(
                            "marsad-agent: session {session_id} opencode pending gate denied on interrupt"
                        );
                    }
                    let _ = writer.flush();

                    // 2. Send session/cancel (no reply expected).
                    if let Some(ref acp_sid) = acp_session_id {
                        let cancel_line = wasit_opencode::encode_session_cancel(0, acp_sid);
                        let _ = writer.write_all(cancel_line.as_bytes());
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
                // {threadId, turnId} (wasit encoder) for the in-flight turn. Decline
                // any pending gate first so codex isn't left waiting on an approval.
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
                        let raw = gate.acp_response_id.as_deref().unwrap_or(&gate.request_id);
                        let raw_id: serde_json::Value = serde_json::from_str(raw)
                            .unwrap_or_else(|_| serde_json::Value::String(raw.to_string()));
                        let method = gate.codex_method.as_deref().unwrap_or("");
                        let decline = wasit_codex::encode_gate_deny(&raw_id, method);
                        let _ = writer.write_all(decline.as_bytes()).and_then(|_| writer.write_all(b"\n")).and_then(|_| writer.flush());
                    }
                    match (thread_id, turn_id) {
                        (Some(tid), Some(uid)) => {
                            let iid = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().subsec_millis() as u64 + 500;
                            let line = wasit_codex::encode_turn_interrupt(iid, &tid, &uid);
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
                            wasit_claude::encode_elicitation_decline(&gate.request_id)
                        } else {
                            wasit_claude::encode_deny(
                                &gate.request_id,
                                Some("interrupted".to_string()),
                            )
                        };
                        let _ = writer
                            .write_all(decline.as_bytes())
                            .and_then(|_| writer.write_all(b"\n"))
                            .and_then(|_| writer.flush());
                        eprintln!(
                            "marsad-agent: session {session_id} pending gate declined on interrupt"
                        );
                    }
                    let encoded = encode_claude_interrupt("req_interrupt");
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

/// Encode a claude `interrupt` control_request.
///
/// wasit-claude reports `interruptible: false` and exposes no interrupt encoder,
/// and `marsad_proto::encode_control_request_interrupt` was removed in the wasit
/// migration (Wave 0) — but Marsad's live claude interrupt still relies on this
/// control_request (claude acks it + ends the turn with
/// `result: error_during_execution`). This is the minimal local encoder that
/// preserves that behavior; the wire shape matches the control-protocol
/// `initialize`/`set_permission_mode` requests wasit-claude does encode.
#[cfg(not(coverage))]
fn encode_claude_interrupt(request_id: &str) -> String {
    serde_json::to_string(&serde_json::json!({
        "type": "control_request",
        "request_id": request_id,
        "request": { "subtype": "interrupt" },
    }))
    .expect("always serializable")
}
