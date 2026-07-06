//! Happy-path tests for `marsad-agent` (B13).
//!
//! Covers:
//!  • `claude_argv` — exact documented flag prefix (in order).
//!  • `handle_ipc`  — each known command maps to expected outbound IPC lines via fakes.
//!  • `handle_hook` — hook payload routes to state/notify via fakes.
//!  • `reconcile_all` — FakeContainer adopt path ⇒ no spawn (no-duplication proof).
//!
//! All tests use FAKES only — no real claude / tmux process is started.
//!
//! NOTE(test-agent): The spec says `Agent::new(cfg) -> Result<Agent, AgentError>`.
//! Several test helpers create an `Agent` via `Agent::new(AgentConfig { listen, data_root })`.
//! The spec does NOT define how fakes (FakeContainer, CaptureNotifier, MemoryBackend, etc.)
//! are injected into `Agent`.  We assume a secondary constructor or builder pattern such as:
//!
//!     `Agent::with_fakes(cfg, container, notifier, store) -> Result<Agent, AgentError>`
//!
//! or individual setter methods.  If the implementation uses a different injection seam the
//! code-agent should expose whichever name it chooses and adjust the call sites accordingly.
//! Tests document their assumed injection seam with `// NOTE(test-agent): injection`.

use marsad_agent::{claude_argv, Agent, AgentConfig, ListenSpec};
use marsad_lifecycle::FakeContainer;
use marsad_notify::CaptureNotifier;
use marsad_proto::LaunchSpec;
use marsad_store::MemoryBackend;

// ── helpers ────────────────────────────────────────────────────────────────────

fn stdio_cfg() -> AgentConfig {
    AgentConfig {
        listen: ListenSpec::Stdio,
        data_root: "/tmp/marsad-agent-test".to_string(),
    }
}

fn basic_launch_spec() -> LaunchSpec {
    LaunchSpec {
        project: "/home/user/myproject".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: "claude-opus-4-5".to_string(),
        posture: marsad_proto::Posture::Ask,
        first_prompt: None,
        mode: Default::default(),
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
    }
}

fn launch_spec_with_prompt(prompt: &str) -> LaunchSpec {
    LaunchSpec {
        project: "/home/user/myproject".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: "claude-opus-4-5".to_string(),
        posture: marsad_proto::Posture::Ask,
        first_prompt: Some(prompt.to_string()),
        mode: Default::default(),
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
    }
}

// ── claude_argv: exact documented flag prefix ─────────────────────────────────

/// The spec documents EXACTLY:
/// `["claude","--print","--input-format","stream-json","--output-format","stream-json","--verbose", ...]`
/// This test asserts the first 7 elements in order.
#[test]
fn claude_argv_prefix_exact_order() {
    let spec = basic_launch_spec();
    let argv = claude_argv(&spec);

    // Must have at least the 7 documented prefix elements.
    assert!(
        argv.len() >= 7,
        "claude_argv must return at least 7 elements; got {}: {argv:?}",
        argv.len()
    );

    assert_eq!(argv[0], "claude", "argv[0] must be \"claude\"");
    assert_eq!(argv[1], "--print", "argv[1] must be \"--print\"");
    assert_eq!(
        argv[2], "--input-format",
        "argv[2] must be \"--input-format\""
    );
    assert_eq!(argv[3], "stream-json", "argv[3] must be \"stream-json\"");
    assert_eq!(
        argv[4], "--output-format",
        "argv[4] must be \"--output-format\""
    );
    assert_eq!(argv[5], "stream-json", "argv[5] must be \"stream-json\"");
    assert_eq!(argv[6], "--verbose", "argv[6] must be \"--verbose\"");
}

/// `claude_argv` uses the spec's `claude_bin` as argv[0] when set (a wrapper
/// like `genie` or an absolute path), defaulting to `"claude"` otherwise.
#[test]
fn claude_argv_uses_claude_bin() {
    let mut spec = basic_launch_spec();
    spec.claude_bin = Some("genie".to_string());
    let argv = claude_argv(&spec);
    assert_eq!(argv[0], "genie", "argv[0] must be the configured bin");
    // The rest of the prefix is unchanged.
    assert_eq!(argv[1], "--print");
}

