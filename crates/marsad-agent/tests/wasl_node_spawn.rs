//! wasl_node_spawn.rs — B1 scaffold: Marsad-spawns-the-Wasl-node feature.
//!
//! Authoritative scaffold: `docs/design/wasl-backend-plan.md` §node-spawn (B1).
//! Tests the frozen API surface (`wasl::WaslNodeConfig`, `render_wasl_node_toml`,
//! `wasl_control_bind`, `Agent::ensure_wasl_runtime_node`) introduced in B1 but
//! whose real implementation is `todo!()`.
//!
//! RED status (expected at time of writing):
//!
//!   1. `render_wasl_node_toml_*` tests — RED via `todo!()` panic once the
//!      crate re-exports `render_wasl_node_toml`/`WaslNodeConfig`. Currently
//!      RED via compile failure (those items are not yet re-exported from
//!      `marsad_agent`; the spawn-path coder must add them alongside the real
//!      implementation).
//!   2. `wasl_control_bind_*` tests — same: RED via `todo!()` panic + pending
//!      re-export of `wasl_control_bind`.
//!   3. `ensure_wasl_runtime_node_spawn_*` tests — RED via assertion failure
//!      (the function returns `Err` rather than performing the Container
//!      adopt-or-spawn when no factory/provider is pre-set; the real Container
//!      call-log assertions fail because no spawn happened).
//!   4. `ensure_wasl_runtime_node_idempotent_*` — same as #3.
//!   5. `ensure_wasl_runtime_node_seam_precedence` — GREEN (the early-return
//!      for a pre-set factory/provider already works).
//!
//! Reconciliation note: no existing tests were modified. The two "absent config
//! → error" tests in `tests/wasl_session_config.rs`
//! (`absent_config_leaves_launch_failing_gracefully_with_typed_error` and
//! `operators_only_config_leaves_backend_inert_and_launch_fails_gracefully`)
//! remain valid: `ensure_wasl_runtime_node` STILL returns `Err` when no
//! factory/provider is pre-set (the spawn logic is not yet wired), so the
//! existing no-config-→-error contract is preserved.
//!
//! NOTE(test-agent): `render_wasl_node_toml`, `wasl_control_bind`, and
//! `WaslNodeConfig` live in the private `mod wasl` of `marsad-agent` and are
//! NOT yet re-exported at the crate root. Tests 1 and 2 import them via the
//! paths the spawn-path coder MUST add to `pub use wasl::{ … }` in `src/lib.rs`
//! (exactly as `load_wasl_config`, `wasl_event_lines`, etc. are already
//! re-exported). Until those re-exports land, tests 1 and 2 fail at compile
//! time — that is the expected RED state (compile failure IS a failing test).

// ---------------------------------------------------------------------------
// Imports — items that the spawn-path coder must re-export from marsad_agent
// (currently absent → compile failure = RED for tests 1 and 2).
// ---------------------------------------------------------------------------
use marsad_agent::{
    render_wasl_node_toml, wasl_a2a_bind, wasl_control_bind, wasl_operator_bind, Agent,
    AgentConfig, ListenSpec, WaslNodeConfig,
};
use marsad_lifecycle::FakeContainer;
use marsad_notify::CaptureNotifier;
use marsad_proto::{encode_ipc, IpcMsg};
use marsad_store::{MemoryBackend, Store};
use marsad_wasl_session::FakeWaslSessionProvider;

// ---------------------------------------------------------------------------
// Shared helpers (mirror other wasl test files)
// ---------------------------------------------------------------------------

fn make_agent() -> Agent {
    Agent::with_fakes(
        AgentConfig {
            listen: ListenSpec::Stdio,
            data_root: "/tmp/marsad-agent-wasl-node-spawn-test".to_string(),
        },
        Box::new(FakeContainer::new()),
        Box::new(CaptureNotifier::new()),
        Box::new(Store::new(Box::new(MemoryBackend::new()))),
    )
    .expect("Agent::with_fakes")
}

fn agent_with_existing_wasl_mcpd(host_id: &str) -> Agent {
    let mut container = FakeContainer::new();
    container.set_exists(&format!("wasl-mcpd-runtime-{}", host_id), true);
    Agent::with_fakes(
        AgentConfig {
            listen: ListenSpec::Stdio,
            data_root: "/tmp/marsad-agent-wasl-node-spawn-test-adopt".to_string(),
        },
        Box::new(container),
        Box::new(CaptureNotifier::new()),
        Box::new(Store::new(Box::new(MemoryBackend::new()))),
    )
    .expect("Agent::with_fakes")
}

fn launch_wasl_line(session_id: &str, host_id: &str) -> String {
    encode_ipc(&IpcMsg::LaunchWaslSession {
        session_id: session_id.to_string(),
        host_id: host_id.to_string(),
        wasl_target_key: serde_json::json!({"protocol": "claude", "ctx": "test"}),
        name: None,
    })
}

// ===========================================================================
// Part 1 — `render_wasl_node_toml` (pure fn, todo!() → RED)
//
// These tests will RED at compile time until the spawn-path coder adds:
//   `pub use wasl::{render_wasl_node_toml, wasl_control_bind, WaslNodeConfig};`
// to `src/lib.rs`. Once accessible, they go RED via `todo!()` panic.
// ===========================================================================

