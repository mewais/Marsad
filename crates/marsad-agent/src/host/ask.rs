//! `ask.user`: raise a question gate on a session for its agent (the `ask_user` MCP tool) and
//! wait for the user's answer.
//!
//! The gate is an external gate of the runtime: journaled, shown and answered exactly like a
//! question the CLI asked itself, and replayed after a daemon restart. The answer arrives as
//! the session's `AnswerQuestion` input event. A retried request (same `request_id`, after the
//! MCP server lost its connection) waits for the gate already raised. The gate is withdrawn
//! when the user interrupts the asking turn (the interrupt itself: a CLI may wait for this
//! tool call to return before it ends the turn), when that turn ends without an answer, and
//! when the caller goes away. Turn ends and interrupts before the gate went up belong to
//! earlier turns and are ignored.

use marsad_proto::ask::{AskUserReply, AskUserRequest};
use marsad_proto::runtime::SessionEventKind;
use marsad_proto::{AgentEvent, Command, MarsadError, SessionId};
use wasit_core::{GateReason, GateRequest};
use wasit_runtime::{SessionManager, StreamEnd};
use wasl_proto::{End, EndCode};

/// Withdraws the gate if the request is dropped before an answer: because the caller went away
/// (unless `reason` was set to say otherwise first).
struct Withdraw {
    manager: SessionManager,
    session: SessionId,
    request_id: String,
    armed: bool,
    reason: GateReason,
}

impl Drop for Withdraw {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let (m, s, r, why) = (
            self.manager.clone(),
            self.session.clone(),
            self.request_id.clone(),
            self.reason.clone(),
        );
        if let Ok(h) = tokio::runtime::Handle::try_current() {
            h.spawn(async move {
                let _ = m.resolve_gate(&s, &r, why).await;
            });
        }
    }
}

pub async fn ask_user(
    manager: &SessionManager,
    r: AskUserRequest,
) -> Result<AskUserReply, MarsadError> {
    let session = SessionId::parse(&r.session)
        .ok_or_else(|| MarsadError::invalid(format!("{:?} is not a session id", r.session)))?;
    if r.request_id.trim().is_empty() || r.question.question.trim().is_empty() {
        return Err(MarsadError::invalid(
            "a question and a request id are required",
        ));
    }
    let snap = manager
        .snapshot(&session)
        .map_err(|_| MarsadError::NotFound {
            what: format!("session {session}"),
        })?;
    let pending = snap
        .pending_gates
        .iter()
        .any(|g| g.gate.request_id() == r.request_id);
    // Watch from before the gate goes up, so the answer cannot be missed.
    let mut from = Some(snap.cursor);
    if !pending {
        let mut q = r.question.clone();
        if q.id.is_empty() {
            q.id = "answer".into();
        }
        manager
            .raise_gate(
                &session,
                GateRequest::Question {
                    request_id: r.request_id.clone(),
                    questions: vec![q],
                },
            )
            .await
            .map_err(|e| MarsadError::invalid(format!("cannot ask the user: {e}")))?;
    }
    // Events before our gate's `GateRequested` belong to earlier turns.
    let mut raised = pending;
    let mut guard = Withdraw {
        manager: manager.clone(),
        session: session.clone(),
        request_id: r.request_id.clone(),
        armed: true,
        reason: End::new(EndCode::CallerGone)
            .gate_reason("the ask_user call stopped waiting for the answer"),
    };
    let reply = 'outer: loop {
        let mut stream = match manager.subscribe(&session, from).await {
            Ok(s) => s,
            Err(e) => break AskUserReply::cancelled(format!("the session is gone: {e}")),
        };
        while let Some(item) = stream.next().await {
            let ev = match item {
                Ok(ev) => ev,
                Err(StreamEnd::Lagged | StreamEnd::Error(_)) => continue 'outer,
            };
            from = Some(ev.cursor);
            match ev.kind {
                SessionEventKind::Input {
                    command: Command::AnswerQuestion { request_id, answer },
                    ..
                } if request_id == r.request_id => {
                    guard.armed = false;
                    break 'outer AskUserReply {
                        answers: answer.answers.into_values().flatten().collect(),
                        cancelled: None,
                    };
                }
                SessionEventKind::Agent {
                    event: AgentEvent::GateResolved { request_id, .. },
                } if request_id == r.request_id => {
                    guard.armed = false;
                    break 'outer AskUserReply::cancelled("the question was withdrawn");
                }
                SessionEventKind::Agent {
                    event: AgentEvent::GateRequested { gate },
                } if gate.request_id() == r.request_id => raised = true,
                SessionEventKind::Input {
                    command: Command::Interrupt,
                    ..
                } if raised => {
                    let why = "the user interrupted the turn";
                    guard.reason = End::new(EndCode::TurnInterrupted).gate_reason(why);
                    break 'outer AskUserReply::cancelled(why);
                }
                SessionEventKind::Agent {
                    event: AgentEvent::TurnCompleted { .. },
                } if raised => {
                    let why = "the turn ended before an answer";
                    guard.reason = End::new(EndCode::TurnEnded).gate_reason(why);
                    break 'outer AskUserReply::cancelled(why);
                }
                SessionEventKind::Exited { .. } | SessionEventKind::Lost { .. } => {
                    guard.armed = false;
                    break 'outer AskUserReply::cancelled("the session ended before an answer");
                }
                _ => {}
            }
        }
        break AskUserReply::cancelled("the session was deleted");
    };
    // Dropping an armed guard takes the gate down (interrupted, or the turn ended unanswered).
    drop(guard);
    Ok(reply)
}

trait Cancelled {
    fn cancelled(reason: impl Into<String>) -> Self;
}

impl Cancelled for AskUserReply {
    fn cancelled(reason: impl Into<String>) -> Self {
        AskUserReply {
            answers: Vec::new(),
            cancelled: Some(reason.into()),
        }
    }
}
