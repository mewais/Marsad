//! WAVE 3b — pty escape-hatch terminal: `handle_ipc` dispatch tests for
//! `PtyStart`/`PtyInput`/`PtyResize`.
//!
//! Mirrors `tests/happy.rs`/`tests/branches.rs`/`tests/errors.rs`'s FAKES-only
//! convention (no real claude/tmux/pty) — this exercises the coverage-mode
//! (`cfg(coverage)`) no-op branches of `handle_pty_input`/`handle_pty_resize`
//! and `handle_pty_start`'s session-existence validation, i.e. the part of
//! WAVE 3b that IS reachable through `Agent::with_fakes` without a live
//! `subscribers` broadcast list (only `serve()` owns that — see
//! `pty::spawn_pty_session`'s doc comment). The REAL pty spawn/IO is tested
//! directly in `tests/pty_live.rs`.

use marsad_agent::{Agent, AgentConfig, ListenSpec};
use marsad_lifecycle::FakeContainer;
use marsad_notify::CaptureNotifier;
use marsad_store::MemoryBackend;

fn stdio_cfg() -> AgentConfig {
    AgentConfig {
        listen: ListenSpec::Stdio,
        data_root: "/tmp/marsad-agent-test".to_string(),
    }
}

fn fresh_agent() -> Agent {
    Agent::with_fakes(
        stdio_cfg(),
        Box::new(FakeContainer::new()),
        Box::new(CaptureNotifier::new()),
        Box::new(marsad_store::Store::new(Box::new(MemoryBackend::new()))),
    )
    .expect("Agent::with_fakes must succeed")
}

/// Launch a session via the fake container and return its session_id.
fn launch_a_session(agent: &mut Agent) -> String {
    let line = r#"{"type":"LaunchSession","project":"/home/user/proj","host_id":"local","model":"claude-opus-4-5"}"#;
    let out = agent
        .handle_ipc(line)
        .expect("LaunchSession must succeed")
        .into_iter()
        .next()
        .expect("LaunchSession must emit an event");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    v["session_id"].as_str().unwrap().to_string()
}

// ---------------------------------------------------------------------------
// PtyStart
// ---------------------------------------------------------------------------

#[test]
fn pty_start_on_known_session_is_a_silent_ok() {
    let mut agent = fresh_agent();
    let sid = launch_a_session(&mut agent);
    let line = format!(r#"{{"type":"PtyStart","session_id":"{sid}","cols":80,"rows":24}}"#);
    let result = agent.handle_ipc(&line);
    assert_eq!(
        result.unwrap(),
        Vec::<String>::new(),
        "PtyStart is fire-and-forget — no ack line (mirrors SetWorkspace)"
    );
}

#[test]
fn pty_start_on_unknown_session_is_a_protocol_error() {
    let mut agent = fresh_agent();
    let line = r#"{"type":"PtyStart","session_id":"no-such-session","cols":80,"rows":24}"#;
    let err = agent
        .handle_ipc(line)
        .expect_err("PtyStart for an unknown session must error");
    assert!(
        err.to_string().contains("PtyStart") && err.to_string().contains("no-such-session"),
        "got: {err}"
    );
}

// ---------------------------------------------------------------------------
// PtyInput — coverage-mode no-op (no live pty registry without `serve()`)
// ---------------------------------------------------------------------------

#[test]
fn pty_input_is_a_silent_ok_even_with_no_live_pty() {
    let mut agent = fresh_agent();
    let sid = launch_a_session(&mut agent);
    let line = format!(r#"{{"type":"PtyInput","session_id":"{sid}","data":"ls -la\n"}}"#);
    let result = agent.handle_ipc(&line);
    assert_eq!(result.unwrap(), Vec::<String>::new());
}

#[test]
fn pty_input_on_unknown_session_is_still_a_silent_ok() {
    // Unlike PtyStart, PtyInput does not gate on `self.sessions` (it only
    // looks up the pty registry, which is empty here regardless) — dropping
    // a stray input for a session with no live pty is a benign no-op, same
    // disposition as `handle_send_prompt`'s "no registered writer" branch.
    let mut agent = fresh_agent();
    let line = r#"{"type":"PtyInput","session_id":"no-such-session","data":"x"}"#;
    let result = agent.handle_ipc(line);
    assert_eq!(result.unwrap(), Vec::<String>::new());
}

// ---------------------------------------------------------------------------
// PtyResize — coverage-mode no-op
// ---------------------------------------------------------------------------

#[test]
fn pty_resize_is_a_silent_ok_even_with_no_live_pty() {
    let mut agent = fresh_agent();
    let sid = launch_a_session(&mut agent);
    let line = format!(r#"{{"type":"PtyResize","session_id":"{sid}","cols":120,"rows":40}}"#);
    let result = agent.handle_ipc(&line);
    assert_eq!(result.unwrap(), Vec::<String>::new());
}

// ---------------------------------------------------------------------------
// Round-trip via the typed parser (proves the match arms actually dispatch,
// not just that some IpcMsg variant exists)
// ---------------------------------------------------------------------------

#[test]
fn all_three_pty_gui_to_agent_variants_parse_and_dispatch_without_panicking() {
    let mut agent = fresh_agent();
    let sid = launch_a_session(&mut agent);
    for line in [
        format!(r#"{{"type":"PtyStart","session_id":"{sid}","cols":80,"rows":24}}"#),
        format!(r#"{{"type":"PtyInput","session_id":"{sid}","data":"echo hi\n"}}"#),
        format!(r#"{{"type":"PtyResize","session_id":"{sid}","cols":100,"rows":30}}"#),
    ] {
        agent
            .handle_ipc(&line)
            .unwrap_or_else(|e| panic!("{line} must dispatch cleanly, got {e:?}"));
    }
}