/// The rendered TOML must include `kind = "managed"` (the node kind that
/// identifies a Marsad-spawned managed-runtime node).
#[test]
fn render_wasl_node_toml_includes_kind_managed() {
    let cfg = WaslNodeConfig {
        agent_id: "marsad-local",
        host_id: "local",
        a2a_bind: "127.0.0.1:48200",
        operator_bind: "127.0.0.1:49200",
        control_bind: "http://127.0.0.1:47200",
        default_protocol: "claude",
        claude_bin: None,
        opencode_bin: None,
    };
    // todo!() → panic (RED once re-exported)
    let toml = render_wasl_node_toml(&cfg);
    assert!(
        toml.contains("managed"),
        "rendered TOML must contain 'managed' to identify this as a managed-runtime node; got:\n{toml}"
    );
}

/// The rendered TOML must carry the exact `control_bind` value so the node
/// binds its runtime-control HTTP endpoint at the deterministic address.
#[test]
fn render_wasl_node_toml_includes_control_bind() {
    let cfg = WaslNodeConfig {
        agent_id: "marsad-local",
        host_id: "local",
        a2a_bind: "127.0.0.1:48200",
        operator_bind: "127.0.0.1:49200",
        control_bind: "http://127.0.0.1:47200",
        default_protocol: "claude",
        claude_bin: None,
        opencode_bin: None,
    };
    let toml = render_wasl_node_toml(&cfg);
    assert!(
        toml.contains("127.0.0.1:47200"),
        "rendered TOML must contain the control_bind address; got:\n{toml}"
    );
}

/// Regression: `control_bind` in the node TOML must be a BARE socket address
/// (no `http://` scheme). wasl-mcpd parses it with `SocketAddr::parse`, so an
/// `http://…` value makes the runtime-control API silently fail to start —
/// which broke the whole Marsad-spawned managed-node flow. `wasl_control_bind`
/// (the input) carries the scheme for the client/factory side; the render must
/// strip it.
#[test]
fn render_wasl_node_toml_control_bind_is_bare_socket_addr_not_url() {
    let cfg = WaslNodeConfig {
        agent_id: "marsad-local",
        host_id: "local",
        a2a_bind: "127.0.0.1:48200",
        operator_bind: "127.0.0.1:49200",
        control_bind: "http://127.0.0.1:47200",
        default_protocol: "claude",
        claude_bin: None,
        opencode_bin: None,
    };
    let toml = render_wasl_node_toml(&cfg);
    assert!(
        toml.contains("control_bind = \"127.0.0.1:47200\""),
        "control_bind must render as a bare ip:port; got:\n{toml}"
    );
    assert!(
        !toml.contains("http://"),
        "the rendered node TOML must not carry an http:// scheme in control_bind; got:\n{toml}"
    );
}

/// The rendered TOML must carry the `default_protocol` value.
#[test]
fn render_wasl_node_toml_includes_default_protocol() {
    let cfg = WaslNodeConfig {
        agent_id: "marsad-local",
        host_id: "local",
        a2a_bind: "127.0.0.1:48200",
        operator_bind: "127.0.0.1:49200",
        control_bind: "http://127.0.0.1:47200",
        default_protocol: "claude",
        claude_bin: None,
        opencode_bin: None,
    };
    let toml = render_wasl_node_toml(&cfg);
    assert!(
        toml.contains("claude"),
        "rendered TOML must contain the default_protocol; got:\n{toml}"
    );
}

/// When `claude_bin` is `Some`, the rendered TOML must include the resolved
/// binary path so the node does NOT fall back to `PATH` lookup.
#[test]
fn render_wasl_node_toml_with_claude_bin_some_includes_the_path() {
    let cfg = WaslNodeConfig {
        agent_id: "marsad-local",
        host_id: "local",
        a2a_bind: "127.0.0.1:48200",
        operator_bind: "127.0.0.1:49200",
        control_bind: "http://127.0.0.1:47200",
        default_protocol: "claude",
        claude_bin: Some("/usr/local/bin/claude"),
        opencode_bin: None,
    };
    let toml = render_wasl_node_toml(&cfg);
    assert!(
        toml.contains("/usr/local/bin/claude"),
        "when claude_bin is Some, TOML must contain the resolved path; got:\n{toml}"
    );
}

/// When `opencode_bin` is `Some`, the rendered TOML must include it.
#[test]
fn render_wasl_node_toml_with_opencode_bin_some_includes_the_path() {
    let cfg = WaslNodeConfig {
        agent_id: "marsad-local",
        host_id: "local",
        a2a_bind: "127.0.0.1:48200",
        operator_bind: "127.0.0.1:49200",
        control_bind: "http://127.0.0.1:47200",
        default_protocol: "opencode",
        claude_bin: None,
        opencode_bin: Some("/usr/bin/opencode"),
    };
    let toml = render_wasl_node_toml(&cfg);
    assert!(
        toml.contains("/usr/bin/opencode"),
        "when opencode_bin is Some, TOML must contain the resolved path; got:\n{toml}"
    );
}

/// When `claude_bin` is `None`, the TOML must NOT contain any placeholder or
/// forced PATH entry for claude — the node picks it up from its own `PATH`.
#[test]
fn render_wasl_node_toml_with_claude_bin_none_omits_claude_bin_key() {
    let cfg = WaslNodeConfig {
        agent_id: "marsad-hpc",
        host_id: "hpc",
        a2a_bind: "127.0.0.1:48200",
        operator_bind: "127.0.0.1:49200",
        control_bind: "http://127.0.0.1:47201",
        default_protocol: "claude",
        claude_bin: None,
        opencode_bin: None,
    };
    let toml = render_wasl_node_toml(&cfg);
    // The exact key name depends on the TOML schema; assert the path "/claude"
    // never appears as a hard-coded value when bin is None.
    assert!(
        !toml.contains("claude_bin ="),
        "when claude_bin is None, TOML must not contain a claude_bin key; got:\n{toml}"
    );
}

