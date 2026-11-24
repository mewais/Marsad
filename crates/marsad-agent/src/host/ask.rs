//! `ask.user`: raise a question gate on a session for its agent (the `ask_user` MCP tool) and
//! wait for the user's answer.
//!
//! The gate is an external gate of the runtime: journaled, shown and answered exactly like a
//! question the CLI asked itself, and replayed after a daemon restart. The answer arrives as
//! the session's `AnswerQuestion` input event.
//!
//! A gate outlives its caller. When the MCP server loses its connection it re-sends the call
//! with the same `request_id`, and the re-sent call joins the gate already raised. So each gate
//! has a keeper that follows the session for the gate's answer for as long as the gate is open,
//! whether or not a call is waiting: an answer that comes while the caller is gone is kept for
//! the re-sent call. The gate is withdrawn at its deadline, and only if no call is waiting for
//! it then; a call that returns after that is told so.
//!
//! The gate belongs to the turn that asked: it is raised bound to that turn, and the session
//! itself resolves it when the user interrupts the turn (at the interrupt, ahead of anything the
//! CLI says: a CLI may wait for this tool call to return before it ends the turn) or the turn
//! ends without an answer, with the runtime's `turn_interrupted` / `turn_ended` and the
//! sentences given here. This module does not watch the turn: it only observes the gate's
//! `GateResolved` and answers the tool call with what the reason says.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use marsad_proto::ask::{AskUserReply, AskUserRequest};
use marsad_proto::runtime::SessionEventKind;
use marsad_proto::{AgentEvent, Command, MarsadError, SessionId};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use wasit_core::{GateReason, GateRequest, QuestionPrompt, TurnBinding};
use wasit_runtime::{SessionManager, StreamEnd};
use wasl_proto::{End, EndCode};

/// How long a question waits for its answer, counted from when the gate is raised. The MCP
/// server gives up at the same length of time.
pub const ANSWER_DEADLINE: Duration = Duration::from_secs(24 * 3600);

/// What the tool call is told, and the gate says, when the asking turn is interrupted.
const INTERRUPTED: &str = "the user interrupted the turn";
/// The same when the asking turn ends without an answer.
const TURN_ENDED: &str = "the turn ended before an answer";
/// The same when the deadline passes with no call waiting for the answer.
const NOT_ANSWERED: &str = "the question was not answered before its deadline";

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

type Fut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What the session tells a gate's watcher, reduced to what this module needs.
#[derive(Debug, Clone)]
pub enum GateEvent {
    /// The subscription lagged or failed: the watcher resubscribes from the last cursor it saw.
    Lagged,
    Answered {
        cursor: u64,
        request_id: String,
        answers: Vec<String>,
    },
    Resolved {
        cursor: u64,
        request_id: String,
        detail: String,
    },
    SessionEnded {
        cursor: u64,
    },
    Other {
        cursor: u64,
    },
}

impl GateEvent {
    fn cursor(&self) -> Option<u64> {
        match self {
            GateEvent::Lagged => None,
            GateEvent::Answered { cursor, .. }
            | GateEvent::Resolved { cursor, .. }
            | GateEvent::SessionEnded { cursor }
            | GateEvent::Other { cursor } => Some(*cursor),
        }
    }
}

/// What a gate needs from its session: the [`SessionManager`] in production, a fake in tests.
pub trait Gateway: Send + Sync + 'static {
    /// The journal cursor now, and whether a gate for `request_id` is pending.
    fn snapshot_gate(&self, session: &SessionId, request_id: &str) -> Result<(u64, bool), String>;
    /// Raise the question gate, bound to the asking turn.
    fn raise<'a>(
        &'a self,
        session: &'a SessionId,
        request_id: &'a str,
        question: QuestionPrompt,
    ) -> Fut<'a, Result<(), String>>;
    /// The session's events after `from`. The channel closes when the subscription ends.
    fn events<'a>(
        &'a self,
        session: &'a SessionId,
        from: u64,
    ) -> Fut<'a, Result<mpsc::Receiver<GateEvent>, String>>;
    /// Take the gate down, with `detail` as its reason.
    fn withdraw<'a>(
        &'a self,
        session: &'a SessionId,
        request_id: &'a str,
        detail: &'static str,
    ) -> Fut<'a, ()>;
}

