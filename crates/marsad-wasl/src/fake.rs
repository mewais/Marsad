//! `FakeWaslConnector` — the agent's test seam (`docs/specs/wasl-connector.md`
//! §2.6).

use std::collections::VecDeque;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use crate::client::WaslConnector;
use crate::error::WaslError;
use crate::event::{DecisionAck, WaslEvent};

/// One recorded `decide()` call: `(gate_id, outcome, reason)`.
type DecisionLogEntry = (String, String, Option<String>);

/// Test fake: `push(event)` feeds the events channel handed to `start()`;
/// `decide` records `(gate_id, outcome, reason)` into an inspectable log and
/// returns a scripted ack (default `Delivered`; `script_ack(…)` overrides,
/// FIFO, including `Err`).
pub struct FakeWaslConnector {
    pub decisions: Arc<Mutex<Vec<DecisionLogEntry>>>,
    scripted: Mutex<VecDeque<Result<DecisionAck, WaslError>>>,
    tx: Mutex<Option<Sender<WaslEvent>>>,
}

impl FakeWaslConnector {
    #[allow(clippy::new_without_default)] // test fake: no test constructs it via `Default`.
    pub fn new() -> Self {
        Self {
            decisions: Arc::new(Mutex::new(Vec::new())),
            scripted: Mutex::new(VecDeque::new()),
            tx: Mutex::new(None),
        }
    }

    /// Feed one event onto the channel handed to `start()`.
    pub fn push(&self, event: WaslEvent) {
        let guard = self.tx.lock().unwrap();
        let tx = guard.as_ref().expect("push() called before start()");
        let _ = tx.send(event);
    }

    /// Queue one scripted ack/error for the next `decide()` call (FIFO).
    /// Once the queue is exhausted, `decide()` falls back to the
    /// `Delivered` default.
    pub fn script_ack(&self, ack: Result<DecisionAck, WaslError>) {
        self.scripted.lock().unwrap().push_back(ack);
    }
}

impl WaslConnector for FakeWaslConnector {
    fn start(&mut self, events: Sender<WaslEvent>) {
        *self.tx.lock().unwrap() = Some(events);
    }

    fn decide(
        &self,
        gate_id: &str,
        outcome: &str,
        reason: Option<&str>,
    ) -> Result<DecisionAck, WaslError> {
        self.decisions.lock().unwrap().push((
            gate_id.to_string(),
            outcome.to_string(),
            reason.map(str::to_string),
        ));
        self.scripted
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(DecisionAck::Delivered))
    }

    fn stop(&mut self) {
        *self.tx.lock().unwrap() = None;
    }
}
