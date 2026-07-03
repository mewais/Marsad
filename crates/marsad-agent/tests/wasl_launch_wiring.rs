//! wasl_launch_wiring.rs — regression coverage for review finding #2
//! ([HIGH] the X4 launch toggle is structurally incomplete; the host node is
//! never started): `Agent::launch`/`launch_with_id` must actually WIRE
//! `adopt_or_spawn_wasl_mcpd` into the launch path for a `wasl:true` local
//! launch (it was implemented + unit-tested in isolation, via
//! `tests/wasl_launch_attach.rs`, but had zero production call sites — a
//! `wasl:true` launch injected `--mcp-config` into claude's argv but nothing
//! ever spawned/adopted the node that config points at). And a `wasl:true`
//! REMOTE launch — for which the `--mcp-config` placeholder is never
//! resolved to a real path (`driver::materialize_wasl_mcp_config` is
//! local-only) — must fail fast with a clear error rather than silently
//! handing claude a broken, unresolved `--mcp-config wasl-mcp.json`
//! argument.
use marsad_agent::{Agent, AgentConfig, AgentError, ListenSpec};
use marsad_lifecycle::FakeContainer;
use marsad_notify::CaptureNotifier;
use marsad_proto::LaunchSpec;
use marsad_store::MemoryBackend;

fn stdio_cfg() -> AgentConfig {
    AgentConfig {
        listen: ListenSpec::Stdio,
        data_root: "/tmp/marsad-agent-test-wasl-launch-wiring".to_string(),
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
    }
}

fn agent_with_container(container: FakeContainer) -> Agent {
    Agent::with_fakes(
        stdio_cfg(),
        Box::new(container),
        Box::new(CaptureNotifier::new()),
        Box::new(marsad_store::Store::new(Box::new(MemoryBackend::new()))),
    )
    .expect("Agent::with_fakes must succeed")
}

// ---------------------------------------------------------------------------
// A LOCAL wasl:true launch must adopt-or-spawn the host wasl-mcpd node —
// via the SAME `Container` seam the session itself uses (the FakeContainer
// call log is the observable proof point, per house convention).
// ---------------------------------------------------------------------------

#[test]
fn local_wasl_true_launch_spawns_the_host_wasl_mcpd_node() {
    // Default FakeContainer: exists() is false for any unconfigured name,
    // so a fresh host must SPAWN wasl-mcpd (not adopt one).
    let mut agent = agent_with_container(FakeContainer::new());
    let mut spec = basic_launch_spec();
    spec.wasl = true;

    let session_id = agent
        .launch(spec)
        .expect("local wasl:true launch must succeed");

    let calls = agent.container_calls();
    assert!(
        calls.iter().any(|c| c == "exists:wasl-mcpd-local"),
        "a wasl:true launch must probe for a live wasl-mcpd; calls: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|c| c.starts_with("spawn:wasl-mcpd-local ")),
        "a wasl:true launch with no live wasl-mcpd must SPAWN one — this is the actual \
         production call site adopt_or_spawn_wasl_mcpd was missing; calls: {calls:?}"
    );
    // The session's own container must still be spawned, independently.
    assert!(
        calls
            .iter()
            .any(|c| c.starts_with(&format!("spawn:marsad-{session_id} "))),
        "the claude session itself must still be spawned; calls: {calls:?}"
    );
}

#[test]
fn local_wasl_true_launch_adopts_an_already_live_wasl_mcpd_without_respawning() {
    let mut container = FakeContainer::new();
    container.set_exists("wasl-mcpd-local", true);
    let mut agent = agent_with_container(container);
    let mut spec = basic_launch_spec();
    spec.wasl = true;

    agent
        .launch(spec)
        .expect("local wasl:true launch must succeed");

    let calls = agent.container_calls();
    assert!(
        calls
            .iter()
            .any(|c| c.starts_with("attach:wasl-mcpd-local")),
        "a live wasl-mcpd must be ADOPTED (attach), not respawned; calls: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c.starts_with("spawn:wasl-mcpd-local")),
        "a live wasl-mcpd must never be duplicated by a launch; calls: {calls:?}"
    );
}