impl Gateway for SessionManager {
    fn snapshot_gate(&self, session: &SessionId, request_id: &str) -> Result<(u64, bool), String> {
        let snap = self.snapshot(session).map_err(|e| e.to_string())?;
        let pending = snap
            .pending_gates
            .iter()
            .any(|g| g.gate.request_id() == request_id);
        Ok((snap.cursor, pending))
    }

    fn raise<'a>(
        &'a self,
        session: &'a SessionId,
        request_id: &'a str,
        question: QuestionPrompt,
    ) -> Fut<'a, Result<(), String>> {
        Box::pin(async move {
            let gate = GateRequest::Question {
                request_id: request_id.to_string(),
                questions: vec![question],
            };
            self.raise_gate_for_turn(session, gate, turn_binding())
                .await
                .map_err(|e| e.to_string())
        })
    }

    fn events<'a>(
        &'a self,
        session: &'a SessionId,
        from: u64,
    ) -> Fut<'a, Result<mpsc::Receiver<GateEvent>, String>> {
        Box::pin(async move {
            let mut stream = self
                .subscribe(session, Some(from))
                .await
                .map_err(|e| e.to_string())?;
            let (tx, rx) = mpsc::channel(64);
            tokio::spawn(async move {
                while let Some(item) = stream.next().await {
                    let ev = match item {
                        Ok(ev) => translate(ev.cursor, ev.kind),
                        // Lagged or failed: the stream ends here; the watcher resubscribes.
                        Err(StreamEnd::Lagged | StreamEnd::Error(_)) => GateEvent::Lagged,
                    };
                    let stop = matches!(ev, GateEvent::Lagged);
                    if tx.send(ev).await.is_err() || stop {
                        return;
                    }
                }
            });
            Ok(rx)
        })
    }

    fn withdraw<'a>(
        &'a self,
        session: &'a SessionId,
        request_id: &'a str,
        detail: &'static str,
    ) -> Fut<'a, ()> {
        Box::pin(async move {
            let why = End::new(EndCode::CallerGone).gate_reason(detail);
            let _ = self.resolve_gate(session, request_id, why).await;
        })
    }
}

fn translate(cursor: u64, kind: SessionEventKind) -> GateEvent {
    match kind {
        SessionEventKind::Input {
            command: Command::AnswerQuestion { request_id, answer },
            ..
        } => GateEvent::Answered {
            cursor,
            request_id,
            answers: answer.answers.into_values().flatten().collect(),
        },
        SessionEventKind::Agent {
            event: AgentEvent::GateResolved { request_id, reason },
        } => GateEvent::Resolved {
            cursor,
            request_id,
            detail: withdrawn_text(reason.as_ref()),
        },
        SessionEventKind::Exited { .. } | SessionEventKind::Lost { .. } => {
            GateEvent::SessionEnded { cursor }
        }
        _ => GateEvent::Other { cursor },
    }
}

/// One question gate, shared by every call that asks for its request id.
struct Gate {
    gateway: Arc<dyn Gateway>,
    session: SessionId,
    request_id: String,
    /// When the gate stops waiting for an answer.
    deadline: Instant,
    state: watch::Sender<GateState>,
}

#[derive(Default)]
struct GateState {
    /// Calls waiting for the answer now. Zero while the MCP caller is gone.
    callers: usize,
    /// Set once: the answer, or why there is none.
    outcome: Option<AskUserReply>,
}

type GateKey = (SessionId, String);

/// The open gates of this daemon. A gate leaves here once its deadline has passed and nothing
/// waits for it, so a re-sent call inside the deadline finds it.
static GATES: LazyLock<Mutex<HashMap<GateKey, Arc<Gate>>>> = LazyLock::new(Default::default);

