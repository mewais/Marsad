//! native_gate.rs — the native-gate identity helpers (plan §8, corrected in v3):
//! `native_gate_key(turn_id, seq)` and `native_gate_command(gate_key, detail)`.
//!
//! **v3 correction (load-bearing).** The v2 design assumed `NativeGateDetail`
//! carried a `gate_id` unique per gate occurrence. It does NOT — the real type
//! (`wasl-runtime/src/lib.rs` §4.1) is only `{summary, preview}`. Per-occurrence
//! identity is derived from `(turn_id, seq)`: the sibling `turn_id` on the
//! event/status combined with the event's own journal `seq` (guaranteed unique
//! per event). This matters because Wasl does NOT prevent two native gates
//! firing within one turn (approve tool A → the turn continues → tool B later
//! gates → SAME `turn_id` both times), so `turn_id` alone must NOT collapse two
//! occurrences.
//!
//! `native_gate_command` encodes the `(turn_id, seq)`-derived `gate_key` into the
//! `command` field of the reused `Gate::Permission` variant so the derived
//! `DecisionId` (which hashes `command` + `cwd`, not `request_id`) is unique and
//! stable per gate occurrence. Per §8 both helpers live in this crate's
//! projection module and "have their own round-trip tests."
//!
//! This test asserts the OBSERVABLE properties v3 pins:
//!   * `native_gate_key` is stable and idempotent for the same `(turn_id, seq)`;
//!   * two DIFFERENT `(turn_id, seq)` pairs — even with an identical
//!     summary/preview — produce distinct keys AND distinct commands;
//!   * **regression pin (the specific bug v3 fixes):** two gates within the SAME
//!     turn (same `turn_id`, different `seq`) must NOT collide onto the same
//!     command / DecisionId-safe string;
//!   * `native_gate_command` is stable for a given `(gate_key, detail)` and
//!     carries the human-readable summary for display.
//!
//! It does NOT construct a `marsad_gates::Gate` (that lives in marsad-core's
//! folding step, tested in marsad-core §10.4) — this crate only owns the pure
//! encoding helpers.
//!
//! This file WILL fail to compile until a later worker implements
//! `marsad_wasl_session`'s v3 public surface (§8 helpers) — expected
//! tests-first signal.

use marsad_wasl_session::{native_gate_command, native_gate_key, WaslNativeGateDetail};

fn detail_with(summary: &str, preview: &str) -> WaslNativeGateDetail {
    WaslNativeGateDetail {
        summary: summary.to_string(),
        preview: preview.to_string(),
    }
}

// ---------------------------------------------------------------------------
// native_gate_key: stable/idempotent for the same (turn_id, seq).
// ---------------------------------------------------------------------------

#[test]
fn native_gate_key_is_stable_for_the_same_turn_and_seq() {
    let a = native_gate_key("t-1", 7);
    let b = native_gate_key("t-1", 7);
    assert_eq!(
        a, b,
        "the same (turn_id, seq) must yield the same gate_key (stable, idempotent)"
    );
    assert!(!a.is_empty());
}

// ---------------------------------------------------------------------------
// native_gate_key: two DIFFERENT (turn_id, seq) pairs produce distinct keys,
// even when the summary/preview would be identical (the key derives ONLY from
// (turn_id, seq), not from the detail).
// ---------------------------------------------------------------------------

#[test]
fn native_gate_key_differs_for_distinct_turn_seq_pairs() {
    // Different turn, same seq.
    assert_ne!(native_gate_key("t-1", 7), native_gate_key("t-2", 7));
    // Same turn, different seq.
    assert_ne!(native_gate_key("t-1", 7), native_gate_key("t-1", 8));
    // Both different.
    assert_ne!(native_gate_key("t-1", 7), native_gate_key("t-2", 8));
}

// ---------------------------------------------------------------------------
// REGRESSION PIN (the critical v3 bug): two native gates within the SAME turn
// (same turn_id, DIFFERENT seq) must NOT collide onto the same command /
// DecisionId-safe string. Wasl does not prevent two native gates per turn, so
// turn_id alone must never collapse two occurrences. If a future refactor keys
// gate identity on turn_id alone, THIS assertion fails.
// ---------------------------------------------------------------------------

#[test]
fn two_native_gates_in_one_turn_do_not_collide() {
    // Identical detail (as if the same tool prompt fired twice in one turn):
    // the summary/preview are the same, ONLY the seq differs.
    let detail = detail_with("run migration", "$ ./migrate");

    let key_a = native_gate_key("turn-shared", 10);
    let key_b = native_gate_key("turn-shared", 11);
    assert_ne!(
        key_a, key_b,
        "two gates in the SAME turn (same turn_id, different seq) must have distinct gate_keys"
    );

    let cmd_a = native_gate_command(&key_a, &detail);
    let cmd_b = native_gate_command(&key_b, &detail);
    assert_ne!(
        cmd_a, cmd_b,
        "two gates in the SAME turn must encode to DISTINCT commands so their derived \
         DecisionIds differ — the regression this pins can never come back"
    );
}

// ---------------------------------------------------------------------------
// native_gate_command: stable for the same (gate_key, detail) → same DecisionId.
// ---------------------------------------------------------------------------

#[test]
fn native_gate_command_is_stable_for_the_same_gate_key() {
    let key = native_gate_key("t-1", 3);
    let d = detail_with("delete build dir", "$ rm -rf ./build");
    let a = native_gate_command(&key, &d);
    let b = native_gate_command(&key, &d);
    assert_eq!(
        a, b,
        "same (gate_key, detail) must encode to the same command (stable DecisionId)"
    );
    assert!(!a.is_empty());
}

// ---------------------------------------------------------------------------
// native_gate_command: distinct gate_keys produce distinct commands even when
// the detail is byte-for-byte identical (the whole point — command must be
// unique per occurrence so the derived DecisionId is).
// ---------------------------------------------------------------------------

#[test]
fn native_gate_command_is_unique_per_gate_key() {
    let d = detail_with("same summary", "same preview");
    let cmd1 = native_gate_command(&native_gate_key("t-1", 1), &d);
    let cmd2 = native_gate_command(&native_gate_key("t-2", 1), &d);
    assert_ne!(
        cmd1, cmd2,
        "distinct gate_keys must encode to distinct commands even with identical details"
    );
}

// ---------------------------------------------------------------------------
// native_gate_command: carries the gate_key (identity) and the human-readable
// summary (for display).
// ---------------------------------------------------------------------------

#[test]
fn native_gate_command_carries_gate_key_and_human_readable_summary() {
    let key = native_gate_key("turn-77", 5);
    let d = detail_with("run the migration", "$ ./migrate");
    let encoded = native_gate_command(&key, &d);
    assert!(
        encoded.contains(&key),
        "the encoding must carry the gate_key: key={key:?} encoded={encoded:?}"
    );
    assert!(
        encoded.contains("run the migration"),
        "the encoding must carry the human-readable summary: {encoded:?}"
    );
}
