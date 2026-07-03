//! wasl_launch_attach.rs — X4 ("launch-time Wasl attach") for `marsad-agent`:
//! `claude_argv`/`build_spawn_cmd` MCP-config injection, and the
//! adopt-or-spawn helper for the host's `wasl-mcpd`.
//!
//! Authoritative spec: NONE FOUND. `docs/specs/wasl-connector.md`'s only
//! "X4" references (lines 549, 651-660, 808, 829-833) name a *different*
//! already-planned feature — the GUI Wasl-approval modal, which the doc says
//! explicitly reuses `adopt_or_spawn_agent` UNCHANGED and needs "no new
//! process plumbing". There is no `wasl-mcpd`, `LaunchSpec.wasl`, or
//! `--mcp-config` concept anywhere in this repo (grepped exhaustively) and
//! no `PLAN.md` to consult. This file works from the API sketched directly
//! in the X4 task brief instead. See the test-agent's report for the full
//! list of gaps/conflicts this surfaces — most importantly: adding a bare
//! `wasl: bool` field to `LaunchSpec`/`IpcMsg::LaunchSession` (as literally
//! specified) breaks ~90 existing FROZEN struct-literal construction sites
//! across marsad-agent/core/proto's `tests/` (Rust struct literals must name
//! every field; `#[serde(default)]` only helps *deserialization*, not
//! construction). That is a real, load-bearing conflict for the owner to
//! resolve, not something this test file can route around.
//!
//! ASSUMED API (does not exist yet — expected to fail to compile until the
//! code-agent lands it; NOTE(test-agent) markers per this crate's own house
//! style, see tests/happy.rs's header):
//!
//!   - `marsad_proto::LaunchSpec` gains `pub wasl: bool` (X4a).
//!   - `claude_argv(&spec)` (driver.rs), when `spec.wasl == true`, appends
//!     `--mcp-config <value>` to the argv it already builds, where `<value>`
//!     references the host's `wasl-mcpd` MCP-server entry. The exact
//!     `<value>` shape (inline JSON vs. a config-file path; the MCP
//!     transport/command wasl-mcpd is invoked with) is UNSPECIFIED anywhere
//!     — this file asserts only the flag's presence/singularity and that its
//!     value is non-empty and names "wasl" (case-insensitively), not an
//!     exact JSON/path shape. Flagged for the owner.
//!   - `spec.wasl == false` (or the field simply absent from older specs)
//!     must produce BYTE-IDENTICAL argv to today's `claude_argv` — the
//!     load-bearing optionality proof (mirrors `docs/specs/wasl-connector.md`
//!     §6's "inert unless configured" doctrine for the X1/X2 connector).
//!   - `Agent` gains `pub fn adopt_or_spawn_wasl_mcpd(&mut self, spec:
//!     &marsad_proto::LaunchSpec) -> bool` (X4b), mirroring
//!     `apps/desktop/src-tauri/src/lib.rs`'s `adopt_or_spawn_agent` shape
//!     (try-connect-else-spawn) but expressed over the SAME `Container`
//!     seam `Agent` already uses for claude sessions (`self.container`,
//!     asserted here via the existing `container_calls()` accessor — no new
//!     fake/trait/constructor-arg needed, so `Agent::with_fakes`'s existing
//!     4-arg signature — used by dozens of FROZEN call sites across this
//!     crate's `tests/` — stays untouched). Container name convention
//!     assumed: `format!("wasl-mcpd-{}", spec.host_id)`, exactly like
//!     `format!("marsad-{session_id}")` for claude sessions. Returns `true`
//!     iff a wasl-mcpd is now considered attached (adopted or freshly
//!     spawned); `false` (and zero container calls) when `spec.wasl` is
//!     false. "Marsad launches it, never speaks through it" is proven two
//!     ways: structurally (the reused `Container` trait exposes only
//!     exists/attach/spawn/kill — no query/consult method exists to call)
//!     and behaviorally (every logged call after adopt-or-spawn is one of
//!     those four verbs, asserted below).
//!
//! This file WILL fail to compile until that surface exists. That is the
//! expected tests-first signal for X4.