fn gates() -> MutexGuard<'static, HashMap<GateKey, Arc<Gate>>> {
    GATES.lock().unwrap_or_else(|e| e.into_inner())
}

/// One call waiting on a gate. Dropping it (the caller went away, or it has its answer) counts
/// it out; the gate is withdrawn only when nothing waits for it and its deadline has passed.
struct Attached(Arc<Gate>);

impl Attached {
    /// Call with the registry lock held, so a gate cannot be forgotten between lookup and join.
    fn join(gate: Arc<Gate>) -> Attached {
        gate.state.send_modify(|s| s.callers += 1);
        Attached(gate)
    }
}

impl Drop for Attached {
    fn drop(&mut self) {
        self.0.state.send_modify(|s| s.callers -= 1);
        let idle_past_deadline =
            self.0.state.borrow().callers == 0 && Instant::now() >= self.0.deadline;
        if idle_past_deadline {
            if let Ok(h) = tokio::runtime::Handle::try_current() {
                h.spawn(settle(self.0.clone()));
            }
        }
    }
}

/// Runs at the deadline, and when the last call leaves after it. Withdraws the gate if nothing
/// waits for it and it has no answer; forgets it once nothing waits for it past its deadline.
async fn settle(gate: Arc<Gate>) {
    let unclaimed = {
        let s = gate.state.borrow();
        s.callers == 0 && s.outcome.is_none()
    };
    if unclaimed {
        gate.gateway
            .withdraw(&gate.session, &gate.request_id, NOT_ANSWERED)
            .await;
        // The session reports the withdrawal to the keeper too; setting it here means a call
        // that returns later never waits on an event that may not come.
        gate.state.send_modify(|s| {
            if s.outcome.is_none() {
                s.outcome = Some(AskUserReply::cancelled(NOT_ANSWERED));
            }
        });
    }
    let mut registry = gates();
    let key = (gate.session.clone(), gate.request_id.clone());
    let idle = gate.state.borrow().callers == 0;
    let ours = registry.get(&key).is_some_and(|g| Arc::ptr_eq(g, &gate));
    if idle && ours && Instant::now() >= gate.deadline {
        registry.remove(&key);
    }
}

async fn settle_at_deadline(gate: Arc<Gate>) {
    tokio::time::sleep_until(gate.deadline).await;
    settle(gate).await;
}

/// Follows the session for the gate's answer, for as long as the gate is open. Raises the
/// question first if the gate was not already pending (after the subscription, so the answer
/// cannot be missed).
async fn keep(gate: Arc<Gate>, from: u64, question: Option<QuestionPrompt>) {
    let outcome = watch_gate(&gate, from, question).await;
    gate.state.send_modify(|s| {
        if s.outcome.is_none() {
            s.outcome = Some(outcome);
        }
    });
}

