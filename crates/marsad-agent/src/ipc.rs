//! Decode inbound IPC lines and dispatch them to the matching `handle_*`
//! method, plus hook-payload ingestion (`handle_hook`/`parse_hook_event`).

use super::{Agent, AgentError};

mod gates;
mod interaction;
mod lifecycle;
mod pty;
mod wasl_peers;

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
                session_mode,
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
                    session_mode,
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
            marsad_proto::IpcMsg::SetSessionConfigOption {
                session_id,
                config_id,
                value,
            } => self.handle_set_session_config_option(&session_id, &config_id, &value),
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
            // X6 Wasl Backend (docs/design/wasl-backend-plan.md §7): the six
            // inbound `*Wasl*` session-control arms. Each routes to a
            // `WaslSessionClient` in the per-session registry; every one that
            // makes a potentially-blocking client call drops the map lock first
            // (§8.1). See `wasl_session.rs`.
            marsad_proto::IpcMsg::LaunchWaslSession {
                session_id,
                host_id,
                wasl_target_key,
                name,
            } => self.handle_launch_wasl_session(session_id, host_id, wasl_target_key, name),
            marsad_proto::IpcMsg::SendWaslPrompt {
                session_id,
                text,
                idempotency_key,
                busy_policy,
                max_wait_ms,
            } => self.handle_send_wasl_prompt(
                session_id,
                text,
                idempotency_key,
                busy_policy,
                max_wait_ms,
            ),
            marsad_proto::IpcMsg::AttachWaslSession {
                session_id,
                after_seq,
                ..
            } => self.handle_attach_wasl_session(session_id, after_seq),
            marsad_proto::IpcMsg::AnswerWaslNativeGate {
                session_id,
                gate_id,
                outcome,
                reason,
            } => self.handle_answer_wasl_native_gate(session_id, gate_id, outcome, reason),
            marsad_proto::IpcMsg::InterruptWaslSession { session_id } => {
                self.handle_interrupt_wasl_session(session_id)
            }
            marsad_proto::IpcMsg::StopWaslSession { session_id, drain } => {
                self.handle_stop_wasl_session(session_id, drain)
            }
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
            // Dynamic peer registration (core → agent). Best-effort: errors are
            // logged but don't fail the IPC call.
            marsad_proto::IpcMsg::RegisterWaslPeer {
                host_id,
                peer_id,
                peer_host,
                peer_endpoint,
            } => self.handle_register_wasl_peer(host_id, peer_id, peer_host, peer_endpoint),
            marsad_proto::IpcMsg::UnregisterWaslPeer { host_id, peer_id } => {
                self.handle_unregister_wasl_peer(host_id, peer_id)
            }
            // Every other tag is an agent→GUI/core message (ack/broadcast) that
            // has no business arriving inbound here — same observable Protocol
            // error class as an unrecognized type.
            other => Err(AgentError::Protocol(format!(
                "misdirected IPC message on GUI channel: {other:?}"
            ))),
        }
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
            // The set of tracked sessions is cfg-agnostic; only the live/paused
            // determination reads the real-IO writer registry. Under coverage that
            // registry is gated out and no reader threads exist, so a tracked
            // session is reported not-live (the loop itself stays covered).
            #[cfg(not(coverage))]
            let writers = self.session_writers.0.lock().unwrap();
            for sid in self.sessions.keys() {
                #[cfg(not(coverage))]
                let live = writers.contains_key(sid);
                #[cfg(coverage)]
                let live = false;
                out.push(marsad_proto::encode_ipc(
                    &marsad_proto::IpcMsg::SessionLiveness {
                        session_id: sid.clone(),
                        live,
                    },
                ));
            }
        }

        // Replay every OPEN gate. A `can_use_tool`/permission gate is otherwise a
        // one-shot broadcast to whoever was subscribed the instant it was raised.
        // A desktop that restarts (reinstall/crash) while this durable daemon and a
        // blocked session stay ALIVE re-subscribes here, but the adopted agent stays
        // stuck on the gate and never re-emits it — so without this replay the gate
        // is invisible in the new client and the session looks hung. `pending_gates`
        // holds exactly the still-unanswered gates (each removed on answer), so we
        // re-deliver each as the same `AgentEvent{session_id, event}` the reader
        // originally broadcast; core's on_agent_line renders it and dedups by
        // request_id, so a replay to an already-populated client is idempotent.
        //
        // Gated to production builds for the same reason as the liveness replay
        // above: `pending_gates` is a `#[cfg(not(coverage))]` real-IO registry.
        #[cfg(not(coverage))]
        {
            let gates = self.pending_gates.0.lock().unwrap();
            for (sid, queue) in gates.iter() {
                for gate in queue.iter() {
                    // `gate.event` is a serialized `wasit_core::AgentEvent::GateRequested`
                    // stored as a `serde_json::Value` for re-subscription replay. Deserialize
                    // it back to the typed variant for the wire format.
                    if let Ok(typed_event) =
                        serde_json::from_value::<wasit_core::AgentEvent>(gate.event.clone())
                    {
                        out.push(marsad_proto::encode_ipc(&marsad_proto::IpcMsg::AgentEvent {
                            session_id: sid.clone(),
                            event: typed_event,
                        }));
                    }
                }
            }
        }

        // Replay the last transcript backfill per session. Same one-shot-broadcast
        // problem as the gates above, one step earlier: the reader broadcasts a
        // REMOTE session's prior conversation ONCE when it re-attaches on agent
        // startup. On a cold app relaunch the durable agent auto-reconnects and
        // broadcasts the backfill BEFORE the desktop re-subscribes — so it hits zero
        // subscribers and the transcript is lost forever. `pending_backfills` caches
        // the ALREADY-ENCODED `TranscriptBackfill` line, so push it verbatim (no
        // re-encode). core's `handle_transcript_backfill` folds it idempotently, so a
        // replay to an already-populated client does not duplicate history.
        //
        // Gated to production builds for the same reason as the replays above:
        // `pending_backfills` is a `#[cfg(not(coverage))]` real-IO registry.
        #[cfg(not(coverage))]
        {
            let backfills = self.pending_backfills.0.lock().unwrap();
            for (_session_id, line) in backfills.iter() {
                out.push(line.clone());
            }
        }
        Ok(out)
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

