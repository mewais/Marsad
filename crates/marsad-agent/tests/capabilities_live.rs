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
    CapabilityProbe, HostProbeCtx, RealClaudeProbe, RealCodexProbe,
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