async fn watch_gate(
    gate: &Gate,
    mut from: u64,
    mut question: Option<QuestionPrompt>,
) -> AskUserReply {
    loop {
        let mut events = match gate.gateway.events(&gate.session, from).await {
            Ok(events) => events,
            Err(e) => return AskUserReply::cancelled(format!("the session is gone: {e}")),
        };
        if let Some(q) = question.take() {
            if let Err(e) = gate.gateway.raise(&gate.session, &gate.request_id, q).await {
                return AskUserReply::cancelled(format!("cannot ask the user: {e}"));
            }
        }
        loop {
            match events.recv().await {
                None => return AskUserReply::cancelled("the session was deleted"),
                Some(GateEvent::Lagged) => break,
                Some(ev) => {
                    if let Some(c) = ev.cursor() {
                        from = c;
                    }
                    match ev {
                        GateEvent::Answered {
                            request_id,
                            answers,
                            ..
                        } if request_id == gate.request_id => {
                            return AskUserReply {
                                answers,
                                cancelled: None,
                            };
                        }
                        // The session took the gate down (its turn was interrupted or ended, or
                        // the process ended) or someone else did: the call is over either way.
                        GateEvent::Resolved {
                            request_id, detail, ..
                        } if request_id == gate.request_id => {
                            return AskUserReply::cancelled(detail);
                        }
                        GateEvent::SessionEnded { .. } => {
                            return AskUserReply::cancelled("the session ended before an answer");
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

/// `ask.user` with the production deadline.
pub async fn ask_user(
    manager: &SessionManager,
    r: AskUserRequest,
) -> Result<AskUserReply, MarsadError> {
    ask_through(Arc::new(manager.clone()), r, ANSWER_DEADLINE).await
}

/// `ask.user` against `gateway`: joins the gate for the request id, or raises it. Returns when
/// the gate has an answer, or with the reason it has none (a call returning late is told the
/// withdrawal). Only a call that cannot be started at all is an error.
pub async fn ask_through(
    gateway: Arc<dyn Gateway>,
    r: AskUserRequest,
    answer_deadline: Duration,
) -> Result<AskUserReply, MarsadError> {
    let AskUserRequest {
        session: session_str,
        request_id,
        mut question,
    } = r;
    let session = SessionId::parse(&session_str)
        .ok_or_else(|| MarsadError::invalid(format!("{session_str:?} is not a session id")))?;
    if request_id.trim().is_empty() || question.question.trim().is_empty() {
        return Err(MarsadError::invalid(
            "a question and a request id are required",
        ));
    }
    if question.id.is_empty() {
        question.id = "answer".into();
    }
    let key = (session.clone(), request_id.clone());
    let attached = {
        let mut registry = gates();
        let gate = match registry.get(&key) {
            Some(gate) => gate.clone(),
            None => {
                let (from, pending) =
                    gateway.snapshot_gate(&session, &request_id).map_err(|_| {
                        MarsadError::NotFound {
                            what: format!("session {session}"),
                        }
                    })?;
                let gate = Arc::new(Gate {
                    gateway: gateway.clone(),
                    session: session.clone(),
                    request_id: request_id.clone(),
                    deadline: Instant::now() + answer_deadline,
                    state: watch::channel(GateState::default()).0,
                });
                registry.insert(key, gate.clone());
                tokio::spawn(settle_at_deadline(gate.clone()));
                let raise = (!pending).then_some(question);
                tokio::spawn(keep(gate.clone(), from, raise));
                gate
            }
        };
        Attached::join(gate)
    };
    let mut rx = attached.0.state.subscribe();
    // The keeper or the deadline always sets the outcome, and this call holds the gate open.
    let reply = rx
        .wait_for(|s| s.outcome.is_some())
        .await
        .ok()
        .and_then(|s| s.outcome.clone())
        .unwrap_or_else(|| AskUserReply::cancelled("the question was lost"));
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

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = "abcdefghijkl";
    const LONG: Duration = Duration::from_secs(60);

    /// A session stand-in. The test plays the owner by sending events into the live
    /// subscription, and watches what the gate raised and withdrew.
    struct Fake {
        pending_at_start: bool,
        feed: Mutex<Option<mpsc::Sender<GateEvent>>>,
        raised: mpsc::UnboundedSender<String>,
        withdrawn: mpsc::UnboundedSender<String>,
    }

    type Seen = mpsc::UnboundedReceiver<String>;

    impl Fake {
        fn new(pending_at_start: bool) -> (Arc<Fake>, Seen, Seen) {
            let (raised, raised_rx) = mpsc::unbounded_channel();
            let (withdrawn, withdrawn_rx) = mpsc::unbounded_channel();
            let fake = Fake {
                pending_at_start,
                feed: Mutex::new(None),
                raised,
                withdrawn,
            };
            (Arc::new(fake), raised_rx, withdrawn_rx)
        }

        /// An event from the session, as the owner's answer or the session's resolution.
        async fn play(&self, ev: GateEvent) {
            let tx = self.feed.lock().unwrap().clone();
            tx.expect("the gate is watching the session")
                .send(ev)
                .await
                .unwrap();
        }
    }

    impl Gateway for Fake {
        fn snapshot_gate(&self, _: &SessionId, _: &str) -> Result<(u64, bool), String> {
            Ok((7, self.pending_at_start))
        }

        fn raise<'a>(
            &'a self,
            _: &'a SessionId,
            request_id: &'a str,
            _: QuestionPrompt,
        ) -> Fut<'a, Result<(), String>> {
            Box::pin(async move {
                let _ = self.raised.send(request_id.to_string());
                Ok(())
            })
        }

        fn events<'a>(
            &'a self,
            _: &'a SessionId,
            _from: u64,
        ) -> Fut<'a, Result<mpsc::Receiver<GateEvent>, String>> {
            Box::pin(async move {
                let (tx, rx) = mpsc::channel(16);
                *self.feed.lock().unwrap() = Some(tx);
                Ok(rx)
            })
        }

        fn withdraw<'a>(
            &'a self,
            _: &'a SessionId,
            request_id: &'a str,
            detail: &'static str,
        ) -> Fut<'a, ()> {
            Box::pin(async move {
                let _ = self.withdrawn.send(detail.to_string());
                // The session reports a withdrawal as a resolution, as the real one does.
                let tx = self.feed.lock().unwrap().clone();
                if let Some(tx) = tx {
                    let _ = tx
                        .send(GateEvent::Resolved {
                            cursor: 99,
                            request_id: request_id.to_string(),
                            detail: detail.to_string(),
                        })
                        .await;
                }
            })
        }
    }

    fn request(request_id: &str) -> AskUserRequest {
        AskUserRequest {
            session: SESSION.into(),
            request_id: request_id.into(),
            question: QuestionPrompt {
                id: "answer".into(),
                question: "Which database?".into(),
                header: String::new(),
                multi_select: false,
                allow_other: false,
                options: Vec::new(),
                secret: false,
            },
        }
    }

    /// The owner answers while the MCP caller is gone (its connection dropped and it has not
    /// re-sent yet). The re-sent call, with the same request id, gets that answer, and the
    /// question is not raised a second time.
    #[tokio::test]
    async fn a_dropped_caller_re_attaches_to_its_gate_and_gets_the_owners_answer() {
        let (fake, mut raised, mut withdrawn) = Fake::new(false);
        let gw: Arc<dyn Gateway> = fake.clone();
        let first = tokio::spawn(ask_through(gw.clone(), request("ask-reattach"), LONG));
        assert_eq!(raised.recv().await.as_deref(), Some("ask-reattach"));
        first.abort();
        let _ = first.await;

        fake.play(GateEvent::Answered {
            cursor: 8,
            request_id: "ask-reattach".into(),
            answers: vec!["postgres".into()],
        })
        .await;
        let reply = ask_through(gw, request("ask-reattach"), LONG)
            .await
            .expect("the re-sent call is accepted");
        assert_eq!(reply.answers, ["postgres"]);
        assert!(reply.cancelled.is_none());
        assert!(
            raised.try_recv().is_err(),
            "the re-sent call must join the gate, not raise the question again"
        );
        assert!(withdrawn.try_recv().is_err());
    }

    /// With no call waiting, the gate is withdrawn at its deadline, and not before.
    #[tokio::test]
    async fn an_unclaimed_gate_is_withdrawn_at_its_deadline_and_not_before() {
        let deadline = Duration::from_millis(400);
        let (fake, mut raised, mut withdrawn) = Fake::new(false);
        let gw: Arc<dyn Gateway> = fake.clone();
        let started = Instant::now();
        let first = tokio::spawn(ask_through(gw, request("ask-deadline"), deadline));
        assert_eq!(raised.recv().await.as_deref(), Some("ask-deadline"));
        // The caller goes away well before the deadline.
        first.abort();
        let _ = first.await;

        let detail = tokio::time::timeout(LONG, withdrawn.recv())
            .await
            .expect("the gate is withdrawn")
            .expect("the withdrawal is reported");
        assert!(
            started.elapsed() >= deadline,
            "withdrawn after {:?}, before the {deadline:?} deadline",
            started.elapsed()
        );
        assert_eq!(detail, NOT_ANSWERED);
    }
}