#[test]
fn local_wasl_true_launch_scopes_the_wasl_mcpd_probe_by_host_id() {
    let mut container = FakeContainer::new();
    container.set_exists("wasl-mcpd-hpc", true);
    let mut agent = agent_with_container(container);
    let mut spec = basic_launch_spec();
    spec.wasl = true;
    spec.host_id = "hpc".to_string();

    agent
        .launch(spec)
        .expect("local wasl:true launch must succeed");

    let calls = agent.container_calls();
    assert!(
        calls.iter().any(|c| c.starts_with("exists:wasl-mcpd-hpc")),
        "the launch-wired probe must use the spec's host_id; calls: {calls:?}"
    );
}

/// The optionality invariant, at the launch-wiring level this time (not
/// just `claude_argv`'s byte-identical-argv proof in
/// `tests/wasl_launch_attach.rs`): a `wasl:false` launch must perform ZERO
/// wasl-mcpd container calls.
#[test]
fn local_wasl_false_launch_never_touches_wasl_mcpd() {
    let mut agent = agent_with_container(FakeContainer::new());
    let mut spec = basic_launch_spec();
    spec.wasl = false;

    agent.launch(spec).expect("launch must succeed");

    let calls = agent.container_calls();
    assert!(
        !calls.iter().any(|c| c.contains("wasl-mcpd")),
        "wasl:false must perform zero wasl-mcpd container calls; calls: {calls:?}"
    );
}

// ---------------------------------------------------------------------------
// A REMOTE wasl:true launch must fail fast with a clear error — never
// silently reach the container with an unresolved --mcp-config literal.
// ---------------------------------------------------------------------------

#[test]
fn remote_wasl_true_launch_fails_fast_instead_of_spawning_with_unresolved_mcp_config() {
    let mut agent = agent_with_container(FakeContainer::new());
    let mut spec = basic_launch_spec();
    spec.wasl = true;
    spec.host_target = Some("ssh:build-box".to_string());

    let err = agent
        .launch(spec)
        .expect_err("a wasl:true remote launch must be rejected, not silently broken");
    assert!(
        matches!(err, AgentError::Protocol(_)),
        "expected AgentError::Protocol for the honest not-supported-yet error, got {err:?}"
    );
    assert!(
        err.to_string().to_lowercase().contains("wasl"),
        "the error must name wasl as the reason; got: {err}"
    );

    // Nothing must have been spawned at all — reject BEFORE any container
    // call, not after a partially-broken launch.
    assert!(
        agent.container_calls().is_empty(),
        "a rejected wasl:true remote launch must perform ZERO container calls; calls: {:?}",
        agent.container_calls()
    );
}

/// A remote launch that does NOT request wasl is unaffected — the fail-fast
/// guard must be scoped to `wasl:true` only.
#[test]
fn remote_wasl_false_launch_is_unaffected_by_the_wasl_remote_guard() {
    let mut agent = agent_with_container(FakeContainer::new());
    let mut spec = basic_launch_spec();
    spec.wasl = false;
    spec.host_target = Some("ssh:build-box".to_string());

    // A real remote launch would go through `run_remote_script` (real IO);
    // under the `#[cfg(coverage)]` unit-test build `spawn_container` always
    // uses the injected `FakeContainer` regardless of `host_target` (see
    // `driver.rs`'s doc comment), so this just proves the wasl:true guard
    // itself doesn't fire for a wasl:false spec — the launch must not be
    // rejected with a wasl-flavored Protocol error.
    let result = agent.launch(spec);
    if let Err(e) = &result {
        assert!(
            !e.to_string().to_lowercase().contains("wasl"),
            "wasl:false must never be rejected by the wasl-remote guard; got: {e}"
        );
    }
}
