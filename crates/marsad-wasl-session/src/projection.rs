//! Pure, total helpers that live in this crate alongside the domain types
//! (`wasl-backend-plan.md` §6.3, §8, §12 Q5):
//!
//! * [`project_status_to_phase`] — the `WaslTargetStatus → marsad_events::Phase`
//!   projection (the §6.3 table), the analogue of `wasl_event_lines`. Pure,
//!   total, and it never adds a Wasl-specific `Phase` variant.
//! * [`status_is_live`] — the v2 `live`-axis helper: distinguishes an
//!   idle-and-ready session (live) from a stopped/exited/faulted one (not live),
//!   independent of the coarser `Phase` projection.
//! * [`native_gate_key`] — derives a native gate's per-occurrence identity from
//!   `(turn_id, seq)` (§8, v3): the real `NativeGateDetail` has no `gate_id`, and
//!   `turn_id` alone is NOT unique per gate occurrence (two native gates can fire
//!   in one turn), so the event's own journal `seq` is combined with `turn_id`.
//! * [`native_gate_command`] — encodes that `(turn_id, seq)`-derived `gate_key`
//!   into the `command` field of the reused `Gate::Permission` variant so the
//!   derived `DecisionId` (which hashes `command` + `cwd`, not `request_id`) is
//!   unique and stable per gate occurrence (§8).
//! * [`new_idempotency_key`] — the v2 UUID-based idempotency-key generator (§12
//!   Q5): a fresh, unique key minted once per human send action.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use marsad_events::Phase;

use crate::types::{WaslEventSeq, WaslNativeGateDetail, WaslTargetStatus};

/// Project a Wasl `TargetStatus` onto Marsad's shared `Phase` (§6.3 table).
///
/// Pure and total: every constructible variant maps to exactly one of the seven
/// existing `Phase`s and it never panics. It deliberately does NOT expand
/// `Phase` — a Wasl session reuses the same UI vocabulary as every other
/// session. `capabilities()` never affects this projection.
pub fn project_status_to_phase(status: &WaslTargetStatus) -> Phase {
    match status {
        // Not running / not connected; the UI's "reconnect?" bucket.
        WaslTargetStatus::Offline => Phase::Detached,
        WaslTargetStatus::Starting => Phase::Starting,
        // Ready, no active turn — the calm "finished a turn, awaiting input"
        // state (§6.3; the `live` axis keeps it distinct from Stopped).
        WaslTargetStatus::Idle { .. } => Phase::Done,
        // Any active turn.
        WaslTargetStatus::Busy { .. } => Phase::Working,
        // Native gate blocks — the attention bucket, like a claude permission gate.
        WaslTargetStatus::AwaitingGate { .. } => Phase::Needs,
        // Still winding down / finishing the active turn.
        WaslTargetStatus::Interrupting { .. } | WaslTargetStatus::Draining { .. } => Phase::Working,
        // A fault surfaces as Error regardless of recoverability (recoverability
        // drives the retry affordance, not the Phase).
        WaslTargetStatus::Faulted { .. } => Phase::Error,
        // Cleanly shut down.
        WaslTargetStatus::Stopped => Phase::Done,
        // Conservative: don't claim Done/Needs on an unrecognized state.
        WaslTargetStatus::Unknown(_) => Phase::Working,
    }
}

/// The v2 `live`-axis helper (§6.3 note). `true` while the underlying Wasl
/// session is running/attached (starting, idle-and-ready, busy, gated, or
/// winding down); `false` once it is stopped, exited, offline, or faulted.
///
/// This is what keeps an idle-and-ready session (`live: true`) distinct from a
/// truly-stopped/exited one (`live: false`) even though both project to
/// `Phase::Done` — so a still-live idle session is never confused with a dead
/// one at the `live` field level.
pub fn status_is_live(status: &WaslTargetStatus) -> bool {
    match status {
        WaslTargetStatus::Starting
        | WaslTargetStatus::Idle { .. }
        | WaslTargetStatus::Busy { .. }
        | WaslTargetStatus::AwaitingGate { .. }
        | WaslTargetStatus::Interrupting { .. }
        | WaslTargetStatus::Draining { .. } => true,
        WaslTargetStatus::Offline
        | WaslTargetStatus::Faulted { .. }
        | WaslTargetStatus::Stopped
        | WaslTargetStatus::Unknown(_) => false,
    }
}

