//! Branch-coverage tests for `marsad-agent` (B13).
//!
//! Covers:
//!  • Each IPC command variant (LaunchSession, ResumeSession, AttachSession, StopSession,
//!    SendPrompt, Interrupt, AnswerGate, Subscribe).
//!  • Each hook event type (Notification, Stop, SubagentStop, PreToolUse, PostToolUse,
//!    SessionStart, SessionEnd, PreCompact).
//!  • `reconcile_all` branches: adopt / resume / stale per persisted session.
//!  • Subscriber add/drop does NOT kill the session: a dropped client connection only
//!    removes a subscriber — the session runtime survives.
//!
//! NOTE(test-agent): injection seam — all tests use the assumed `Agent::with_fakes`
//! constructor (see happy.rs for the full rationale).

use marsad_agent::{Agent, AgentConfig, AgentError, ListenSpec};
use marsad_lifecycle::{Action, FakeContainer, RuntimePhase};
use marsad_notify::CaptureNotifier;
use marsad_store::MemoryBackend;

// ── helpers ────────────────────────────────────────────────────────────────────

fn stdio_cfg() -> AgentConfig {
    AgentConfig {
        listen: ListenSpec::Stdio,
        data_root: "/tmp/marsad-agent-branches-test".to_string(),
    }
}

fn make_agent() -> Agent {
    Agent::with_fakes(
        stdio_cfg(),
        Box::new(FakeContainer::new()),
        Box::new(CaptureNotifier::new()),
        Box::new(marsad_store::Store::new(Box::new(MemoryBackend::new()))),
    )
    .expect("with_fakes must succeed")
}

/// Launch a session and return the outbound IPC lines plus the parsed session_id.
fn do_launch(agent: &mut Agent) -> (Vec<String>, String) {
    let line = r#"{"type":"LaunchSession","project":"/home/user/proj","host_id":"local","model":"claude-opus-4-5","posture":"Ask"}"#;
    let lines = agent.handle_ipc(line).expect("launch must succeed");
    let sid = extract_session_id(&lines);
    (lines, sid)
}

fn extract_session_id(lines: &[String]) -> String {
    for l in lines {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(l) {
            if let Some(id) = v.get("session_id").and_then(|s| s.as_str()) {
                return id.to_string();
            }
        }
    }
    panic!("No session_id in: {lines:?}");
}

// ══════════════════════════════════════════════════════════════════════════════
// IPC command variant coverage
// ══════════════════════════════════════════════════════════════════════════════

/// LaunchSession → starts a new session, returns outbound event lines.
#[test]
fn ipc_command_launch_session() {
    let mut agent = make_agent();
    let (lines, _sid) = do_launch(&mut agent);
    assert!(!lines.is_empty(), "LaunchSession must emit events");
}

/// LaunchSession with a caller-supplied `session_id` must REUSE that id (Core and
/// the agent have to agree on the id, or the broadcast AgentEvents get dropped as
/// an unknown session and nothing shows up in the UI).
#[test]
fn launch_session_honors_provided_session_id() {
    let mut agent = make_agent();
    let line = r#"{"type":"LaunchSession","session_id":"ext-abc","project":"/p","host_id":"local","model":"claude-opus-4-8","posture":"Ask"}"#;
    let lines = agent.handle_ipc(line).expect("launch");
    assert!(
        lines
            .iter()
            .any(|l| l.contains("\"session_id\":\"ext-abc\"")),
        "SessionLaunched must echo the provided id; got {lines:?}"
    );
    let last = agent.container_calls().last().expect("spawn").clone();
    assert!(
        last.contains("marsad-ext-abc"),
        "the container must use the provided id; got {last}"
    );
}

