//! wasl_session_observe.rs — Wasl **Backend** per-session OBSERVE-THREAD pump.
//!
//! Authoritative design: `docs/design/wasl-backend-plan.md` (v3), especially
//! §4.3 (control surface), §5.1-style scripted fake driving, §6.1 (the observe
//! thread that pumps `attach(after_seq)` and broadcasts mapped IPC lines to
//! subscribers), the §6.1 reconnect note (resume from a persisted `last_seq`),
//! and §4.4's `WaslStreamItem::Truncated{earliest, latest}` "re-snapshot +
//! resubscribe from `latest`" contract.
//!
//! ── WHY THIS FILE EXISTS (audit gap being closed) ────────────────────────────
//! The existing `tests/wasl_session.rs` proves the pure event/status MAPPING
//! helpers (`wasl_session_event_lines` / `wasl_session_status_line`) and the six
//! synchronous `*Wasl*` `handle_ipc` arms — but it deliberately does NOT prove
//! any actual OBSERVE THREAD, punting it to `serve()` ("spawned only in
//! `serve()` … unreachable from a unit test"). The audit found the consequence:
//! `serve()` registers the provider factory but NEVER spawns a per-session
//! thread that pumps a launched Wasl session's `attach(after_seq)` event stream
//! into the subscriber broadcast list. Without it a real launched Wasl-managed
//! session accepts prompts but NO reply / transcript / native-gate / status
//! change ever reaches core at runtime — the feature is not functional
//! end-to-end. Separately, `WaslStreamItem::Truncated` is surfaced by the client
//! (100% tested there) but NOTHING consumes it to re-attach from `latest`.
//!
//! This file specifies, TEST-FIRST, the observe-pump logic that closes both
//! gaps. It mirrors how `reader.rs`'s per-session thread is made testable: the
//! spawned thread + real broadcast list + backoff sleep stay in the
//! `#[cfg(not(coverage))]` `serve()` layer, but the LOAD-BEARING pump logic is
//! extracted into a pure, synchronous helper proven here against the in-repo
//! `FakeWaslSessionClient` (exactly as `reconnect_backoff_next` / `conn_on_drop`
//! are the pure, unit-tested cores of `spawn_session_reader`). No socket, no real
//! process — the fake IS the substrate (this crate's fakes-only house rule).
//!
//! ── INVENTED / ASSUMED PRODUCTION SURFACE (does not exist yet) ────────────────
//! `docs/design/wasl-backend-plan.md` is a DESIGN doc; there is no
//! `docs/specs/wasl-backend.md` pinning exact Rust seam names for the observe
//! thread. §6.1 pins the BEHAVIOR ("a per-session observe thread pumps `attach`'s
//! `WaslEventStream` and broadcasts mapped `IpcMsg` lines … byte-identical
//! mechanics to `spawn_session_reader` and to X1's forwarder threads"), but not
//! the seam. This file assumes the pump seam below, chosen to mirror `reader.rs`'s
//! testable-pure-core precedent AND to reuse the ALREADY-BUILT
//! `wasl_session_event_lines` / `wasl_session_status_line` mappers verbatim (the
//! pump must NOT reinvent the mapping — design step 4 of the brief). If the
//! code-agent lands a differently-shaped seam, it should adjust these call sites;
//! the BEHAVIOR pinned here is the contract, not the names:
//!
//!   1. A pure, synchronous pump:
//!
//!        pub fn pump_wasl_observe(
//!            session_id: &str,
//!            client: &dyn marsad_wasl_session::WaslSessionClient,
//!            after_seq: u64,
//!            emit: &mut dyn FnMut(String),
//!        ) -> WaslObserveEnd
//!
//!      It calls `client.attach(after_seq)`, loops `next()`, and for each item:
//!        • `WaslStreamItem::Event(ev)` → for every line
//!          `wasl_session_event_lines(session_id, ev.seq, &ev)` produces, calls
//!          `emit(line)`; records `ev.seq` as the new high-water mark; and if the
//!          event is a TERMINAL one (`SessionExited`), notes that a terminal was
//!          seen.
//!        • `WaslStreamItem::Truncated{earliest, latest}` → does NOT emit an event
//!          line for it; instead RE-ATTACHES via `client.attach(latest)`
//!          (discarding the old stream) and keeps pumping from there — so a
//!          retention-window truncation is transparently recovered (§4.4).
//!        • `None` (clean end-of-stream) → returns, reporting the high-water seq
//!          and whether a terminal event was seen.
//!
//!      Return type carrying the pump's verdict:
//!
//!        pub struct WaslObserveEnd {
//!            /// Highest event seq emitted (the resume cursor for a reconnect,
//!            /// and what a `WaslSessionBinding.last_seq` would persist — §6.1).
//!            pub last_seq: u64,
//!            /// True iff a TERMINAL event (`SessionExited`) was observed before
//!            /// the stream ended — the session is genuinely gone.
//!            pub terminal: bool,
//!        }
//!
//!   2. A pure reconnect-policy decision (the Wasl analogue of
//!      `reader.rs::conn_on_probe`, deciding what the observe THREAD does when
//!      `pump_wasl_observe` returns):
//!
//!        pub fn wasl_observe_should_reconnect(end: &WaslObserveEnd) -> bool
//!
//!      RECONNECT-ON-DISCONNECT POLICY (design choice, reasoned in the report):
//!      Marsad does NOT own the Wasl process, so a stream that ended AFTER a
//!      terminal `SessionExited` means the session is truly gone — the thread
//!      ends gracefully and does NOT reconnect (reconnecting would attach to
//!      nothing). A stream that ended WITHOUT a terminal event is a genuine
//!      transport disconnect while the Wasl session may still be alive; per §6.1
//!      the thread should re-`attach(last_seq)` (resume with no gap/dup via Wasl's
//!      `events(after_seq)` replay), bounded by `reader.rs`'s existing
//!      `reconnect_backoff_next` / `RECONNECT_*` constants (reused, not
//!      re-invented). Hence: reconnect ⇔ NOT terminal.
//!
//! This file WILL fail to compile / link until the code-agent lands
//! `pump_wasl_observe`, `WaslObserveEnd`, and `wasl_observe_should_reconnect`
//! (re-exported at the crate root next to `wasl_session_event_lines`). That is
//! the expected tests-first RED signal.

