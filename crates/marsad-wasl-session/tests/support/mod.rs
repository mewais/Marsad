//! support/mod.rs — shared, dependency-free test helpers for the
//! `marsad-wasl-session` black-box suite. No IO, no real process, no sockets —
//! everything runs against the in-memory `FakeWaslSessionClient` (plan §5).
//!
//! Kept intentionally small: constructors for the common domain values plus a
//! `drain` helper that pulls a `WaslEventStream` to end-of-stream. Mirrors the
//! `support::` convention `marsad-wasl/tests/` uses, minus the HTTP mock server
//! (there is no wire here — the fake is the whole substrate).
//!
//! v3 corrections applied (plan revision log v3): `WaslNativeGateDetail` is only
//! `{summary, preview}` (no `gate_id`/`tool`); `WaslTurnRequest` carries a fresh
//! single-hop `WaslCausation`; and `WaslEventStream::next()` yields a
//! `WaslStreamItem` (an `Event` or a distinct `Truncated`), so `drain` unwraps
//! the `Event(..)` variant and a `drain_items` helper is added for tests that
//! must see `Truncated` distinctly.

#![allow(dead_code)]

use marsad_wasl_session::{
    WaslBusyPolicy, WaslCapabilities, WaslCausation, WaslEventStream, WaslNativeGateDetail,
    WaslSessionEvent, WaslStreamItem, WaslTargetStatus, WaslTurnRequest,
};

/// A fully-capable session (every RuntimeCapability true). Informational only —
/// capabilities never affect admission or the status projection (plan §6.3).
pub fn all_caps() -> WaslCapabilities {
    WaslCapabilities {
        persistent_process: true,
        persistent_conversation: true,
        structured_turns: true,
        streaming_output: true,
        attachable: true,
        interruptible: true,
        structured_provenance: true,
        resumable: true,
    }
}

/// A session that declares it cannot be interrupted (`interruptible: false`),
/// all other caps true — for the "capabilities are informational" scenarios.
pub fn not_interruptible_caps() -> WaslCapabilities {
    WaslCapabilities {
        interruptible: false,
        ..all_caps()
    }
}

/// Idle-and-ready status with a fixed timestamp (Marsad never does clock math).
pub fn idle_status() -> WaslTargetStatus {
    WaslTargetStatus::Idle {
        since: "2026-07-02T12:00:00.000Z".to_string(),
    }
}

/// Busy status with a fixed active turn, no queued turns.
pub fn busy_status() -> WaslTargetStatus {
    WaslTargetStatus::Busy {
        active_turn: "t-active".to_string(),
        source: "LocalHuman".to_string(),
        phase: "generating".to_string(),
        since: "2026-07-02T12:00:01.000Z".to_string(),
        queued: 0,
    }
}

/// A native-gate detail (v3: only `{summary, preview}` — the real
/// `NativeGateDetail` has NO `gate_id` and NO `tool`, plan §4.4/§8).
pub fn gate_detail() -> WaslNativeGateDetail {
    WaslNativeGateDetail {
        summary: "delete the build dir".to_string(),
        preview: "$ rm -rf ./build".to_string(),
    }
}

/// One human prompt with the given idempotency key + text, carrying a fresh
/// single-hop causation (plan §4.4: Marsad only ever originates root turns).
pub fn sample_request(key: &str, text: &str) -> WaslTurnRequest {
    WaslTurnRequest {
        idempotency_key: key.to_string(),
        text: text.to_string(),
        causation: WaslCausation::fresh(format!("root-{key}")),
    }
}

/// The default policy for a human prompt (plan §12 Q4: Queue with bounded wait).
pub fn queue_policy() -> WaslBusyPolicy {
    WaslBusyPolicy::Queue { max_wait_ms: 5_000 }
}

pub fn reject_policy() -> WaslBusyPolicy {
    WaslBusyPolicy::Reject
}

pub fn queue_related_policy() -> WaslBusyPolicy {
    WaslBusyPolicy::QueueRelated { max_wait_ms: 5_000 }
}

/// Pull a stream to clean end-of-stream, collecting every stream ITEM verbatim
/// (v3: `next()` yields `WaslStreamItem` — either an `Event` or a distinct
/// `Truncated`). Use this when a test must observe `Truncated` items too.
pub fn drain_items(stream: &mut dyn WaslEventStream) -> Vec<WaslStreamItem> {
    let mut out = Vec::new();
    while let Some(item) = stream.next() {
        out.push(item);
    }
    out
}

/// Pull a stream to clean end-of-stream, collecting only the normal `Event`s
/// (unwrapping `WaslStreamItem::Event`). A `Truncated` control item in the
/// stream PANICS — callers that expect a truncation must use [`drain_items`]
/// instead, so a truncation can never be silently folded into the event list
/// (plan §4.4: a truncation is an unmissable signal, never mixed with events).
pub fn drain(stream: &mut dyn WaslEventStream) -> Vec<WaslSessionEvent> {
    let mut out = Vec::new();
    for item in drain_items(stream) {
        match item {
            WaslStreamItem::Event(ev) => out.push(ev),
            WaslStreamItem::Truncated { earliest, latest } => panic!(
                "drain() saw an unexpected Truncated{{earliest:{earliest}, latest:{latest}}}; \
                 a truncation is a distinct signal — use drain_items() when one is expected"
            ),
        }
    }
    out
}
