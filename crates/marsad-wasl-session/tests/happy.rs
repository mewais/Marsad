//! happy.rs — the nominal caller-side flows the whole phase is about
//! (test plan §10.1): prompt-when-idle, the full native-gate resolution loop,
//! the provider's idempotent `get_or_start` / `list`, and the §6.1 reconnect
//! scenario — all against the in-memory fake (no real Wasl actor anywhere).
//!
//! This file WILL fail to compile until a later worker implements
//! `marsad_wasl_session`'s public surface (§4/§5) — expected tests-first signal.
mod support;

use marsad_wasl_session::{
    FakeWaslSessionClient, FakeWaslSessionProvider, WaslGateDecision, WaslSessionClient,
    WaslSessionEventKind, WaslSessionProvider, WaslStreamItem, WaslTargetKey, WaslTargetStatus,
    WaslTurnId, WaslTurnReceipt,
};

use support::{all_caps, drain, gate_detail, idle_status, sample_request};

// ---------------------------------------------------------------------------
// prompt-when-idle: Idle → Started, then TurnAccepted → AssistantDelta →
// TurnCompleted drains from attach.
// ---------------------------------------------------------------------------

#[test]
fn prompt_when_idle_succeeds_and_the_happy_turn_drains_from_attach() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());
    fake.script_status(idle_status());

    // The atomic admission: Started (default when nothing else is scripted).
    let receipt = fake.prompt(sample_request("k1", "hello"), support::queue_policy());
    match receipt {
        WaslTurnReceipt::Started { .. } => {}
        other => panic!("prompt-when-idle must return Started, got {other:?}"),
    }

    // The reply arrives via the event stream, not the receipt.
    fake.script_event(WaslSessionEventKind::TurnAccepted {
        turn_id: "t1".to_string(),
        source: "LocalHuman".to_string(),
    });
    fake.script_event(WaslSessionEventKind::AssistantDelta {
        turn_id: "t1".to_string(),
        text: "hi there".to_string(),
    });
    fake.script_event(WaslSessionEventKind::TurnCompleted {
        turn_id: "t1".to_string(),
        reply: "hi there".to_string(),
        certainty: "committed".to_string(),
    });

    let mut stream = fake.attach(0);
    let events = drain(&mut *stream);
    assert_eq!(events.len(), 3);
    assert!(matches!(
        events[0].kind,
        WaslSessionEventKind::TurnAccepted { .. }
    ));
    assert!(matches!(
        events[1].kind,
        WaslSessionEventKind::AssistantDelta { .. }
    ));
    match &events[2].kind {
        WaslSessionEventKind::TurnCompleted {
            turn_id,
            reply,
            certainty,
        } => {
            assert_eq!(turn_id, "t1");
            assert_eq!(reply, "hi there");
            assert_eq!(certainty, "committed");
        }
        other => panic!("expected TurnCompleted, got {other:?}"),
    }
}

#[test]
fn script_happy_turn_convenience_produces_accept_delta_complete() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());
    fake.script_status(idle_status());
    fake.script_happy_turn("t7", "done");

    let mut stream = fake.attach(0);
    let events = drain(&mut *stream);

    // At minimum: an accept, at least one delta, and a completion for t7.
    assert!(events.iter().any(
        |e| matches!(&e.kind, WaslSessionEventKind::TurnAccepted { turn_id, .. } if turn_id == "t7")
    ));
    assert!(events
        .iter()
        .any(|e| matches!(&e.kind, WaslSessionEventKind::AssistantDelta { .. })));
    assert!(events.iter().any(|e| matches!(
        &e.kind,
        WaslSessionEventKind::TurnCompleted { turn_id, reply, .. } if turn_id == "t7" && reply == "done"
    )));
}

// ---------------------------------------------------------------------------
// native-gate loop: gate blocks → resolve(Approve) → status advances to Busy →
// follow-on TurnCompleted drains. Proven with no real Wasl actor (plan §5's
// resolve_native_gate default advances the scripted status).
// ---------------------------------------------------------------------------

