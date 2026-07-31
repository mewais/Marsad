//! Production subprocess/$HOME-rooted CapabilityProbe implementations (never unit-tested).
//!
//! Extracted from `capabilities.rs` verbatim; `use super::*` inherits the
//! parent module's imports and cross-section helpers.

use super::*;

// ===========================================================================
// Part 5 — real, subprocess/$HOME-rooted probe implementations. Production
// only: never unit-tested (no live `codex`/`agy`/`claude` on the test
// machine, per the house guardrail — the `run` fixture-driven live suites
// prove these against the real CLIs). Compiled out under coverage so the
// crate's 100% bar stays on the pure/logic layer (§3.1: "Real subprocess/
// FS-rooted-at-$HOME paths follow the established RealGit/RealTranscript
// pattern").
// ===========================================================================

/// Run a discovery SCRIPT over the host connection, setup-first, and return its
/// raw stdout — or `Err` on an unreachable host / a timeout / a spawn failure.
///
/// - Remote host (`Some(target)`): `run_remote_script_bounded` pipes the script
///   to `bash -l -s` over ssh/wsl/plink (login shell so `$HOME`/PATH/setup env
///   resolve), bounded by `deadline`. `None` back from it = host unreachable /
///   the bounded probe timed out.
/// - Local host (`None`): the SAME script is fed to a LOCAL `bash -l -s` (so
///   `setup` runs identically), bounded by the same deadline. This is what makes
///   local discovery real (setup honored) rather than a bare `which`.
#[cfg(not(coverage))]
fn run_discovery(
    host_target: Option<&str>,
    script: &str,
    deadline: Duration,
) -> Result<String, String> {
    match host_target {
        Some(target) => crate::remote::run_remote_script_bounded(target, script, deadline)
            .ok_or_else(|| format!("host '{target}' unreachable or probe timed out")),
        None => run_local_script(script, deadline),
    }
}

/// Run a shell `script` on the LOCAL machine via `bash -l -s` (login shell,
/// script on stdin), bounded by `deadline`. Mirrors `run_remote_script_bounded`
/// applied to a local child so `setup` + section markers behave identically to
/// the remote path.
#[cfg(not(coverage))]
fn run_local_script(script: &str, deadline: Duration) -> Result<String, String> {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    let mut child = Command::new("bash")
        .arg("-l")
        .arg("-s")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("bash spawn failed: {e}"))?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "bash: no stdin".to_string())?;
        stdin
            .write_all(script.as_bytes())
            .map_err(|e| format!("bash: write failed: {e}"))?;
    } // stdin dropped → EOF so `bash -s` runs and exits.

    // Drain stdout on a dedicated thread so a LARGE output (e.g. `codex debug
    // models` is ~200 KB JSON) can never fill the OS pipe buffer and DEADLOCK
    // the child — the child would block on write, never exit, and the poll below
    // would falsely time out. Reading concurrently keeps the pipe flowing.
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        if let Some(mut so) = stdout {
            let _ = so.read_to_string(&mut out);
        }
        out
    });

    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return Ok(reader.join().unwrap_or_default());
            }
            Ok(None) => {
                if start.elapsed() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = reader.join();
                    return Err("local discovery: timed out".to_string());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("bash: wait failed: {e}")),
        }
    }
}

/// Real codex probe: setup-first `command -v` + `<cmd> debug models` +
/// `~/.codex/sessions` listing, run over the host connection (remote) or locally
/// (setup honored). No `which`; no local-only optimism.
#[cfg(not(coverage))]
pub struct RealCodexProbe;