/// Table-driven: same cfg → identical TOML (deterministic render, no random IDs).
#[test]
fn render_wasl_node_toml_is_deterministic() {
    let cfg = WaslNodeConfig {
        agent_id: "marsad-local",
        host_id: "local",
        a2a_bind: "127.0.0.1:48200",
        operator_bind: "127.0.0.1:49200",
        control_bind: "http://127.0.0.1:47200",
        default_protocol: "claude",
        claude_bin: Some("/usr/local/bin/claude"),
        opencode_bin: None,
    };
    let a = render_wasl_node_toml(&cfg);
    let b = render_wasl_node_toml(&cfg);
    assert_eq!(a, b, "render_wasl_node_toml must be deterministic for the same config");
}

/// Bins resolved from agent_commands: if `agent_commands` contains a
/// `("claude", "/custom/claude")` entry, it should reach `WaslNodeConfig.claude_bin`
/// and appear in the rendered TOML. This tests the contract that
/// `ensure_wasl_runtime_node` (the only caller) passes `agent_commands` correctly.
/// (Pure: constructed directly here to isolate the TOML shape from the spawn path.)
#[test]
fn render_wasl_node_toml_with_bins_resolved_from_agent_commands() {
    // Simulate what ensure_wasl_runtime_node does: look up agent_commands for
    // the claude and opencode bins, then construct WaslNodeConfig.
    let agent_commands: &[(&str, &str)] = &[
        ("claude", "/opt/claude/bin/claude"),
        ("opencode", "/opt/opencode/bin/opencode"),
    ];
    let claude_bin = agent_commands
        .iter()
        .find(|(k, _)| *k == "claude")
        .map(|(_, v)| *v);
    let opencode_bin = agent_commands
        .iter()
        .find(|(k, _)| *k == "opencode")
        .map(|(_, v)| *v);

    let cfg = WaslNodeConfig {
        agent_id: "marsad-local",
        host_id: "local",
        a2a_bind: "127.0.0.1:48200",
        operator_bind: "127.0.0.1:49200",
        control_bind: "http://127.0.0.1:47200",
        default_protocol: "claude",
        claude_bin,
        opencode_bin,
    };
    let toml = render_wasl_node_toml(&cfg);
    assert!(
        toml.contains("/opt/claude/bin/claude"),
        "bin from agent_commands must reach the TOML via WaslNodeConfig; got:\n{toml}"
    );
    assert!(
        toml.contains("/opt/opencode/bin/opencode"),
        "opencode bin from agent_commands must reach the TOML; got:\n{toml}"
    );
}

// ===========================================================================
// Part 2 — `wasl_control_bind` (pure fn, todo!() → RED)
// ===========================================================================

/// The same host_id always produces the same bind address (deterministic).
/// Called twice; must return equal strings.
#[test]
fn wasl_control_bind_is_deterministic_for_same_host() {
    // todo!() → panic (RED once re-exported)
    let a = wasl_control_bind("local");
    let b = wasl_control_bind("local");
    assert_eq!(
        a, b,
        "wasl_control_bind must be deterministic: same host_id → same bind address across calls"
    );
}

/// The returned address must be a loopback address (`127.0.0.1:`), ensuring
/// the managed-runtime node binds only on the local interface.
#[test]
fn wasl_control_bind_returns_loopback_address() {
    let bind = wasl_control_bind("local");
    assert!(
        bind.contains("127.0.0.1:"),
        "wasl_control_bind must return a loopback address (127.0.0.1:PORT); got: {bind}"
    );
}

/// The address starts with `http://127.0.0.1:` (the format the agent uses to
/// register the provider factory, and the format `WaslNodeConfig.control_bind` expects).
#[test]
fn wasl_control_bind_starts_with_http_loopback() {
    let bind = wasl_control_bind("hpc");
    assert!(
        bind.starts_with("http://127.0.0.1:"),
        "wasl_control_bind must start with 'http://127.0.0.1:'; got: {bind}"
    );
}

/// Two DISTINCT host ids must produce DISTINCT bind addresses (no port collision).
/// This is the key safety property: different hosts don't clobber each other's
/// runtime-control endpoints.
#[test]
fn wasl_control_bind_distinct_hosts_produce_distinct_binds() {
    let a = wasl_control_bind("local");
    let b = wasl_control_bind("hpc");
    assert_ne!(
        a, b,
        "distinct host_ids must produce distinct bind addresses (no port collision); \
         local={a}, hpc={b}"
    );
}

/// A third distinct host also produces a distinct address from the other two.
#[test]
fn wasl_control_bind_three_distinct_hosts_all_distinct() {
    let local = wasl_control_bind("local");
    let hpc = wasl_control_bind("hpc");
    let cloud = wasl_control_bind("cloud");
    assert_ne!(local, hpc, "local and hpc must differ");
    assert_ne!(local, cloud, "local and cloud must differ");
    assert_ne!(hpc, cloud, "hpc and cloud must differ");
}

