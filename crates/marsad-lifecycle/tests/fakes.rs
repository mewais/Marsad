//! Exercise the `Container` trait via `FakeContainer`.
//! Verifies exists/attach/spawn/kill, set_exists, fail_attach, fail_spawn,
//! and the call-log accessor used by other test files to prove
//! no-duplication on Adopt.
//!
//! `FakeContainer` exposes:
//!   - `calls() -> &[String]`  — ordered log of trait calls in the format
//!     "exists:<name>", "attach:<name>", "spawn:<name>", "kill:<name>".
//!   - `fail_spawn(&mut self, name: &str)` — makes the next spawn(name,..)
//!     return Err(Spawn).
//! Both are part of the blessed spec API (lifecycle.md §Public API).

use marsad_lifecycle::{Container, FakeContainer, LifecycleError};

// ── helpers ─────────────────────────────────────────────────────────────────

fn make_fake() -> FakeContainer {
    FakeContainer::new()
}

// ── exists ───────────────────────────────────────────────────────────────────

#[test]
fn fake_exists_defaults_false() {
    let f = make_fake();
    assert!(!f.exists("any-session"));
}

#[test]
fn fake_set_exists_true_then_query() {
    let mut f = make_fake();
    f.set_exists("sess-1", true);
    assert!(f.exists("sess-1"));
}

#[test]
fn fake_set_exists_false_after_true() {
    let mut f = make_fake();
    f.set_exists("sess-1", true);
    f.set_exists("sess-1", false);
    assert!(!f.exists("sess-1"));
}

#[test]
fn fake_exists_is_per_name() {
    let mut f = make_fake();
    f.set_exists("a", true);
    // "b" was never set → still false
    assert!(!f.exists("b"));
}

// ── attach ───────────────────────────────────────────────────────────────────

#[test]
fn fake_attach_succeeds_when_not_programmed_to_fail() {
    let mut f = make_fake();
    f.set_exists("sess-ok", true);
    assert!(f.attach("sess-ok").is_ok());
}

#[test]
fn fake_fail_attach_returns_error() {
    let mut f = make_fake();
    f.set_exists("sess-bad", true);
    f.fail_attach("sess-bad");
    let err = f.attach("sess-bad").unwrap_err();
    // Must be the AttachRace variant (the spec maps attach failure → AttachRace)
    assert!(
        matches!(err, LifecycleError::AttachRace(_)),
        "expected AttachRace, got {err:?}"
    );
}

// ── spawn ────────────────────────────────────────────────────────────────────

#[test]
fn fake_spawn_succeeds_by_default() {
    let mut f = make_fake();
    assert!(f.spawn("sess-new", "claude --resume abc123").is_ok());
}

// ── kill ─────────────────────────────────────────────────────────────────────

#[test]
fn fake_kill_succeeds_by_default() {
    let mut f = make_fake();
    f.set_exists("sess-kill", true);
    assert!(f.kill("sess-kill").is_ok());
}

// ── call log ────────────────────────────────────────────────────────────────
// Calls are recorded as "exists:<name>", "attach:<name>", "spawn:<name>",
// "kill:<name>" per the spec (lifecycle.md §Public API, FakeContainer.calls()).

#[test]
fn fake_records_exists_call() {
    let mut f = make_fake();
    let _ = f.exists("s");
    assert!(
        f.calls().iter().any(|c| c == "exists:s"),
        "call log should contain 'exists:s', got {:?}", f.calls()
    );
}

#[test]
fn fake_records_attach_call() {
    let mut f = make_fake();
    f.set_exists("s", true);
    let _ = f.attach("s");
    assert!(
        f.calls().iter().any(|c| c == "attach:s"),
        "call log should contain 'attach:s', got {:?}", f.calls()
    );
}

#[test]
fn fake_records_spawn_call() {
    let mut f = make_fake();
    let _ = f.spawn("s", "cmd");
    assert!(
        f.calls().iter().any(|c| c.starts_with("spawn:s")),
        "call log should contain 'spawn:s...', got {:?}", f.calls()
    );
}

#[test]
fn fake_records_kill_call() {
    let mut f = make_fake();
    let _ = f.kill("s");
    assert!(
        f.calls().iter().any(|c| c == "kill:s"),
        "call log should contain 'kill:s', got {:?}", f.calls()
    );
}

#[test]
fn fake_call_log_is_ordered() {
    let mut f = make_fake();
    f.set_exists("s", true);
    let _ = f.exists("s");
    let _ = f.attach("s");
    let calls = f.calls();
    let exists_pos = calls.iter().position(|c| c.starts_with("exists:")).unwrap();
    let attach_pos = calls.iter().position(|c| c.starts_with("attach:")).unwrap();
    assert!(
        exists_pos < attach_pos,
        "exists must be recorded before attach in call log"
    );
}

#[test]
fn fake_call_log_format_includes_name() {
    // Verifies the "verb:name" format for each method.
    let mut f = make_fake();
    f.set_exists("marsad-A", true);
    let _ = f.exists("marsad-A");
    let _ = f.attach("marsad-A");
    let _ = f.spawn("marsad-A", "claude --resume x");
    let _ = f.kill("marsad-A");

    let calls = f.calls();
    assert!(calls.contains(&"exists:marsad-A".to_string()), "calls: {calls:?}");
    assert!(calls.contains(&"attach:marsad-A".to_string()), "calls: {calls:?}");
    assert!(calls.iter().any(|c| c.starts_with("spawn:marsad-A")), "calls: {calls:?}");
    assert!(calls.contains(&"kill:marsad-A".to_string()), "calls: {calls:?}");
}

#[test]
fn fake_fail_spawn_causes_spawn_error() {
    let mut f = make_fake();
    f.fail_spawn("marsad-X");
    let err = f.spawn("marsad-X", "claude --resume abc").unwrap_err();
    assert!(
        matches!(err, LifecycleError::Spawn(_)),
        "fail_spawn must produce Spawn error, got {err:?}"
    );
}

#[test]
fn fake_fail_spawn_does_not_affect_other_names() {
    let mut f = make_fake();
    f.fail_spawn("marsad-bad");
    // A different name should still succeed
    assert!(f.spawn("marsad-ok", "cmd").is_ok());
}