#[cfg(not(coverage))]
impl CapabilityProbe for RealCodexProbe {
    fn driver(&self) -> &'static str {
        "codex"
    }

    fn launchable(&self) -> bool {
        true
    }

    fn probe(&self, ctx: &HostProbeCtx) -> Capabilities {
        let command = ctx.resolve_command("codex");
        let script = build_codex_discovery_script(&command, ctx.setup.as_deref());
        let mut errors = Vec::new();
        let (cli_available, models, sessions) =
            match run_discovery(ctx.host_target.as_deref(), &script, ctx.deadline) {
                Ok(out) => {
                    let secs = parse_discovery_sections(&out);
                    (
                        parse_avail(secs.get("avail")),
                        parse_codex_debug_models(secs.get("models")),
                        parse_remote_codex_session_list(secs.get("sessions")),
                    )
                }
                Err(e) => {
                    errors.push(format!("codex discovery: {e}"));
                    (false, vec![], vec![])
                }
            };
        Capabilities {
            driver: "codex".to_string(),
            host_id: ctx.host_id.clone(),
            cli_available,
            launchable: self.launchable(),
            models: ModelCatalog {
                models,
                source: ModelListSource::Listed,
                free_text: true,
            },
            available_modes: vec![],
            session_scope: Some(SessionScope::AllKnown),
            sessions,
            fetched_at_ms: now_ms(),
            errors,
        }
    }
}

/// Real agy/gemini probe: setup-first `command -v` + `<cmd> models`, run over
/// the host connection (remote) or locally (setup honored).
#[cfg(not(coverage))]
pub struct RealAgyProbe;

#[cfg(not(coverage))]
impl CapabilityProbe for RealAgyProbe {
    fn driver(&self) -> &'static str {
        "gemini"
    }

    fn probe(&self, ctx: &HostProbeCtx) -> Capabilities {
        let command = ctx.resolve_command("gemini");
        let script = build_agy_discovery_script(&command, ctx.setup.as_deref());
        let mut errors = Vec::new();
        let (cli_available, models) =
            match run_discovery(ctx.host_target.as_deref(), &script, ctx.deadline) {
                Ok(out) => {
                    let secs = parse_discovery_sections(&out);
                    (
                        parse_avail(secs.get("avail")),
                        parse_agy_models(secs.get("models")),
                    )
                }
                Err(e) => {
                    errors.push(format!("gemini discovery: {e}"));
                    (false, vec![])
                }
            };
        Capabilities {
            driver: "gemini".to_string(),
            host_id: ctx.host_id.clone(),
            cli_available,
            // gemini (agy) has NO launch driver in Marsad — the launch UI greys
            // it with the "not yet launchable" note. Uses the trait default.
            launchable: self.launchable(),
            models: ModelCatalog {
                models,
                source: ModelListSource::Listed,
                free_text: false,
            },
            available_modes: vec![],
            session_scope: Some(SessionScope::AllKnown),
            sessions: vec![],
            fetched_at_ms: now_ms(),
            errors,
        }
    }
}

/// Real claude probe: setup-first `command -v` (real availability, local AND
/// remote) + the host's `ANTHROPIC_DEFAULT_{OPUS,SONNET,HAIKU}_MODEL` env
/// (claude has no "list models" command — the env is the only truthful per-host
/// source), offered as the pickable `opus/sonnet/haiku` families annotated with
/// the host's resolved versions where set (`free_text: true`). Sessions are
/// enumerated per requested project over the connection. The provider re-merges
/// the passively-observed model superset afterward (§3.4).
#[cfg(not(coverage))]
pub struct RealClaudeProbe;