#[test]
fn native_gate_loop_blocks_then_resolves_then_completes() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());

    // script_native_gate sets AwaitingGate AND enqueues NativeGateRequested so
    // both the status projection and the gate pipeline can fire (plan §5). v3:
    // the detail is only {summary, preview}; per-gate identity is (turn_id, seq).
    let detail = gate_detail();
    fake.script_native_gate(detail.clone());

    // The status is now AwaitingGate (attention bucket); grab the blocked turn.
    let turn_id = match fake.status() {
        WaslTargetStatus::AwaitingGate {
            detail: got,
            active_turn,
            ..
        } => {
            assert_eq!(got, detail);
            active_turn
        }
        other => panic!("script_native_gate must set AwaitingGate, got {other:?}"),
    };

    // The gate request is on the observe stream (carrying its turn_id).
    let mut stream = fake.attach(0);
    let before = drain(&mut *stream);
    assert!(before.iter().any(|e| matches!(
        &e.kind,
        WaslSessionEventKind::NativeGateRequested { detail: d, .. } if *d == detail
    )));

    // Resolve it, KEYED BY turn_id (v3 §8: the real gate has no gate id). The
    // default fake behavior advances AwaitingGate → Busy and enqueues a
    // follow-on TurnCompleted (the "gate blocks → resolved → completes" loop).
    fake.resolve_native_gate(&WaslTurnId(turn_id.clone()), WaslGateDecision::Approve)
        .expect("resolve must succeed");

    // The decision was recorded, keyed by turn_id.
    let log = fake.resolved_gates.lock().unwrap().clone();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].0, turn_id);
    assert_eq!(log[0].1, WaslGateDecision::Approve);

    // Status advanced out of AwaitingGate to Busy.
    assert!(
        matches!(fake.status(), WaslTargetStatus::Busy { .. }),
        "after resolving the gate the session must be Busy, got {:?}",
        fake.status()
    );

    // The follow-on completion drains from a fresh attach after the gate seq.
    let last_seq = before.iter().map(|e| e.seq).max().unwrap_or(0);
    let mut stream2 = fake.attach(last_seq);
    let after = drain(&mut *stream2);
    assert!(
        after
            .iter()
            .any(|e| matches!(&e.kind, WaslSessionEventKind::TurnCompleted { .. })),
        "a follow-on TurnCompleted must drain after the gate is resolved: {after:?}"
    );
}

// ---------------------------------------------------------------------------
// causation (v3 §4.4): Marsad always originates a fresh single-hop causation
// (fresh root_request, no parent, empty path, hop 0) — it only ever sends
// LocalHuman root turns. This proves the construction path produces the correct
// default AND that it round-trips through the fake's `prompts` log verbatim.
// ---------------------------------------------------------------------------

#[test]
fn turn_request_carries_a_fresh_single_hop_causation_that_round_trips() {
    let fake = FakeWaslSessionClient::new("wasl-target-1", all_caps());

    // Two distinct sends: each must carry a fresh, distinct root_request and the
    // "fresh single-hop" shape (no parent, empty path, hop 0).
    fake.prompt(sample_request("k1", "first"), support::queue_policy());
    fake.prompt(sample_request("k2", "second"), support::queue_policy());

    let log = fake.prompts.lock().unwrap().clone();
    assert_eq!(log.len(), 2);

    for req in &log {
        let c = &req.causation;
        assert_eq!(
            c.parent_request, None,
            "Marsad never chains: parent must be None"
        );
        assert!(c.path.is_empty(), "Marsad's path must be empty (root turn)");
        assert_eq!(c.hop, 0, "Marsad only originates root turns: hop must be 0");
        assert!(
            !c.root_request.is_empty(),
            "a fresh root_request must be present"
        );
    }
    // Two sends → two DISTINCT root_requests (a fresh causation per send).
    assert_ne!(
        log[0].causation.root_request, log[1].causation.root_request,
        "each send must mint a fresh root_request"
    );
}

#[test]
fn causation_fresh_constructor_builds_the_documented_default() {
    // The only constructor Marsad uses (§4.4 WaslCausation::fresh): fresh root,
    // no parent, empty path, hop 0.
    let c = marsad_wasl_session::WaslCausation::fresh("root-abc".to_string());
    assert_eq!(c.root_request, "root-abc");
    assert_eq!(c.parent_request, None);
    assert!(c.path.is_empty());
    assert_eq!(c.hop, 0);

    // And it round-trips through serde unchanged.
    let json = serde_json::to_string(&c).expect("serialize");
    let back: marsad_wasl_session::WaslCausation =
        serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, c);
}

// ---------------------------------------------------------------------------
// provider: get_or_start is idempotent per key — it returns the SAME underlying
// target_id on a reattach, and a `WaslTargetSnapshot` (NOT a `created: bool`).
// v3 §4.3/§6.1: real Wasl has NO `created` flag; `POST /v1/targets` is
// create-or-reuse and ALWAYS returns a `TargetSnapshot`, so fresh-vs-resume is
// decided by the caller comparing `snapshot.last_seq` against its persisted seq.
// The fake mirrors this: a fresh target's snapshot reports `last_seq = 0`; a
// reattach reports the accumulated `last_seq`. list() reflects started sessions.
// ---------------------------------------------------------------------------