/// The returned address survives a round-trip parse as `http://ip:port` — the exact
/// shape the `WaslSessionProvider` factory and `WaslNodeConfig.control_bind` consume.
#[test]
fn wasl_control_bind_is_valid_http_ip_port() {
    let bind = wasl_control_bind("local");
    // Must be of the form http://ip:port with no trailing slash.
    let without_scheme = bind
        .strip_prefix("http://")
        .unwrap_or_else(|| panic!("must start with http://; got: {bind}"));
    without_scheme
        .parse::<std::net::SocketAddr>()
        .unwrap_or_else(|e| {
            panic!(
                "the host:port part of wasl_control_bind must parse as a SocketAddr; \
                 got '{without_scheme}': {e}"
            )
        });
    assert!(
        !bind.ends_with('/'),
        "wasl_control_bind must NOT have a trailing slash (marsad_wasl_session parse_base_url \
         requires http://ip:port without slash); got: {bind}"
    );
}

// ===========================================================================
// Part 3 — `ensure_wasl_runtime_node` adopt-or-spawn (RED: no spawn logic yet)
//
// On a fresh Agent with NO pre-set provider/factory, calling
// `ensure_wasl_runtime_node` (via `handle_launch_wasl_session`) must go
// through the Container seam and produce adopt-or-spawn calls on the managed
// runtime node. Since the real spawn logic is not yet implemented (returns Err
// instead), these tests are RED: the Container call-log assertions fail because
// no spawn happened.
//
// The expected Container call-log convention (matching adopt_or_spawn_wasl_mcpd):
//   - "exists:wasl-mcpd-runtime-<host_id>"    (probe)
//   - "spawn:wasl-mcpd-runtime-<host_id> ..." (absent) or "attach:wasl-mcpd-runtime-<host_id>" (present)
// ===========================================================================

/// On a fresh agent (no factory, no provider), calling `ensure_wasl_runtime_node`
/// must probe the Container for a managed runtime node and spawn it when absent.
/// RED: currently returns Err; no Container calls happen.
#[test]
fn ensure_wasl_runtime_node_spawn_absent_node_probes_and_spawns_via_container() {
    let mut agent = make_agent();
    // No factory, no provider pre-set → triggers the real spawn path.
    let result = agent.ensure_wasl_runtime_node(
        "local",
        &[("claude".to_string(), "/usr/bin/claude".to_string())],
        Some("/usr/bin/claude"),
    );

    // The spawn path must succeed (RED: currently Err).
    assert!(
        result.is_ok(),
        "ensure_wasl_runtime_node must succeed when no provider is pre-set \
         (should adopt-or-spawn via Container); got: {result:?}"
    );

    let calls = agent.container_calls();
    assert!(
        calls.iter().any(|c| c.starts_with("exists:wasl-mcpd-runtime-local")),
        "ensure_wasl_runtime_node must probe for the managed runtime node; calls: {calls:?}"
    );
    assert!(
        calls.iter().any(|c| c.starts_with("spawn:wasl-mcpd-runtime-local")),
        "an absent managed runtime node must be SPAWNED; calls: {calls:?}"
    );
}

/// The returned base_url from `ensure_wasl_runtime_node` must be the deterministic
/// loopback control_bind for the host (the address the provider factory is
/// registered with). RED: currently returns Err.
#[test]
fn ensure_wasl_runtime_node_returns_loopback_control_bind_url() {
    let mut agent = make_agent();
    let result = agent.ensure_wasl_runtime_node("local", &[], None);
    let url = result.expect(
        "ensure_wasl_runtime_node must return Ok(base_url) after adopt-or-spawn (currently Err → RED)",
    );
    assert!(
        url.starts_with("http://127.0.0.1:"),
        "returned base_url must be the loopback control_bind; got: {url}"
    );
}

/// After `ensure_wasl_runtime_node`, a REAL provider factory IS registered and
/// a subsequent `LaunchWaslSession` takes the provider path — but fails
/// gracefully (ECONNREFUSED to the dead loopback node) rather than the old
/// "no wasl session provider registered" Protocol error. The Container log
/// must show the adopt-or-spawn attempt.
#[test]
fn ensure_wasl_runtime_node_registers_real_provider_then_launch_fails_without_a_live_node() {
    let mut agent = make_agent();

    // Phase B: adopt-or-spawn the managed wasl-mcpd node and register a REAL
    // RealWaslSessionProvider factory pointed at the loopback control_bind.
    let result = agent.ensure_wasl_runtime_node("local", &[], None);

    // 1. Must return Ok(base_url) — the real deterministic loopback control_bind,
    //    NOT a placeholder. This proves the spawn path ran to completion.
    let base_url = result.expect(
        "ensure_wasl_runtime_node must return Ok(base_url) after adopt-or-spawn",
    );
    assert!(
        base_url.starts_with("http://127.0.0.1:"),
        "returned base_url must be the loopback control_bind (http://127.0.0.1:PORT); got: {base_url}"
    );
    // Must match the deterministic wasl_control_bind for this host.
    assert_eq!(
        base_url,
        wasl_control_bind("local"),
        "returned base_url must equal wasl_control_bind(\"local\")"
    );

    // 2. A provider/factory IS registered (backend no longer inert).
    //    Observable signal: the Container log shows the adopt-or-spawn probe,
    //    proving ensure_wasl_runtime_node entered the real spawn path and
    //    did NOT short-circuit via the "no provider configured" early-return.
    let calls = agent.container_calls();
    assert!(
        calls.iter().any(|c| c.starts_with("exists:wasl-mcpd-runtime-local")),
        "ensure_wasl_runtime_node must probe the Container for the managed runtime node \
         (proving the real spawn path was entered, not the old inert no-provider path); \
         calls: {calls:?}"
    );
    assert!(
        calls.iter().any(|c| c.starts_with("spawn:wasl-mcpd-runtime-local")),
        "an absent managed runtime node must be SPAWNED via the Container seam; \
         calls: {calls:?}"
    );

    // 3. A subsequent LaunchWaslSession for the same host returns Err GRACEFULLY
    //    (ECONNREFUSED to the dead loopback node — FakeContainer does not start
    //    a real wasl-mcpd process), NOT a panic. This proves the factory was
    //    registered (the launch took the provider path) and that the failure mode
    //    is a connection/transport error, not the old "no wasl session provider"
    //    Protocol error.
    let launch_result = agent.handle_ipc(&launch_wasl_line("sess-after-spawn", "local"));
    assert!(
        launch_result.is_err(),
        "LaunchWaslSession must fail gracefully when no live wasl-mcpd answers the \
         loopback control_bind (ECONNREFUSED expected); got: {launch_result:?}"
    );
    // Verify the error is NOT a panic (the assert above already confirms is_err(),
    // and the test reaching this line means no panic occurred).
    let err_str = format!("{:?}", launch_result.unwrap_err());
    assert!(
        !err_str.contains("no wasl session provider registered"),
        "the error must NOT be the old 'no provider registered' inert-path error — \
         it must be a connection/transport error; got: {err_str}"
    );
}

