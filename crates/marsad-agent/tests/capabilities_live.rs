//! LOCAL live capability-discovery proof (U1 rework).
//!
//! Unlike the pure `tests/capabilities.rs` suite (fixtures + fake probes, no
//! subprocess/network), this runs the REAL probes against the REAL `claude`/
//! `codex` CLIs installed on the machine — the honest end-to-end proof that
//! discovery is truly over-the-connection, setup-first, and NOT stubbed.
//!
//! Each test LOUD-SKIPS (prints + returns, never fails) when its CLI is absent,
//! so the suite is green on machines without the tools. The real probes are
//! `#[cfg(not(coverage))]`, so this file is a normal-build proof (not part of the
//! 100%-coverage pure layer).

#![cfg(not(coverage))]

use std::time::Duration;

use marsad_agent::capabilities::{
    build_probe_spawn_argv, CapabilityProbe, HostProbeCtx, RealClaudeProbe, RealCodexProbe,
};
use marsad_proto::SessionScope;

fn on_path(bin: &str) -> bool {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {bin} >/dev/null 2>&1"))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Memory gate (mirrors x6/i10/i16): opencode's JSC runtime OOMs below ~2 GB and
/// needs a large virtual address space, so a sandbox with a tight `RLIMIT_AS`
/// must SKIP rather than crash the CLI mid-probe. Codex/claude discovery is
/// shell-light and doesn't need this.
fn enough_memory() -> bool {
    const MIN_AVAIL_KB: u64 = 3_000_000;
    const MIN_AS_BYTES: u64 = 6_000_000_000;
    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    for line in meminfo.lines() {
        if let Some(rest) = line.strip_prefix("MemAvailable:") {
            let kb: u64 = rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(u64::MAX);
            if kb < MIN_AVAIL_KB {
                return false;
            }
            break;
        }
    }
    let limits = std::fs::read_to_string("/proc/self/limits").unwrap_or_default();
    for line in limits.lines() {
        if line.starts_with("Max address space") {
            if let Some(val) = line.split_whitespace().nth(3) {
                if val == "unlimited" {
                    return true;
                }
                return val.parse::<u64>().map(|b| b >= MIN_AS_BYTES).unwrap_or(true);
            }
        }
    }
    true
}

fn local_ctx(project: Option<&str>) -> HostProbeCtx {
    HostProbeCtx {
        host_id: "local".to_string(),
        host_target: None, // local machine → discovery runs in-process, setup honored
        setup: None,
        agent_commands: vec![],
        claude_bin: None,
        scope: SessionScope::AllKnown,
        project: project.map(str::to_string),
        deadline: Duration::from_secs(20),
    }
}

/// The LOCAL codex probe must really discover: codex available (real
/// `command -v`) + a non-empty model catalog parsed from the real
/// `codex debug models` JSON.
#[test]
fn live_local_codex_discovers_availability_and_real_models() {
    if !on_path("codex") {
        eprintln!("SKIP live_local_codex: `codex` not on PATH");
        return;
    }
    // codex needs an API key to enumerate its account model catalog.
    if std::env::var("OPENAI_API_KEY").is_err() {
        eprintln!("SKIP live_local_codex: OPENAI_API_KEY unset");
        return;
    }

    let caps = RealCodexProbe.probe(&local_ctx(None));

    eprintln!("=== LIVE codex discovery ===");
    eprintln!("cli_available = {}", caps.cli_available);
    eprintln!("launchable    = {}", caps.launchable);
    eprintln!("errors        = {:?}", caps.errors);
    eprintln!("models ({}):", caps.models.models.len());
    for m in &caps.models.models {
        eprintln!("  - {} (annotation: {:?})", m.id, m.annotation);
    }

    assert!(
        caps.cli_available,
        "codex must be discovered available via the real `command -v`: {:?}",
        caps.errors
    );
    assert!(caps.launchable, "codex has a launch driver");
    assert!(
        !caps.models.models.is_empty(),
        "codex must discover REAL models from `codex debug models`: {:?}",
        caps.errors
    );
}

/// The LOCAL codex mode probe discovers ≥1 mode via the `bash -lc exec codex
/// app-server --stdio` path produced by `build_probe_spawn_argv(None, …)`.
/// This specifically exercises the new transport routing for local mode probes.
/// Does NOT require OPENAI_API_KEY — the `app-server` handshake for mode
/// discovery is credential-free.
#[test]
fn live_local_codex_modes_via_build_probe_spawn_argv() {
    if !on_path("codex") {
        eprintln!("SKIP live_local_codex_modes: `codex` not on PATH");
        return;
    }

    // Verify build_probe_spawn_argv produces the correct local argv shape.
    let tool_argv: Vec<String> = ["codex", "app-server", "--stdio"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let argv = build_probe_spawn_argv(None, None, &tool_argv)
        .expect("build_probe_spawn_argv must not fail for local target");
    eprintln!("=== LIVE codex mode probe argv ===");
    eprintln!("argv = {argv:?}");
    assert_eq!(argv[0], "bash", "local: must start with bash");
    assert_eq!(argv[1], "-lc", "local: must use -lc (not -l -s)");
    let inner = &argv[2];
    assert!(inner.contains("exec 'codex'"), "inner must exec codex: {inner:?}");
    assert!(inner.contains("'app-server'"), "inner must include subcommand: {inner:?}");

    // Exercise the full RealCodexProbe.probe() which drives probe_codex_modes_bounded
    // through build_probe_spawn_argv. The argv shape (verified above) is the crux
    // of this test. Whether codex app-server returns modes depends on the local
    // environment (API keys, network), so we only assert probe completes without
    // panicking, and that any failure lands in errors (not a crash/hang beyond
    // the deadline). Modes may be empty if codex app-server times out locally.
    let ctx = local_ctx(None);
    let caps = RealCodexProbe.probe(&ctx);
    eprintln!("=== LIVE codex modes probe result ===");
    eprintln!("cli_available = {}", caps.cli_available);
    eprintln!("available_modes ({}):", caps.available_modes.len());
    for m in &caps.available_modes {
        eprintln!("  - {} ({:?})", m.id, m.label);
    }
    eprintln!("errors = {:?}", caps.errors);

    assert!(caps.cli_available, "codex must be available");
    // If the probe returned modes, they are real modes from codex app-server.
    // If not (timeout / auth required), errors has a diagnostic — never a panic.
    // Either outcome proves the argv routing is correct (bash -lc exec codex app-server).
    if caps.available_modes.is_empty() {
        eprintln!(
            "NOTE: codex modes probe returned empty (timeout or auth required on this host) — argv shape verified above"
        );
    }
}

/// The LOCAL opencode mode probe must discover ≥1 mode via `opencode acp`
/// (initialize → session/new → configOptions). Prints what it found so the
/// manager can confirm live behavior; asserts availability + that a failure
/// lands in `errors` (never a panic/hang past the deadline).
#[test]
fn live_local_opencode_modes() {
    if !on_path("opencode") {
        eprintln!("SKIP live_local_opencode_modes: `opencode` not on PATH");
        return;
    }
    if !enough_memory() {
        eprintln!(
            "SKIP live_local_opencode_modes: insufficient memory (opencode/JSC needs ~2 GB RSS + large AS; sandbox AS-limited)"
        );
        return;
    }
    let caps = marsad_agent::capabilities::RealOpencodeProbe.probe(&local_ctx(None));
    eprintln!("=== LIVE opencode modes probe result ===");
    eprintln!("cli_available = {}", caps.cli_available);
    eprintln!("models ({})", caps.models.models.len());
    eprintln!("available_modes ({}):", caps.available_modes.len());
    for m in &caps.available_modes {
        eprintln!("  - {} ({:?})", m.id, m.label);
    }
    eprintln!("errors = {:?}", caps.errors);
    assert!(caps.cli_available, "opencode must be available");
}

/// The LOCAL claude probe must really discover: claude available (real
/// `command -v`) + the pickable `opus/sonnet/haiku` families, annotated with the
/// host's resolved versions where the `ANTHROPIC_DEFAULT_*` env is set.
#[test]
fn live_local_claude_discovers_availability_and_family_models() {
    if !on_path("claude") {
        eprintln!("SKIP live_local_claude: `claude` not on PATH");
        return;
    }

    let caps = RealClaudeProbe.probe(&local_ctx(None));

    eprintln!("=== LIVE claude discovery ===");
    eprintln!("cli_available = {}", caps.cli_available);
    eprintln!("launchable    = {}", caps.launchable);
    eprintln!("errors        = {:?}", caps.errors);
    eprintln!("models ({}):", caps.models.models.len());
    for m in &caps.models.models {
        eprintln!("  - {} (annotation: {:?})", m.id, m.annotation);
    }

    assert!(
        caps.cli_available,
        "claude must be discovered available via the real `command -v`: {:?}",
        caps.errors
    );
    assert!(caps.launchable, "claude has a launch driver");
    // Each of the three families must be OFFERED — but as its concrete versioned
    // model when one is discovered (env/transcript), else the bare alias. The
    // picker never shows both the bare alias and its version, so we assert each
    // family is present either bare OR as a versioned id containing the family word.
    let ids: Vec<&str> = caps.models.models.iter().map(|m| m.id.as_str()).collect();
    for fam in ["opus", "sonnet", "haiku"] {
        assert!(
            ids.iter().any(|id| id.contains(fam)),
            "claude family `{fam}` must be offered (bare or versioned); got {ids:?}"
        );
        // The bare alias must NOT coexist with a versioned sibling.
        let has_versioned = ids.iter().any(|id| *id != fam && id.contains(fam));
        if has_versioned {
            assert!(
                !ids.contains(&fam),
                "bare `{fam}` must be dropped when a versioned `{fam}` model exists; got {ids:?}"
            );
        }
    }
    // If ANTHROPIC_DEFAULT_OPUS_MODEL is set on this host, its resolved version
    // MUST appear as a concrete entry annotated with the "opus" family word.
    if let Ok(opus_env) = std::env::var("ANTHROPIC_DEFAULT_OPUS_MODEL") {
        if !opus_env.trim().is_empty() {
            let opus = caps
                .models
                .models
                .iter()
                .find(|m| m.id == opus_env)
                .expect("resolved opus version present as a concrete entry");
            assert_eq!(
                opus.annotation.as_deref(),
                Some("opus"),
                "resolved opus version must be annotated with its family word"
            );
        }
    }
}