use marsad_agent::{
    pump_wasl_observe, wasl_observe_should_reconnect, WaslObserveEnd,
};
use marsad_proto::{parse_ipc, IpcMsg};
use marsad_wasl_session::{
    FakeWaslSessionClient, WaslCapabilities, WaslSessionEvent, WaslSessionEventKind,
    WaslTargetStatus,
};

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn all_caps() -> WaslCapabilities {
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

/// Drain the observe pump against `client`, collecting every emitted broadcast
/// line into a Vec (the test's stand-in for the subscriber list the real thread
/// broadcasts to). Returns `(lines, end)`.
fn drain(
    session_id: &str,
    client: &FakeWaslSessionClient,
    after_seq: u64,
) -> (Vec<String>, WaslObserveEnd) {
    let mut lines: Vec<String> = Vec::new();
    let end = pump_wasl_observe(session_id, client, after_seq, &mut |l| lines.push(l));
    (lines, end)
}

/// Parse every emitted line as a `WaslSessionEvent` IPC line, returning the
/// (seq, typed-event) pairs in order. Panics if a line isn't a
/// `WaslSessionEvent` — the pump must emit ONLY event lines for events (status
/// lines are emitted through a different path; see the status test below).
fn parsed_events(lines: &[String]) -> Vec<(u64, WaslSessionEvent)> {
    lines
        .iter()
        .map(|l| match parse_ipc(l).expect("emitted line must be a valid IpcMsg") {
            IpcMsg::WaslSessionEvent { seq, event, .. } => {
                let ev: WaslSessionEvent = serde_json::from_value(event)
                    .expect("event Value must re-parse to the typed WaslSessionEvent");
                (seq, ev)
            }
            other => panic!("expected WaslSessionEvent line, got {other:?}"),
        })
        .collect()
}

// ===========================================================================
// Part 1 — the pump broadcasts each event through the EXISTING mapping helper
// ===========================================================================

/// The pump maps every `Event` item to the SAME broadcast line
/// `wasl_session_event_lines` produces, carrying the event's own `seq`, and
/// re-parseable to the exact typed event. This is the observe thread's core
/// responsibility (§6.1): a launched session's reply/transcript reaches core.
#[test]
fn pump_broadcasts_each_event_as_a_wasl_session_event_line() {
    let client = FakeWaslSessionClient::new("target-pump", all_caps());
    client.script_happy_turn("turn-1", "hello world");

    let (lines, end) = drain("sess-pump", &client, 0);

    let evs = parsed_events(&lines);
    // script_happy_turn = TurnAccepted(seq1) → AssistantDelta(seq2) → TurnCompleted(seq3).
    assert_eq!(evs.len(), 3, "every scripted event must be broadcast once");
    assert_eq!(evs[0].0, 1);
    assert_eq!(evs[1].0, 2);
    assert_eq!(evs[2].0, 3);
    assert!(matches!(
        evs[0].1.kind,
        WaslSessionEventKind::TurnAccepted { .. }
    ));
    assert!(matches!(
        evs[1].1.kind,
        WaslSessionEventKind::AssistantDelta { .. }
    ));
    assert!(matches!(
        evs[2].1.kind,
        WaslSessionEventKind::TurnCompleted { .. }
    ));

    // A happy turn is not a session exit → not terminal; the high-water seq is 3.
    assert_eq!(end.last_seq, 3, "the pump tracks the highest emitted seq");
    assert!(
        !end.terminal,
        "a completed turn is NOT a terminal session-exit"
    );
}

/// The pump honors `attach(after_seq)`: starting from a non-zero cursor, it
/// broadcasts ONLY events past that cursor (no gap, no dup) — the exact resume
/// behavior a §6.1 reconnect from a persisted `last_seq` relies on.
#[test]
fn pump_resumes_from_after_seq_without_gap_or_dup() {
    let client = FakeWaslSessionClient::new("target-resume", all_caps());
    for i in 1..=5 {
        client.script_event(WaslSessionEventKind::AssistantDelta {
            turn_id: "t1".to_string(),
            text: format!("chunk-{i}"),
        });
    }

    let (lines, end) = drain("sess-resume", &client, 2);

    let seqs: Vec<u64> = parsed_events(&lines).into_iter().map(|(s, _)| s).collect();
    assert_eq!(
        seqs,
        vec![3, 4, 5],
        "pumping from after_seq=2 must broadcast exactly the newer events, once each"
    );
    assert_eq!(end.last_seq, 5);
    assert!(!end.terminal);
}

// ===========================================================================
// Part 2 — Truncated mid-stream → re-attach from `latest` → keep delivering
//
// THE load-bearing new consumer: nothing today re-snapshots/resubscribes on a
// `WaslStreamItem::Truncated`. This proves the pump transparently recovers.
// ===========================================================================

/// A `Truncated{earliest, latest}` mid-stream is NOT emitted as an event line;
/// the pump RE-ATTACHES from `latest` and continues delivering the subsequent
/// events. Proven end-to-end against a fake scripted with events, then a
/// truncation, then more events: the pump must deliver the events that live at
/// seqs above `latest`, and never crash / silently stop.
#[test]
fn pump_recovers_from_a_truncation_by_reattaching_from_latest() {
    let client = FakeWaslSessionClient::new("target-trunc", all_caps());
    // Seqs 1,2 are the "old" events. The retention window then truncates: the
    // cursor floor moves to earliest=1, latest=2 (everything <=2 rolled off).
    // Seqs 3,4 are the "new" events that must still arrive after the re-attach.
    client.script_event(WaslSessionEventKind::AssistantDelta {
        turn_id: "t1".to_string(),
        text: "old-1".to_string(),
    });
    client.script_event(WaslSessionEventKind::AssistantDelta {
        turn_id: "t1".to_string(),
        text: "old-2".to_string(),
    });
    client.script_truncated(1, 2);
    client.script_event(WaslSessionEventKind::AssistantDelta {
        turn_id: "t1".to_string(),
        text: "new-3".to_string(),
    });
    client.script_event(WaslSessionEventKind::TurnCompleted {
        turn_id: "t1".to_string(),
        reply: "done".to_string(),
        certainty: "committed".to_string(),
    });

    // Start from BELOW the eventual floor so the first attach yields the old
    // events then the truncation control item (the fake surfaces Truncated only
    // when the cursor is below `earliest`).
    let (lines, end) = drain("sess-trunc", &client, 0);

    let evs = parsed_events(&lines);
    // No emitted line may be a mis-folded truncation — parsed_events already
    // guarantees every line is a WaslSessionEvent (a Truncated would have to be
    // dropped, never turned into an event line). The pump must have re-attached
    // from latest=2 and delivered the new events (seqs 3, 4).
    let seqs: Vec<u64> = evs.iter().map(|(s, _)| *s).collect();
    assert!(
        seqs.contains(&3) && seqs.contains(&4),
        "after a truncation the pump must re-attach from `latest` and keep \
         delivering later events; got seqs {seqs:?}"
    );
    assert!(
        !seqs.is_empty(),
        "the pump must not silently stop when it hits a truncation"
    );
    // The last event past the truncation is a completed turn — not a session
    // exit — so the pump ends non-terminal at the new high-water mark.
    assert_eq!(
        end.last_seq, 4,
        "the resume cursor advances past the truncation to the newest event"
    );
    assert!(!end.terminal);
}

/// After the pump re-attaches from `latest`, a stale truncation must NOT be
/// re-surfaced — otherwise the pump would re-snapshot forever. Proven by the
/// convergence of the recovery: the drain in the test above terminates (does not
/// hang / loop). Here we pin it explicitly with a truncation whose window has NO
/// newer events: the pump re-attaches from `latest`, sees a clean end, and stops.
#[test]
fn pump_truncation_recovery_converges_to_clean_end_when_no_newer_events() {
    let client = FakeWaslSessionClient::new("target-trunc2", all_caps());
    client.script_truncated(100, 140);

    let (lines, end) = drain("sess-trunc2", &client, 0);

    assert!(
        lines.is_empty(),
        "a truncation with no newer events must produce zero broadcast lines \
         (the truncation itself is never an event line), and must not loop"
    );
    // Re-attaching from latest=140 yields a clean end; the high-water seq is the
    // resubscribe point (nothing newer was emitted).
    assert!(
        !end.terminal,
        "a bare truncation is not a session exit — not terminal"
    );
}

// ===========================================================================
// Part 3 — stream end: terminal (SessionExited) vs bare disconnect
// ===========================================================================

/// A stream that ends AFTER a terminal `SessionExited` reports `terminal = true`
/// — the session is genuinely gone. The pump ends gracefully (returns; no panic,
/// no busy-loop) and the terminal event is broadcast like any other.
#[test]
fn pump_ends_terminal_after_session_exited() {
    let client = FakeWaslSessionClient::new("target-exit", all_caps());
    client.script_event(WaslSessionEventKind::TurnCompleted {
        turn_id: "t1".to_string(),
        reply: "bye".to_string(),
        certainty: "committed".to_string(),
    });
    client.script_event(WaslSessionEventKind::SessionExited { code: Some(0) });

    let (lines, end) = drain("sess-exit", &client, 0);

    let evs = parsed_events(&lines);
    assert!(
        matches!(
            evs.last().map(|(_, e)| &e.kind),
            Some(WaslSessionEventKind::SessionExited { .. })
        ),
        "the terminal SessionExited must be broadcast before the pump ends"
    );
    assert!(
        end.terminal,
        "a SessionExited before end-of-stream marks the pump terminal"
    );
}

/// A `script_crash` (Faulted status + terminal SessionExited) also ends the pump
/// terminal — the crash reaches core as a broadcast event, then the pump stops.
#[test]
fn pump_ends_terminal_on_a_scripted_crash() {
    let client = FakeWaslSessionClient::new("target-crash", all_caps());
    client.script_crash(Some(137));

    let (lines, end) = drain("sess-crash", &client, 0);

    let evs = parsed_events(&lines);
    assert!(
        evs.iter()
            .any(|(_, e)| matches!(e.kind, WaslSessionEventKind::SessionExited { .. })),
        "a crash must broadcast a terminal SessionExited"
    );
    assert!(end.terminal, "a crash is terminal");
}

/// A stream that ends WITHOUT any terminal event is a genuine transport
/// disconnect (the Wasl session may still be alive). The pump reports
/// `terminal = false` so the thread can decide to reconnect (§6.1).
#[test]
fn pump_ends_non_terminal_on_a_bare_disconnect() {
    let client = FakeWaslSessionClient::new("target-drop", all_caps());
    // A mid-turn delta with NO completion / exit: the stream just ends.
    client.script_event(WaslSessionEventKind::AssistantDelta {
        turn_id: "t1".to_string(),
        text: "partial".to_string(),
    });

    let (_lines, end) = drain("sess-drop", &client, 0);

    assert_eq!(end.last_seq, 1);
    assert!(
        !end.terminal,
        "a stream ending with no terminal event is a bare disconnect, not a session exit"
    );
}

/// An empty stream (no events at all) ends cleanly and non-terminally — no
/// panic, no busy-loop, zero broadcast lines.
#[test]
fn pump_on_empty_stream_ends_cleanly() {
    let client = FakeWaslSessionClient::new("target-empty", all_caps());
    let (lines, end) = drain("sess-empty", &client, 0);
    assert!(lines.is_empty());
    assert_eq!(end.last_seq, 0);
    assert!(!end.terminal);
}

// ===========================================================================
// Part 4 — the reconnect-on-disconnect policy decision (pure)
// ===========================================================================

/// The policy: reconnect ⇔ the pump ended NON-terminally (a bare disconnect
/// while the Wasl session may still be alive). A terminal exit must NOT
/// reconnect (Marsad does not own the process; there is nothing to reconnect
/// to). This is the Wasl analogue of `reader.rs::conn_on_probe`'s "Dead never
/// reconnects, a still-alive-but-unreachable host keeps trying" rule.
#[test]
fn reconnect_policy_reconnects_only_on_a_bare_disconnect() {
    let bare = WaslObserveEnd {
        last_seq: 7,
        terminal: false,
    };
    let terminal = WaslObserveEnd {
        last_seq: 9,
        terminal: true,
    };
    assert!(
        wasl_observe_should_reconnect(&bare),
        "a bare disconnect (session maybe alive) must trigger a resume-reconnect"
    );
    assert!(
        !wasl_observe_should_reconnect(&terminal),
        "a terminal session-exit must NOT reconnect — the process is Wasl's and is gone"
    );
}

// ===========================================================================
// Part 5 — absent / inert regression pin (§2 load-bearing optionality)
// ===========================================================================

/// The pump is a pure function driven only when a Wasl session is actually
/// launched and observed. A client whose stream is empty (the "never produced an
/// event" case) drives ZERO broadcast lines — structurally proving that with no
/// Wasl session ever launched, no observe work and no broadcast traffic occurs.
/// (The agent-level "no session launched ⇒ no observe thread" invariant holds
/// structurally because `pump_wasl_observe` is only ever called with a client
/// obtained from a `LaunchWaslSession`; this pins that an un-driven pump emits
/// nothing.)
#[test]
fn an_unobserved_session_emits_no_broadcast_lines() {
    let client = FakeWaslSessionClient::new("target-inert", all_caps());
    // Put the client in a live-but-idle status; do NOT script any stream events.
    client.script_status(WaslTargetStatus::Idle {
        since: String::new(),
    });

    let (lines, end) = drain("sess-inert", &client, 0);

    assert!(
        lines.is_empty(),
        "an idle session with no stream events must produce zero broadcast lines"
    );
    assert_eq!(end.last_seq, 0);
    assert!(!end.terminal);
}