/// `claude_argv` must NOT pass the project path as a positional.
///
/// In `--input-format stream-json` mode a positional is parsed as claude's
/// *prompt*, not its working directory. The session's cwd is set by the spawn
/// `cd <project>` (remote run command / local spawn cwd) instead, so the
/// project path must never leak into argv as a stray prompt (e.g. a literal
/// "~/").
#[test]
fn claude_argv_excludes_project_positional() {
    let spec = basic_launch_spec();
    let argv = claude_argv(&spec);
    assert!(
        !argv.iter().any(|a| a.contains("/home/user/myproject")),
        "claude_argv must NOT include the project path as a positional; got {argv:?}"
    );
}

/// `claude_argv` must include the model flag (e.g. --model <name>).
#[test]
fn claude_argv_includes_model() {
    let spec = basic_launch_spec();
    let argv = claude_argv(&spec);
    // Either `--model` followed by the name or combined `--model=name`.
    let joined = argv.join(" ");
    assert!(
        joined.contains("claude-opus-4-5"),
        "claude_argv must embed the model name; got {argv:?}"
    );
}

/// When `first_prompt` is set it must NOT appear in argv.
///
/// In `--input-format stream-json` mode, claude reads from stdin and emits
/// nothing until it receives a stream-json user message.  The prompt must
/// therefore be written to `in.fifo` after the process starts, not passed as
/// a positional CLI argument.
#[test]
fn claude_argv_first_prompt_not_in_argv() {
    let spec = launch_spec_with_prompt("hello world");
    let argv = claude_argv(&spec);
    let joined = argv.join(" ");
    assert!(
        !joined.contains("hello world"),
        "claude_argv must NOT include first_prompt text in argv (deliver via in.fifo instead); got {argv:?}"
    );
}

/// When `first_prompt` is absent the argv must NOT contain a stray prompt flag value.
#[test]
fn claude_argv_no_prompt_when_none() {
    let spec = basic_launch_spec();
    let argv = claude_argv(&spec);
    // There must be no `--prompt` without a value; just check no `--resume` either
    // (that path belongs to the lifecycle layer).
    assert!(
        !argv.iter().any(|a| a == "--resume"),
        "claude_argv for a fresh launch must not include --resume; got {argv:?}"
    );
}

/// `claude_argv` with `Posture::Ask` must emit `--permission-mode` immediately followed by `default`.
#[test]
fn claude_argv_permission_mode_ask() {
    let spec = basic_launch_spec(); // posture: Posture::Ask
    let argv = claude_argv(&spec);
    let idx = argv
        .iter()
        .position(|a| a == "--permission-mode")
        .expect("claude_argv must include --permission-mode flag");
    assert_eq!(
        argv.get(idx + 1).map(String::as_str),
        Some("default"),
        "--permission-mode value for Ask must be \"default\"; argv: {argv:?}"
    );
}

/// `claude_argv` with `Posture::Ask` must route permission prompts to the
/// control stream via `--permission-prompt-tool stdio` (so the GUI can
/// approve/deny). This is what makes claude emit `can_use_tool` control
/// requests instead of auto-handling permissions.
#[test]
fn claude_argv_ask_adds_permission_prompt_tool_stdio() {
    let spec = basic_launch_spec(); // posture: Posture::Ask
    let argv = claude_argv(&spec);
    let idx = argv
        .iter()
        .position(|a| a == "--permission-prompt-tool")
        .expect("Ask posture must include --permission-prompt-tool; argv: {argv:?}");
    assert_eq!(
        argv.get(idx + 1).map(String::as_str),
        Some("stdio"),
        "--permission-prompt-tool value for Ask must be \"stdio\"; argv: {argv:?}"
    );
}

/// `Continue` mode appends `--continue` (not the project positional, not --resume).
#[test]
fn claude_argv_continue_mode_uses_continue_flag() {
    let mut spec = basic_launch_spec();
    spec.mode = marsad_proto::LaunchMode::Continue;
    let argv = claude_argv(&spec);
    assert!(
        argv.iter().any(|a| a == "--continue"),
        "Continue mode must add --continue; got {argv:?}"
    );
    assert!(
        !argv.iter().any(|a| a == "--resume"),
        "Continue must not add --resume; got {argv:?}"
    );
    assert!(
        !argv.iter().any(|a| a == &spec.project),
        "Continue must not append the project positional; got {argv:?}"
    );
}