use marsad_agent::{claude_argv, Agent, AgentConfig, ListenSpec};
use marsad_lifecycle::FakeContainer;
use marsad_notify::CaptureNotifier;
use marsad_proto::LaunchSpec;
use marsad_store::MemoryBackend;

// ── helpers ────────────────────────────────────────────────────────────────────

fn stdio_cfg() -> AgentConfig {
    AgentConfig {
        listen: ListenSpec::Stdio,
        data_root: "/tmp/marsad-agent-test-wasl-launch".to_string(),
    }
}

fn strs(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// Mirrors `tests/happy.rs`'s `basic_launch_spec()` exactly, plus the new
/// `wasl` field (defaulted to `false` here — callers flip it explicitly).
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
// claude_argv — the critical optionality proof: wasl=false/default is
// BYTE-IDENTICAL to today's argv, for every posture/mode combination
// tests/happy.rs already pins.
// ---------------------------------------------------------------------------

#[test]
fn claude_argv_wasl_false_byte_identical_to_today_ask_baseline() {
    let mut spec = basic_launch_spec();
    spec.wasl = false;
    let argv = claude_argv(&spec);
    assert_eq!(
        argv,
        strs(&[
            "claude",
            "--print",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--model",
            "claude-opus-4-5",
            "--permission-mode",
            "default",
            "--permission-prompt-tool",
            "stdio",
        ]),
        "wasl:false must produce EXACTLY today's argv, no more no less"
    );
}

#[test]
fn claude_argv_wasl_false_byte_identical_to_today_auto_approve_safe_baseline() {
    let mut spec = basic_launch_spec();
    spec.wasl = false;
    spec.posture = marsad_proto::Posture::AutoApproveSafe;
    let argv = claude_argv(&spec);
    assert_eq!(
        argv,
        strs(&[
            "claude",
            "--print",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--model",
            "claude-opus-4-5",
            "--permission-mode",
            "acceptEdits",
            "--permission-prompt-tool",
            "stdio",
        ])
    );
}

#[test]
fn claude_argv_wasl_false_byte_identical_to_today_full_autonomy_baseline() {
    let mut spec = basic_launch_spec();
    spec.wasl = false;
    spec.posture = marsad_proto::Posture::FullAutonomy;
    let argv = claude_argv(&spec);
    assert_eq!(
        argv,
        strs(&[
            "claude",
            "--print",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--model",
            "claude-opus-4-5",
            "--permission-mode",
            "bypassPermissions",
            "--permission-prompt-tool",
            "stdio",
        ])
    );
}

#[test]
fn claude_argv_wasl_false_byte_identical_to_today_continue_baseline() {
    let mut spec = basic_launch_spec();
    spec.wasl = false;
    spec.mode = marsad_proto::LaunchMode::Continue;
    let argv = claude_argv(&spec);
    assert_eq!(
        argv,
        strs(&[
            "claude",
            "--print",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--model",
            "claude-opus-4-5",
            "--permission-mode",
            "default",
            "--permission-prompt-tool",
            "stdio",
            "--continue",
        ])
    );
}

#[test]
fn claude_argv_wasl_false_byte_identical_to_today_resume_baseline() {
    let mut spec = basic_launch_spec();
    spec.wasl = false;
    spec.mode = marsad_proto::LaunchMode::Resume("sess-uuid-123".to_string());
    let argv = claude_argv(&spec);
    assert_eq!(
        argv,
        strs(&[
            "claude",
            "--print",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--model",
            "claude-opus-4-5",
            "--permission-mode",
            "default",
            "--permission-prompt-tool",
            "stdio",
            "--resume",
            "sess-uuid-123",
        ])
    );
}

#[test]
fn claude_argv_wasl_false_byte_identical_to_today_disallowed_tools_baseline() {
    let mut spec = basic_launch_spec();
    spec.wasl = false;
    spec.disallowed_tools = vec!["Bash".to_string(), "Edit".to_string()];
    let argv = claude_argv(&spec);
    assert_eq!(
        argv,
        strs(&[
            "claude",
            "--print",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--model",
            "claude-opus-4-5",
            "--permission-mode",
            "default",
            "--permission-prompt-tool",
            "stdio",
            "--disallowedTools",
            "Bash,Edit",
        ])
    );
}

#[test]
fn claude_argv_wasl_false_byte_identical_to_today_claude_bin_baseline() {
    let mut spec = basic_launch_spec();
    spec.wasl = false;
    spec.claude_bin = Some("genie".to_string());
    let argv = claude_argv(&spec);
    assert_eq!(
        argv,
        strs(&[
            "genie",
            "--print",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--model",
            "claude-opus-4-5",
            "--permission-mode",
            "default",
            "--permission-prompt-tool",
            "stdio",
        ])
    );
}

// ---------------------------------------------------------------------------
// claude_argv — wasl=true injects --mcp-config
// ---------------------------------------------------------------------------

#[test]
fn claude_argv_wasl_true_injects_mcp_config_flag_referencing_wasl_mcpd() {
    let mut spec = basic_launch_spec();
    spec.wasl = true;
    let argv = claude_argv(&spec);

    let idx = argv
        .iter()
        .position(|a| a == "--mcp-config")
        .unwrap_or_else(|| panic!("wasl:true must add a --mcp-config flag; argv: {argv:?}"));
    let value = argv
        .get(idx + 1)
        .unwrap_or_else(|| panic!("--mcp-config must be followed by a value; argv: {argv:?}"));
    assert!(!value.is_empty(), "--mcp-config value must not be empty");
    assert!(
        value.to_lowercase().contains("wasl"),
        "--mcp-config value must reference the host's wasl-mcpd MCP-server entry; got: {value}"
    );

    assert_eq!(
        argv.iter().filter(|a| *a == "--mcp-config").count(),
        1,
        "wasl attach must add the flag exactly once; argv: {argv:?}"
    );
}

/// wasl=true must be purely ADDITIVE — every flag `claude_argv` emits today
/// must still be present (in some order) alongside `--mcp-config`.
#[test]
fn claude_argv_wasl_true_does_not_remove_or_replace_existing_flags() {
    let mut spec = basic_launch_spec();
    spec.wasl = true;
    let argv = claude_argv(&spec);

    assert_eq!(argv[0], "claude");
    assert!(argv.iter().any(|a| a == "--print"));
    assert!(argv.iter().any(|a| a == "--input-format"));
    assert!(argv.iter().any(|a| a == "stream-json"));
    assert!(argv.iter().any(|a| a == "--verbose"));

    let model_idx = argv
        .iter()
        .position(|a| a == "--model")
        .expect("--model must still be present when wasl:true");
    assert_eq!(
        argv.get(model_idx + 1).map(String::as_str),
        Some("claude-opus-4-5")
    );

    let perm_idx = argv
        .iter()
        .position(|a| a == "--permission-mode")
        .expect("--permission-mode must still be present when wasl:true");
    assert_eq!(argv.get(perm_idx + 1).map(String::as_str), Some("default"));

    let prompt_idx = argv
        .iter()
        .position(|a| a == "--permission-prompt-tool")
        .expect("--permission-prompt-tool must still be present when wasl:true");
    assert_eq!(argv.get(prompt_idx + 1).map(String::as_str), Some("stdio"));
}

/// wasl=true combined with a non-empty `disallowed_tools` must emit BOTH
/// flags without interference.
#[test]
fn claude_argv_wasl_true_and_disallowed_tools_coexist() {
    let mut spec = basic_launch_spec();
    spec.wasl = true;
    spec.disallowed_tools = vec!["Bash".to_string(), "Edit".to_string()];
    let argv = claude_argv(&spec);

    assert!(
        argv.iter().any(|a| a == "--mcp-config"),
        "--mcp-config must still appear; argv: {argv:?}"
    );
    let dt_idx = argv
        .iter()
        .position(|a| a == "--disallowedTools")
        .expect("--disallowedTools must still appear; argv: {argv:?}");
    assert_eq!(argv.get(dt_idx + 1).map(String::as_str), Some("Bash,Edit"));
}

#[test]
fn claude_argv_wasl_true_is_deterministic() {
    let mut spec = basic_launch_spec();
    spec.wasl = true;
    let a = claude_argv(&spec);
    let b = claude_argv(&spec);
    assert_eq!(
        a, b,
        "claude_argv must be deterministic even with wasl:true"
    );
}

// ---------------------------------------------------------------------------
// build_spawn_cmd (private — exercised indirectly through Agent::launch +
// FakeContainer's captured "spawn:<name> <cmd>" call log, since it is not a
// pub fn reachable from a black-box tests/ file).
// ---------------------------------------------------------------------------

#[test]
fn launch_with_wasl_true_spawn_command_contains_mcp_config_flag() {
    let mut agent = agent_with_container(FakeContainer::new());
    let mut spec = basic_launch_spec();
    spec.wasl = true;

    let session_id = agent.launch(spec).expect("launch must succeed");

    let calls = agent.container_calls();
    let spawn_call = calls
        .iter()
        .find(|c| c.starts_with(&format!("spawn:marsad-{session_id} ")))
        .unwrap_or_else(|| {
            panic!("expected a spawn call for marsad-{session_id}; calls: {calls:?}")
        });
    assert!(
        spawn_call.contains("--mcp-config"),
        "a wasl:true launch's spawned claude command must contain --mcp-config; got: {spawn_call}"
    );
}

#[test]
fn launch_with_wasl_false_spawn_command_has_no_mcp_config_flag() {
    let mut agent = agent_with_container(FakeContainer::new());
    let mut spec = basic_launch_spec();
    spec.wasl = false;

    let session_id = agent.launch(spec).expect("launch must succeed");

    let calls = agent.container_calls();
    let spawn_call = calls
        .iter()
        .find(|c| c.starts_with(&format!("spawn:marsad-{session_id} ")))
        .unwrap_or_else(|| {
            panic!("expected a spawn call for marsad-{session_id}; calls: {calls:?}")
        });
    assert!(
        !spawn_call.contains("--mcp-config"),
        "a wasl:false launch's spawned claude command must NOT contain --mcp-config; got: {spawn_call}"
    );
}

// ---------------------------------------------------------------------------
// Agent::adopt_or_spawn_wasl_mcpd — X4b
// ---------------------------------------------------------------------------

#[test]
fn adopt_or_spawn_wasl_mcpd_is_noop_when_wasl_false() {
    let mut agent = agent_with_container(FakeContainer::new());
    let mut spec = basic_launch_spec();
    spec.wasl = false;

    let attached = agent.adopt_or_spawn_wasl_mcpd(&spec);

    assert!(
        !attached,
        "wasl:false must not report an attached wasl-mcpd"
    );
    assert!(
        agent.container_calls().is_empty(),
        "wasl:false must perform ZERO container calls (no probe/spawn/attach); calls: {:?}",
        agent.container_calls()
    );
}

#[test]
fn adopt_or_spawn_wasl_mcpd_adopts_a_live_daemon_without_spawning() {
    let mut container = FakeContainer::new();
    container.set_exists("wasl-mcpd-local", true);
    let mut agent = agent_with_container(container);
    let mut spec = basic_launch_spec();
    spec.wasl = true; // host_id == "local" (basic_launch_spec default)

    let attached = agent.adopt_or_spawn_wasl_mcpd(&spec);

    assert!(attached, "a live wasl-mcpd must be reported as attached");
    let calls = agent.container_calls();
    assert!(
        calls.iter().any(|c| c == "exists:wasl-mcpd-local"),
        "must probe for a live wasl-mcpd; calls: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|c| c.starts_with("attach:wasl-mcpd-local")),
        "a live wasl-mcpd must be ADOPTED (attach), not respawned; calls: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c.starts_with("spawn:wasl-mcpd-local")),
        "a live wasl-mcpd must NEVER be spawned — no-duplication; calls: {calls:?}"
    );
}

#[test]
fn adopt_or_spawn_wasl_mcpd_spawns_when_absent() {
    // Default FakeContainer: exists() returns false for any unconfigured name.
    let mut agent = agent_with_container(FakeContainer::new());
    let mut spec = basic_launch_spec();
    spec.wasl = true;

    let attached = agent.adopt_or_spawn_wasl_mcpd(&spec);

    assert!(
        attached,
        "spawning a wasl-mcpd must report attached==true on success"
    );
    let calls = agent.container_calls();
    assert!(
        calls.iter().any(|c| c == "exists:wasl-mcpd-local"),
        "must probe before deciding to spawn; calls: {calls:?}"
    );
    assert!(
        calls.iter().any(|c| c.starts_with("spawn:wasl-mcpd-local")),
        "no live daemon must trigger a spawn; calls: {calls:?}"
    );
    assert!(
        !calls
            .iter()
            .any(|c| c.starts_with("attach:wasl-mcpd-local")),
        "must not attach when nothing was live; calls: {calls:?}"
    );
}

#[test]
fn adopt_or_spawn_wasl_mcpd_scopes_container_name_by_host_id() {
    let mut container = FakeContainer::new();
    container.set_exists("wasl-mcpd-hpc", true);
    let mut agent = agent_with_container(container);
    let mut spec = basic_launch_spec();
    spec.wasl = true;
    spec.host_id = "hpc".to_string();

    let attached = agent.adopt_or_spawn_wasl_mcpd(&spec);

    assert!(attached);
    let calls = agent.container_calls();
    assert!(
        calls.iter().any(|c| c.starts_with("exists:wasl-mcpd-hpc")),
        "the probe must be scoped by the launch spec's host_id; calls: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c.contains("wasl-mcpd-local")),
        "must not touch a different host's wasl-mcpd; calls: {calls:?}"
    );
}

/// "Marsad LAUNCHES it, never speaks through it" — no A2A/consult traffic.
/// Structurally enforced by reusing the `Container` trait (exists/attach/
/// spawn/kill ONLY — it has no query/request/consult method to call);
/// behaviorally pinned here by asserting every logged call is one of those
/// four lifecycle verbs, even across a repeated adopt-or-spawn (e.g. a
/// resume/reconnect) that should not accumulate extra traffic either.
#[test]
fn adopt_or_spawn_wasl_mcpd_never_speaks_through_it_only_lifecycle_calls() {
    let mut agent = agent_with_container(FakeContainer::new());
    let mut spec = basic_launch_spec();
    spec.wasl = true;

    agent.adopt_or_spawn_wasl_mcpd(&spec);
    agent.adopt_or_spawn_wasl_mcpd(&spec);

    let calls = agent.container_calls();
    assert!(
        !calls.is_empty(),
        "sanity: the fixture must have produced some calls"
    );
    for call in calls {
        let verb = call.split(':').next().unwrap_or("");
        assert!(
            ["exists", "attach", "spawn", "kill"].contains(&verb),
            "adopt_or_spawn_wasl_mcpd must never log a non-lifecycle verb \
             (no A2A/consult call from Marsad to wasl-mcpd); got {call:?} in {calls:?}"
        );
    }
}

#[test]
fn adopt_or_spawn_wasl_mcpd_does_not_touch_session_containers() {
    // A launched claude session and a wasl-mcpd attach are independent
    // container names — adopt_or_spawn_wasl_mcpd must never spawn/attach the
    // session's own "marsad-<id>" container, and launch() must never spawn
    // wasl-mcpd on its own (only the dedicated helper may).
    let mut agent = agent_with_container(FakeContainer::new());
    let mut spec = basic_launch_spec();
    spec.wasl = true;

    let session_id = agent.launch(spec.clone()).expect("launch must succeed");
    agent.adopt_or_spawn_wasl_mcpd(&spec);

    let calls = agent.container_calls();
    let session_name = format!("marsad-{session_id}");
    assert!(
        !calls
            .iter()
            .any(|c| c.contains("wasl-mcpd") && c.contains(&session_name)),
        "the session container and the wasl-mcpd container must never be conflated; calls: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|c| c.starts_with(&format!("spawn:{session_name} "))),
        "the claude session itself must still be spawned exactly as before; calls: {calls:?}"
    );
}
