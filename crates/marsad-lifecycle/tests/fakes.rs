//! Exercise the `Container` trait via `FakeContainer`.
//! Verifies exists/attach/spawn/kill, set_exists, fail_attach,
//! and the call-log accessor used by other test files to prove
//! no-duplication on Adopt.
//!
//! NOTE(test-agent): The spec declares `FakeContainer` with only
//! `new()`, `set_exists()`, and `fail_attach()`.  To make the
//! "no spawn on Adopt" invariant provable (branches.rs) the
//! implementation will need to expose a call-log accessor.
//! We assume `FakeContainer::calls() -> &[String]` (or similar)
//! exists on the concrete type; the code-agent must add it.
//! If the final API differs, update only this comment and the
//! assertion below — do not change the test semantics.

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
// NOTE(test-agent): We rely on `FakeContainer::calls() -> &[String]` to
// record method names ("exists", "attach", "spawn", "kill").  The
// code-agent must implement this accessor.

#[test]
fn fake_records_exists_call() {
    let mut f = make_fake();
    let _ = f.exists("s");
    assert!(
        f.calls().iter().any(|c| c.contains("exists")),
        "call log should contain 'exists'"
    );
}

#[test]
fn fake_records_attach_call() {
    let mut f = make_fake();
    f.set_exists("s", true);
    let _ = f.attach("s");
    assert!(
        f.calls().iter().any(|c| c.contains("attach")),
        "call log should contain 'attach'"
    );
}

#[test]
fn fake_records_spawn_call() {
    let mut f = make_fake();
    let _ = f.spawn("s", "cmd");
    assert!(
        f.calls().iter().any(|c| c.contains("spawn")),
        "call log should contain 'spawn'"
    );
}

#[test]
fn fake_records_kill_call() {
    let mut f = make_fake();
    let _ = f.kill("s");
    assert!(
        f.calls().iter().any(|c| c.contains("kill")),
        "call log should contain 'kill'"
    );
}

#[test]
fn fake_call_log_is_ordered() {
    let mut f = make_fake();
    f.set_exists("s", true);
    let _ = f.exists("s");
    let _ = f.attach("s");
    let calls = f.calls();
    let exists_pos = calls.iter().position(|c| c.contains("exists")).unwrap();
    let attach_pos = calls.iter().position(|c| c.contains("attach")).unwrap();
    assert!(
        exists_pos < attach_pos,
        "exists must be recorded before attach"
    );
}