// Gated to the same condition as `pending_gates`/`session_writers` (real-IO
// registries exist only when `not(coverage)`), so the inline test compiles under
// plain `cargo test` and is excluded from the coverage build like those fields.
#[cfg(all(test, not(coverage)))]
mod subscribe_gate_replay_tests {
    use super::*;
    use crate::{AgentConfig, ListenSpec, PendingGate};

    fn fresh_agent() -> Agent {
        Agent::with_fakes(
            AgentConfig {
                listen: ListenSpec::Stdio,
                data_root: "/tmp/marsad-agent-test".to_string(),
            },
            Box::new(marsad_lifecycle::FakeContainer::new()),
            Box::new(marsad_notify::CaptureNotifier::new()),
            Box::new(marsad_store::Store::new(Box::new(
                marsad_store::MemoryBackend::new(),
            ))),
        )
        .expect("Agent::with_fakes must succeed")
    }

    fn seed_gate(a: &Agent, sid: &str, req: &str) {
        // Store the gate event as a serialized AgentEvent::GateRequested so the
        // handle_subscribe replay path can deserialize it back to the typed form.
        let typed_event = wasit_core::AgentEvent::GateRequested {
            gate: wasit_core::GateRequest::Permission {
                request_id: req.to_string(),
                tool_name: "Bash".to_string(),
                command: "bash cmd".to_string(),
                cwd: "/proj".to_string(),
                explanation: String::new(),
                is_elicitation: false,
                elicitation_choices: vec![],
                permission_suggestions: vec![],
                raw_input: serde_json::json!({ "tool_name": "Bash" }),
            },
        };
        a.pending_gates
            .0
            .lock()
            .unwrap()
            .entry(sid.to_string())
            .or_default()
            .push_back(PendingGate {
                request_id: req.to_string(),
                input: serde_json::json!({ "tool_name": "Bash" }),
                elicitation: false,
                acp_response_id: None,
                codex_method: None,
                event: serde_json::to_value(&typed_event)
                    .unwrap_or_else(|_| serde_json::json!({})),
            });
    }

    fn agent_event_gates(out: &[String]) -> Vec<(String, wasit_core::AgentEvent)> {
        out.iter()
            .filter_map(|l| match marsad_proto::parse_ipc(l) {
                Ok(marsad_proto::IpcMsg::AgentEvent { session_id, event }) => {
                    Some((session_id, event))
                }
                _ => None,
            })
            .collect()
    }

