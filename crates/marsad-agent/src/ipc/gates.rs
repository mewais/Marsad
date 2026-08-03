//! Gate/question answer IPC arms - permission/plan/edit gates, questions, WASL gates.
//!
//! Uses wasit_claude / wasit_codex / wasit_opencode encoders to build the
//! driver-native response line. The old marsad_proto control encoders and
//! crate::acp / codex_appserver / codex_translate modules are gone.

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
        #[cfg(not(coverage))]
        {

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

            let gate_driver = self
                .store
                .0
                .restore()
                .ok()
                .and_then(|r| r.sessions.into_iter().find(|s| s.id == session_id))
                .map(|s| s.driver)
                .unwrap_or_else(|| "claude".to_string());

            if gate_driver == "opencode" {
                if let Some(gate) = pending {
                    let raw_id = recover_opencode_raw_id(
                        gate.acp_response_id.as_deref().unwrap_or(&gate.request_id),
                    );
                    let option_id = if allow {
                        elicitation_content
                            .as_ref()
                            .and_then(|v| v.get("optionId"))
                            .and_then(|o| o.as_str())
                            .map(|s| s.to_string())
                    } else {
                        None
                    };
                    let line = if allow {
                        wasit_opencode::encode_gate_approve(&raw_id, option_id.as_deref())
                    } else {
                        wasit_opencode::encode_gate_deny(&raw_id)
                    };
                    write_to_session(
                        session_id,
                        &self.session_writers.0,
                        &line,
                        "opencode gate answered",
                    );
                }
                let event = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::GateAnswered {
                    session_id: session_id.to_string(),
                });
                return Ok(vec![event]);
            }

            if gate_driver == "codex" {
                if let Some(gate) = pending {
                    let raw_id = recover_codex_raw_id(
                        gate.acp_response_id.as_deref().unwrap_or(&gate.request_id),
                    );
                    let method = gate
                        .codex_method
                        .as_deref()
                        .unwrap_or("item/commandExecution/requestApproval");
                    let line = build_codex_gate_response(
                        &raw_id,
                        method,
                        allow,
                        elicitation_content.as_ref(),
                    );
                    write_to_session(
                        session_id,
                        &self.session_writers.0,
                        &line,
                        "codex gate answered",
                    );
                }
                let event = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::GateAnswered {
                    session_id: session_id.to_string(),
                });
                return Ok(vec![event]);
            }

            // Claude: use wasit_claude encoders.
            if let Some(gate) = pending {
                let encoded = if gate.elicitation {
                    if allow {
                        wasit_claude::encode_elicitation_accept(
                            &gate.request_id,
                            elicitation_content.clone(),
                        )
                    } else {
                        wasit_claude::encode_elicitation_decline(&gate.request_id)
                    }
                } else {
                    if allow {
                        wasit_claude::encode_allow(
                            &gate.request_id,
                            gate.input.clone(),
                            permission_updates.clone(),
                        )
                    } else {
                        wasit_claude::encode_deny(&gate.request_id, deny_reason.clone())
                    }
                };
                write_to_session(
                    session_id,
                    &self.session_writers.0,
                    &encoded,
                    "claude gate answered",
                );
            }
        }

        #[cfg(coverage)]
        let (_, _, _, _) = (request_id, allow, deny_reason, elicitation_content);

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
        #[cfg(not(coverage))]
        {

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
                let encoded = if gate.codex_method.is_some() {
                    let raw_id = recover_codex_raw_id(
                        gate.acp_response_id.as_deref().unwrap_or(&gate.request_id),
                    );
                    wasit_codex::encode_user_input_answer(&raw_id, answers.clone())
                } else {
                    wasit_claude::encode_question_answer(
                        &gate.request_id,
                        &gate.input,
                        &answers,
                    )
                };
                write_to_session(
                    session_id,
                    &self.session_writers.0,
                    &encoded,
                    "question answered",
                );
            }
        }

        #[cfg(coverage)]
        let (_, _) = (request_id, answers);
        let event = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::QuestionAnswered {
            session_id: session_id.to_string(),
        });
        Ok(vec![event])
    }

    /// X2: relay one explicit human decision on a Wasl gate.
    pub(super) fn handle_answer_wasl_gate(
        &mut self,
        host_id: String,
        gate_id: String,
        outcome: String,
        reason: Option<String>,
    ) -> Result<Vec<String>, AgentError> {
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
                    "marsad-agent: wasl gate {gate_id} on host {host_id} already decided"
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

// ── Pure helpers ──────────────────────────────────────────────────────────────

/// Write a line to a session writer; log on missing writer.
#[cfg(not(coverage))]
#[allow(clippy::type_complexity)]
fn write_to_session(
    session_id: &str,
    writers: &std::sync::Arc<
        std::sync::Mutex<
            std::collections::HashMap<
                String,
                std::sync::Arc<std::sync::Mutex<Box<dyn std::io::Write + Send>>>,
            >,
        >,
    >,
    line: &str,
    label: &str,
) {
    use std::io::Write;
    let lock = writers.lock().unwrap();
    if let Some(shared) = lock.get(session_id) {
        let mut w = shared.lock().unwrap();
        let _ = w.write_all(line.as_bytes());
        let _ = w.write_all(b"\n");
        let _ = w.flush();
        eprintln!("marsad-agent: session {session_id} {label}");
    } else {
        eprintln!("marsad-agent: session {session_id} {label} DROPPED — no writer");
    }
}

/// Recover the raw JSON-RPC id for a codex gate from the stored
/// `acp_response_id`. The stored value is now the canonical JSON serialization
/// of the real wire id (produced by `serde_json::to_string(&raw_id)` in
/// `capture_pending_gate`), so we parse it back: a numeric id stored as "7"
/// deserializes to `json!(7)`; a string id stored as `"\"call_abc\""` stays a
/// string. If parsing fails (legacy stored value without quotes) we return the
/// raw string as-is for backward-compat.
#[cfg(not(coverage))]
fn recover_codex_raw_id(stored: &str) -> serde_json::Value {
    serde_json::from_str(stored)
        .unwrap_or_else(|_| serde_json::Value::String(stored.to_string()))
}

/// Recover the raw JSON-RPC id for an opencode gate from the stored
/// `acp_response_id`. Same canonical-JSON round-trip as `recover_codex_raw_id`.
#[cfg(not(coverage))]
fn recover_opencode_raw_id(stored: &str) -> serde_json::Value {
    serde_json::from_str(stored)
        .unwrap_or_else(|_| serde_json::Value::String(stored.to_string()))
}

/// Build a codex JSON-RPC gate response using the wasit_codex encoders,
/// keyed off the method name.
#[cfg(not(coverage))]
fn build_codex_gate_response(
    raw_id: &serde_json::Value,
    method: &str,
    allow: bool,
    elicitation_content: Option<&serde_json::Value>,
) -> String {
    match method {
        "mcpServer/elicitation/request" => {
            if allow {
                wasit_codex::encode_elicitation_accept(raw_id, elicitation_content.cloned())
            } else {
                wasit_codex::encode_elicitation_decline(raw_id)
            }
        }
        "item/tool/requestUserInput" => {
            let answers = elicitation_content
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            wasit_codex::encode_user_input_answer(raw_id, answers)
        }
        _ => {
            // item/commandExecution/requestApproval, item/fileChange/requestApproval,
            // item/permissions/requestApproval, execCommandApproval, applyPatchApproval.
            if allow {
                wasit_codex::encode_gate_approve(raw_id, method, false)
            } else {
                wasit_codex::encode_gate_deny(raw_id, method)
            }
        }
    }
}