#[cfg(not(coverage))]
impl CapabilityProbe for RealClaudeProbe {
    fn driver(&self) -> &'static str {
        "claude"
    }

    fn launchable(&self) -> bool {
        true
    }

    fn probe(&self, ctx: &HostProbeCtx) -> Capabilities {
        let command = ctx.resolve_command("claude");
        let script =
            build_claude_discovery_script(&command, ctx.setup.as_deref(), ctx.project.as_deref());
        let mut errors = Vec::new();
        let (cli_available, models, sessions, version) =
            match run_discovery(ctx.host_target.as_deref(), &script, ctx.deadline) {
                Ok(out) => {
                    let secs = parse_discovery_sections(&out);
                    // DIAG: dump the raw sessions section (carries the MARSAD_DIAG
                    // line with the resolved base/cwd/enc/dir/count) so an empty
                    // session list on a host like etx can be diagnosed from agent.log
                    // rather than guessed. Only when a project was in scope.
                    if ctx.project.is_some() {
                        eprintln!(
                            "marsad-agent: claude sessions probe for {:?} project={:?} raw:\n{}",
                            ctx.host_target,
                            ctx.project,
                            secs.get("sessions")
                        );
                    }
                    // Family catalog annotated with the host's REAL resolved
                    // versions from the env section (families are only offered
                    // when claude actually resolved on the host).
                    let resolved = parse_claude_env_models(secs.get("env"));
                    // Extra models from env overrides, config files, and transcripts.
                    let extra_ids = parse_claude_extra_models(secs.get("extra_models"));
                    let extra_observed: Vec<ModelInfo> = extra_ids
                        .into_iter()
                        .map(|id| ModelInfo { id, annotation: None })
                        .collect();
                    let is_available = parse_avail(secs.get("avail"));
                    let models = if is_available {
                        let resolved_ref: HashMap<&str, String> =
                            resolved.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
                        build_claude_model_catalog(&resolved_ref, &extra_observed)
                    } else {
                        // claude didn't resolve on the host: no families offered
                        // (the passively-observed superset is still merged by the
                        // provider afterward, so nothing ever run is lost).
                        vec![]
                    };
                    let ver = parse_claude_version(secs.get("version"));
                    (
                        is_available,
                        models,
                        parse_remote_claude_session_list(secs.get("sessions")),
                        ver,
                    )
                }
                Err(e) => {
                    errors.push(format!("claude discovery: {e}"));
                    (false, vec![], vec![], String::new())
                }
            };
        // Stamp the REQUESTED project onto every discovered session. The remote
        // lister can't recover the original path from claude's lossy on-disk
        // encoding, so the caller (which knows the exact path it asked for) is the
        // source of truth. This makes the frontend's per-path filter meaningful
        // and lets Core merge/replace sessions by project without cross-path
        // contamination.
        let sessions: Vec<DiscoveredSession> = match ctx.project.as_deref() {
            Some(p) if !p.trim().is_empty() => sessions
                .into_iter()
                .map(|mut s| {
                    s.project = Some(p.to_string());
                    s
                })
                .collect(),
            _ => sessions,
        };
        // `session_scope` marks what THIS update authoritatively covers so Core
        // knows how to merge it: a path-scoped probe replaces exactly project p's
        // rows; a capability probe (AllKnown) carries no session authority and
        // must not wipe a prior scoped result.
        let session_scope = match ctx.project.as_deref() {
            Some(p) if !p.trim().is_empty() => SessionScope::Projects(vec![p.to_string()]),
            _ => SessionScope::AllKnown,
        };
        // Derive available modes from the probed claude version. An empty version
        // (probe error / claude not installed) leaves available_modes empty — the
        // UI shows no mode picker for this host, which is honest.
        let available_modes = marsad_events::claude_available_modes(&version);
        Capabilities {
            driver: "claude".to_string(),
            host_id: ctx.host_id.clone(),
            cli_available,
            launchable: self.launchable(),
            models: ModelCatalog {
                models,
                source: ModelListSource::Observed,
                free_text: true,
            },
            available_modes,
            session_scope: Some(session_scope),
            sessions,
            fetched_at_ms: now_ms(),
            errors,
        }
    }
}

#[cfg(not(coverage))]
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ===========================================================================
// opencode probe + discovery helpers (§6 / §8)
// ===========================================================================

/// Build the opencode discovery script: after `setup`, check availability
/// of the resolved command word, then run `<command> models` (lines of
/// `provider/model`) and `opencode session list --format json` for resume.
///
/// Output is sectioned by the standard `@@MARSAD_SEC_BEGIN:`/`@@MARSAD_SEC_END:`
/// sentinels (same as the other builders) so `parse_discovery_sections` works.
pub fn build_opencode_discovery_script(command: &str, setup: Option<&str>) -> String {
    let word = command_word(command);
    let avail = section("avail", &format!("command -v {} 2>/dev/null", sh_squote(word)));
    let models = section(
        "models",
        &format!(
            "command -v {w} >/dev/null 2>&1 && {cmd} models 2>/dev/null || true",
            w = sh_squote(word),
            cmd = command,
        ),
    );
    let sessions = section(
        "sessions",
        &format!(
            "command -v {w} >/dev/null 2>&1 && {cmd} session list --format json 2>/dev/null || true",
            w = sh_squote(word),
            cmd = command,
        ),
    );
    with_setup(setup, &format!("{avail}{models}{sessions}"))
}