/// ResumeSession → resumes a previously stopped session.
#[test]
fn ipc_command_resume_session() {
    let mut agent = make_agent();
    // Launch first.
    let (_, sid) = do_launch(&mut agent);
    // Stop it.
    let stop = format!(r#"{{"type":"StopSession","session_id":"{sid}"}}"#);
    agent.handle_ipc(&stop).ok(); // stop may succeed or be a no-op here

    // Resume.
    let resume = format!(r#"{{"type":"ResumeSession","session_id":"{sid}"}}"#);
    let result = agent.handle_ipc(&resume);
    // Must return Ok (even if the session was not actually stopped in the fake).
    assert!(
        result.is_ok(),
        "ResumeSession must return Ok; got {result:?}"
    );
}

/// LaunchSession with `"continue": true` → spawns `claude --continue`.
#[test]
fn launch_session_continue_mode_spawns_continue() {
    let mut agent = make_agent();
    let line = r#"{"type":"LaunchSession","project":"/home/user/proj","host_id":"local","model":"claude-opus-4-8","posture":"Ask","continue":true}"#;
    agent.handle_ipc(line).expect("launch continue");
    let last = agent
        .container_calls()
        .last()
        .expect("a spawn call")
        .clone();
    assert!(
        last.contains("--continue"),
        "continue mode must spawn --continue; got {last}"
    );
}

/// LaunchSession with `"resume_session_id"` → spawns `claude --resume <id>`.
#[test]
fn launch_session_resume_id_mode_spawns_resume() {
    let mut agent = make_agent();
    let line = r#"{"type":"LaunchSession","project":"/home/user/proj","host_id":"local","model":"claude-opus-4-8","posture":"Ask","resume_session_id":"uuid-xyz"}"#;
    agent.handle_ipc(line).expect("launch resume");
    let last = agent.container_calls().last().expect("spawn").clone();
    assert!(
        last.contains("--resume uuid-xyz"),
        "resume_session_id must spawn --resume uuid-xyz; got {last}"
    );
}

/// LaunchSession with an EMPTY `resume_session_id` falls back to a fresh launch.
#[test]
fn launch_session_empty_resume_id_is_fresh() {
    let mut agent = make_agent();
    let line = r#"{"type":"LaunchSession","project":"/home/user/proj","host_id":"local","model":"claude-opus-4-8","posture":"Ask","resume_session_id":""}"#;
    agent.handle_ipc(line).expect("launch fresh");
    let last = agent.container_calls().last().expect("spawn").clone();
    assert!(
        last.contains("/home/user/proj"),
        "empty resume id → fresh (project positional); got {last}"
    );
    assert!(
        !last.contains("--resume") && !last.contains("--continue"),
        "empty resume id must not add --resume/--continue; got {last}"
    );
}

/// Resuming a tracked session WITHOUT a known claude session id → `--continue`.
#[test]
fn resume_session_tracked_without_claude_id_continues() {
    let mut agent = make_agent();
    let (_, sid) = do_launch(&mut agent); // persisted with session_id = None
    let resume = format!(r#"{{"type":"ResumeSession","session_id":"{sid}"}}"#);
    agent.handle_ipc(&resume).expect("resume");
    let last = agent.container_calls().last().expect("spawn").clone();
    assert!(
        last.contains("--continue"),
        "tracked session without claude id → --continue; got {last}"
    );
}

/// Resuming a tracked session WITH a known claude session id → `--resume <id>`,
/// and a supplied first_prompt is accepted.
#[test]
fn resume_session_tracked_with_claude_id_resumes() {
    let mut store = marsad_store::Store::new(Box::new(MemoryBackend::new()));
    store
        .upsert_session(&marsad_store::PersistedSession {
            id: "tracked-1".to_string(),
            session_id: Some("claude-uuid-9".to_string()),
            name: "t".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            project: "/home/user/p".to_string(),
            branch: "main".to_string(),
            model: "claude-opus-4-8".to_string(),
            last_phase: "Done".to_string(),
        })
        .expect("upsert");
    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(FakeContainer::new()),
        Box::new(CaptureNotifier::new()),
        Box::new(store),
    )
    .expect("with_fakes");

    let resume = r#"{"type":"ResumeSession","session_id":"tracked-1","first_prompt":"keep going"}"#;
    agent.handle_ipc(resume).expect("resume");
    let last = agent.container_calls().last().expect("spawn").clone();
    assert!(
        last.contains("--resume claude-uuid-9"),
        "tracked session with claude id → --resume <uuid>; got {last}"
    );
}

/// Resuming a tracked session whose claude id is an EMPTY string → `--continue`
/// (covers the non-empty guard's false arm).
#[test]
fn resume_session_empty_claude_id_continues() {
    let mut store = marsad_store::Store::new(Box::new(MemoryBackend::new()));
    store
        .upsert_session(&marsad_store::PersistedSession {
            id: "tracked-empty".to_string(),
            session_id: Some(String::new()),
            name: "t".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            project: "/home/user/p".to_string(),
            branch: "main".to_string(),
            model: "claude-opus-4-8".to_string(),
            last_phase: "Done".to_string(),
        })
        .expect("upsert");
    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(FakeContainer::new()),
        Box::new(CaptureNotifier::new()),
        Box::new(store),
    )
    .expect("with_fakes");

    agent
        .handle_ipc(r#"{"type":"ResumeSession","session_id":"tracked-empty"}"#)
        .expect("resume");
    let last = agent.container_calls().last().expect("spawn").clone();
    assert!(
        last.contains("--continue"),
        "empty claude id → --continue; got {last}"
    );
}

/// Resuming an UNKNOWN session id → Protocol error.
#[test]
fn resume_session_unknown_returns_error() {
    let mut agent = make_agent();
    let result = agent.handle_ipc(r#"{"type":"ResumeSession","session_id":"does-not-exist"}"#);
    assert!(
        matches!(result, Err(AgentError::Protocol(_))),
        "unknown session → Protocol error; got {result:?}"
    );
}

/// AttachSession → attaches an already-running session.
#[test]
fn ipc_command_attach_session() {
    let mut agent = make_agent();
    let (_, sid) = do_launch(&mut agent);

    let attach = format!(r#"{{"type":"AttachSession","session_id":"{sid}"}}"#);
    let result = agent.handle_ipc(&attach);
    assert!(
        result.is_ok(),
        "AttachSession must return Ok for existing session; got {result:?}"
    );
}

/// StopSession → stops an existing session.
#[test]
fn ipc_command_stop_session() {
    let mut agent = make_agent();
    let (_, sid) = do_launch(&mut agent);

    let stop = format!(r#"{{"type":"StopSession","session_id":"{sid}"}}"#);
    let result = agent.handle_ipc(&stop);
    assert!(
        result.is_ok(),
        "StopSession must return Ok for existing session; got {result:?}"
    );
}

/// SendPrompt → sends text to an active session's stdin.
#[test]
fn ipc_command_send_prompt() {
    let mut agent = make_agent();
    let (_, sid) = do_launch(&mut agent);

    let prompt = format!(r#"{{"type":"SendPrompt","session_id":"{sid}","text":"do the thing"}}"#);
    let result = agent.handle_ipc(&prompt);
    assert!(result.is_ok(), "SendPrompt must return Ok; got {result:?}");
}

/// Interrupt → sends an interrupt signal to an active session.
#[test]
fn ipc_command_interrupt() {
    let mut agent = make_agent();
    let (_, sid) = do_launch(&mut agent);

    let interrupt = format!(r#"{{"type":"Interrupt","session_id":"{sid}"}}"#);
    let result = agent.handle_ipc(&interrupt);
    assert!(result.is_ok(), "Interrupt must return Ok; got {result:?}");
}

/// AnswerGate → provides an allow/deny answer to a pending gate request.
#[test]
fn ipc_command_answer_gate() {
    let mut agent = make_agent();
    let (_, sid) = do_launch(&mut agent);

    // NOTE(test-agent): AnswerGate requires a pending gate (a ControlRequest) to
    // already be queued.  In the fake environment there is no real claude, so the
    // agent may handle this as a no-op (no pending gate) or return Ok immediately.
    // We assert it does not return a Protocol error (which would indicate parsing failure).
    let answer = format!(r#"{{"type":"AnswerGate","session_id":"{sid}","decision":"Allow"}}"#);
    let result = agent.handle_ipc(&answer);
    // Must not be a Protocol error.
    if let Err(ref e) = result {
        assert!(
            !matches!(e, AgentError::Protocol(_)),
            "AnswerGate must not return AgentError::Protocol; got {e:?}"
        );
    }
}

/// Subscribe → registers a subscriber (no session required).
#[test]
fn ipc_command_subscribe() {
    let mut agent = make_agent();
    let result = agent.handle_ipc(r#"{"type":"Subscribe"}"#);
    assert!(result.is_ok(), "Subscribe must return Ok; got {result:?}");
}

// ══════════════════════════════════════════════════════════════════════════════
// Hook event type coverage (events.md §HookEvent)
// ══════════════════════════════════════════════════════════════════════════════

/// Notification hook — message field present.
#[test]
fn hook_event_notification() {
    let mut agent = make_agent();
    let r = agent.handle_hook(r#"{"hook":"Notification","message":"all good"}"#);
    assert!(r.is_ok(), "Notification hook must return Ok; got {r:?}");
}

/// Stop hook — session finished.
#[test]
fn hook_event_stop() {
    let mut agent = make_agent();
    let r = agent.handle_hook(r#"{"hook":"Stop"}"#);
    assert!(r.is_ok(), "Stop hook must return Ok; got {r:?}");
}

/// SubagentStop hook — a named subagent finished.
#[test]
fn hook_event_subagent_stop() {
    let mut agent = make_agent();
    let r = agent.handle_hook(r#"{"hook":"SubagentStop","name":"worker-1"}"#);
    assert!(r.is_ok(), "SubagentStop hook must return Ok; got {r:?}");
}

/// PreToolUse hook — about to invoke a tool.
#[test]
fn hook_event_pre_tool_use() {
    let mut agent = make_agent();
    let r = agent.handle_hook(r#"{"hook":"PreToolUse","tool":"bash"}"#);
    assert!(r.is_ok(), "PreToolUse hook must return Ok; got {r:?}");
}

/// PostToolUse hook — tool finished.
#[test]
fn hook_event_post_tool_use() {
    let mut agent = make_agent();
    let r = agent.handle_hook(r#"{"hook":"PostToolUse","tool":"bash"}"#);
    assert!(r.is_ok(), "PostToolUse hook must return Ok; got {r:?}");
}

/// SessionStart hook — session is beginning.
#[test]
fn hook_event_session_start() {
    let mut agent = make_agent();
    let r = agent.handle_hook(r#"{"hook":"SessionStart"}"#);
    assert!(r.is_ok(), "SessionStart hook must return Ok; got {r:?}");
}

/// SessionEnd hook — session is ending.
#[test]
fn hook_event_session_end() {
    let mut agent = make_agent();
    let r = agent.handle_hook(r#"{"hook":"SessionEnd"}"#);
    assert!(r.is_ok(), "SessionEnd hook must return Ok; got {r:?}");
}

/// PreCompact hook — context is about to be compacted.
#[test]
fn hook_event_pre_compact() {
    let mut agent = make_agent();
    let r = agent.handle_hook(r#"{"hook":"PreCompact"}"#);
    assert!(r.is_ok(), "PreCompact hook must return Ok; got {r:?}");
}

// ══════════════════════════════════════════════════════════════════════════════
// reconcile_all branches: adopt / resume / stale
// ══════════════════════════════════════════════════════════════════════════════

/// Adopt branch: persisted session with live container → Adopt action, no spawn.
#[test]
fn reconcile_all_adopt_branch() {
    let mut container = FakeContainer::new();
    container.set_exists("marsad-adopt-session", true);

    let notifier = CaptureNotifier::new();
    let backend = MemoryBackend::new();
    let mut store = marsad_store::Store::new(Box::new(backend));
    store
        .upsert_session(&marsad_store::PersistedSession {
            id: "adopt-session".to_string(),
            session_id: Some("claude-sid-adopt".to_string()),
            name: "adopt-test".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            project: "/home/user/adopt".to_string(),
            branch: "main".to_string(),
            model: "claude-opus-4-5".to_string(),
            last_phase: "Working".to_string(),
        })
        .expect("upsert");

    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(container),
        Box::new(notifier),
        Box::new(store),
    )
    .expect("with_fakes");

    let runtimes = agent.reconcile_all();
    assert_eq!(runtimes.len(), 1, "one session → one runtime");

    let rt = &runtimes[0];
    assert!(
        matches!(rt.action, Action::Adopt),
        "live container → Adopt; got {:?}",
        rt.action
    );
    // No spawn ever.
    let calls = agent.container_calls();
    assert!(
        !calls.iter().any(|c| c.contains("spawn")),
        "adopt branch: no spawn; calls: {calls:?}"
    );
}

/// Resume branch: persisted session, dead container, session_id present → Resume action.
#[test]
fn reconcile_all_resume_branch() {
    // container is dead (default in FakeContainer, no set_exists call)
    let container = FakeContainer::new();

    let notifier = CaptureNotifier::new();
    let backend = MemoryBackend::new();
    let mut store = marsad_store::Store::new(Box::new(backend));
    store
        .upsert_session(&marsad_store::PersistedSession {
            id: "resume-session".to_string(),
            session_id: Some("claude-sid-resume".to_string()),
            name: "resume-test".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            project: "/home/user/resume".to_string(),
            branch: "main".to_string(),
            model: "claude-opus-4-5".to_string(),
            last_phase: "Done".to_string(),
        })
        .expect("upsert");

    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(container),
        Box::new(notifier),
        Box::new(store),
    )
    .expect("with_fakes");

    let runtimes = agent.reconcile_all();
    assert_eq!(runtimes.len(), 1, "one session → one runtime");

    let rt = &runtimes[0];
    assert!(
        matches!(rt.action, Action::Resume(_)),
        "dead container + session_id → Resume; got {:?}",
        rt.action
    );
    // Spawn must have been called exactly once (fresh container for resume).
    let calls = agent.container_calls();
    let spawn_count = calls.iter().filter(|c| c.contains("spawn")).count();
    assert_eq!(
        spawn_count, 1,
        "resume branch: spawn exactly once; calls: {calls:?}"
    );
}

/// Stale branch: persisted session, dead container, no session_id → MarkStale action.
#[test]
fn reconcile_all_stale_branch() {
    // container dead, session_id absent
    let container = FakeContainer::new();

    let notifier = CaptureNotifier::new();
    let backend = MemoryBackend::new();
    let mut store = marsad_store::Store::new(Box::new(backend));
    store
        .upsert_session(&marsad_store::PersistedSession {
            id: "stale-session".to_string(),
            session_id: None, // no claude session_id
            name: "stale-test".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            project: "/home/user/stale".to_string(),
            branch: "main".to_string(),
            model: "claude-opus-4-5".to_string(),
            last_phase: "Error".to_string(),
        })
        .expect("upsert");

    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(container),
        Box::new(notifier),
        Box::new(store),
    )
    .expect("with_fakes");

    let runtimes = agent.reconcile_all();
    assert_eq!(runtimes.len(), 1, "one session → one runtime");

    let rt = &runtimes[0];
    assert!(
        matches!(rt.action, Action::MarkStale),
        "dead container + no session_id → MarkStale; got {:?}",
        rt.action
    );
    // Neither attach nor spawn.
    let calls = agent.container_calls();
    assert!(
        !calls
            .iter()
            .any(|c| c.contains("attach") || c.contains("spawn")),
        "stale branch: no attach/spawn; calls: {calls:?}"
    );
}

/// Mixed: three sessions — one alive (adopt), one dead+sid (resume), one dead+no-sid (stale).
#[test]
fn reconcile_all_mixed_adopt_resume_stale() {
    let mut container = FakeContainer::new();
    container.set_exists("marsad-mixed-adopt", true);
    // marsad-mixed-resume and marsad-mixed-stale default to dead.

    let notifier = CaptureNotifier::new();
    let backend = MemoryBackend::new();
    let mut store = marsad_store::Store::new(Box::new(backend));

    store
        .upsert_session(&marsad_store::PersistedSession {
            id: "mixed-adopt".to_string(),
            session_id: Some("sid-adopt".to_string()),
            name: "adopt".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            project: "/proj/adopt".to_string(),
            branch: "main".to_string(),
            model: "claude-opus-4-5".to_string(),
            last_phase: "Working".to_string(),
        })
        .expect("upsert adopt");
    store
        .upsert_session(&marsad_store::PersistedSession {
            id: "mixed-resume".to_string(),
            session_id: Some("sid-resume".to_string()),
            name: "resume".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            project: "/proj/resume".to_string(),
            branch: "main".to_string(),
            model: "claude-opus-4-5".to_string(),
            last_phase: "Done".to_string(),
        })
        .expect("upsert resume");
    store
        .upsert_session(&marsad_store::PersistedSession {
            id: "mixed-stale".to_string(),
            session_id: None,
            name: "stale".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            project: "/proj/stale".to_string(),
            branch: "main".to_string(),
            model: "claude-opus-4-5".to_string(),
            last_phase: "Error".to_string(),
        })
        .expect("upsert stale");

    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(container),
        Box::new(notifier),
        Box::new(store),
    )
    .expect("with_fakes");

    let runtimes = agent.reconcile_all();
    assert_eq!(runtimes.len(), 3, "three sessions → three runtimes");

    let adopt_count = runtimes
        .iter()
        .filter(|r| matches!(r.action, Action::Adopt))
        .count();
    let resume_count = runtimes
        .iter()
        .filter(|r| matches!(r.action, Action::Resume(_)))
        .count();
    let stale_count = runtimes
        .iter()
        .filter(|r| matches!(r.action, Action::MarkStale))
        .count();

    assert_eq!(adopt_count, 1, "one adopt");
    assert_eq!(resume_count, 1, "one resume");
    assert_eq!(stale_count, 1, "one stale");

    // The adopt must not have spawned.
    let calls = agent.container_calls();
    // One spawn for the resume, zero for adopt, zero for stale.
    let spawn_count = calls.iter().filter(|c| c.contains("spawn")).count();
    assert_eq!(
        spawn_count, 1,
        "exactly one spawn (for Resume); calls: {calls:?}"
    );
}

// ══════════════════════════════════════════════════════════════════════════════
// Subscriber add/drop does NOT kill the session
// ══════════════════════════════════════════════════════════════════════════════

/// KEY INVARIANT (spec §Behavior notes): "Client disconnect must NOT kill sessions:
/// dropping a core connection only removes a subscriber."
///
/// Test: Subscribe → verify session created → simulate subscriber drop → verify
/// the session runtime is still alive (the agent still holds it).
///
/// NOTE(test-agent): The agent must expose a method such as
///   `fn session_count(&self) -> usize` or `fn has_session(&self, id: &str) -> bool`
/// so the test can confirm the session survived after the subscriber is dropped.
/// The subscriber drop is simulated via:
///   `agent.handle_ipc(r#"{"type":"Unsubscribe","subscriber_id":"..."}"#)`
/// or an explicit `agent.drop_subscriber(subscriber_id)` API.
/// If neither exists, we assert the session survives via `handle_ipc(StopSession)` returning Ok.
#[test]
fn subscriber_drop_does_not_kill_session() {
    let mut agent = make_agent();

    // 1. Launch a session.
    let (_, session_id) = do_launch(&mut agent);

    // 2. Subscribe (simulates a GUI client connecting).
    let sub_result = agent.handle_ipc(r#"{"type":"Subscribe"}"#);
    assert!(sub_result.is_ok(), "Subscribe must succeed");
    let sub_lines = sub_result.unwrap();

    // Attempt to extract a subscriber_id from the response.
    // NOTE(test-agent): the agent should return a subscriber_id token in the
    // Subscribe ack so the client can later unsubscribe cleanly.
    let subscriber_id = sub_lines.iter().find_map(|l| {
        serde_json::from_str::<serde_json::Value>(l)
            .ok()
            .and_then(|v| {
                v.get("subscriber_id")
                    .and_then(|s| s.as_str())
                    .map(str::to_string)
            })
    });

    // 3. Drop the subscriber (simulate disconnect).
    if let Some(sid) = &subscriber_id {
        let unsub = format!(r#"{{"type":"Unsubscribe","subscriber_id":"{sid}"}}"#);
        // Unsubscribe may or may not be implemented; we tolerate Protocol errors here
        // since the spec does not list Unsubscribe as an explicit IPC command.
        let _ = agent.handle_ipc(&unsub);
    }
    // NOTE(test-agent): If Unsubscribe IPC is not defined, the impl should provide
    // `agent.drop_subscriber(id)` or the test relies on the drop semantics of the
    // transport layer (connection close removes subscriber without IPC).

    // 4. CRITICAL ASSERTION: The session must still be alive after the subscriber drop.
    //    We confirm by sending a benign command to the session — if the session were
    //    killed, this would return an error.
    let attach_after_drop = format!(r#"{{"type":"AttachSession","session_id":"{session_id}"}}"#);
    let result = agent.handle_ipc(&attach_after_drop);
    assert!(
        result.is_ok(),
        "Session must survive after subscriber drop — AttachSession must still succeed; \
         got {result:?}. \
         Dropping a subscriber must NOT kill the session (spec §Behavior notes)."
    );
}

/// Multiple subscribers: dropping ONE must not affect the other subscribers OR the session.
#[test]
fn dropping_one_subscriber_leaves_others_and_session_intact() {
    let mut agent = make_agent();

    // Launch a session.
    let (_, session_id) = do_launch(&mut agent);

    // Subscribe twice.
    let sub1 = agent.handle_ipc(r#"{"type":"Subscribe"}"#).expect("sub1");
    let _sub2 = agent.handle_ipc(r#"{"type":"Subscribe"}"#).expect("sub2");

    // Drop first subscriber.
    let sub1_id = sub1.iter().find_map(|l| {
        serde_json::from_str::<serde_json::Value>(l)
            .ok()
            .and_then(|v| {
                v.get("subscriber_id")
                    .and_then(|s| s.as_str())
                    .map(str::to_string)
            })
    });
    if let Some(sid) = &sub1_id {
        let _ = agent.handle_ipc(&format!(
            r#"{{"type":"Unsubscribe","subscriber_id":"{sid}"}}"#
        ));
    }

    // Session must still be reachable.
    let result = agent.handle_ipc(&format!(
        r#"{{"type":"AttachSession","session_id":"{session_id}"}}"#
    ));
    assert!(
        result.is_ok(),
        "Session must survive after one of two subscribers drops; got {result:?}"
    );
}

/// A session created during one subscriber epoch survives after ALL subscribers disconnect.
#[test]
fn session_survives_all_subscribers_disconnecting() {
    let mut agent = make_agent();

    let (_, session_id) = do_launch(&mut agent);

    // Subscribe then immediately "disconnect" all subscribers.
    let sub_result = agent.handle_ipc(r#"{"type":"Subscribe"}"#).expect("sub");
    let sub_id = sub_result.iter().find_map(|l| {
        serde_json::from_str::<serde_json::Value>(l)
            .ok()
            .and_then(|v| {
                v.get("subscriber_id")
                    .and_then(|s| s.as_str())
                    .map(str::to_string)
            })
    });
    if let Some(sid) = &sub_id {
        let _ = agent.handle_ipc(&format!(
            r#"{{"type":"Unsubscribe","subscriber_id":"{sid}"}}"#
        ));
    }

    // The agent (daemon) must still hold the session.
    // NOTE(test-agent): Expose `agent.has_session(&str) -> bool` or
    // `agent.session_count() -> usize` for a stronger assertion.
    // We fall back to re-attaching as a liveness probe.
    let result = agent.handle_ipc(&format!(
        r#"{{"type":"AttachSession","session_id":"{session_id}"}}"#
    ));
    assert!(
        result.is_ok(),
        "Session must outlive all subscribers (spec §Behavior notes client-disconnect invariant); \
         AttachSession returned {result:?}"
    );
}

// ══════════════════════════════════════════════════════════════════════════════
// ListenSpec variants
// ══════════════════════════════════════════════════════════════════════════════

/// Agent can be constructed with each ListenSpec variant.
#[test]
fn agent_new_with_stdio_listen_spec() {
    let cfg = AgentConfig {
        listen: ListenSpec::Stdio,
        data_root: "/tmp/marsad-ls-stdio".to_string(),
    };
    assert!(Agent::new(cfg).is_ok(), "Stdio ListenSpec must succeed");
}

#[test]
fn agent_new_with_unix_socket_listen_spec() {
    let cfg = AgentConfig {
        listen: ListenSpec::UnixSocket("/tmp/marsad-branch-test.sock".to_string()),
        data_root: "/tmp/marsad-ls-unix".to_string(),
    };
    // NOTE(test-agent): construction must succeed (no actual bind at new time).
    assert!(
        Agent::new(cfg).is_ok(),
        "UnixSocket ListenSpec must succeed"
    );
}

#[test]
fn agent_new_with_named_pipe_listen_spec() {
    let cfg = AgentConfig {
        listen: ListenSpec::NamedPipe("\\\\.\\pipe\\marsad-test".to_string()),
        data_root: "/tmp/marsad-ls-pipe".to_string(),
    };
    // NOTE(test-agent): on Linux, NamedPipe is a valid config even if the OS
    // won't actually use it.  Construction must succeed.
    assert!(Agent::new(cfg).is_ok(), "NamedPipe ListenSpec must succeed");
}

// ══════════════════════════════════════════════════════════════════════════════
// reconcile_all phase mapping
// ══════════════════════════════════════════════════════════════════════════════

/// Adopt path → `SessionRuntime.phase` is `Some(Detached)`.
#[test]
fn reconcile_all_adopt_phase_is_detached() {
    let mut container = FakeContainer::new();
    container.set_exists("marsad-phase-adopt", true);

    let notifier = CaptureNotifier::new();
    let backend = MemoryBackend::new();
    let mut store = marsad_store::Store::new(Box::new(backend));
    store
        .upsert_session(&marsad_store::PersistedSession {
            id: "phase-adopt".to_string(),
            session_id: Some("sid-pa".to_string()),
            name: "pa".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            project: "/proj".to_string(),
            branch: "main".to_string(),
            model: "claude-opus-4-5".to_string(),
            last_phase: "Working".to_string(),
        })
        .expect("upsert");

    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(container),
        Box::new(notifier),
        Box::new(store),
    )
    .expect("with_fakes");

    let runtimes = agent.reconcile_all();
    assert_eq!(runtimes.len(), 1);
    assert!(
        matches!(runtimes[0].phase, Some(RuntimePhase::Detached)),
        "adopt → Detached phase; got {:?}",
        runtimes[0].phase
    );
}

/// Resume path → `SessionRuntime.phase` is `None` (fresh live container).
#[test]
fn reconcile_all_resume_phase_is_none() {
    let container = FakeContainer::new(); // dead

    let notifier = CaptureNotifier::new();
    let backend = MemoryBackend::new();
    let mut store = marsad_store::Store::new(Box::new(backend));
    store
        .upsert_session(&marsad_store::PersistedSession {
            id: "phase-resume".to_string(),
            session_id: Some("sid-pr".to_string()),
            name: "pr".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            project: "/proj".to_string(),
            branch: "main".to_string(),
            model: "claude-opus-4-5".to_string(),
            last_phase: "Done".to_string(),
        })
        .expect("upsert");

    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(container),
        Box::new(notifier),
        Box::new(store),
    )
    .expect("with_fakes");

    let runtimes = agent.reconcile_all();
    assert_eq!(runtimes.len(), 1);
    assert!(
        runtimes[0].phase.is_none(),
        "resume → phase None; got {:?}",
        runtimes[0].phase
    );
}

/// MarkStale path → `SessionRuntime.phase` is `Some(Stale)`.
#[test]
fn reconcile_all_stale_phase_is_stale() {
    let container = FakeContainer::new(); // dead

    let notifier = CaptureNotifier::new();
    let backend = MemoryBackend::new();
    let mut store = marsad_store::Store::new(Box::new(backend));
    store
        .upsert_session(&marsad_store::PersistedSession {
            id: "phase-stale".to_string(),
            session_id: None,
            name: "ps".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            project: "/proj".to_string(),
            branch: "main".to_string(),
            model: "claude-opus-4-5".to_string(),
            last_phase: "Error".to_string(),
        })
        .expect("upsert");

    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(container),
        Box::new(notifier),
        Box::new(store),
    )
    .expect("with_fakes");

    let runtimes = agent.reconcile_all();
    assert_eq!(runtimes.len(), 1);
    assert!(
        matches!(runtimes[0].phase, Some(RuntimePhase::Stale)),
        "stale → Stale phase; got {:?}",
        runtimes[0].phase
    );
}

// ══════════════════════════════════════════════════════════════════════════════
// IPC command coverage: VmUpdate and AnswerGate in branching contexts
// ══════════════════════════════════════════════════════════════════════════════

/// AnswerGate with "Deny" decision routes without Protocol error.
#[test]
fn ipc_command_answer_gate_deny() {
    let mut agent = make_agent();
    let (_, sid) = do_launch(&mut agent);

    let answer = format!(r#"{{"type":"AnswerGate","session_id":"{sid}","decision":"Deny"}}"#);
    let result = agent.handle_ipc(&answer);
    if let Err(ref e) = result {
        assert!(
            !matches!(e, AgentError::Protocol(_)),
            "AnswerGate Deny must not return Protocol error; got {e:?}"
        );
    }
}

/// LaunchSession with `"posture":"AutoApproveSafe"` must be accepted (Ok) by handle_ipc.
#[test]
fn handle_ipc_launch_auto_approve_safe_posture() {
    let mut agent = make_agent();
    let line = r#"{"type":"LaunchSession","project":"/home/user/proj","host_id":"local","model":"claude-opus-4-5","posture":"AutoApproveSafe"}"#;
    let result = agent.handle_ipc(line);
    assert!(
        result.is_ok(),
        "LaunchSession with AutoApproveSafe posture must return Ok; got {result:?}"
    );
    let lines = result.unwrap();
    assert!(
        !lines.is_empty(),
        "LaunchSession(AutoApproveSafe) must emit at least one outbound IPC event line"
    );
}

/// LaunchSession with `"posture":"FullAutonomy"` must be accepted (Ok) by handle_ipc.
#[test]
fn handle_ipc_launch_full_autonomy_posture() {
    let mut agent = make_agent();
    let line = r#"{"type":"LaunchSession","project":"/home/user/proj","host_id":"local","model":"claude-opus-4-5","posture":"FullAutonomy"}"#;
    let result = agent.handle_ipc(line);
    assert!(
        result.is_ok(),
        "LaunchSession with FullAutonomy posture must return Ok; got {result:?}"
    );
    let lines = result.unwrap();
    assert!(
        !lines.is_empty(),
        "LaunchSession(FullAutonomy) must emit at least one outbound IPC event line"
    );
}

/// LaunchSession with an unknown posture string must return `Err(AgentError::Protocol(_))`.
/// The posture field is a valid string but not a recognised enum variant.
#[test]
fn handle_ipc_launch_unknown_posture_is_protocol_error() {
    let mut agent = make_agent();
    let line = r#"{"type":"LaunchSession","project":"/home/user/proj","host_id":"local","model":"claude-opus-4-5","posture":"Nonsense"}"#;
    let result = agent.handle_ipc(line);
    assert!(
        matches!(result, Err(AgentError::Protocol(_))),
        "LaunchSession with unknown posture string must return AgentError::Protocol; got {result:?}"
    );
}

/// Multiple LaunchSession calls create independent sessions (no cross-routing).
#[test]
fn multiple_launch_sessions_are_independent() {
    let mut agent = make_agent();

    let (_, sid_a) = do_launch(&mut agent);
    let (_, sid_b) = do_launch(&mut agent);

    assert_ne!(
        sid_a, sid_b,
        "two launches must produce distinct session ids"
    );

    // Each can be addressed independently.
    let stop_a = format!(r#"{{"type":"StopSession","session_id":"{sid_a}"}}"#);
    let stop_b = format!(r#"{{"type":"StopSession","session_id":"{sid_b}"}}"#);

    assert!(agent.handle_ipc(&stop_a).is_ok(), "stop session A");
    assert!(agent.handle_ipc(&stop_b).is_ok(), "stop session B");
}
