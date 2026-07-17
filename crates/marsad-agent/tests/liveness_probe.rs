//! Pure-function tests for the liveness verdict parser (the reliable probe's
//! core). Empty / garbled / banner-only output must be `Unreachable`, never
//! silently `Dead` and never an endless-retry ambiguity — that distinction is
//! what keeps an unreachable durable session from being falsely Paused.

use marsad_agent::{
    conn_on_drop, conn_on_probe, parse_liveness, Liveness, ProbeAction, LIVENESS_SENTINEL,
};
use marsad_proto::ConnHealth;

#[test]
fn sentinel_live_and_dead() {
    assert_eq!(parse_liveness(Some("MARSAD_LIVENESS:LIVE")), Liveness::Live);
    assert_eq!(parse_liveness(Some("MARSAD_LIVENESS:DEAD")), Liveness::Dead);
    assert_eq!(parse_liveness(Some("MARSAD_LIVENESS:LIVE\n")), Liveness::Live);
}

#[test]
fn sentinel_after_login_banner_is_still_found() {
    // HPC login node prints a MOTD/banner before the verdict.
    let out = "Welcome to narval\nLmod is loading modules...\nMARSAD_LIVENESS:DEAD\n";
    assert_eq!(parse_liveness(Some(out)), Liveness::Dead);
    let out2 = "banner line\n\n  MARSAD_LIVENESS:LIVE  \ntrailing\n";
    assert_eq!(parse_liveness(Some(out2)), Liveness::Live);
}

#[test]
fn none_is_unreachable() {
    // Probe couldn't run / timed out.
    assert_eq!(parse_liveness(None), Liveness::Unreachable);
}

#[test]
fn empty_or_banner_only_is_unreachable_not_dead() {
    // The exact narval bug: login shell returns empty → must be Unreachable so the
    // session is NOT falsely finalized (and NOT retried forever as "ambiguous").
    assert_eq!(parse_liveness(Some("")), Liveness::Unreachable);
    assert_eq!(parse_liveness(Some("\n\n   \n")), Liveness::Unreachable);
    assert_eq!(
        parse_liveness(Some("Welcome to the cluster\nno verdict here\n")),
        Liveness::Unreachable
    );
}

#[test]
fn garbled_sentinel_value_is_unreachable() {
    assert_eq!(
        parse_liveness(Some("MARSAD_LIVENESS:???")),
        Liveness::Unreachable
    );
}

#[test]
fn sentinel_constant_matches_script_shape() {
    // Guards the wire contract between remote_liveness_script's echo and the parser.
    assert_eq!(LIVENESS_SENTINEL, "MARSAD_LIVENESS:");
    let live_line = format!("{LIVENESS_SENTINEL}LIVE");
    assert_eq!(parse_liveness(Some(&live_line)), Liveness::Live);
}

// ── Connection-health state machine transitions ──────────────────────────────

#[test]
fn drop_is_reconnecting_when_it_had_been_working() {
    // A drop from a connection that produced output is a plain blip → Reconnecting.
    assert_eq!(conn_on_drop(5), ConnHealth::Reconnecting);
    assert_eq!(conn_on_drop(1), ConnHealth::Reconnecting);
}

#[test]
fn drop_is_unstable_when_it_never_restabilized() {
    // Re-attached then dropped again before any line flowed → a flap → Unstable.
    assert_eq!(conn_on_drop(0), ConnHealth::Unstable);
}

#[test]
fn probe_live_reattaches_without_emitting() {
    let step = conn_on_probe(Liveness::Live, false);
    assert_eq!(step.action, ProbeAction::Reattach);
    assert_eq!(step.emit, None); // the re-attach emits Connected
}

#[test]
fn probe_dead_pauses() {
    let step = conn_on_probe(Liveness::Dead, false);
    assert_eq!(step.action, ProbeAction::Pause);
    // Only DEAD finalizes.
    let step_maxed = conn_on_probe(Liveness::Dead, true);
    assert_eq!(step_maxed.action, ProbeAction::Pause);
}

#[test]
fn probe_unreachable_never_pauses_and_escalates_at_backoff_max() {
    // Early (backoff still growing) → keep probing, show Reconnecting.
    let early = conn_on_probe(Liveness::Unreachable, false);
    assert_eq!(early.action, ProbeAction::KeepProbing);
    assert_eq!(early.emit, Some(ConnHealth::Reconnecting));

    // Sustained (backoff saturated) → keep probing, show Disconnected (NOT Paused).
    let sustained = conn_on_probe(Liveness::Unreachable, true);
    assert_eq!(sustained.action, ProbeAction::KeepProbing);
    assert_eq!(sustained.emit, Some(ConnHealth::Disconnected));

    // Unreachable is NEVER a Pause, at any backoff.
    assert_ne!(conn_on_probe(Liveness::Unreachable, false).action, ProbeAction::Pause);
    assert_ne!(conn_on_probe(Liveness::Unreachable, true).action, ProbeAction::Pause);
}
