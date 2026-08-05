//! Production subprocess/$HOME-rooted CapabilityProbe implementations (never unit-tested).
//!
//! Extracted from `capabilities.rs` verbatim; `use super::*` inherits the
//! parent module's imports and cross-section helpers.

use super::*;

// ===========================================================================
// Bounded line reader — shared by both mode-probe handshakes.
//
// Spawns a dedicated thread that reads lines from `reader` and sends each
// `String` over an `mpsc::Sender`.  The handshake loop calls
// `rx.recv_timeout(remaining_deadline)` instead of a blocking `read_line`,
// so the deadline is honoured even when the child is alive-but-silent.
//
// This is ALWAYS compiled (no `cfg(not(coverage))` gate) so the unit test
// below can exercise it directly with a pipe that never yields a line.
// ===========================================================================

/// Spawn a background thread that reads newline-delimited lines from `reader`
/// and sends each `String` (including the trailing `\n`) over the returned
/// channel.  The thread exits when `reader` reaches EOF or an IO error.
///
/// The caller drives the handshake by calling `rx.recv_timeout(remaining)`
/// instead of blocking `read_line`.  On `RecvTimeoutError::Timeout` the
/// caller should kill the child and return `Err`; on `Disconnected` (EOF /
/// thread exited) the caller should also return `Err`.
pub fn spawn_bounded_line_reader<R: std::io::BufRead + Send + 'static>(
    reader: R,
) -> std::sync::mpsc::Receiver<String> {
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut reader = reader;
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => break,          // EOF
                Ok(_) => {
                    if tx.send(line).is_err() {
                        break; // receiver dropped
                    }
                }
                Err(_) => break,         // IO error
            }
        }
    });
    rx
}

// ===========================================================================
// Part 5a — pure spawn-argv builder (always compiled, unit-tested)
//
// `build_probe_spawn_argv` is the single source of truth for how a mode
// probe is launched across ALL transports (local, SSH, WSL, plink, wslssh).
// It is a PURE function (no IO, no subprocess), always compiled (no
// `cfg(not(coverage))` gate), and unit-tested in `tests/capabilities.rs`
// alongside the other pure helpers.
// ===========================================================================

/// Build the complete argv for a mode-probe subprocess, routing through the
/// correct transport for `host_target`.
///
/// # Semantics
///
/// ```text
/// prefix = exec_prefix_for(host_target).unwrap_or_default()
/// inner  = "{maybe_setup}exec {quoted_tool_argv}"
/// argv   = prefix + ["bash", "-lc", inner]
/// ```
///
/// where `maybe_setup = setup.filter(!is_empty).map(|s| "{s}; ").unwrap_or("")`
/// and each `tool_argv` element is POSIX single-quoted via `sh_squote`.
///
/// Using `bash -lc <arg>` (login shell, command as an argument) rather than
/// `bash -l -s` keeps the child's STDIN free for the JSON-RPC handshake, and
/// `exec` replaces bash with the tool binary so it directly owns stdin/stdout
/// (critical for remote piping over ssh/wsl/plink).
///
/// For local (`host_target` None / "local" / "localhost"): `exec_prefix_for`
/// returns `None` → argv is `["bash", "-lc", inner]` run locally. Same code
/// path, no special-casing.
///
/// # Errors
///
/// Returns `Err` if `tool_argv` is empty (no binary to spawn).
pub fn build_probe_spawn_argv(
    host_target: Option<&str>,
    setup: Option<&str>,
    tool_argv: &[String],
) -> Result<Vec<String>, String> {
    if tool_argv.is_empty() {
        return Err("tool_argv must not be empty".to_string());
    }
    let prefix = crate::remote::exec_prefix_for(host_target).unwrap_or_default();
    // Quote each element of tool_argv for safe embedding in the bash -lc command.
    let quoted: Vec<String> = tool_argv.iter().map(|s| sh_squote(s)).collect();
    let joined_quoted = quoted.join(" ");
    // Prepend setup if non-empty: "setup_cmd; exec quoted_tool_argv"
    let maybe_setup = setup
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("{s}; "))
        .unwrap_or_default();
    let inner = format!("{maybe_setup}exec {joined_quoted}");
    let mut argv = prefix;
    argv.push("bash".to_string());
    argv.push("-lc".to_string());
    argv.push(inner);
    Ok(argv)
}

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
///
/// Task B: performs a bounded `codex app-server` handshake at connect time
/// to discover available collaboration modes via `collaborationMode/list`.
/// On any failure, `available_modes` is empty (honest) and the error is
/// appended to `errors`.
///
/// Task D: when `ctx.project` is `Some(non-empty)`, filters the discovered
/// sessions to only those whose `cwd` matches the requested project
/// (honest path-equality; trim trailing slashes). Sets `session_scope`
/// to `Projects([project])` in that case, mirroring how `RealClaudeProbe`
/// scopes its sessions.
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
        let (cli_available, models, mut sessions) =
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

        // Task B: probe modes via a bounded codex app-server handshake (only
        // when the CLI is available — no point spinning up a binary that isn't
        // there). The modes deadline is a fraction of the host deadline; on
        // any failure it stays empty (honest, no fabrication).
        let available_modes = if cli_available {
            let modes_deadline = ctx.deadline.min(Duration::from_secs(10));
            match probe_codex_modes_bounded(&command, ctx.host_target.as_deref(), ctx.setup.as_deref(), modes_deadline) {
                Ok(modes) => {
                    eprintln!(
                        "marsad-agent: codex probe on {:?} → {} mode(s) at connect time",
                        ctx.host_target,
                        modes.len()
                    );
                    modes
                }
                Err(e) => {
                    eprintln!(
                        "marsad-agent: codex modes probe on {:?} failed: {e}",
                        ctx.host_target
                    );
                    errors.push(format!("codex modes probe: {e}"));
                    vec![]
                }
            }
        } else {
            vec![]
        };

        // Task D: when a project was requested, filter sessions to those whose
        // payload.cwd matches (honest path-equality), and scope accordingly.
        let session_scope = match ctx.project.as_deref() {
            Some(p) if !p.trim().is_empty() => {
                sessions = filter_codex_sessions_by_project(sessions, p);
                SessionScope::Projects(vec![p.to_string()])
            }
            _ => SessionScope::AllKnown,
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
            available_modes,
            session_scope: Some(session_scope),
            sessions,
            fetched_at_ms: now_ms(),
            errors,
        }
    }
}