/// Parse `opencode models` stdout: one `provider/model` line per entry.
///
/// `raw` is the body of the `models` discovery section.
/// Returns `Vec<ModelInfo>` with `source: Listed` and `free_text: true`.
/// The `--verbose` variant appends cost info after whitespace; only the first
/// whitespace-separated token (the `provider/model` id) is kept.
pub fn parse_opencode_models(raw: &str) -> Vec<ModelInfo> {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            // Strip optional cost suffix (e.g. "  $3.00/1M in, $15.00/1M out").
            // The id is the first whitespace-delimited token.
            let id = line.split_whitespace().next().unwrap_or(line);
            ModelInfo {
                id: id.to_string(),
                annotation: None,
            }
        })
        .collect()
}

/// Parse `opencode session list --format json` stdout: a list of
/// `(session_id, Option<project>)` pairs.
///
/// `raw` is the body of the `sessions` discovery section.
/// Malformed JSON or a non-array root yields an empty list — never a panic.
/// Entries missing an `id` field are skipped. The `directory` field maps to
/// `project`; missing/null `directory` yields `None`.
pub fn parse_opencode_sessions(raw: &str) -> Vec<(String, Option<String>)> {
    let arr = match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(serde_json::Value::Array(a)) => a,
        _ => return Vec::new(),
    };
    arr.into_iter()
        .filter_map(|entry| {
            let id = entry.get("id").and_then(|v| v.as_str())?.to_string();
            if id.is_empty() {
                return None;
            }
            let project = entry
                .get("directory")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            Some((id, project))
        })
        .collect()
}

/// Real opencode probe: setup-first `command -v` + `opencode models` +
/// `opencode session list`, run over the host connection or locally.
/// Registered in the probes list in `serve.rs` alongside claude/codex/agy.
#[cfg(not(coverage))]
pub struct RealOpencodeProbe;

#[cfg(not(coverage))]
impl CapabilityProbe for RealOpencodeProbe {
    fn driver(&self) -> &'static str {
        "opencode"
    }

    fn launchable(&self) -> bool {
        true
    }

    fn probe(&self, ctx: &HostProbeCtx) -> Capabilities {
        let command = ctx.resolve_command("opencode");
        let script = build_opencode_discovery_script(&command, ctx.setup.as_deref());
        let mut errors = Vec::new();
        let (cli_available, models, sessions) =
            match run_discovery(ctx.host_target.as_deref(), &script, ctx.deadline) {
                Ok(out) => {
                    let secs = parse_discovery_sections(&out);
                    let raw_sessions = secs.get("sessions");
                    let discovered: Vec<DiscoveredSession> = parse_opencode_sessions(raw_sessions)
                        .into_iter()
                        .map(|(id, project)| DiscoveredSession {
                            id,
                            project,
                            snippet: String::new(),
                            modified_ms: 0,
                        })
                        .collect();
                    (
                        parse_avail(secs.get("avail")),
                        parse_opencode_models(secs.get("models")),
                        discovered,
                    )
                }
                Err(e) => {
                    errors.push(format!("opencode discovery: {e}"));
                    (false, vec![], vec![])
                }
            };
        Capabilities {
            driver: "opencode".to_string(),
            host_id: ctx.host_id.clone(),
            cli_available,
            launchable: self.launchable(),
            models: ModelCatalog {
                models,
                source: ModelListSource::Listed,
                free_text: true,
            },
            available_modes: vec![],
            session_scope: Some(SessionScope::AllKnown),
            sessions,
            fetched_at_ms: now_ms(),
            errors,
        }
    }
}