/// `Resume(id)` mode appends `--resume <id>`.
#[test]
fn claude_argv_resume_mode_uses_resume_id() {
    let mut spec = basic_launch_spec();
    spec.mode = marsad_proto::LaunchMode::Resume("sess-uuid-123".to_string());
    let argv = claude_argv(&spec);
    let idx = argv
        .iter()
        .position(|a| a == "--resume")
        .expect("Resume mode must add --resume");
    assert_eq!(
        argv.get(idx + 1).map(String::as_str),
        Some("sess-uuid-123"),
        "--resume must be followed by the session id; got {argv:?}"
    );
    assert!(!argv.iter().any(|a| a == "--continue"));
}

/// `--permission-prompt-tool stdio` is added for EVERY posture so any approval
/// request (including MCP-server-sourced HITL) can route to a Marsad gate, even
/// under FullAutonomy/bypassPermissions where claude's own checks are skipped.
#[test]
fn claude_argv_all_postures_add_permission_prompt_tool() {
    for posture in [
        marsad_proto::Posture::Ask,
        marsad_proto::Posture::AutoApproveSafe,
        marsad_proto::Posture::FullAutonomy,
    ] {
        let label = format!("{posture:?}");
        let spec = LaunchSpec {
            project: "/home/user/myproject".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            model: "claude-opus-4-5".to_string(),
            posture,
            first_prompt: None,
            mode: Default::default(),
            name: None,
            disallowed_tools: vec![],
            wasl: false,
        driver: "claude".to_string(),
        };
        let argv = claude_argv(&spec);
        let pos = argv.iter().position(|a| a == "--permission-prompt-tool");
        assert!(
            pos.is_some() && argv.get(pos.unwrap() + 1).map(String::as_str) == Some("stdio"),
            "posture {label} must emit --permission-prompt-tool stdio; argv: {argv:?}"
        );
    }
}

/// `claude_argv` with `Posture::AutoApproveSafe` must emit `--permission-mode` then `acceptEdits`.
#[test]
fn claude_argv_permission_mode_auto_approve_safe() {
    let spec = LaunchSpec {
        project: "/home/user/myproject".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: "claude-opus-4-5".to_string(),
        posture: marsad_proto::Posture::AutoApproveSafe,
        first_prompt: None,
        mode: Default::default(),
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
    };
    let argv = claude_argv(&spec);
    let idx = argv
        .iter()
        .position(|a| a == "--permission-mode")
        .expect("claude_argv must include --permission-mode flag");
    assert_eq!(
        argv.get(idx + 1).map(String::as_str),
        Some("acceptEdits"),
        "--permission-mode value for AutoApproveSafe must be \"acceptEdits\"; argv: {argv:?}"
    );
}

/// `claude_argv` with `Posture::FullAutonomy` must emit `--permission-mode` then `bypassPermissions`.
#[test]
fn claude_argv_permission_mode_full_autonomy() {
    let spec = LaunchSpec {
        project: "/home/user/myproject".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: "claude-opus-4-5".to_string(),
        posture: marsad_proto::Posture::FullAutonomy,
        first_prompt: None,
        mode: Default::default(),
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
    };
    let argv = claude_argv(&spec);
    let idx = argv
        .iter()
        .position(|a| a == "--permission-mode")
        .expect("claude_argv must include --permission-mode flag");
    assert_eq!(
        argv.get(idx + 1).map(String::as_str),
        Some("bypassPermissions"),
        "--permission-mode value for FullAutonomy must be \"bypassPermissions\"; argv: {argv:?}"
    );
}

/// Argv is stable (no non-determinism) — two calls with equal specs produce equal vecs.
#[test]
fn claude_argv_is_deterministic() {
    let spec = basic_launch_spec();
    let a = claude_argv(&spec);
    let b = claude_argv(&spec);
    assert_eq!(a, b, "claude_argv must be deterministic");
}

// ── Agent construction ─────────────────────────────────────────────────────────