    fn gate_request_id(ev: &wasit_core::AgentEvent) -> Option<&str> {
        match ev {
            wasit_core::AgentEvent::GateRequested {
                gate: wasit_core::GateRequest::Permission { request_id, .. },
            }
            | wasit_core::AgentEvent::GateRequested {
                gate: wasit_core::GateRequest::Plan { request_id, .. },
            }
            | wasit_core::AgentEvent::GateRequested {
                gate: wasit_core::GateRequest::Question { request_id, .. },
            } => Some(request_id.as_str()),
            _ => None,
        }
    }

    #[test]
    fn subscribe_acks_then_replays_open_gate_as_agent_event() {
        let mut a = fresh_agent();
        seed_gate(&a, "s1", "req-1");

        let out = a.handle_subscribe().expect("subscribe");

        // First line is always the Subscribed ack.
        assert!(matches!(
            marsad_proto::parse_ipc(&out[0]),
            Ok(marsad_proto::IpcMsg::Subscribed { .. })
        ));
        // The open gate is replayed as an AgentEvent::GateRequested with the same request_id.
        let gates = agent_event_gates(&out);
        assert_eq!(gates.len(), 1, "exactly one gate replayed");
        assert_eq!(gates[0].0, "s1");
        assert_eq!(
            gate_request_id(&gates[0].1),
            Some("req-1"),
            "gate event must carry request_id=req-1"
        );
        assert!(
            matches!(&gates[0].1, wasit_core::AgentEvent::GateRequested { .. }),
            "replayed event must be GateRequested"
        );
    }

    #[test]
    fn subscribe_replays_a_burst_and_is_not_consumed_on_re_subscribe() {
        let mut a = fresh_agent();
        // A single turn can block on a burst of gates (deque preserves all).
        seed_gate(&a, "s1", "req-1");
        seed_gate(&a, "s1", "req-2");

        let first = a.handle_subscribe().expect("subscribe #1");
        let second = a.handle_subscribe().expect("subscribe #2");

        // Both open gates replayed, in order.
        let g1 = agent_event_gates(&first);
        assert_eq!(g1.len(), 2);
        assert_eq!(gate_request_id(&g1[0].1), Some("req-1"));
        assert_eq!(gate_request_id(&g1[1].1), Some("req-2"));
        // The registry is the durable source of truth — NOT consumed by a replay,
        // so a second (re)subscribe replays the still-open gates again.
        assert_eq!(agent_event_gates(&second).len(), 2);
    }

    #[test]
    fn subscribe_with_no_open_gates_replays_none() {
        let mut a = fresh_agent();
        let out = a.handle_subscribe().expect("subscribe");
        assert!(agent_event_gates(&out).is_empty());
    }

    // Seed the pending-backfill cache with the ALREADY-ENCODED IPC line, exactly
    // as the reader stores it at the broadcast site.
    fn seed_backfill(a: &Agent, sid: &str, jsonl: &str) -> String {
        let line = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::TranscriptBackfill {
            session_id: sid.to_string(),
            jsonl: jsonl.to_string(),
        });
        a.pending_backfills
            .0
            .lock()
            .unwrap()
            .insert(sid.to_string(), line.clone());
        line
    }

    #[test]
    fn subscribe_replays_cached_transcript_backfill() {
        let mut a = fresh_agent();
        let expected = seed_backfill(&a, "s1", "{\"type\":\"user\"}\n");

        let out = a.handle_subscribe().expect("subscribe");

        // Ack + liveness are present; the cached backfill line is replayed verbatim
        // (it is already an encoded IPC line, so it appears byte-identical in out).
        assert!(matches!(
            marsad_proto::parse_ipc(&out[0]),
            Ok(marsad_proto::IpcMsg::Subscribed { .. })
        ));
        assert!(
            out.iter().any(|l| l == &expected),
            "handle_subscribe output must contain the exact cached TranscriptBackfill line"
        );
        // And it round-trips to the right session_id/jsonl.
        let backfills: Vec<_> = out
            .iter()
            .filter_map(|l| match marsad_proto::parse_ipc(l) {
                Ok(marsad_proto::IpcMsg::TranscriptBackfill { session_id, jsonl }) => {
                    Some((session_id, jsonl))
                }
                _ => None,
            })
            .collect();
        assert_eq!(backfills.len(), 1, "exactly one backfill replayed");
        assert_eq!(backfills[0].0, "s1");
        assert_eq!(backfills[0].1, "{\"type\":\"user\"}\n");

        // The cache is the durable source of truth — NOT consumed by a replay, so a
        // second (re)subscribe replays it again (mirrors the gate-replay contract).
        let second = a.handle_subscribe().expect("subscribe #2");
        assert!(second.iter().any(|l| l == &expected));
    }
}