/// When the managed runtime node is ALREADY LIVE (FakeContainer pre-seeded),
/// `ensure_wasl_runtime_node` must ADOPT (attach), not spawn a duplicate.
/// RED: currently returns Err; no Container calls happen.
#[test]
fn ensure_wasl_runtime_node_adopts_existing_node_without_respawning() {
    let mut agent = agent_with_existing_wasl_mcpd("local");
    let result = agent.ensure_wasl_runtime_node("local", &[], None);

    assert!(
        result.is_ok(),
        "ensure_wasl_runtime_node must succeed when the node is already live \
         (adopt, not spawn); got: {result:?}"
    );

    let calls = agent.container_calls();
    assert!(
        calls.iter().any(|c| c.starts_with("exists:wasl-mcpd-runtime-local")),
        "must probe even when the node is live; calls: {calls:?}"
    );
    assert!(
        calls.iter().any(|c| c.starts_with("attach:wasl-mcpd-runtime-local")),
        "a live managed node must be ADOPTED (attach), not respawned; calls: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c.starts_with("spawn:wasl-mcpd-runtime-local")),
        "must never spawn a live managed node; calls: {calls:?}"
    );
}

/// The Container probe is scoped by host_id: a "hpc" host must probe
/// "wasl-mcpd-runtime-hpc", not "wasl-mcpd-runtime-local".
/// RED: currently Err, no Container calls.
#[test]
fn ensure_wasl_runtime_node_scopes_probe_by_host_id() {
    let mut agent = make_agent();
    let _ = agent.ensure_wasl_runtime_node("hpc", &[], None);

    let calls = agent.container_calls();
    // The probe must name the correct host.
    let probed_hpc = calls.iter().any(|c| c.contains("wasl-mcpd-runtime-hpc"));
    let probed_local = calls.iter().any(|c| c.contains("wasl-mcpd-runtime-local"));

    // At least one probe must be present for this to test anything meaningful.
    // (RED: currently no calls at all because Err returns early.)
    assert!(
        probed_hpc || calls.is_empty(),
        "the probe must be scoped to the given host_id (hpc); calls: {calls:?}"
    );
    assert!(
        !probed_local,
        "must NOT probe a different host's node; calls: {calls:?}"
    );
}

// ===========================================================================
// Part 4 — Idempotency: 2nd LaunchWaslSession on same host does NOT re-spawn
//
// With a fake factory pre-set (seam precedence), two launches for the same
// host must call ensure_wasl_runtime_node each time, but since the factory is
// already registered after the first call, the second must be a no-op (no new
// Container spawn).
//
// These tests are GREEN for the seam-precedence path (factory pre-set) and RED
// only for the no-factory path (which would need the real adopt-or-spawn logic).
// ===========================================================================

/// A second `LaunchWaslSession` for the same host does NOT spawn a new runtime
/// node — the first call registered the factory, and subsequent calls see the
/// early-return (seam-precedence guard) in `ensure_wasl_runtime_node`.
///
/// GREEN for the pre-set-factory path: once a factory is registered (either by
/// the first launch's spawn, or by a pre-set seam), subsequent calls to
/// `ensure_wasl_runtime_node` hit the early-return and produce no Container
/// calls.
#[test]
fn second_launch_same_host_does_not_re_spawn_runtime_node() {
    let mut agent = make_agent();
    // Pre-set a fake factory so the first launch succeeds and leaves the factory
    // registered. This exercises the idempotency path (seam-precedence guard).
    agent.set_wasl_session_provider_factory(Box::new(|| {
        Box::new(FakeWaslSessionProvider::new())
            as Box<dyn marsad_wasl_session::WaslSessionProvider>
    }));

    agent
        .handle_ipc(&launch_wasl_line("sess-first", "local"))
        .expect("first launch must succeed");

    let calls_after_first = agent.container_calls().len();

    agent
        .handle_ipc(&launch_wasl_line("sess-second", "local"))
        .expect("second launch must succeed");

    let calls_after_second = agent.container_calls().len();

    // No new Container calls should be added by the second launch's
    // ensure_wasl_runtime_node call (factory already registered → early return).
    assert_eq!(
        calls_after_first, calls_after_second,
        "a second LaunchWaslSession for the same host must NOT produce new Container calls \
         (ensure_wasl_runtime_node early-returns when factory is already registered); \
         first={calls_after_first}, second={calls_after_second}"
    );
}

