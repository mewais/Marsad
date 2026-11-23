//! `ask.user`: raise a question gate on a session for its agent (the `ask_user` MCP tool) and
//! wait for the user's answer.
//!
//! The gate is an external gate of the runtime: journaled, shown and answered exactly like a
//! question the CLI asked itself, and replayed after a daemon restart. The answer arrives as
//! the session's `AnswerQuestion` input event. A retried request (same `request_id`, after the
//! MCP server lost its connection) waits for the gate already raised.
//!
//! The gate belongs to the turn that asked: it is raised bound to that turn, and the session
//! itself resolves it when the user interrupts the turn (at the interrupt, ahead of anything the
//! CLI says: a CLI may wait for this tool call to return before it ends the turn) or the turn
//! ends without an answer, with the runtime's `turn_interrupted` / `turn_ended` and the
//! sentences given here. This module does not watch the turn: it only observes the gate's
//! `GateResolved` and answers the tool call with what the reason says. What stays its own is the
//! caller going away, for which it takes the gate down itself.

use marsad_proto::ask::{AskUserReply, AskUserRequest};
use marsad_proto::runtime::SessionEventKind;
use marsad_proto::{AgentEvent, Command, MarsadError, SessionId};
use wasit_core::{GateReason, GateRequest, TurnBinding};
use wasit_runtime::{SessionManager, StreamEnd};
use wasl_proto::{End, EndCode};

/// What the tool call is told, and the gate says, when the asking turn is interrupted.
const INTERRUPTED: &str = "the user interrupted the turn";
/// The same when the asking turn ends without an answer.
const TURN_ENDED: &str = "the turn ended before an answer";

/// The gate lives as long as the turn that asked.
fn turn_binding() -> TurnBinding {
    TurnBinding::Turn {
        interrupted: Some(INTERRUPTED.into()),
        ended: Some(TURN_ENDED.into()),
    }
}

/// What the tool call is told about a gate the session resolved without an answer: the reason's
/// own sentence (this module worded the turn's endings when it raised the gate), else by code.
fn withdrawn_text(reason: Option<&GateReason>) -> String {
    match reason {
        Some(GateReason {
            detail: Some(detail),
            ..
        }) => detail.clone(),
        Some(r) if r.code == GateReason::PROCESS_ENDED || r.code == GateReason::SESSION_STOPPED => {
            "the session ended before an answer".into()
        }
        _ => "the question was withdrawn".into(),
    }
}

/// Withdraws the gate if the request is dropped before an answer: because the caller went away.
struct Withdraw {
    manager: SessionManager,
    session: SessionId,
    request_id: String,
    armed: bool,
}

impl Drop for Withdraw {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let (m, s, r) = (
            self.manager.clone(),
            self.session.clone(),
            self.request_id.clone(),
        );
        let why = End::new(EndCode::CallerGone)
            .gate_reason("the ask_user call stopped waiting for the answer");
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
            .raise_gate_for_turn(
                &session,
                GateRequest::Question {
                    request_id: r.request_id.clone(),
                    questions: vec![q],
                },
                turn_binding(),
            )
            .await
            .map_err(|e| MarsadError::invalid(format!("cannot ask the user: {e}")))?;
    }
    let mut guard = Withdraw {
        manager: manager.clone(),
        session: session.clone(),
        request_id: r.request_id.clone(),
        armed: true,
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
                // The session took the gate down (its turn was interrupted or ended, or the
                // process ended) or someone else did: the call is over either way.
                SessionEventKind::Agent {
                    event: AgentEvent::GateResolved { request_id, reason },
                } if request_id == r.request_id => {
                    guard.armed = false;
                    break 'outer AskUserReply::cancelled(withdrawn_text(reason.as_ref()));
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
    // Dropping an armed guard takes the gate down: the caller stopped waiting.
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