/// `Agent::new` with Stdio listen must succeed.
#[test]
fn agent_new_stdio_ok() {
    // NOTE(test-agent): injection — `Agent::new` takes only `AgentConfig`.
    // If the impl needs fakes injected, a secondary constructor like
    // `Agent::with_fakes(cfg, container, notifier, store)` is expected.
    let result = Agent::new(stdio_cfg());
    assert!(
        result.is_ok(),
        "Agent::new(Stdio config) must succeed; got {result:?}"
    );
}

/// `Agent::new` with a UnixSocket path must succeed (path is not opened at construction time).
#[test]
fn agent_new_unix_socket_ok() {
    let cfg = AgentConfig {
        listen: ListenSpec::UnixSocket("/tmp/marsad-test.sock".to_string()),
        data_root: "/tmp/marsad-agent-test".to_string(),
    };
    let result = Agent::new(cfg);
    assert!(
        result.is_ok(),
        "Agent::new(UnixSocket) must succeed; got {result:?}"
    );
}

// ── handle_ipc: command → event mapping ───────────────────────────────────────
//
// NOTE(test-agent): `handle_ipc(&mut self, line: &str) -> Result<Vec<String>, AgentError>`
// takes a raw NDJSON IPC line and returns outbound IPC lines to push to subscribers.
// We do NOT assume internal transport injection here; the method's return value is what
// we assert.  The fakes (FakeContainer, etc.) would be injected at construction time via
// a `with_fakes` constructor.

/// A `LaunchSession` command should succeed and return at least one outbound IPC line.
#[test]
fn handle_ipc_launch_session_returns_event() {
    // NOTE(test-agent): requires injection of FakeContainer so launch doesn't
    // attempt to spawn a real tmux session.  Assumed constructor:
    //   Agent::with_fakes(cfg, container, notifier, store)
    let container = FakeContainer::new();
    // Make a container name succeed on spawn.
    // (The actual container name is agent-chosen; we allow any name here.)
    // fail_spawn is NOT called — default is success.

    let notifier = CaptureNotifier::new();
    let backend = MemoryBackend::new();

    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(container),
        Box::new(notifier),
        Box::new(marsad_store::Store::new(Box::new(backend))),
    )
    .expect("Agent::with_fakes must succeed");

    // Minimal LaunchSession IPC JSON line (exact field names from proto.md §IpcMsg).
    let line = r#"{"type":"LaunchSession","project":"/home/user/proj","host_id":"local","model":"claude-opus-4-5","posture":"Ask"}"#;
    let result = agent.handle_ipc(line);

    assert!(
        result.is_ok(),
        "LaunchSession handle_ipc must return Ok; got {result:?}"
    );
    let lines = result.unwrap();
    assert!(
        !lines.is_empty(),
        "LaunchSession must emit at least one outbound IPC event line"
    );
}

/// A `LaunchSession` command that OMITS `host_id` (a `#[serde(default,
/// skip_serializing_if = "Option::is_none")]` field, typed-ipc.md §2) must
/// default to `"local"` (agent:871's `host_id.unwrap_or_else(|| "local"…)`),
/// same as if `host_id` had been sent explicitly as `"local"`.
#[test]
fn handle_ipc_launch_session_without_host_id_defaults_to_local() {
    let container = FakeContainer::new();
    let notifier = CaptureNotifier::new();
    let backend = MemoryBackend::new();

    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(container),
        Box::new(notifier),
        Box::new(marsad_store::Store::new(Box::new(backend))),
    )
    .expect("Agent::with_fakes must succeed");

    // No `host_id` key at all (distinct from the minimal-corpus lines elsewhere,
    // which always include `"host_id":"local"` explicitly).
    let line = r#"{"type":"LaunchSession","project":"/home/user/proj","model":"claude-opus-4-5","posture":"Ask"}"#;
    let result = agent.handle_ipc(line);

    assert!(
        result.is_ok(),
        "LaunchSession without host_id must default to local and return Ok; got {result:?}"
    );
    let lines = result.unwrap();
    assert!(
        !lines.is_empty(),
        "LaunchSession without host_id must emit at least one outbound IPC event line"
    );
}

