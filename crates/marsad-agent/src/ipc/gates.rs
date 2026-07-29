//! Gate/question answer IPC arms - permission/plan/edit gates, questions, WASL gates.
//!
//! Extracted from ipc.rs verbatim; impl Agent is reopened here and
//! `use super::*` brings Agent/AgentError. Handlers are pub(super) so the
//! handle_ipc dispatcher and sibling arms can call them.

use super::*;

impl Agent {
    pub(super) fn handle_answer_gate(
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

                    // Reply with the RAW ACP JSON-RPC id (stored separately from the
                    // unique routing request_id). Parse it back to its JSON type so a
                    // numeric id like 0 correlates (opencode sent `"id":0`, not "0").
                    let raw_id = gate.acp_response_id.as_deref().unwrap_or(&gate.request_id);
                    let acp_req_id: serde_json::Value = serde_json::from_str(raw_id)
                        .unwrap_or_else(|_| serde_json::Value::String(raw_id.to_string()));

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

            if gate_driver == "codex" {
                // codex app-server: reply to the ServerRequest in the shape its
                // method requires (keyed off the codex_method stored on the gate):
                //   command/file approval → {decision: accept|acceptForSession|decline}
                //   legacy exec/applyPatch → {decision: approved|denied}
                //   permissions escalation → {permissions: <granted profile>}
                //   elicitation            → {action: accept|decline, content}
                if let Some(gate) = pending {
                    use crate::codex_appserver as cx;
                    let raw_id = gate.acp_response_id.as_deref().unwrap_or(&gate.request_id);
                    let req_id: serde_json::Value = serde_json::from_str(raw_id)
                        .unwrap_or_else(|_| serde_json::Value::String(raw_id.to_string()));
                    let method = gate.codex_method.as_deref().unwrap_or("");
                    let result_val = if method == cx::REQ_ELICITATION {
                        serde_json::json!({
                            "action": if allow { "accept" } else { "decline" },
                            "content": elicitation_content.clone().unwrap_or_else(|| serde_json::json!({})),
                        })
                    } else if method == cx::REQ_PERMISSION_APPROVAL {
                        crate::codex_translate::codex_permissions_response(allow, &gate.input)
                    } else if cx::is_legacy_approval(method) {
                        cx::legacy_review_decision(allow)
                    } else {
                        // Command/file approval: accept | decline. (codex also supports
                        // `acceptForSession` = "always allow"; that affordance isn't
                        // surfaced in the gate's suggestions yet, so we don't fabricate it.)
                        cx::approval_decision(allow, false)
                    };
                    let response_line = crate::acp::encode_response(req_id, result_val);
                    let writers = self.session_writers.0.lock().unwrap();
                    if let Some(shared) = writers.get(session_id) {
                        let mut writer = shared.lock().unwrap();
                        let _ = writer.write_all(response_line.as_bytes());
                        let _ = writer.write_all(b"\n");
                        let _ = writer.flush();
                        eprintln!("marsad-agent: session {session_id} codex gate answered ({method}, allow={allow})");
                    } else {
                        eprintln!("marsad-agent: session {session_id} codex gate answer DROPPED — no writer");
                    }
                }
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

    pub(super) fn handle_answer_question(
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
                // codex app-server: reply to item/tool/requestUserInput with
                // {answers} keyed by the codex question id. claude: the AskUserQuestion
                // control_response.
                let encoded = if gate.codex_method.is_some() {
                    let raw_id = gate.acp_response_id.as_deref().unwrap_or(&gate.request_id);
                    let req_id: serde_json::Value = serde_json::from_str(raw_id)
                        .unwrap_or_else(|_| serde_json::Value::String(raw_id.to_string()));
                    let ans = crate::codex_translate::codex_answers_by_id(&gate.input, &answers);
                    crate::acp::encode_response(req_id, serde_json::json!({ "answers": ans }))
                } else {
                    marsad_proto::encode_control_response_question(
                        &gate.request_id,
                        &gate.input,
                        &answers,
                    )
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

        // Coverage builds: no IO — just return the ack.
        #[cfg(coverage)]
        let (_, _) = (request_id, answers);
        let event = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::QuestionAnswered {
            session_id: session_id.to_string(),
        });
        Ok(vec![event])
    }

    /// X2 (docs/specs/wasl-connector.md §4.3): relay one explicit human
    /// decision on a Wasl gate to the connector registered for `host_id`.
    ///
    /// Absent `host_id` in the registry (including the no-config case, where
    /// the map is empty) is the ENTIRE no-config surface of this arm: a
    /// `Protocol` error, no side effects. The agent never originates a
    /// decision — `decide()` is called from nowhere else.
    pub(super) fn handle_answer_wasl_gate(
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
}