/// Derive a native gate's per-occurrence identity from `(turn_id, seq)` (§8, v3).
///
/// The real `NativeGateDetail` has no `gate_id`, and the sibling `turn_id` is
/// unique per TURN but NOT per gate occurrence — Wasl does not prevent two native
/// gates firing within one turn (approve tool A → the turn continues → tool B
/// later gates → SAME `turn_id` both times). The event's own journal `seq` IS
/// guaranteed unique per event (Wasl `EventSeq` is per-target monotonic and
/// gap-free), so combining it with `turn_id` yields a genuinely-unique-per-
/// occurrence identity. Stable and idempotent for the same `(turn_id, seq)`.
///
/// Resolution back to Wasl still uses the bare `turn_id` (that is what
/// `resolve_native_gate(turn_id, decision)` takes) — the `seq` is only for
/// Marsad's own per-occurrence ledger identity, never sent to Wasl.
pub fn native_gate_key(turn_id: &str, seq: WaslEventSeq) -> String {
    format!("{turn_id}#{seq}")
}

/// Encode a native gate into the `command` field of a reused
/// `marsad_gates::Gate::Permission` (§8).
///
/// `marsad_gates::decision_id` hashes a permission gate's `command` + `cwd` (not
/// its `request_id`), whereas a native gate's stable per-occurrence identity is
/// the `(turn_id, seq)`-derived `gate_key` (see [`native_gate_key`]). So the
/// encoding is a `\x00`-delimited, `gate_key`-prefixed string that ALSO carries
/// the human-readable summary:
///
/// * stable — the same `(gate_key, detail)` encodes to the same string (⇒ same
///   `DecisionId`);
/// * unique per occurrence — distinct `gate_key`s encode distinctly even when the
///   summary is identical (the whole point: `command` must be unique so the
///   derived `DecisionId` is);
/// * human-readable — the summary is carried for display.
pub fn native_gate_command(gate_key: &str, detail: &WaslNativeGateDetail) -> String {
    // gate_key first (identity), then the human-readable summary, \x00-delimited
    // so it is unambiguous and the gate_key can never be confused with the
    // summary.
    format!("{}\x00{}", gate_key, detail.summary)
}

/// A monotonic counter mixed into each generated key so two keys minted within
/// the same clock tick are still distinct.
static KEY_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Mint a fresh idempotency key for one human send action (§4.4, §12 Q5).
///
/// The value is a canonical UUID-shaped string (`8-4-4-4-12` lowercase hex, with
/// the version-4 and RFC-4122 variant nibbles set). Uniqueness — not ordering —
/// is the requirement, and is what the black-box tests assert: it is non-empty
/// and never repeats across sends. It is derived without an external `uuid`
/// dependency (the crate's dep set stays `serde`/`serde_json`/`thiserror`/
/// `marsad-events`, house style — cf. `marsad-gates`' hand-rolled FNV): 128 bits
/// are filled from a SplitMix64 stream seeded by the wall-clock nanosecond count
/// mixed with a process-lifetime monotonic counter, so the sole property callers
/// rely on (a distinct key per send) holds.
pub fn new_idempotency_key() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let count = KEY_COUNTER.fetch_add(1, Ordering::Relaxed);
    // Seed with time XOR (counter spread across the word) so distinct sends in
    // the same nanosecond still diverge from the first draw.
    let mut state = nanos ^ count.wrapping_mul(0x9E37_79B9_7F4A_7C15);

    let hi = splitmix64(&mut state);
    let lo = splitmix64(&mut state);
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&hi.to_be_bytes());
    bytes[8..].copy_from_slice(&lo.to_be_bytes());

    // Set the version (4) and variant (RFC 4122, 10xx) nibbles so the string is
    // a well-formed v4 UUID rather than arbitrary hex.
    bytes[6] = (bytes[6] & 0x0F) | 0x40;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;

    format_uuid(&bytes)
}

/// Compute the deterministic bare A2A bind address for a Wasl node with the
/// given `host_id` (`127.0.0.1:<port>`), using FNV-1a in port window
/// `[48000, 48999]`.
///
/// This is the same formula `marsad-agent::wasl::wasl_a2a_bind` uses — shared
/// here so callers that need to compute a peer endpoint (e.g. `marsad-core`'s
/// dynamic peer registration) don't have to depend on `marsad-agent`.
///
/// Returns a bare `ip:port` (no `http://` scheme) — callers that need the full
/// `http://` URL should prepend it themselves.
pub fn wasl_a2a_bind(host_id: &str) -> String {
    const FNV_OFFSET: u32 = 2_166_136_261;
    const FNV_PRIME: u32 = 16_777_619;
    let mut hash = FNV_OFFSET;
    for byte in host_id.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    let port = 48_000u32 + (hash % 1_000);
    format!("127.0.0.1:{port}")
}

/// SplitMix64 — a small, fast, deterministic scrambler used only to spread the
/// seed bits across the 128-bit key. Advances `state` in place.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Render 16 bytes as a canonical lowercase `8-4-4-4-12` UUID string.
fn format_uuid(b: &[u8; 16]) -> String {
    let mut s = String::with_capacity(36);
    for (i, byte) in b.iter().enumerate() {
        if i == 4 || i == 6 || i == 8 || i == 10 {
            s.push('-');
        }
        s.push_str(&format!("{byte:02x}"));
    }
    s
}