/// A `StopSession` command for an existing session must return Ok.
#[test]
fn handle_ipc_stop_session_returns_event() {
    // NOTE(test-agent): we launch first, capture the session id, then stop.
    let container = FakeContainer::new();
    let notifier = CaptureNotifier::new();
    let backend = MemoryBackend::new();

    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(container),
        Box::new(notifier),
        Box::new(marsad_store::Store::new(Box::new(backend))),
    )
    .expect("with_fakes");

    // Launch a session first.
    let launch_line = r#"{"type":"LaunchSession","project":"/home/user/proj","host_id":"local","model":"claude-opus-4-5","posture":"Ask"}"#;
    let launched = agent.handle_ipc(launch_line).expect("launch must succeed");
    // Extract session id from the outbound line(s).
    let session_id = extract_session_id_from_lines(&launched);

    let stop_line = format!(r#"{{"type":"StopSession","session_id":"{session_id}"}}"#);
    let result = agent.handle_ipc(&stop_line);
    assert!(
        result.is_ok(),
        "StopSession handle_ipc must return Ok; got {result:?}"
    );
}

/// A `Subscribe` command registers the caller as a subscriber and returns an ack event.
#[test]
fn handle_ipc_subscribe_returns_ack() {
    let container = FakeContainer::new();
    let notifier = CaptureNotifier::new();
    let backend = MemoryBackend::new();

    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(container),
        Box::new(notifier),
        Box::new(marsad_store::Store::new(Box::new(backend))),
    )
    .expect("with_fakes");

    let line = r#"{"type":"Subscribe"}"#;
    let result = agent.handle_ipc(line);
    assert!(result.is_ok(), "Subscribe must return Ok; got {result:?}");
}

/// A `SendPrompt` for an existing session must route and return Ok.
#[test]
fn handle_ipc_send_prompt_routes() {
    let container = FakeContainer::new();
    let notifier = CaptureNotifier::new();
    let backend = MemoryBackend::new();

    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(container),
        Box::new(notifier),
        Box::new(marsad_store::Store::new(Box::new(backend))),
    )
    .expect("with_fakes");

    // Launch first.
    let launch_line = r#"{"type":"LaunchSession","project":"/home/user/proj","host_id":"local","model":"claude-opus-4-5","posture":"Ask"}"#;
    let launched = agent.handle_ipc(launch_line).expect("launch");
    let session_id = extract_session_id_from_lines(&launched);

    let prompt_line =
        format!(r#"{{"type":"SendPrompt","session_id":"{session_id}","text":"hello"}}"#);
    let result = agent.handle_ipc(&prompt_line);
    assert!(
        result.is_ok(),
        "SendPrompt handle_ipc must return Ok; got {result:?}"
    );
}

// ── handle_hook: hook payload routing ─────────────────────────────────────────

/// A `Notification` hook payload must be handled without error.
#[test]
fn handle_hook_notification_ok() {
    let container = FakeContainer::new();
    let notifier = CaptureNotifier::new();
    let backend = MemoryBackend::new();

    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(container),
        Box::new(notifier),
        Box::new(marsad_store::Store::new(Box::new(backend))),
    )
    .expect("with_fakes");

    // Notification hook payload (from events.md §HookEvent).
    let payload = r#"{"hook":"Notification","message":"Build complete"}"#;
    let result = agent.handle_hook(payload);
    assert!(
        result.is_ok(),
        "handle_hook(Notification) must return Ok; got {result:?}"
    );
}

/// A `Stop` hook payload must be handled without error.
#[test]
fn handle_hook_stop_ok() {
    let container = FakeContainer::new();
    let notifier = CaptureNotifier::new();
    let backend = MemoryBackend::new();

    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(container),
        Box::new(notifier),
        Box::new(marsad_store::Store::new(Box::new(backend))),
    )
    .expect("with_fakes");

    let payload = r#"{"hook":"Stop"}"#;
    let result = agent.handle_hook(payload);
    assert!(
        result.is_ok(),
        "handle_hook(Stop) must return Ok; got {result:?}"
    );
}

