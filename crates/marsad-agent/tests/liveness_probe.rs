//! Pure-function tests for the liveness verdict parser (the reliable probe's
//! core). Empty / garbled / banner-only output must be `Unreachable`, never
//! silently `Dead` and never an endless-retry ambiguity — that distinction is
//! what keeps an unreachable durable session from being falsely Paused.

use marsad_agent::{
    conn_on_drop, parse_liveness, reconnect_action, Liveness, ReconnectAction, LIVENESS_SENTINEL,
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

// ── Signal-driven reconnect decision (from the reader child's exit, not a probe) ─

#[test]
fn clean_reader_exit_means_process_died_pause() {
    // `ssh … tail --pid=<pid>` returns 0 when the watched process dies → Pause.
    assert_eq!(reconnect_action(Some(0), false), ReconnectAction::Pause);
}

#[test]
fn transport_error_reattaches_not_pauses() {
    // The REAL ETX signals: ssh dies 255 and/or its stderr shows "closed by remote
    // host"/"Connection reset". Either → a network drop → Reattach (the durable
    // setsid session is presumed alive, NEVER falsely Paused).
    assert_eq!(reconnect_action(Some(255), false), ReconnectAction::Reattach);
    assert_eq!(reconnect_action(Some(0), true), ReconnectAction::Reattach); // stderr overrides a 0
    assert_eq!(reconnect_action(None, false), ReconnectAction::Reattach); // killed/unwaitable
    assert_eq!(reconnect_action(Some(1), false), ReconnectAction::Reattach); // unknown nonzero
}
