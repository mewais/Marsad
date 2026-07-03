//! fakes.rs — `FakeWaslConnector`, the agent's test seam (spec §2.6 / §7.1):
//! pushed events arrive on the channel handed to `start()`; `decide` records
//! its arguments into an inspectable log and returns a scripted ack/error
//! (default `Delivered` when nothing is scripted); `stop` is idempotent.
//! Also proves `WaslConnector` is object-safe (`Box<dyn WaslConnector>`),
//! the seam the agent's registry actually stores.
//!
//! This file WILL fail to compile until a later worker implements
//! `marsad_wasl::{FakeWaslConnector, WaslConnector}` (§2.6) — expected
//! tests-first signal.
use std::sync::mpsc::channel;

use marsad_wasl::{DecisionAck, FakeWaslConnector, Hello, WaslConnector, WaslError, WaslEvent};

fn sample_hello() -> Hello {
    Hello {
        api_version: 1,
        agent_id: "wasl-a".to_string(),
        host_id: "host-1".to_string(),
        version: "0.1.0".to_string(),
        last_seq: 42,
        resume: "snapshot".to_string(),
    }
}

// ---------------------------------------------------------------------------
// push() feeds the channel handed to start(), in order.
// ---------------------------------------------------------------------------

#[test]
fn pushed_events_arrive_on_the_started_channel_in_order() {
    let mut fake = FakeWaslConnector::new();
    let (tx, rx) = channel();
    fake.start(tx);

    fake.push(WaslEvent::Connected {
        hello: sample_hello(),
    });
    fake.push(WaslEvent::GateVanished {
        gate_id: "gate-1".to_string(),
    });
    fake.push(WaslEvent::Disconnected {
        error: "boom".to_string(),
    });

    assert_eq!(
        rx.recv().unwrap(),
        WaslEvent::Connected {
            hello: sample_hello()
        }
    );
    assert_eq!(
        rx.recv().unwrap(),
        WaslEvent::GateVanished {
            gate_id: "gate-1".to_string()
        }
    );
    assert_eq!(
        rx.recv().unwrap(),
        WaslEvent::Disconnected {
            error: "boom".to_string()
        }
    );
}

// ---------------------------------------------------------------------------
// decide() records (gate_id, outcome, reason) and defaults to Delivered.
// ---------------------------------------------------------------------------

#[test]
fn decide_defaults_to_delivered_when_nothing_scripted() {
    let fake = FakeWaslConnector::new();
    let ack = fake
        .decide("gate-1", "approved", None)
        .expect("default ack must be Ok");
    assert_eq!(ack, DecisionAck::Delivered);
}

#[test]
fn decide_records_approve_and_deny_with_reason() {
    let fake = FakeWaslConnector::new();
    fake.decide("gate-1", "approved", None).unwrap();
    fake.decide("gate-2", "denied", Some("not during business hours"))
        .unwrap();

    let log = fake.decisions.lock().unwrap().clone();
    assert_eq!(
        log,
        vec![
            ("gate-1".to_string(), "approved".to_string(), None),
            (
                "gate-2".to_string(),
                "denied".to_string(),
                Some("not during business hours".to_string())
            ),
        ]
    );
}

// ---------------------------------------------------------------------------
// script_ack() overrides the next decide() call(s), FIFO; falls back to the
// Delivered default once the scripted queue is exhausted. Also covers
// scripting an Err (transport/400-class failure).
// ---------------------------------------------------------------------------

#[test]
fn script_ack_overrides_in_fifo_order_then_falls_back_to_default() {
    let fake = FakeWaslConnector::new();
    fake.script_ack(Ok(DecisionAck::AlreadyDecided));
    fake.script_ack(Ok(DecisionAck::UnknownGate));

    assert_eq!(
        fake.decide("g1", "approved", None).unwrap(),
        DecisionAck::AlreadyDecided
    );
    assert_eq!(
        fake.decide("g2", "approved", None).unwrap(),
        DecisionAck::UnknownGate
    );
    // Queue exhausted: back to the Delivered default.
    assert_eq!(
        fake.decide("g3", "approved", None).unwrap(),
        DecisionAck::Delivered
    );
}

#[test]
fn script_ack_can_script_an_error() {
    let fake = FakeWaslConnector::new();
    fake.script_ack(Err(WaslError::Protocol("boom".to_string())));

    let err = fake
        .decide("g4", "denied", Some("no"))
        .expect_err("scripted Err must surface");
    assert!(matches!(err, WaslError::Protocol(_)));

    // The attempt is still recorded even though the ack was an error.
    let log = fake.decisions.lock().unwrap().clone();
    assert_eq!(
        log,
        vec![(
            "g4".to_string(),
            "denied".to_string(),
            Some("no".to_string())
        )]
    );
}

// ---------------------------------------------------------------------------
// stop() is idempotent.
// ---------------------------------------------------------------------------

#[test]
fn stop_is_idempotent_and_does_not_panic() {
    let mut fake = FakeWaslConnector::new();
    fake.stop();
    fake.stop();
    // Still usable afterwards — stop() doesn't poison the fake.
    assert_eq!(
        fake.decide("gate-1", "approved", None).unwrap(),
        DecisionAck::Delivered
    );
}

// ---------------------------------------------------------------------------
// WaslConnector is object-safe — the agent stores connectors as
// Box<dyn WaslConnector> (§4.2's `wasl_connectors` registry).
// ---------------------------------------------------------------------------

#[test]
fn fake_wasl_connector_is_usable_as_a_boxed_trait_object() {
    let fake = FakeWaslConnector::new();
    let decisions = fake.decisions.clone();

    let mut boxed: Box<dyn WaslConnector> = Box::new(fake);
    let (tx, _rx) = channel();
    boxed.start(tx);

    let ack = boxed
        .decide("gate-5", "approved", None)
        .expect("decide through trait object");
    assert_eq!(ack, DecisionAck::Delivered);
    assert_eq!(
        decisions.lock().unwrap().as_slice(),
        &[("gate-5".to_string(), "approved".to_string(), None)]
    );

    boxed.stop();
    boxed.stop(); // idempotent through the trait object too
}