fn key(v: &str) -> WaslTargetKey {
    WaslTargetKey(serde_json::json!({ "protocol": "claude", "ctx": v }))
}

#[test]
fn provider_get_or_start_is_idempotent_returning_the_same_target() {
    let provider = FakeWaslSessionProvider::new();

    let (client1, snap1) = provider.get_or_start(&key("s1")).expect("first start");
    let target1 = client1.target_id().to_string();
    // A brand-new fake target's journal is empty → last_seq == 0 (the wire-honest
    // "genuinely fresh" signal; there is no `created` bool).
    assert_eq!(
        snap1.last_seq, 0,
        "first get_or_start for a new key spawns a fresh target whose journal is \
         empty → snapshot.last_seq == 0"
    );
    assert_eq!(
        snap1.target_id, target1,
        "the snapshot's target_id matches the bound client's"
    );
    assert_eq!(snap1.key, key("s1"), "the snapshot carries its target key");

    let (client2, snap2) = provider.get_or_start(&key("s1")).expect("second start");
    assert_eq!(
        client2.target_id(),
        target1,
        "reattach must bind to the SAME underlying Wasl target (create-or-reuse, \
         never a second competing target)"
    );
    assert_eq!(
        snap2.target_id, target1,
        "the reattach snapshot names the same surviving target"
    );
}

#[test]
fn provider_distinct_keys_get_distinct_targets() {
    let provider = FakeWaslSessionProvider::new();
    let (c1, _) = provider.get_or_start(&key("s1")).unwrap();
    let (c2, _) = provider.get_or_start(&key("s2")).unwrap();
    assert_ne!(
        c1.target_id(),
        c2.target_id(),
        "distinct keys must not cross-route to one target"
    );
}

#[test]
fn provider_list_reflects_started_sessions() {
    let provider = FakeWaslSessionProvider::new();
    assert!(
        provider.list().unwrap().is_empty(),
        "no sessions started yet"
    );

    let (c1, _) = provider.get_or_start(&key("s1")).unwrap();
    let (c2, _) = provider.get_or_start(&key("s2")).unwrap();

    let mut listed = provider.list().unwrap();
    listed.sort();
    let mut expected = vec![c1.target_id().to_string(), c2.target_id().to_string()];
    expected.sort();
    assert_eq!(listed, expected, "list() must report every started target");

    // A repeat get_or_start does NOT add a second entry.
    let _ = provider.get_or_start(&key("s1")).unwrap();
    assert_eq!(
        provider.list().unwrap().len(),
        2,
        "reattach must not grow the list"
    );
}

