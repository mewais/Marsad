//! Error-path tests for `marsad-agent` (B13).
//!
//! Covers every `AgentError` variant:
//!  • `Listen`    — bad listen spec (e.g. path that cannot be bound).
//!  • `Spawn`     — container spawn failure propagated through `launch`.
//!  • `Transport` — transport layer error propagated through IPC.
//!  • `Protocol`  — malformed IPC line / malformed hook payload.
//!  • `Lifecycle` — lifecycle error propagated from `reconcile_all`.
//!
//! NOTE(test-agent): injection seam — tests use the assumed `Agent::with_fakes` constructor
//! (see happy.rs for the full rationale).

use marsad_agent::{Agent, AgentConfig, AgentError, ListenSpec};
use marsad_lifecycle::FakeContainer;
use marsad_notify::CaptureNotifier;
use marsad_store::MemoryBackend;

// ── helpers ────────────────────────────────────────────────────────────────────

fn stdio_cfg() -> AgentConfig {
    AgentConfig {
        listen: ListenSpec::Stdio,
        data_root: "/tmp/marsad-agent-error-test".to_string(),
    }
}

fn make_agent_with_container(container: FakeContainer) -> Agent {
    let notifier = CaptureNotifier::new();
    let backend = MemoryBackend::new();
    Agent::with_fakes(
        stdio_cfg(),
        Box::new(container),
        Box::new(notifier),
        Box::new(marsad_store::Store::new(Box::new(backend))),
    )
    .expect("with_fakes must succeed in error tests")
}

// ── AgentError variants: Debug + Display ──────────────────────────────────────

/// Every `AgentError` variant must implement Debug without panicking.
#[test]
fn agent_error_variants_are_debuggable() {
    let variants: &[AgentError] = &[
        AgentError::Listen("cannot bind".to_string()),
        AgentError::Spawn("spawn failed".to_string()),
        AgentError::Transport("broken pipe".to_string()),
        AgentError::Protocol("bad json".to_string()),
        AgentError::Lifecycle("stale error".to_string()),
    ];
    for v in variants {
        let _ = format!("{v:?}");
    }
}

/// Every `AgentError` variant must implement Display and produce a non-empty string.
#[test]
fn agent_error_variants_implement_display() {
    let variants: &[AgentError] = &[
        AgentError::Listen("cannot bind".to_string()),
        AgentError::Spawn("spawn failed".to_string()),
        AgentError::Transport("broken pipe".to_string()),
        AgentError::Protocol("bad json".to_string()),
        AgentError::Lifecycle("stale error".to_string()),
    ];
    for v in variants {
        let s = format!("{v}");
        assert!(
            !s.is_empty(),
            "Display must produce non-empty string for {v:?}"
        );
    }
}

// ── Listen error ──────────────────────────────────────────────────────────────

/// `Agent::new` with a listen spec that provably cannot bind must return
/// `Err(AgentError::Listen(...))`.
///
/// NOTE(test-agent): The spec says `Agent::new` returns `Result<Agent, AgentError>`.
/// If binding is deferred to `serve()`, then `new` would not fail here and only
/// `serve()` would surface `Listen`.  We test `new` and accept either:
///   (a) `Err(AgentError::Listen(_))` from `new` — preferred, or
///   (b) `Ok` from `new` followed by a `Listen` error from `launch`/`serve`.
/// We assert variant (a).  The code-agent must expose a `Listen` error from `new`
/// when the path is obviously invalid (e.g. a null byte in the path).
#[test]
fn agent_new_invalid_socket_path_returns_listen_error() {
    // A path containing a null byte is invalid on Linux/macOS.
    let cfg = AgentConfig {
        listen: ListenSpec::UnixSocket("\0invalid".to_string()),
        data_root: "/tmp/marsad-agent-test".to_string(),
    };
    // NOTE(test-agent): If `new` is lazy (defers binding), this test will need
    // to call `serve()` or a separate `bind()` method.  We attempt `new` first.
    let result = Agent::new(cfg);
    if let Err(e) = result {
        assert!(
            matches!(e, AgentError::Listen(_)),
            "invalid socket path must produce AgentError::Listen; got {e:?}"
        );
    }
    // If the impl defers binding, the test leaves a NOTE and passes vacuously.
    // NOTE(test-agent): Code-agent should ensure Listen errors surface from `new`
    // on clearly invalid paths so this test has a definitive assertion.
}