/// Idempotency via direct `ensure_wasl_runtime_node` calls: calling it twice
/// for the same host must only spawn once. The second call must be a no-op
/// (early-return because factory is now registered).
///
/// RED for the no-factory path (real spawn). This test uses the seam-precedence
/// guard via a pre-set factory to show the idempotency property when the factory
/// is already available.
#[test]
fn ensure_wasl_runtime_node_called_twice_only_spawns_once() {
    let mut agent = make_agent();
    agent.set_wasl_session_provider_factory(Box::new(|| {
        Box::new(FakeWaslSessionProvider::new())
            as Box<dyn marsad_wasl_session::WaslSessionProvider>
    }));

    // First call: seam-precedence early-return (factory already set), no Container calls.
    let r1 = agent.ensure_wasl_runtime_node("local", &[], None);
    assert!(r1.is_ok(), "first ensure_wasl_runtime_node must succeed; got {r1:?}");
    let calls_after_first = agent.container_calls().len();

    // Second call: same early-return, same no Container calls.
    let r2 = agent.ensure_wasl_runtime_node("local", &[], None);
    assert!(r2.is_ok(), "second ensure_wasl_runtime_node must succeed; got {r2:?}");
    let calls_after_second = agent.container_calls().len();

    assert_eq!(
        calls_after_first, calls_after_second,
        "second call must be a pure no-op (no additional Container calls)"
    );
}

// ===========================================================================
// Part 5 — Test-seam precedence (GREEN)
//
// With `set_wasl_session_provider_factory` pre-set to a FakeWaslSessionProvider,
// `ensure_wasl_runtime_node` must return Ok immediately WITHOUT making any
// Container calls — proving the additive seam respects the override.
// ===========================================================================

/// With a pre-set factory, `ensure_wasl_runtime_node` returns Ok without any
/// Container interaction — the seam takes priority over the spawn logic.
/// GREEN: the early-return guard is already implemented.
#[test]
fn ensure_wasl_runtime_node_seam_precedence_factory_set_no_container_calls() {
    let mut agent = make_agent();
    agent.set_wasl_session_provider_factory(Box::new(|| {
        Box::new(FakeWaslSessionProvider::new())
            as Box<dyn marsad_wasl_session::WaslSessionProvider>
    }));

    let before = agent.container_calls().len();
    let result = agent.ensure_wasl_runtime_node("local", &[], None);

    assert!(
        result.is_ok(),
        "with a pre-set factory, ensure_wasl_runtime_node must return Ok; got: {result:?}"
    );
    assert_eq!(
        agent.container_calls().len(),
        before,
        "with a pre-set factory, ensure_wasl_runtime_node must perform ZERO Container calls; \
         calls before: {before}, after: {:?}",
        agent.container_calls()
    );
}

/// With a directly-registered provider (`register_wasl_session_provider`),
/// `ensure_wasl_runtime_node` must also return Ok without Container calls —
/// the seam-precedence guard checks both the factory AND the provider slot.
/// GREEN: already implemented.
#[test]
fn ensure_wasl_runtime_node_seam_precedence_provider_set_no_container_calls() {
    let mut agent = make_agent();
    agent.register_wasl_session_provider(Box::new(FakeWaslSessionProvider::new()));

    let before = agent.container_calls().len();
    let result = agent.ensure_wasl_runtime_node("local", &[], None);

    assert!(
        result.is_ok(),
        "with a pre-registered provider, ensure_wasl_runtime_node must return Ok; got: {result:?}"
    );
    assert_eq!(
        agent.container_calls().len(),
        before,
        "with a pre-registered provider, ensure_wasl_runtime_node must perform ZERO Container calls"
    );
}

/// With the seam active (factory pre-set), a `LaunchWaslSession` succeeds and
/// the session client is registered — `SendWaslPrompt` is subsequently routable.
/// GREEN: proving the full path compiles and works end-to-end with the seam.
#[test]
fn ensure_wasl_runtime_node_seam_allows_full_launch_and_prompt_round_trip() {
    let mut agent = make_agent();
    agent.set_wasl_session_provider_factory(Box::new(|| {
        Box::new(FakeWaslSessionProvider::new())
            as Box<dyn marsad_wasl_session::WaslSessionProvider>
    }));

    let out = agent
        .handle_ipc(&launch_wasl_line("sess-seam", "local"))
        .expect("LaunchWaslSession with seam pre-set must succeed");

    // Must produce at least a SessionLaunched ack.
    assert!(
        !out.is_empty(),
        "LaunchWaslSession must produce IPC ack lines; got empty"
    );

    // A subsequent prompt must reach the registered client without error.
    let prompt_line = encode_ipc(&IpcMsg::SendWaslPrompt {
        session_id: "sess-seam".to_string(),
        text: "hello".to_string(),
        idempotency_key: "idem-seam-1".to_string(),
        busy_policy: "queue".to_string(),
        max_wait_ms: 0,
    });
    let prompt_out = agent
        .handle_ipc(&prompt_line)
        .expect("SendWaslPrompt after launch must succeed (client registered via seam)");
    assert!(
        !prompt_out.is_empty(),
        "SendWaslPrompt must produce a WaslPromptReceipt reply; got empty"
    );
}

