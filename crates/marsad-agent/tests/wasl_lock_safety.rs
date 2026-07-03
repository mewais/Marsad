//! wasl_lock_safety.rs — regression coverage for review finding #1
//! ([CRITICAL] a hung/slow Wasl operator freezes the entire marsad-agent
//! daemon): `handle_answer_wasl_gate` must not hold the agent's
//! `wasl_connectors` registry lock across the (blocking, network-bound)
//! `connector.decide()` call.
//!
//! This is proven without threads (the `Agent` type is deliberately not
//! `Send`-across-threads-shareable — see `serve.rs`'s "Concurrency model"
//! doc comment) via `Mutex` POISONING: if `decide()` panics WHILE the
//! registry lock is held, that lock is left poisoned and every subsequent
//! operation touching the registry (e.g. answering a gate on a different,
//! still-healthy host) panics too on `.lock().unwrap()`. If the lock is
//! correctly released before `decide()` runs, a panic inside `decide()`
//! can't poison it, and the registry stays fully usable afterward.
use std::sync::mpsc::Sender;

use marsad_agent::{Agent, AgentConfig, ListenSpec};
use marsad_lifecycle::FakeContainer;
use marsad_notify::CaptureNotifier;
use marsad_proto::{encode_ipc, parse_ipc, IpcMsg};
use marsad_store::{MemoryBackend, Store};
use marsad_wasl::{DecisionAck, FakeWaslConnector, WaslConnector, WaslError, WaslEvent};

fn make_agent() -> Agent {
    Agent::with_fakes(
        AgentConfig {
            listen: ListenSpec::Stdio,
            data_root: "/tmp/marsad-agent-wasl-lock-safety-test".to_string(),
        },
        Box::new(FakeContainer::new()),
        Box::new(CaptureNotifier::new()),
        Box::new(Store::new(Box::new(MemoryBackend::new()))),
    )
    .expect("Agent::with_fakes")
}

fn answer_wasl_gate_line(host_id: &str, gate_id: &str, outcome: &str) -> String {
    encode_ipc(&IpcMsg::AnswerWaslGate {
        host_id: host_id.to_string(),
        gate_id: gate_id.to_string(),
        outcome: outcome.to_string(),
        reason: None,
    })
}

/// A connector whose `decide()` always panics — stands in for "the blocking
/// network call blew up" (a real timeout/IO error wouldn't panic in
/// production, but a panic is the sharpest tool for detecting, from purely
/// black-box observations, whether a `Mutex` was held across the call).
struct PanickingConnector;

impl WaslConnector for PanickingConnector {
    fn start(&mut self, _events: Sender<WaslEvent>) {}

    fn decide(
        &self,
        _gate_id: &str,
        _outcome: &str,
        _reason: Option<&str>,
    ) -> Result<DecisionAck, WaslError> {
        panic!("simulated catastrophic decide() failure");
    }

    fn stop(&mut self) {}
}

#[test]
fn decide_panic_does_not_poison_the_connectors_registry_lock() {
    let mut agent = make_agent();
    agent.register_wasl_connector("wedged-host", Box::new(PanickingConnector));

    let line = answer_wasl_gate_line("wedged-host", "gate-1", "approved");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| agent.handle_ipc(&line)));
    assert!(
        result.is_err(),
        "sanity check: decide() panicking must propagate out of handle_ipc"
    );

    // If `handle_answer_wasl_gate` held the `wasl_connectors` mutex across
    // the `decide()` call (the bug), the panic above unwound through the
    // held `MutexGuard`, poisoning it — every future `.lock().unwrap()` on
    // that same registry (incl. from an entirely different, healthy host)
    // would itself panic. Prove the registry is still fully usable: both
    // registering a new connector AND answering a gate on it must succeed.
    agent.register_wasl_connector("healthy-host", Box::new(FakeWaslConnector::new()));
    let line2 = answer_wasl_gate_line("healthy-host", "gate-2", "approved");
    let out = agent.handle_ipc(&line2).unwrap_or_else(|e| {
        panic!(
            "wasl_connectors registry must still be usable after a decide() panic on an \
             unrelated host — a poisoned lock here proves handle_answer_wasl_gate held the \
             registry mutex across the blocking decide() call. Got error: {e}"
        )
    });
    assert_eq!(out.len(), 1);
    assert_eq!(
        parse_ipc(&out[0]).unwrap(),
        IpcMsg::WaslGateAnswered {
            host_id: "healthy-host".to_string(),
            gate_id: "gate-2".to_string(),
        }
    );
}