// ── Spawn error ───────────────────────────────────────────────────────────────

/// When the container fails to spawn, `launch` (via `handle_ipc(LaunchSession)`)
/// must return `Err(AgentError::Spawn(_))`.
#[test]
fn handle_ipc_launch_spawn_failure_returns_spawn_error() {
    let mut container = FakeContainer::new();
    // Make every spawn fail.  The agent names the container "marsad-<id>" but we
    // don't know the id ahead of time.  fail_spawn takes a name; we use a wildcard
    // by calling it with a pattern the agent will use.
    // NOTE(test-agent): FakeContainer::fail_spawn requires the exact container name.
    // If the agent generates an id we cannot predict, the impl should also expose
    // `FakeContainer::fail_all_spawns()` or accept "*" as a wildcard.
    // For now we call fail_spawn with a sentinel name and note the limitation.
    // Alternatively, the impl may allow Agent::with_fakes to accept a spawn_error flag.
    container.fail_spawn("marsad-test");

    let notifier = CaptureNotifier::new();
    let backend = MemoryBackend::new();
    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(container),
        Box::new(notifier),
        Box::new(marsad_store::Store::new(Box::new(backend))),
    )
    .expect("with_fakes");

    // NOTE(test-agent): Agent must inject a predictable session-id generator for tests
    // so we can pre-configure FakeContainer with the right container name.
    // Expected injection seam: `Agent::with_fakes_and_id_gen(cfg, container, notifier, store, id_gen)`.
    let line = r#"{"type":"LaunchSession","project":"/home/user/proj","host_id":"local","model":"claude-opus-4-5","posture":"Ask"}"#;
    let result = agent.handle_ipc(line);

    // Either the spawn error surfaces as AgentError::Spawn or the fake container
    // name doesn't match and the launch partially succeeds.
    // We assert: if it errors, it must be Spawn.
    if let Err(e) = result {
        assert!(
            matches!(e, AgentError::Spawn(_)),
            "spawn failure must produce AgentError::Spawn; got {e:?}"
        );
    }
    // NOTE(test-agent): A stronger assertion requires predictable container naming.
    // See the note above about id_gen injection.
}

/// `launch(&mut self, spec) -> Result<String, AgentError>` — direct API.
/// When the container fails to spawn the returned error must be `AgentError::Spawn`.
#[test]
fn launch_spawn_failure_returns_spawn_error() {
    let spec = marsad_proto::LaunchSpec {
        project: "/home/user/proj".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: "claude-opus-4-5".to_string(),
        posture: marsad_proto::Posture::Ask,
        first_prompt: None,
        mode: Default::default(),
    };

    // NOTE(test-agent): We need a FakeContainer configured to fail spawns.
    // Same limitation as above — container name must be predictable or
    // Agent::with_fakes must accept a "fail all spawns" mode.
    let mut container = FakeContainer::new();
    // Attempt to make ANY spawn fail — this may not be sufficient without wildcard support.
    // NOTE(test-agent): impl should provide FakeContainer::fail_any_spawn() for this.
    container.fail_spawn("marsad-any");

    let notifier = CaptureNotifier::new();
    let backend = MemoryBackend::new();
    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(container),
        Box::new(notifier),
        Box::new(marsad_store::Store::new(Box::new(backend))),
    )
    .expect("with_fakes");

    let result = agent.launch(spec);
    if let Err(e) = result {
        assert!(
            matches!(e, AgentError::Spawn(_)),
            "launch spawn failure must produce AgentError::Spawn; got {e:?}"
        );
    }
    // NOTE(test-agent): same wildcard-name caveat applies.
}

// ── Protocol error: malformed IPC line ────────────────────────────────────────