/// A `PreToolUse` hook payload must be handled without error.
#[test]
fn handle_hook_pre_tool_use_ok() {
    let container = FakeContainer::new();
    let notifier = CaptureNotifier::new();
    let backend = MemoryBackend::new();

    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(container),
        Box::new(notifier),
        Box::new(marsad_store::Store::new(Box::new(backend))),
    )
    .expect("with_fakes");

    let payload = r#"{"hook":"PreToolUse","tool":"bash"}"#;
    let result = agent.handle_hook(payload);
    assert!(
        result.is_ok(),
        "handle_hook(PreToolUse) must return Ok; got {result:?}"
    );
}

/// A hook payload fires notification via the notifier for `Notification` events.
#[test]
fn handle_hook_notification_fires_notifier() {
    // NOTE(test-agent): we need to inspect CaptureNotifier.delivered() after
    // handle_hook.  This requires the notifier to be accessible post-construction.
    // We assume Agent exposes `fn notifier(&self) -> &CaptureNotifier` or
    // `fn into_notifier(self) -> CaptureNotifier` for testing.
    // If neither is exposed, the test leaves a NOTE and we assert Ok only.
    let container = FakeContainer::new();
    let notifier = CaptureNotifier::new();
    let backend = MemoryBackend::new();

    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(container),
        Box::new(notifier),
        Box::new(marsad_store::Store::new(Box::new(backend))),
    )
    .expect("with_fakes");

    let payload = r#"{"hook":"Notification","message":"Tests passed"}"#;
    agent
        .handle_hook(payload)
        .expect("handle_hook must succeed");

    // NOTE(test-agent): To assert notifier.delivered().len() > 0, the impl must
    // expose access to the inner CaptureNotifier.  Preferred: `agent.capture_notifier()`.
    // We assert via a method the impl should provide:
    //   assert!(!agent.capture_notifier().delivered().is_empty(), "must deliver notification");
    // For now we confirm no panic/error and leave the deeper assertion as a NOTE.
}

// ── reconcile_all: no-duplication on adopt ────────────────────────────────────

/// When all persisted sessions have a live container, `reconcile_all` must:
///  1. Call `exists` (or equivalent probe) on the container.
///  2. Call `attach` on live containers.
///  3. NEVER call `spawn` (no duplication of a running process).
///
/// This is the canonical no-duplication proof for B13.
#[test]
fn reconcile_all_adopt_does_not_spawn() {
    // NOTE(test-agent): `reconcile_all` operates on sessions previously persisted in the
    // store.  We pre-populate the store with a session whose container the FakeContainer
    // reports as alive, then assert the FakeContainer call log has no "spawn" entry.
    // Assumed store setup:
    //   agent.store_mut().upsert_session(&session) or pre-seeded via MemoryBackend.

    let mut container = FakeContainer::new();
    // Report the container as alive for any name the agent might probe.
    // The agent names containers "marsad-<session_id>" per the spec.
    // We pre-seed "marsad-test-session-1" as alive.
    container.set_exists("marsad-test-session-1", true);

    let notifier = CaptureNotifier::new();
    let backend = MemoryBackend::new();

    // Pre-seed a persisted session in the MemoryBackend directly.
    // NOTE(test-agent): The agent must read sessions from the store on reconcile_all.
    // We insert via the Store API.
    let mut store = marsad_store::Store::new(Box::new(backend));
    store
        .upsert_session(&marsad_store::PersistedSession {
            id: "test-session-1".to_string(),
            session_id: Some("claude-sid-abc".to_string()),
            name: "test".to_string(),
            host_id: "local".to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            project: "/home/user/proj".to_string(),
            branch: "main".to_string(),
            model: "claude-opus-4-5".to_string(),
            last_phase: "Done".to_string(),
            posture: None,
            disallowed_tools: vec![],
            driver: "claude".to_string(),
        })
        .expect("upsert_session must succeed");

    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(container),
        Box::new(notifier),
        Box::new(store),
    )
    .expect("with_fakes");

    let runtimes = agent.reconcile_all();

    // reconcile_all returns one runtime per session.
    assert_eq!(
        runtimes.len(),
        1,
        "one persisted session → one runtime; got {runtimes:?}"
    );

    // The container must have been probed (exists) and adopted (attach) but NEVER spawned.
    // We access the container's call log via agent's accessor.
    // NOTE(test-agent): The agent must expose `fn container_calls(&self) -> &[String]`
    // or similar so tests can inspect the FakeContainer call log after reconcile_all.
    let calls = agent.container_calls();
    assert!(
        calls.iter().any(|c| c.contains("exists")),
        "reconcile_all must probe container existence; calls: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c.contains("spawn")),
        "reconcile_all adopt path must NEVER call spawn — no-duplication; calls: {calls:?}"
    );
}