/// Spawn `codex app-server --stdio` (locally or via `host_target` transport),
/// perform the minimal JSON-RPC handshake (`initialize` → `thread/start` →
/// `collaborationMode/list`), and return the parsed mode list. Bounded by `deadline`.
///
/// The handshake sequence mirrors what `wasit_codex::CodexDriver::spawn`
/// does: send `initialize` (id=1), wait for its response, send
/// `thread/start` (id=2) with a temp cwd, wait for `thread/started`
/// notification (which carries the thread_id), send
/// `collaborationMode/list` (id=5) with the thread_id, and parse the
/// id=5 response via `wasit_codex::codex_modes_from_collab_list`.
///
/// On any failure (spawn error, timeout, parse error) returns `Err`. The
/// caller treats `Err` as empty modes + appends to `errors`.
///
/// `command` is the FULL command string (e.g. `"codex"` or `"/opt/bin/codex
/// --sandbox"`). The subprocess is spawned through the transport for
/// `host_target` (local / SSH / WSL / plink / wslssh) via
/// `build_probe_spawn_argv`. For local `host_target`, the spawn is identical
/// to direct local execution; for remotes, the JSON-RPC handshake runs over
/// the transport's stdin/stdout (exactly as the live session does). `setup`
/// (if any) is prepended to the inner bash command so the tool resolves
/// in the same env as a real launched session.
#[cfg(not(coverage))]
fn probe_codex_modes_bounded(
    command: &str,
    host_target: Option<&str>,
    setup: Option<&str>,
    deadline: Duration,
) -> Result<Vec<marsad_proto::SessionMode>, String> {
    use std::io::{BufReader, Write};
    use std::process::{Command, Stdio};
    use std::sync::mpsc::RecvTimeoutError;

    // Build the tool_argv: command words + subcommand.
    let tool_argv: Vec<String> = command
        .split_whitespace()
        .map(str::to_string)
        .chain(["app-server".to_string(), "--stdio".to_string()])
        .collect();

    let argv = build_probe_spawn_argv(host_target, setup, &tool_argv)
        .map_err(|e| format!("build_probe_spawn_argv: {e}"))?;
    let bin = &argv[0];

    let mut child = Command::new(bin)
        .args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn {bin}: {e}"))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "no stdin".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "no stdout".to_string())?;

    // Spawn a dedicated reader thread. The handshake loop uses recv_timeout
    // instead of blocking read_line so the deadline fires even if the child is
    // alive-but-silent (never writes a newline, never exits).
    let rx = spawn_bounded_line_reader(BufReader::new(stdout));

    let start = std::time::Instant::now();
    let remaining = || deadline.saturating_sub(start.elapsed());

    /// Kill the child and drain the reader thread on every early-exit path
    /// so no zombie/leaked thread is left behind.
    macro_rules! cleanup_and_err {
        ($child:expr, $msg:expr) => {{
            let _ = $child.kill();
            let _ = $child.wait();
            return Err($msg);
        }};
    }

    // Helper: read one line via the bounded channel. Kills child on timeout/EOF.
    macro_rules! recv_line {
        ($child:expr, $timeout_msg:expr) => {{
            match rx.recv_timeout(remaining()) {
                Ok(line) => line,
                Err(RecvTimeoutError::Timeout) => {
                    cleanup_and_err!($child, format!("timeout: {}", $timeout_msg));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    cleanup_and_err!($child, format!("child exited unexpectedly: {}", $timeout_msg));
                }
            }
        }};
    }

    // 1. Send initialize (id=1).
    let init = wasit_codex::encode_initialize("marsad-probe", env!("CARGO_PKG_VERSION"));
    stdin.write_all(init.as_bytes()).map_err(|e| format!("write initialize: {e}"))?;
    stdin.write_all(b"\n").map_err(|e| format!("write initialize newline: {e}"))?;
    stdin.flush().map_err(|e| format!("flush: {e}"))?;

    // 2. Wait for initialize response (id=1).
    loop {
        if start.elapsed() >= deadline {
            cleanup_and_err!(child, "timeout waiting for initialize response".to_string());
        }
        let line = recv_line!(child, "waiting for initialize response");
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let val: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if val.get("method").is_some() {
            continue; // notification, skip
        }
        if val.get("id").and_then(|v| v.as_u64()) == Some(1) {
            if val.get("error").is_some() {
                cleanup_and_err!(child, "initialize error response".to_string());
            }
            break; // got the initialize ack
        }
    }

    // 3. Send thread/start (id=2) with a dummy cwd (modes are not cwd-dependent).
    let thread_req = wasit_codex::encode_thread_start(2, "/tmp", None, "");
    stdin.write_all(thread_req.as_bytes()).map_err(|e| format!("write thread/start: {e}"))?;
    stdin.write_all(b"\n").map_err(|e| format!("write thread/start newline: {e}"))?;
    stdin.flush().map_err(|e| format!("flush: {e}"))?;

    // 4. Wait for thread/started notification (carries thread_id).
    let thread_id = loop {
        if start.elapsed() >= deadline {
            cleanup_and_err!(child, "timeout waiting for thread/started".to_string());
        }
        let line = recv_line!(child, "waiting for thread/started");
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let val: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if val.get("method").and_then(|m| m.as_str()) == Some("thread/started") {
            let tid = val
                .get("params")
                .and_then(|p| p.get("thread"))
                .and_then(|t| t.get("id"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_default();
            break tid;
        }
        // Also watch for id=2 response (thread/start ack) in case it comes first.
        // It does NOT carry the thread_id in the response for codex — the
        // thread_id comes in the `thread/started` notification.
        // Also skip error responses.
        if val.get("id").and_then(|v| v.as_u64()) == Some(2) && val.get("error").is_some() {
            cleanup_and_err!(child, "thread/start error response".to_string());
        }
    };

    if thread_id.is_empty() {
        cleanup_and_err!(child, "thread/started notification missing thread.id".to_string());
    }

    // 5. Send collaborationMode/list (id=5).
    let collab_req = wasit_codex::encode_collab_mode_list(5, &thread_id);
    stdin.write_all(collab_req.as_bytes()).map_err(|e| format!("write collab/list: {e}"))?;
    stdin.write_all(b"\n").map_err(|e| format!("write collab/list newline: {e}"))?;
    stdin.flush().map_err(|e| format!("flush: {e}"))?;

    // 6. Wait for collaborationMode/list response (id=5).
    let modes = loop {
        if start.elapsed() >= deadline {
            cleanup_and_err!(child, "timeout waiting for collaborationMode/list response".to_string());
        }
        let line = recv_line!(child, "waiting for collaborationMode/list response");
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let val: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if val.get("method").is_some() {
            continue; // notification
        }
        if val.get("id").and_then(|v| v.as_u64()) == Some(5) {
            if let Some(err) = val.get("error") {
                cleanup_and_err!(
                    child,
                    format!("collaborationMode/list error response: {err}")
                );
            }
            let result = val.get("result").cloned().unwrap_or(serde_json::Value::Null);
            break wasit_codex::codex_modes_from_collab_list(&result);
        }
    };

    // Clean up: kill child (it's a probe, not a real session).
    let _ = child.kill();
    let _ = child.wait();
    Ok(modes)
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
///
/// Task B: performs a bounded `opencode acp` handshake at connect time to
/// discover available modes via `configOptions` in the `session/new` result.
/// Opencode has no non-session way to list modes (they are user-config-defined
/// and only appear in the ACP handshake response), so a minimal ACP session
/// is the only honest source. On any failure, `available_modes` is empty
/// (honest) and the error is appended to `errors`.
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

        // Task B: probe modes via a bounded opencode acp handshake (only when
        // the CLI is available). Opencode modes are user-config-defined and only
        // available from the ACP `session/new` configOptions — there is no
        // non-session subcommand. On any failure stay empty (honest).
        let available_modes = if cli_available {
            let modes_deadline = ctx.deadline.min(Duration::from_secs(10));
            match probe_opencode_modes_bounded(&command, ctx.host_target.as_deref(), ctx.setup.as_deref(), modes_deadline)
            {
                Ok((modes, _current)) => {
                    eprintln!(
                        "marsad-agent: opencode probe on {:?} → {} mode(s) at connect time",
                        ctx.host_target,
                        modes.len()
                    );
                    modes
                }
                Err(e) => {
                    eprintln!(
                        "marsad-agent: opencode modes probe on {:?} failed: {e}",
                        ctx.host_target
                    );
                    errors.push(format!("opencode modes probe: {e}"));
                    vec![]
                }
            }
        } else {
            vec![]
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
            available_modes,
            session_scope: Some(SessionScope::AllKnown),
            sessions,
            fetched_at_ms: now_ms(),
            errors,
        }
    }
}

/// Spawn `opencode acp` (locally or via `host_target` transport), perform the
/// minimal ACP JSON-RPC handshake (`initialize` → `session/new`), and return
/// the parsed `(available_modes, current_mode_id)` from the `configOptions`
/// in the `session/new` result. Bounded by `deadline`.
///
/// This mirrors what `wasit_opencode::OpencodeDriver::spawn` does: send
/// `initialize` (id=1), wait for its response, send `session/new` (id=2)
/// with a temp cwd, and parse the `configOptions` from the result via
/// `wasit_opencode::parse_config_modes`.
///
/// The subprocess is spawned through the transport for `host_target`
/// (local / SSH / WSL / plink / wslssh) via `build_probe_spawn_argv`.
/// `setup` (if any) is prepended so the tool resolves in the same env as a
/// real launched session.
///
/// On any failure returns `Err`. The caller treats `Err` as empty modes +
/// appends to `errors`.
#[cfg(not(coverage))]
fn probe_opencode_modes_bounded(
    command: &str,
    host_target: Option<&str>,
    setup: Option<&str>,
    deadline: Duration,
) -> Result<(Vec<marsad_proto::SessionMode>, String), String> {
    use std::io::{BufReader, Write};
    use std::process::{Command, Stdio};
    use std::sync::mpsc::RecvTimeoutError;

    // Build the tool_argv: command words + subcommand.
    let tool_argv: Vec<String> = command
        .split_whitespace()
        .map(str::to_string)
        .chain(["acp".to_string()])
        .collect();

    let argv = build_probe_spawn_argv(host_target, setup, &tool_argv)
        .map_err(|e| format!("build_probe_spawn_argv: {e}"))?;
    let bin = &argv[0];

    let mut child = Command::new(bin)
        .args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn {bin}: {e}"))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "no stdin".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "no stdout".to_string())?;

    // Spawn a dedicated reader thread. The handshake loop uses recv_timeout
    // instead of blocking read_line so the deadline fires even if the child is
    // alive-but-silent (never writes a newline, never exits).
    let rx = spawn_bounded_line_reader(BufReader::new(stdout));

    let start = std::time::Instant::now();
    let remaining = || deadline.saturating_sub(start.elapsed());

    /// Kill the child and drain the reader thread on every early-exit path
    /// so no zombie/leaked thread is left behind.
    macro_rules! cleanup_and_err {
        ($child:expr, $msg:expr) => {{
            let _ = $child.kill();
            let _ = $child.wait();
            return Err($msg);
        }};
    }

    // Helper: read one line via the bounded channel. Kills child on timeout/EOF.
    macro_rules! recv_line {
        ($child:expr, $timeout_msg:expr) => {{
            match rx.recv_timeout(remaining()) {
                Ok(line) => line,
                Err(RecvTimeoutError::Timeout) => {
                    cleanup_and_err!($child, format!("timeout: {}", $timeout_msg));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    cleanup_and_err!($child, format!("child exited unexpectedly: {}", $timeout_msg));
                }
            }
        }};
    }

    // 1. Send initialize (id=1).
    let init = wasit_opencode::encode_initialize("marsad-probe", env!("CARGO_PKG_VERSION"));
    stdin.write_all(init.as_bytes()).map_err(|e| format!("write initialize: {e}"))?;
    stdin.write_all(b"\n").map_err(|e| format!("write initialize newline: {e}"))?;
    stdin.flush().map_err(|e| format!("flush: {e}"))?;

    // 2. Wait for initialize response (id=1).
    loop {
        if start.elapsed() >= deadline {
            cleanup_and_err!(child, "timeout waiting for initialize response".to_string());
        }
        let line = recv_line!(child, "waiting for initialize response");
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let val: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if val.get("method").is_some() {
            continue;
        }
        if val.get("id").and_then(|v| v.as_u64()) == Some(1) {
            if val.get("error").is_some() {
                cleanup_and_err!(child, "initialize error response".to_string());
            }
            break;
        }
    }

    // 3. Send session/new (id=2) with a temp cwd (modes are global config).
    let session_new = wasit_opencode::encode_session_new(2, "/tmp", None);
    stdin.write_all(session_new.as_bytes()).map_err(|e| format!("write session/new: {e}"))?;
    stdin.write_all(b"\n").map_err(|e| format!("write session/new newline: {e}"))?;
    stdin.flush().map_err(|e| format!("flush: {e}"))?;

    // 4. Wait for session/new response (id=2) carrying configOptions.
    let modes = loop {
        if start.elapsed() >= deadline {
            cleanup_and_err!(child, "timeout waiting for session/new response".to_string());
        }
        let line = recv_line!(child, "waiting for session/new response");
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let val: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if val.get("method").is_some() {
            continue; // notification
        }
        if val.get("id").and_then(|v| v.as_u64()) == Some(2) {
            if val.get("error").is_some() {
                cleanup_and_err!(child, "session/new error response".to_string());
            }
            let result = val.get("result").cloned().unwrap_or(serde_json::Value::Null);
            break wasit_opencode::parse_config_modes(&result);
        }
    };

    let _ = child.kill();
    let _ = child.wait();
    Ok(modes)
}

// ===========================================================================
// Tests for the bounded line reader contract (always compiled so the coverage
// build can reach them — see `spawn_bounded_line_reader` above).
// ===========================================================================

#[cfg(test)]
mod bounded_reader_tests {
    use super::spawn_bounded_line_reader;
    use std::io::BufReader;
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::{Duration, Instant};

    /// Proves that `spawn_bounded_line_reader` + `recv_timeout` times out
    /// quickly when the underlying reader never yields a line, because the
    /// underlying subprocess is alive-but-silent (never writes to stdout).
    ///
    /// Spawns `bash -lc "exec sleep 30"` (the same transport shape that the
    /// real mode probes use, via `build_probe_spawn_argv`) with a 1-second
    /// `recv_timeout` deadline and asserts the probe returns `Err(Timeout)`
    /// in well under the `sleep` duration (~3s budget).
    ///
    /// This is the canonical regression test for the blocking-read bug:
    /// before the fix, `read_line` on a silent-child stdout would hang for
    /// the full 30-second `sleep` duration.
    #[test]
    fn bounded_probe_via_build_probe_spawn_argv_times_out_on_silent_child() {
        use crate::capabilities::build_probe_spawn_argv;
        use std::process::{Command, Stdio};

        // Build argv the same way the real probes do: local host, no setup,
        // tool_argv = ["sleep", "30"].  This produces
        // ["bash", "-lc", "exec 'sleep' '30'"], exactly mirroring the probe
        // spawn path.
        let tool_argv = vec!["sleep".to_string(), "30".to_string()];
        let argv = build_probe_spawn_argv(None, None, &tool_argv)
            .expect("build_probe_spawn_argv must succeed for local target");

        let bin = &argv[0];
        let mut child = Command::new(bin)
            .args(&argv[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("bash must be available on test machine");

        let stdout = child.stdout.take().expect("stdout must be piped");
        let rx = spawn_bounded_line_reader(BufReader::new(stdout));

        let deadline = Duration::from_secs(1);
        let start = Instant::now();
        // Use recv_timeout exactly as the fixed handshake loops do.
        let result = rx.recv_timeout(deadline);
        let elapsed = start.elapsed();

        // Kill the silent child so it doesn't linger as a zombie.
        let _ = child.kill();
        let _ = child.wait();

        assert!(
            elapsed < Duration::from_secs(4),
            "recv_timeout must fire near the 1s deadline, NOT hang for 30s; took {elapsed:?}"
        );
        assert!(
            matches!(result, Err(RecvTimeoutError::Timeout)),
            "expected RecvTimeoutError::Timeout for a silent child, got: {result:?}"
        );
    }
}