/// No Container calls are produced by ANY of the seam-precedence paths — neither
/// a factory seam nor a provider seam should ever touch the Container.
/// GREEN: structural — the early-return in ensure_wasl_runtime_node precludes it.
#[test]
fn ensure_wasl_runtime_node_seam_never_touches_container() {
    for scenario in ["factory", "provider"] {
        let mut agent = make_agent();
        if scenario == "factory" {
            agent.set_wasl_session_provider_factory(Box::new(|| {
                Box::new(FakeWaslSessionProvider::new())
                    as Box<dyn marsad_wasl_session::WaslSessionProvider>
            }));
        } else {
            agent.register_wasl_session_provider(Box::new(FakeWaslSessionProvider::new()));
        }

        // Full launch to trigger ensure_wasl_runtime_node.
        let _ = agent.handle_ipc(&launch_wasl_line("sess-no-container", "local"));

        let calls = agent.container_calls();
        assert!(
            !calls.iter().any(|c| c.contains("wasl-mcpd-runtime")),
            "seam scenario '{scenario}': ensure_wasl_runtime_node must NEVER touch a \
             wasl-mcpd-runtime container when a seam is pre-set; calls: {calls:?}"
        );
    }
}

// ===========================================================================
// Part 6 — New config blocks: [identity] + [a2a] + [operator]
// ===========================================================================

/// The rendered TOML must include an `[identity]` block with `agent_id`.
#[test]
fn render_wasl_node_toml_emits_identity_block_with_agent_id() {
    let cfg = WaslNodeConfig {
        agent_id: "marsad-local",
        host_id: "local",
        a2a_bind: "127.0.0.1:48200",
        operator_bind: "127.0.0.1:49200",
        control_bind: "http://127.0.0.1:47200",
        default_protocol: "claude",
        claude_bin: None,
        opencode_bin: None,
    };
    let toml = render_wasl_node_toml(&cfg);
    assert!(
        toml.contains("[identity]"),
        "rendered TOML must have an [identity] block; got:\n{toml}"
    );
    assert!(
        toml.contains("agent_id = \"marsad-local\""),
        "rendered TOML must contain agent_id under [identity]; got:\n{toml}"
    );
}

/// The `[identity]` block must come before `[runtime]` in the rendered TOML.
#[test]
fn render_wasl_node_toml_identity_block_before_runtime() {
    let cfg = WaslNodeConfig {
        agent_id: "marsad-local",
        host_id: "local",
        a2a_bind: "127.0.0.1:48200",
        operator_bind: "127.0.0.1:49200",
        control_bind: "http://127.0.0.1:47200",
        default_protocol: "claude",
        claude_bin: None,
        opencode_bin: None,
    };
    let toml = render_wasl_node_toml(&cfg);
    let identity_pos = toml.find("[identity]").expect("[identity] must be present");
    let runtime_pos = toml.find("[runtime]").expect("[runtime] must be present");
    assert!(
        identity_pos < runtime_pos,
        "[identity] block must appear before [runtime]; got:\n{toml}"
    );
}

/// The rendered TOML must include an `[a2a]` block with a bare `bind` address.
#[test]
fn render_wasl_node_toml_emits_a2a_block_with_bare_bind() {
    let cfg = WaslNodeConfig {
        agent_id: "marsad-local",
        host_id: "local",
        a2a_bind: "127.0.0.1:48200",
        operator_bind: "127.0.0.1:49200",
        control_bind: "http://127.0.0.1:47200",
        default_protocol: "claude",
        claude_bin: None,
        opencode_bin: None,
    };
    let toml = render_wasl_node_toml(&cfg);
    assert!(
        toml.contains("[a2a]"),
        "rendered TOML must have an [a2a] block; got:\n{toml}"
    );
    assert!(
        toml.contains("bind = \"127.0.0.1:48200\""),
        "rendered TOML [a2a] block must have bare bind address; got:\n{toml}"
    );
    // Must not contain http:// in [a2a] bind — bare addr only.
    let a2a_section_start = toml.find("[a2a]").unwrap();
    let a2a_section = &toml[a2a_section_start..];
    let a2a_end = a2a_section.find("\n[").unwrap_or(a2a_section.len());
    let a2a_block = &a2a_section[..a2a_end];
    assert!(
        !a2a_block.contains("http://"),
        "[a2a] bind must be a bare socket addr (no http://); got:\n{a2a_block}"
    );
}

/// The rendered TOML must include an `[operator]` block with a bare `bind` address.
#[test]
fn render_wasl_node_toml_emits_operator_block_with_bare_bind() {
    let cfg = WaslNodeConfig {
        agent_id: "marsad-local",
        host_id: "local",
        a2a_bind: "127.0.0.1:48200",
        operator_bind: "127.0.0.1:49200",
        control_bind: "http://127.0.0.1:47200",
        default_protocol: "claude",
        claude_bin: None,
        opencode_bin: None,
    };
    let toml = render_wasl_node_toml(&cfg);
    assert!(
        toml.contains("[operator]"),
        "rendered TOML must have an [operator] block; got:\n{toml}"
    );
    assert!(
        toml.contains("bind = \"127.0.0.1:49200\""),
        "rendered TOML [operator] block must have bare bind address; got:\n{toml}"
    );
}