#[test]
fn provider_is_usable_as_a_boxed_trait_object() {
    let provider: Box<dyn WaslSessionProvider> = Box::new(FakeWaslSessionProvider::new());
    let (client, snapshot) = provider
        .get_or_start(&key("s1"))
        .expect("start via trait object");
    assert_eq!(
        snapshot.last_seq, 0,
        "a fresh target via the trait object reports last_seq == 0"
    );
    assert!(!client.target_id().is_empty());
    assert_eq!(provider.list().unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// reconnect after core restart (§6.1, provable against the fake): a surviving
// target is seeded (with a chosen `last_seq`); the client-side registry is
// "dropped and recreated"; get_or_start reattaches to the SAME target and
// returns a `WaslTargetSnapshot` whose `last_seq` is what the caller compares
// against its persisted `binding.last_seq` to decide fresh-vs-resume — NOT a
// `created` bool. Both branches are exercised:
//   • snapshot.last_seq >= persisted → resume (attach from persisted seq);
//   • snapshot.last_seq  < persisted → target was replaced → reset to 0.
// v3 §5: `script_pre_existing(key, target_id, last_seq)` seeds a surviving
// target with a chosen `last_seq` so both branches are testable in isolation
// (no prior get_or_start needed in the same test).
// ---------------------------------------------------------------------------

#[test]
fn reconnect_reattaches_to_a_pre_existing_target_without_respawning() {
    let provider = FakeWaslSessionProvider::new();
    // Seed a surviving target (a Wasl session that outlived a Marsad restart),
    // WITHOUT a prior get_or_start in this test (plan §5 script_pre_existing).
    // It has already emitted up to seq 7 before the restart.
    provider.script_pre_existing(&key("survivor"), "wasl-survivor-1", 7);

    let (client, snapshot) = provider.get_or_start(&key("survivor")).expect("reattach");
    assert_eq!(
        client.target_id(),
        "wasl-survivor-1",
        "reconnecting must reattach to the SAME surviving target, never respawn"
    );
    assert_eq!(
        snapshot.last_seq, 7,
        "the reattach snapshot surfaces the surviving target's accumulated \
         last_seq (7) so the caller can run the §6.1 comparison"
    );
    assert_eq!(
        snapshot.target_id, "wasl-survivor-1",
        "the snapshot names the surviving target"
    );
}

/// §6.1 branch (a) RESUME: a surviving target whose `snapshot.last_seq` is
/// >= what Marsad persisted (here: snapshot 9 >= persisted 5) is the SAME
/// still-alive session — the caller resumes from its persisted seq. The provider
/// surfaces `last_seq = 9` verbatim; the fresh-vs-resume DECISION is the caller's
/// (`snapshot.last_seq >= persisted`), proven here at the value level.
#[test]
fn reconnect_snapshot_last_seq_ge_persisted_means_resume() {
    let provider = FakeWaslSessionProvider::new();
    provider.script_pre_existing(&key("resume"), "wasl-resume-1", 9);
    let persisted_seq: u64 = 5;

    let (_client, snapshot) = provider.get_or_start(&key("resume")).expect("reattach");

    assert!(
        snapshot.last_seq >= persisted_seq,
        "snapshot.last_seq ({}) >= persisted ({}) → same live session, resume from \
         the persisted seq (attach(after_seq = {}))",
        snapshot.last_seq,
        persisted_seq,
        persisted_seq
    );
}

/// §6.1 branch (b) REPLACED/RESET: a target whose `snapshot.last_seq` is LESS
/// than what Marsad persisted (here: snapshot 2 < persisted 40) means the real
/// target was replaced/restarted — a fresh target's journal starts over below
/// what Marsad has on file. The caller must treat it as fresh: reset local seq
/// tracking to 0 and re-attach(0). The provider surfaces `last_seq = 2` verbatim
/// so the caller's comparison fires the reset branch.
#[test]
fn reconnect_snapshot_last_seq_lt_persisted_means_replaced_reset_to_zero() {
    let provider = FakeWaslSessionProvider::new();
    // The surviving key now points at a REPLACED target whose journal restarted
    // (last_seq = 2), below what Marsad had folded (40).
    provider.script_pre_existing(&key("replaced"), "wasl-replaced-1", 2);
    let persisted_seq: u64 = 40;

    let (_client, snapshot) = provider.get_or_start(&key("replaced")).expect("reattach");

    assert!(
        snapshot.last_seq < persisted_seq,
        "snapshot.last_seq ({}) < persisted ({}) → the target was replaced; the \
         caller resets local seq tracking to 0 and re-attaches from 0",
        snapshot.last_seq,
        persisted_seq
    );
}

#[test]
fn reconnect_replays_from_last_seq_with_no_duplicate_and_no_gap() {
    // Model "core restarts": the same fake client survives; before the restart
    // core folded events up to seq 3. On reattach it resumes from after_seq=3.
    let fake = FakeWaslSessionClient::new("wasl-survivor-1", all_caps());
    for i in 1..=5 {
        fake.script_event(WaslSessionEventKind::AssistantDelta {
            turn_id: "t1".to_string(),
            text: format!("d{i}"),
        });
    }

    // Pre-restart observe: consume seq 1..=3. v3: next() yields a WaslStreamItem;
    // these are all normal Events (no truncation here), so unwrap the Event(..).
    let mut pre = fake.attach(0);
    let pre_events: Vec<u64> = {
        let mut v = Vec::new();
        while let Some(item) = pre.next() {
            let ev = match item {
                WaslStreamItem::Event(ev) => ev,
                WaslStreamItem::Truncated { .. } => {
                    panic!("unexpected truncation in this scenario")
                }
            };
            v.push(ev.seq);
            if ev.seq == 3 {
                break;
            }
        }
        v
    };
    assert_eq!(pre_events, vec![1, 2, 3]);
    drop(pre); // client-side observe state dropped (the "restart")

    // Post-restart reattach from the last folded seq: must yield exactly 4,5.
    let mut post = fake.attach(3);
    let post_events: Vec<u64> = drain(&mut *post).iter().map(|e| e.seq).collect();
    assert_eq!(
        post_events,
        vec![4, 5],
        "reattach(after_seq=3) must neither repeat a folded event nor skip one"
    );
}