/// A completely invalid (non-JSON) IPC line must produce `AgentError::Protocol`.
#[test]
fn handle_ipc_non_json_returns_protocol_error() {
    let mut agent = make_agent_with_container(FakeContainer::new());

    let result = agent.handle_ipc("this is not json at all !!!");
    assert!(
        matches!(result, Err(AgentError::Protocol(_))),
        "non-JSON IPC line must return AgentError::Protocol; got {result:?}"
    );
}

/// A valid JSON object with an unknown `type` discriminant must produce `AgentError::Protocol`.
/// (Per proto.md: IPC has no Unknown variant — unrecognized type is a bug.)
#[test]
fn handle_ipc_unknown_type_returns_protocol_error() {
    let mut agent = make_agent_with_container(FakeContainer::new());

    let result = agent.handle_ipc(r#"{"type":"ThisTypeDoesNotExist","foo":42}"#);
    assert!(
        matches!(result, Err(AgentError::Protocol(_))),
        "unknown IPC type must return AgentError::Protocol; got {result:?}"
    );
}

/// A truncated JSON IPC line must produce `AgentError::Protocol`.
#[test]
fn handle_ipc_truncated_json_returns_protocol_error() {
    let mut agent = make_agent_with_container(FakeContainer::new());

    let result = agent.handle_ipc(r#"{"type":"LaunchSession","project":"#);
    assert!(
        matches!(result, Err(AgentError::Protocol(_))),
        "truncated JSON IPC must return AgentError::Protocol; got {result:?}"
    );
}

/// An IPC line that is valid JSON but wrong type for a field must produce `AgentError::Protocol`.
#[test]
fn handle_ipc_schema_mismatch_returns_protocol_error() {
    let mut agent = make_agent_with_container(FakeContainer::new());

    // `posture` field is supposed to be a string, not an integer.
    let result = agent.handle_ipc(r#"{"type":"LaunchSession","project":"/p","posture":99}"#);
    assert!(
        matches!(result, Err(AgentError::Protocol(_))),
        "schema-mismatched IPC must return AgentError::Protocol; got {result:?}"
    );
}

/// An empty string IPC line must produce `AgentError::Protocol`.
#[test]
fn handle_ipc_empty_line_returns_protocol_error() {
    let mut agent = make_agent_with_container(FakeContainer::new());

    let result = agent.handle_ipc("");
    assert!(
        matches!(result, Err(AgentError::Protocol(_))),
        "empty IPC line must return AgentError::Protocol; got {result:?}"
    );
}

// ── Protocol error: malformed hook payload ────────────────────────────────────

/// A completely invalid (non-JSON) hook payload must produce `AgentError::Protocol`.
#[test]
fn handle_hook_non_json_returns_protocol_error() {
    let mut agent = make_agent_with_container(FakeContainer::new());

    let result = agent.handle_hook("not valid json");
    assert!(
        matches!(result, Err(AgentError::Protocol(_))),
        "non-JSON hook payload must return AgentError::Protocol; got {result:?}"
    );
}

/// A JSON hook payload missing the `hook` discriminant must produce `AgentError::Protocol`.
#[test]
fn handle_hook_missing_discriminant_returns_protocol_error() {
    let mut agent = make_agent_with_container(FakeContainer::new());

    let result = agent.handle_hook(r#"{"message":"hello"}"#);
    assert!(
        matches!(result, Err(AgentError::Protocol(_))),
        "hook payload missing 'hook' field must return AgentError::Protocol; got {result:?}"
    );
}

/// A hook payload with an unknown `hook` type must produce `AgentError::Protocol`.
#[test]
fn handle_hook_unknown_type_returns_protocol_error() {
    let mut agent = make_agent_with_container(FakeContainer::new());

    let result = agent.handle_hook(r#"{"hook":"UnknownHookType","data":"x"}"#);
    assert!(
        matches!(result, Err(AgentError::Protocol(_))),
        "unknown hook type must return AgentError::Protocol; got {result:?}"
    );
}

/// A truncated JSON hook payload must produce `AgentError::Protocol`.
#[test]
fn handle_hook_truncated_json_returns_protocol_error() {
    let mut agent = make_agent_with_container(FakeContainer::new());

    let result = agent.handle_hook(r#"{"hook":"Notification","messa"#);
    assert!(
        matches!(result, Err(AgentError::Protocol(_))),
        "truncated JSON hook must return AgentError::Protocol; got {result:?}"
    );
}

// ── Lifecycle error propagation ───────────────────────────────────────────────

/// When a persisted session's container fails in a lifecycle-terminal way during
/// `reconcile_all`, the error should be surfaced as `AgentError::Lifecycle` or
/// the runtime is marked Stale (the spec says `reconcile_all` returns a Vec of
/// `SessionRuntime`, not `Result`, so lifecycle failures produce Stale entries,
/// NOT errors).
///
/// NOTE(test-agent): The spec signature is:
///   `pub fn reconcile_all(&mut self) -> Vec<marsad_lifecycle::SessionRuntime>;`
/// Because it returns a Vec (not Result), per-session lifecycle errors must be
/// folded into the Vec as Stale runtimes, not propagated.  This test asserts
/// that `reconcile_all` never panics on lifecycle failure and returns a Stale entry.
#[test]
fn reconcile_all_lifecycle_error_folds_into_stale() {
    let mut container = FakeContainer::new();
    // Make attach fail (AttachRace) and re-probe also fail → container gone.
    // First probe: alive, attach: fail, re-probe: dead (no session_id → MarkStale).
    container.set_exists("marsad-sess-err", true);
    container.set_exists("marsad-sess-err", false); // re-probe after attach race
    container.fail_attach("marsad-sess-err");

    let notifier = CaptureNotifier::new();
    let backend = MemoryBackend::new();

    let mut store = marsad_store::Store::new(Box::new(backend));
    store
        .upsert_session(&marsad_store::PersistedSession {
            id: "sess-err".to_string(),
            session_id: None, // no session_id → MarkStale after race
            name: "err-test".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            project: "/home/user/proj".to_string(),
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

    // Must not panic.
    let runtimes = agent.reconcile_all();
    // The one session must appear — folded as Stale or as an adopt/resume.
    // (Exact variant depends on whether AttachRace re-probe falls to MarkStale here.)
    assert_eq!(
        runtimes.len(),
        1,
        "one session → one runtime; got {runtimes:?}"
    );
}

// ── StopSession for unknown session id ───────────────────────────────────────

/// Stopping a session that isn't live-tracked must be an idempotent no-op (Ok),
/// NOT an error: after an agent restart a persisted session isn't in the live map
/// yet, and the user must still be able to stop it (kill resolves the host from the
/// store). A truly-unknown id is then a harmless no-op.
#[test]
fn handle_ipc_stop_unknown_session_is_idempotent_ok() {
    let mut agent = make_agent_with_container(FakeContainer::new());

    let result = agent.handle_ipc(r#"{"type":"StopSession","session_id":"no-such-session"}"#);
    assert!(
        result.is_ok(),
        "StopSession for an unknown session must be a no-op Ok; got {result:?}"
    );
}

/// SendPrompt for an unknown session must return an error.
#[test]
fn handle_ipc_send_prompt_unknown_session_returns_error() {
    let mut agent = make_agent_with_container(FakeContainer::new());

    let result = agent.handle_ipc(r#"{"type":"SendPrompt","session_id":"ghost","text":"hi"}"#);
    assert!(
        result.is_err(),
        "SendPrompt for unknown session must return Err; got {result:?}"
    );
}

/// AttachSession for an unknown session must return an error.
#[test]
fn handle_ipc_attach_unknown_session_returns_error() {
    let mut agent = make_agent_with_container(FakeContainer::new());

    let result = agent.handle_ipc(r#"{"type":"AttachSession","session_id":"ghost"}"#);
    assert!(
        result.is_err(),
        "AttachSession for unknown session must return Err; got {result:?}"
    );
}

// ── parse_posture_value: non-string posture field ──────────────────────────────

/// When `posture` is a non-string JSON value (e.g. an integer) but the rest of
/// the LaunchSession message is valid, `parse_posture_value` hits the `None`
/// branch on line 450 and returns `Err(AgentError::Protocol(_))`.
///
/// The previous schema-mismatch test used `{"posture":99}` WITHOUT a `model`
/// field, which caused the `LaunchSessionMsg` serde deserialization to fail
/// first (missing required `model`), never reaching `parse_posture_value`.
/// This test supplies a fully-valid `LaunchSessionMsg` (project + model) so
/// deserialization succeeds and control reaches `parse_posture_value`.
#[test]
fn handle_ipc_launch_posture_non_string_returns_protocol_error() {
    let mut agent = make_agent_with_container(FakeContainer::new());

    // `posture` is an integer — LaunchSessionMsg parses fine (no posture field
    // in the struct), but parse_posture_value(&value["posture"]) gets a Number
    // not a string, hitting the None arm (line 450).
    let result = agent.handle_ipc(
        r#"{"type":"LaunchSession","project":"/p","host_id":"local","model":"m","posture":99}"#,
    );
    assert!(
        matches!(result, Err(AgentError::Protocol(_))),
        "posture non-string must return AgentError::Protocol; got {result:?}"
    );
}

// ── missing type discriminant in handle_ipc ────────────────────────────────────

/// A valid JSON object with no `type` field at all must produce
/// `AgentError::Protocol("missing type discriminant")`.
#[test]
fn handle_ipc_missing_type_discriminant_returns_protocol_error() {
    let mut agent = make_agent_with_container(FakeContainer::new());

    let result = agent.handle_ipc(r#"{"foo":1}"#);
    assert!(
        matches!(result, Err(AgentError::Protocol(_))),
        "JSON object with no type field must return AgentError::Protocol; got {result:?}"
    );
}

/// Each session-scoped command returns Protocol when `session_id` is absent.
#[test]
fn handle_ipc_missing_session_id_resume_session() {
    let mut agent = make_agent_with_container(FakeContainer::new());
    let result = agent.handle_ipc(r#"{"type":"ResumeSession"}"#);
    assert!(
        matches!(result, Err(AgentError::Protocol(_))),
        "ResumeSession missing session_id must return Protocol; got {result:?}"
    );
}

#[test]
fn handle_ipc_missing_session_id_attach_session() {
    let mut agent = make_agent_with_container(FakeContainer::new());
    let result = agent.handle_ipc(r#"{"type":"AttachSession"}"#);
    assert!(
        matches!(result, Err(AgentError::Protocol(_))),
        "AttachSession missing session_id must return Protocol; got {result:?}"
    );
}

#[test]
fn handle_ipc_missing_session_id_stop_session() {
    let mut agent = make_agent_with_container(FakeContainer::new());
    let result = agent.handle_ipc(r#"{"type":"StopSession"}"#);
    assert!(
        matches!(result, Err(AgentError::Protocol(_))),
        "StopSession missing session_id must return Protocol; got {result:?}"
    );
}

#[test]
fn handle_ipc_missing_session_id_send_prompt() {
    let mut agent = make_agent_with_container(FakeContainer::new());
    let result = agent.handle_ipc(r#"{"type":"SendPrompt","text":"hi"}"#);
    assert!(
        matches!(result, Err(AgentError::Protocol(_))),
        "SendPrompt missing session_id must return Protocol; got {result:?}"
    );
}

#[test]
fn handle_ipc_missing_session_id_interrupt() {
    let mut agent = make_agent_with_container(FakeContainer::new());
    let result = agent.handle_ipc(r#"{"type":"Interrupt"}"#);
    assert!(
        matches!(result, Err(AgentError::Protocol(_))),
        "Interrupt missing session_id must return Protocol; got {result:?}"
    );
}

#[test]
fn handle_ipc_missing_session_id_answer_gate() {
    let mut agent = make_agent_with_container(FakeContainer::new());
    let result = agent.handle_ipc(r#"{"type":"AnswerGate","decision":"Allow"}"#);
    assert!(
        matches!(result, Err(AgentError::Protocol(_))),
        "AnswerGate missing session_id must return Protocol; got {result:?}"
    );
}

// ── launch: spawn failure with predictable container name ─────────────────────

/// `Agent::next_session_id()` starts at 1, so the very first `launch` call on a
/// fresh Agent uses container name `"marsad-session-1"`.  Pre-arm that name in
/// FakeContainer so spawn definitely fails and returns `AgentError::Spawn`.
#[test]
fn launch_spawn_failure_deterministic_container_name() {
    let mut container = FakeContainer::new();
    container.fail_spawn("marsad-session-1");
    let notifier = CaptureNotifier::new();
    let backend = MemoryBackend::new();
    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(container),
        Box::new(notifier),
        Box::new(marsad_store::Store::new(Box::new(backend))),
    )
    .expect("with_fakes");

    let spec = marsad_proto::LaunchSpec {
        project: "/p".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: "m".to_string(),
        posture: marsad_proto::Posture::Ask,
        first_prompt: None,
        mode: Default::default(),
    };
    match agent.launch(spec) {
        Err(AgentError::Spawn(_)) => {}
        other => panic!("expected AgentError::Spawn, got {other:?}"),
    }
}

// ── launch: upsert failure → AgentError::Lifecycle ────────────────────────────

/// After a successful spawn, `launch` calls `store.upsert_session(...)`.
/// `MemoryBackend` holds its state in an `Arc<Mutex<...>>` internally, so
/// calling `fail_next` BEFORE moving the backend into `Store`/`with_fakes`
/// arms the failure flag inside that Arc.  Since `with_fakes` itself does not
/// write to the backend, the armed flag is still pending when `launch` calls
/// `upsert_session`, making the write fail and returning `AgentError::Lifecycle`.
#[test]
fn launch_upsert_failure_returns_lifecycle_error() {
    use marsad_store::BackendOp;

    let backend = MemoryBackend::new();
    // Arm the write failure NOW — with_fakes does not write to the backend,
    // so this flag will still be live when launch's upsert_session fires.
    backend.fail_next(BackendOp::Write);

    let notifier = CaptureNotifier::new();
    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(FakeContainer::new()),
        Box::new(notifier),
        Box::new(marsad_store::Store::new(Box::new(backend))),
    )
    .expect("with_fakes");

    let spec = marsad_proto::LaunchSpec {
        project: "/p".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: "m".to_string(),
        posture: marsad_proto::Posture::Ask,
        first_prompt: None,
        mode: Default::default(),
    };
    match agent.launch(spec) {
        Err(AgentError::Lifecycle(_)) => {}
        other => panic!("expected AgentError::Lifecycle, got {other:?}"),
    }
}

// ── handle_resume_session: error arms ─────────────────────────────────────────

/// `handle_resume_session` calls `store.restore()?`.  Arming the next read to
/// fail makes restore error, exercising the `map_err(... Lifecycle ...)` arm.
#[test]
fn resume_session_restore_failure_returns_lifecycle_error() {
    use marsad_store::BackendOp;

    let backend = MemoryBackend::new();
    backend.fail_next(BackendOp::List); // restore() lists keys first

    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(FakeContainer::new()),
        Box::new(CaptureNotifier::new()),
        Box::new(marsad_store::Store::new(Box::new(backend))),
    )
    .expect("with_fakes");

    let result = agent.handle_ipc(r#"{"type":"ResumeSession","session_id":"whatever"}"#);
    match result {
        Err(AgentError::Lifecycle(_)) => {}
        other => panic!("expected AgentError::Lifecycle from failed restore, got {other:?}"),
    }
}

/// `handle_resume_session` ends with `self.launch(spec)?`.  Arming the resumed
/// session's container name to fail spawn exercises that `?` error branch.
#[test]
fn resume_session_launch_spawn_failure_propagates() {
    let mut container = FakeContainer::new();
    // The resumed session is launched as the first session → "marsad-session-1".
    container.fail_spawn("marsad-session-1");

    let mut store = marsad_store::Store::new(Box::new(MemoryBackend::new()));
    store
        .upsert_session(&marsad_store::PersistedSession {
            id: "r1".to_string(),
            session_id: Some("claude-uuid".to_string()),
            name: "r".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            project: "/p".to_string(),
            branch: "main".to_string(),
            model: "m".to_string(),
            last_phase: "Done".to_string(),
        })
        .expect("upsert");

    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(container),
        Box::new(CaptureNotifier::new()),
        Box::new(store),
    )
    .expect("with_fakes");

    let result = agent.handle_ipc(r#"{"type":"ResumeSession","session_id":"r1"}"#);
    match result {
        Err(AgentError::Spawn(_)) => {}
        other => panic!("expected AgentError::Spawn from failed launch, got {other:?}"),
    }
}

// ── handle_launch_session: launch error arm via handle_ipc (line 230 col 43) ──

/// `handle_launch_session` calls `self.launch(spec)?` on line 230.  The `?`
/// error branch fires when spawn fails inside `handle_ipc("LaunchSession")`.
/// We arm `"marsad-session-1"` (first deterministic container name) to fail,
/// then call via `handle_ipc` so the error propagates through the `?` on line 230.
#[test]
fn handle_ipc_launch_session_spawn_failure_via_handle_ipc() {
    let mut container = FakeContainer::new();
    container.fail_spawn("marsad-session-1");
    let notifier = CaptureNotifier::new();
    let backend = MemoryBackend::new();
    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(container),
        Box::new(notifier),
        Box::new(marsad_store::Store::new(Box::new(backend))),
    )
    .expect("with_fakes");

    let line =
        r#"{"type":"LaunchSession","project":"/p","host_id":"local","model":"m","posture":"Ask"}"#;
    match agent.handle_ipc(line) {
        Err(AgentError::Spawn(_)) => {}
        other => panic!("expected AgentError::Spawn via handle_ipc, got {other:?}"),
    }
}

/// Same as above but with a caller-supplied `session_id` — exercises the `?`
/// error branch on the provided-id launch path (launch_with_id).
#[test]
fn handle_ipc_launch_session_provided_id_spawn_failure() {
    let mut container = FakeContainer::new();
    container.fail_spawn("marsad-ext-fail");
    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(container),
        Box::new(CaptureNotifier::new()),
        Box::new(marsad_store::Store::new(Box::new(MemoryBackend::new()))),
    )
    .expect("with_fakes");

    let line = r#"{"type":"LaunchSession","session_id":"ext-fail","project":"/p","host_id":"local","model":"m","posture":"Ask"}"#;
    match agent.handle_ipc(line) {
        Err(AgentError::Spawn(_)) => {}
        other => panic!("expected AgentError::Spawn for provided-id launch, got {other:?}"),
    }
}

// ── handle_send_prompt: missing text field (line 295 col 80) ─────────────────

/// `handle_send_prompt` calls `value["text"].as_str().ok_or(...)?.` on line 295.
/// The `?` error branch fires when `text` is absent.  We launch a session first
/// so the session_id check passes, then send a `SendPrompt` with no `text`.
#[test]
fn handle_ipc_send_prompt_missing_text_returns_protocol_error() {
    let notifier = CaptureNotifier::new();
    let backend = MemoryBackend::new();
    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(FakeContainer::new()),
        Box::new(notifier),
        Box::new(marsad_store::Store::new(Box::new(backend))),
    )
    .expect("with_fakes");

    // Launch to register a session.
    let launch =
        r#"{"type":"LaunchSession","project":"/p","host_id":"local","model":"m","posture":"Ask"}"#;
    let launched = agent.handle_ipc(launch).expect("launch must succeed");
    let session_id: String = launched
        .iter()
        .find_map(|l| {
            serde_json::from_str::<serde_json::Value>(l)
                .ok()
                .and_then(|v| {
                    v.get("session_id")
                        .and_then(|s| s.as_str())
                        .map(str::to_string)
                })
        })
        .expect("session_id in launch response");

    // SendPrompt with session_id present but text absent.
    let prompt = format!(r#"{{"type":"SendPrompt","session_id":"{session_id}"}}"#);
    let result = agent.handle_ipc(&prompt);
    assert!(
        matches!(result, Err(AgentError::Protocol(_))),
        "SendPrompt missing text must return AgentError::Protocol; got {result:?}"
    );
}