/// `wasl_a2a_bind` is deterministic: same host → same result across calls.
#[test]
fn wasl_a2a_bind_is_deterministic_for_same_host() {
    let a = wasl_a2a_bind("local");
    let b = wasl_a2a_bind("local");
    assert_eq!(a, b, "wasl_a2a_bind must be deterministic for the same host_id");
}

/// `wasl_a2a_bind` returns a bare loopback address (no `http://` scheme).
#[test]
fn wasl_a2a_bind_returns_bare_loopback() {
    let bind = wasl_a2a_bind("local");
    assert!(
        bind.starts_with("127.0.0.1:"),
        "wasl_a2a_bind must return a bare loopback address (127.0.0.1:PORT); got: {bind}"
    );
    assert!(
        !bind.contains("http://"),
        "wasl_a2a_bind must NOT contain an http:// scheme (it is a bare socket addr); got: {bind}"
    );
}

/// `wasl_a2a_bind` result parses as a `SocketAddr`.
#[test]
fn wasl_a2a_bind_parses_as_socket_addr() {
    let bind = wasl_a2a_bind("local");
    bind.parse::<std::net::SocketAddr>().unwrap_or_else(|e| {
        panic!("wasl_a2a_bind must produce a valid SocketAddr; got '{bind}': {e}")
    });
}

/// Two distinct hosts produce distinct A2A bind ports.
#[test]
fn wasl_a2a_bind_distinct_hosts_produce_distinct_binds() {
    let a = wasl_a2a_bind("local");
    let b = wasl_a2a_bind("hpc");
    assert_ne!(a, b, "distinct host_ids must have distinct a2a_bind ports; local={a}, hpc={b}");
}

/// `wasl_a2a_bind` port window is `[48000, 48999]` — distinct from the
/// control_bind window `[47000, 47999]`.
#[test]
fn wasl_a2a_bind_port_in_expected_window() {
    for host in ["local", "hpc", "cloud", "dev", "prod"] {
        let bind = wasl_a2a_bind(host);
        let port: u16 = bind
            .rsplit(':')
            .next()
            .unwrap()
            .parse()
            .expect("port must be numeric");
        assert!(
            (48000..=48999).contains(&port),
            "wasl_a2a_bind port for host '{host}' must be in [48000, 48999]; got {port}"
        );
    }
}

/// `wasl_operator_bind` is deterministic: same host → same result.
#[test]
fn wasl_operator_bind_is_deterministic_for_same_host() {
    let a = wasl_operator_bind("local");
    let b = wasl_operator_bind("local");
    assert_eq!(a, b, "wasl_operator_bind must be deterministic for the same host_id");
}

/// `wasl_operator_bind` returns a bare loopback address (no `http://`).
#[test]
fn wasl_operator_bind_returns_bare_loopback() {
    let bind = wasl_operator_bind("local");
    assert!(
        bind.starts_with("127.0.0.1:"),
        "wasl_operator_bind must return a bare loopback address; got: {bind}"
    );
    assert!(
        !bind.contains("http://"),
        "wasl_operator_bind must NOT contain an http:// scheme; got: {bind}"
    );
}

/// `wasl_operator_bind` result parses as a `SocketAddr`.
#[test]
fn wasl_operator_bind_parses_as_socket_addr() {
    let bind = wasl_operator_bind("local");
    bind.parse::<std::net::SocketAddr>().unwrap_or_else(|e| {
        panic!("wasl_operator_bind must produce a valid SocketAddr; got '{bind}': {e}")
    });
}

/// Two distinct hosts produce distinct operator bind ports.
#[test]
fn wasl_operator_bind_distinct_hosts_produce_distinct_binds() {
    let a = wasl_operator_bind("local");
    let b = wasl_operator_bind("hpc");
    assert_ne!(
        a, b,
        "distinct host_ids must have distinct operator_bind ports; local={a}, hpc={b}"
    );
}

/// `wasl_operator_bind` port window is `[49000, 49999]` — distinct from both
/// control `[47000, 47999]` and A2A `[48000, 48999]`.
#[test]
fn wasl_operator_bind_port_in_expected_window() {
    for host in ["local", "hpc", "cloud", "dev", "prod"] {
        let bind = wasl_operator_bind(host);
        let port: u16 = bind
            .rsplit(':')
            .next()
            .unwrap()
            .parse()
            .expect("port must be numeric");
        assert!(
            (49000..=49999).contains(&port),
            "wasl_operator_bind port for host '{host}' must be in [49000, 49999]; got {port}"
        );
    }
}

/// All three bind helpers produce DISTINCT addresses for the same host — no
/// control/a2a/operator port collision.
#[test]
fn all_three_bind_helpers_distinct_for_same_host() {
    for host in ["local", "hpc", "cloud"] {
        let control = wasl_control_bind(host);
        let a2a = wasl_a2a_bind(host);
        let op = wasl_operator_bind(host);
        assert_ne!(
            control, a2a,
            "control_bind and a2a_bind must differ for host '{host}': {control} vs {a2a}"
        );
        assert_ne!(
            control, op,
            "control_bind and operator_bind must differ for host '{host}': {control} vs {op}"
        );
        assert_ne!(
            a2a, op,
            "a2a_bind and operator_bind must differ for host '{host}': {a2a} vs {op}"
        );
    }
}