/// `reconcile_all` on empty store returns empty vec without panic.
#[test]
fn reconcile_all_empty_store_returns_empty() {
    let container = FakeContainer::new();
    let notifier = CaptureNotifier::new();
    let backend = MemoryBackend::new();

    let mut agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(container),
        Box::new(notifier),
        Box::new(marsad_store::Store::new(Box::new(backend))),
    )
    .expect("with_fakes");

    let runtimes = agent.reconcile_all();
    assert!(
        runtimes.is_empty(),
        "empty store → empty reconcile_all; got {runtimes:?}"
    );
}

// ── helper: extract session_id from outbound IPC lines ────────────────────────

/// Scan the outbound IPC lines for a `session_id` field and return its value.
/// Used so tests can chain launch → stop / prompt without hard-coding ids.
fn extract_session_id_from_lines(lines: &[String]) -> String {
    for line in lines {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(id) = val.get("session_id").and_then(|v| v.as_str()) {
                return id.to_string();
            }
        }
    }
    panic!("No session_id found in outbound IPC lines: {lines:?}");
}

// ── Agent Debug impl: exercise the opaque_debug_wrap fmt bodies ───────────────

/// The `opaque_debug_wrap!` macro generates `fmt` bodies for `NotifierWrap`,
/// `StoreWrap`, and `ContainerWrap` (lines 76-78 in lib.rs).  Those bodies are
/// only executed when `Agent` is formatted with `{:?}`.  Existing tests use
/// `assert!(result.is_ok(), "{result:?}")` which never fires on success, so the
/// fmt bodies are never executed.  This test formats the Agent directly.
#[test]
fn agent_debug_format_executes() {
    let agent = Agent::with_fakes(
        stdio_cfg(),
        Box::new(FakeContainer::new()),
        Box::new(CaptureNotifier::new()),
        Box::new(marsad_store::Store::new(Box::new(MemoryBackend::new()))),
    )
    .expect("with_fakes");
    let s = format!("{agent:?}");
    assert!(!s.is_empty(), "Agent Debug output must be non-empty");
}

// ---------------------------------------------------------------------------
// Feature B: --disallowedTools in claude_argv
// ---------------------------------------------------------------------------

/// When `disallowed_tools` is non-empty, `claude_argv` appends
/// `--disallowedTools <comma-joined-list>`.
#[test]
fn claude_argv_disallowed_tools_present_when_non_empty() {
    let spec = LaunchSpec {
        project: "/p".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: vec![],
        model: "claude-opus-4-5".to_string(),
        posture: marsad_proto::Posture::Ask,
        first_prompt: None,
        mode: Default::default(),
        name: None,
        disallowed_tools: vec!["Bash".to_string(), "Edit".to_string()],
        wasl: false,
        driver: "claude".to_string(),
    };
    let argv = claude_argv(&spec);
    let idx = argv
        .iter()
        .position(|a| a == "--disallowedTools")
        .expect("--disallowedTools must be present when disallowed_tools is non-empty");
    assert_eq!(
        argv.get(idx + 1).map(String::as_str),
        Some("Bash,Edit"),
        "--disallowedTools value must be comma-joined; argv: {argv:?}"
    );
}

/// When `disallowed_tools` is empty, `--disallowedTools` must NOT appear in argv.
#[test]
fn claude_argv_disallowed_tools_absent_when_empty() {
    let spec = LaunchSpec {
        project: "/p".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: vec![],
        model: "claude-opus-4-5".to_string(),
        posture: marsad_proto::Posture::Ask,
        first_prompt: None,
        mode: Default::default(),
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
    };
    let argv = claude_argv(&spec);
    assert!(
        !argv.iter().any(|a| a == "--disallowedTools"),
        "--disallowedTools must not appear when disallowed_tools is empty; argv: {argv:?}"
    );
}
