//! Build the `claude` invocation (`claude_argv`) and drive the
//! session container: `build_spawn_cmd` (the shell command handed to
//! `Container::spawn`), `spawn_container`/`kill_container` (routing a session to
//! the local platform container or to a remote host), and the store lookups
//! (`session_host_target`/`session_transcript_project_uuid`) those need.

use super::{Agent, AgentError};

#[cfg(not(coverage))]
use super::remote::{
    exec_prefix_for, remote_durable_launch_script, remote_kill_script, run_remote_script,
};

impl Agent {
    /// Build the shell command string passed to `Container::spawn`.
    ///
    /// - Coverage / unit-test build: returns `argv.join(" ")` (the fake container
    ///   ignores the command body; what matters is that spawn is called).
    /// - Production build: if a real container is active, returns the full
    ///   FIFO-wired tmux invocation per the realio.md spec:
    ///   `cd <project> && cat in.fifo | claude <args> > out.fifo 2>err.log`
    ///   Otherwise falls back to `argv.join(" ")`.
    ///
    /// NOTE ON `cat in.fifo |` INSTEAD OF `< in.fifo` (found via live i03 testing
    /// against real tmux 3.1c + claude 2.1.198): redirecting claude's stdin
    /// directly from a *named* FIFO (`claude ... < in.fifo`) makes claude exit 0
    /// with **zero** output and no session transcript whenever the process has a
    /// controlling terminal — which every tmux pane has, even fully detached
    /// (`tmux new-session -d`) ones with no client attached. The exact same
    /// invocation works perfectly when claude's stdin is instead an *anonymous*
    /// pipe (confirmed: `echo ... | claude ...` inside the same tmux pane works;
    /// only `claude ... < named_fifo` inside tmux is broken). Piping the FIFO
    /// through `cat` converts claude's stdin fd from a named pipe to an
    /// anonymous one, sidestepping the bug entirely — verified against the real
    /// binaries, not simulated.  Ruled out during diagnosis: PATH, HOME,
    /// CLAUDECODE/CLAUDE_CODE_* env leakage, and controlling-terminal
    /// detachment via `setsid` (none of these changed the outcome). See
    /// docs/specs/realio.md for the updated shape.
    #[cfg(coverage)]
    fn build_spawn_cmd(
        &self,
        _container_name: &str,
        _project: &str,
        argv: &[String],
        _setup: Option<&str>,
        _env: &[(String, String)],
        _driver: &str,
    ) -> String {
        argv.join(" ")
    }

    #[cfg(not(coverage))]
    fn build_spawn_cmd(
        &self,
        container_name: &str,
        project: &str,
        argv: &[String],
        setup: Option<&str>,
        env: &[(String, String)],
        driver: &str,
    ) -> String {
        if self.real_container.0.is_some() {
            // Platform-specific command construction.
            //
            // On Unix: build a FIFO-wired shell invocation.  The session dir
            // contains in.fifo and out.fifo; the tmux shell does the redirection.
            //
            // On Windows: the marsad-winshim.exe process receives the claude
            // command via `--cmd` and handles stdin/stdout redirection itself via
            // STARTUPINFO handles.  We pass only the raw claude invocation — no
            // shell, no FIFO paths.  The shim wraps it in `cmd.exe /C <cmd>`.
            #[cfg(unix)]
            {
                let session_dir = std::path::PathBuf::from(&self.config.data_root)
                    .join("sessions")
                    .join(container_name);
                let in_fifo = session_dir.join("in.fifo");
                let out_fifo = session_dir.join("out.fifo");
                let err_log = session_dir.join("err.log");

                let mut prefix = String::new();
                if let Some(s) = setup.filter(|s| !s.is_empty()) {
                    prefix.push_str(s);
                    prefix.push('\n');
                }
                for (k, v) in env {
                    prefix.push_str(&format!(
                        "export {}={}\n",
                        shell_quote(k),
                        shell_quote(v)
                    ));
                }

                if driver == "opencode" {
                    // opencode ACP: persistent process, claude-shaped (in.fifo →
                    // opencode acp → out.log). Same FIFO-wired single-invocation
                    // shape as claude — NOT an eval-loop. We write to out.log (a
                    // REGULAR FILE, not out.fifo) so the reader thread can open it
                    // independently at any time (a named FIFO blocks on open() until
                    // a paired reader/writer appears — the codex-branch comment
                    // explains the deadlock). `cat in.fifo |` is load-bearing (same
                    // named-FIFO/anonymous-pipe reason as the claude path above).
                    let out_log = session_dir.join("out.log");
                    let bin = argv.first().map(String::as_str).unwrap_or("opencode");
                    let opencode_args = &argv[1..];
                    format!(
                        "{}cd {} && cat {} | {} {} >> {} 2>>{}",
                        prefix,
                        shell_quote(project),
                        shell_quote(&in_fifo.display().to_string()),
                        shell_quote(bin),
                        opencode_args
                            .iter()
                            .map(|a| shell_quote(a))
                            .collect::<Vec<_>>()
                            .join(" "),
                        out_log.display(),
                        err_log.display(),
                    )
                } else if driver == "codex" {
                    // Codex app-server: persistent JSON-RPC process, claude-shaped
                    // (in.fifo → `codex app-server` → out.log). SAME single-invocation
                    // FIFO-wired shape as opencode — NOT the old `codex exec`
                    // eval-loop. `argv` here is `codex app-server …` (from
                    // `codex_appserver_argv`); the reader performs the JSON-RPC
                    // handshake (`initialize` → id==1 response → `thread/start` →
                    // `thread/started` → turns) over this pipe.
                    //
                    // REGRESSION FIXED: this branch previously hardcoded the old
                    // eval-loop (`cat in.fifo | while read __cmd; do eval "$__cmd"`),
                    // ignoring `argv`. But the rest of the stack (reader handshake,
                    // wasit_codex encoders, parser expecting `thread/started`) had
                    // been migrated to the app-server JSON-RPC protocol, so the
                    // eval-loop `eval`'d the reader's JSON-RPC `initialize` line as a
                    // shell command → error to err.log, nothing usable in out.log,
                    // no id==1 response, no SessionInit (i12 stalled at S1). Running
                    // `codex app-server` directly — as the argv/remote paths already
                    // did — restores the handshake.
                    //
                    // We write to `out.log` (a REGULAR FILE), NOT `out.fifo`: a named
                    // FIFO blocks on open() until a paired reader/writer appears, but
                    // codex's first write can't happen until after the handshake,
                    // which needs the reader connected first — a deadlock. A regular
                    // file never blocks on open(); the reader polls out.log with
                    // read_line, sleeping on EOF and exiting when in.fifo closes.
                    //
                    // `cat in.fifo |` is load-bearing (same named-FIFO/anonymous-pipe
                    // reason as claude's path — see the build_spawn_cmd doc comment).
                    //
                    // `>>` (append): reconnects append to the same log so the reader
                    // sees events from all turns in sequence.
                    let out_log = session_dir.join("out.log");
                    let bin = argv.first().map(String::as_str).unwrap_or("codex");
                    let codex_args = &argv[1..];
                    format!(
                        "{}cd {} && cat {} | {} {} >> {} 2>>{}",
                        prefix,
                        shell_quote(project),
                        shell_quote(&in_fifo.display().to_string()),
                        shell_quote(bin),
                        codex_args
                            .iter()
                            .map(|a| shell_quote(a))
                            .collect::<Vec<_>>()
                            .join(" "),
                        out_log.display(),
                        err_log.display(),
                    )
                } else {
                    // Claude (and any other driver): the original single-invocation shape.
                    // argv[0] is the configured claude bin; the rest are its arguments.
                    // We build (with optional setup + env exports prepended):
                    //   <setup>
                    //   export 'K'='V'
                    //   cd <project> && cat in.fifo | <bin> <args> > out.fifo 2>err.log
                    //
                    // `cat in.fifo |` (rather than `< in.fifo`) is load-bearing — see
                    // the NOTE on the doc comment above `build_spawn_cmd`.
                    let bin = argv.first().map(String::as_str).unwrap_or("claude");
                    let claude_args = &argv[1..];
                    format!(
                        "{}cd {} && cat {} | {} {} > {} 2>{}",
                        prefix,
                        shell_quote(project),
                        shell_quote(&in_fifo.display().to_string()),
                        shell_quote(bin),
                        claude_args
                            .iter()
                            .map(|a| shell_quote(a))
                            .collect::<Vec<_>>()
                            .join(" "),
                        out_fifo.display(),
                        err_log.display(),
                    )
                }
            }

            #[cfg(windows)]
            {
                // The shim handles I/O redirection.  We pass the raw claude
                // invocation so the shim can exec it via `cmd.exe /C <cmd>`.
                // argv[0] is "claude"; the rest are its arguments.
                // We build: cd /d "<project>" && claude <args>
                // (/d switches the drive on Windows cmd.exe).
                //
                // QUOTING: Windows paths must NOT use Rust's {:?} Debug format —
                // that escapes backslashes (e.g. "C:\foo" becomes "C:\\foo"),
                // which cmd.exe interprets as literal double-backslashes.
                // Windows paths never contain double-quote characters, so wrapping
                // in double quotes is always safe and produces correct cmd.exe syntax.
                //
                // Codex on Windows uses the same shim path for now (codex is not
                // currently supported on Windows in Stage 2 — the shim would need
                // a separate eval-loop container command, which is a documented gap).
                let _ = container_name; // suppress unused-variable on Windows
                let _ = driver;
                // Best-effort env + setup prepend for the cmd.exe shim:
                //   set K=V& <setup>& cd /d "proj" && <bin> args
                let bin = argv.first().map(String::as_str).unwrap_or("claude");
                let claude_args = &argv[1..];
                let mut prefix = String::new();
                for (k, v) in env {
                    prefix.push_str(&format!("set {k}={v}& "));
                }
                if let Some(s) = setup.filter(|s| !s.is_empty()) {
                    prefix.push_str(&format!("{s}& "));
                }
                format!(
                    "{}cd /d \"{}\" && {} {}",
                    prefix,
                    project,
                    bin,
                    claude_args.join(" ")
                )
            }
        } else {
            argv.join(" ")
        }
    }

    /// Spawn the session container — locally, or on the session's remote host.
    ///
    /// Coverage/test build: always the injected fake (host_target ignored), so the
    /// existing container-call assertions hold and the measured surface is stable.
    #[cfg(coverage)]
    pub(crate) fn spawn_container(
        &mut self,
        container_name: &str,
        spec: &marsad_proto::LaunchSpec,
    ) -> Result<(), AgentError> {
        let argv = session_argv(spec);
        let cmd = self.build_spawn_cmd(
            container_name,
            &spec.project,
            &argv,
            spec.setup.as_deref(),
            &spec.env,
            &spec.driver,
        );
        self.active_container()
            .spawn(container_name, &cmd)
            .map_err(|e| AgentError::Spawn(format!("{e}")))
    }

    /// Real-IO build: local hosts use the platform container; remote hosts
    /// (`exec_prefix_for` → `Some`) launch a DURABLE detached session on the
    /// remote: `mkfifo in.fifo`, keepalive holder (keeps write end open), and
    /// `setsid` + `nohup` claude reading from the FIFO and appending to `out.log`.
    /// The FIFO + log survive agent restarts — the reader thread connects by
    /// spawning a `cat >> in.fifo` writer child and a `tail -n0 -f out.log` reader
    /// child over a fresh ssh/wsl connection.
    #[cfg(not(coverage))]
    pub(crate) fn spawn_container(
        &mut self,
        container_name: &str,
        spec: &marsad_proto::LaunchSpec,
    ) -> Result<(), AgentError> {
        match exec_prefix_for(spec.host_target.as_deref()) {
            None => {
                let mut argv = session_argv(spec);
                // X4a: resolve the `--mcp-config` placeholder (if `spec.wasl`
                // added one) to the true session directory now that
                // `container_name` — hence the session id — is known, and
                // write the file there. No-op when `wasl:false` (no
                // placeholder present). Also a no-op for codex sessions:
                // codex receives its MCP servers via `-c mcp_servers.*`
                // overrides in `wasit_codex::codex_argv` (not --mcp-config),
                // so no placeholder is injected and nothing needs rewriting.
                self.materialize_wasl_mcp_config(container_name, &mut argv);
                let cmd = self.build_spawn_cmd(
                    container_name,
                    &spec.project,
                    &argv,
                    spec.setup.as_deref(),
                    &spec.env,
                    &spec.driver,
                );
                self.active_container()
                    .spawn(container_name, &cmd)
                    .map_err(|e| AgentError::Spawn(format!("{e}")))
            }
            Some(_) => {
                // Extract the session_id from the container name ("marsad-<sid>").
                let sid = container_name
                    .strip_prefix("marsad-")
                    .unwrap_or(container_name);
                let run_cmd = self
                    .session_run_cmds
                    .0
                    .lock()
                    .unwrap()
                    .get(sid)
                    .cloned()
                    .unwrap_or_default();
                let script = remote_durable_launch_script(sid, &run_cmd);
                let host = spec.host_target.as_deref().unwrap_or("");
                eprintln!(
                    "marsad-agent: remote session {container_name} on {host:?} — launching durable session (FIFO + setsid)"
                );
                // run_remote_script returns None on connection failure (spawn error).
                run_remote_script(host, &script).ok_or_else(|| {
                    AgentError::Spawn(format!(
                        "remote durable launch failed for {container_name} on {host:?}"
                    ))
                })?;
                Ok(())
            }
        }
    }

    /// X4a (docs/specs/wasl-connector.md §X4a): if `argv` carries the
    /// `--mcp-config`/`WASL_MCP_CONFIG_FILENAME` placeholder `claude_argv`
    /// injected for a `wasl:true` launch, rewrite it to the real absolute
    /// `<session_dir>/wasl-mcp.json` path and write the file there
    /// (`wasl-mcpd` as a stdio MCP server, keyed to this session). A no-op —
    /// zero I/O — when no placeholder is present (every `wasl:false` launch,
    /// the byte-identical baseline). Local sessions only: remote (SSH/WSL)
    /// launches keep the unresolved placeholder for now — a documented gap,
    /// not a silent one, mirroring the operator-URL-reachability caveat
    /// (§9) this same doc already carries for remote hosts.
    ///
    /// Best-effort: a write failure is logged, never fatal — config problems
    /// must never take the agent down (same doctrine as `load_wasl_config`).
    #[cfg(not(coverage))]
    fn materialize_wasl_mcp_config(&self, container_name: &str, argv: &mut [String]) {
        let Some(idx) = argv.iter().position(|a| a == "--mcp-config") else {
            return;
        };
        let Some(value) = argv.get_mut(idx + 1) else {
            return;
        };
        if value != WASL_MCP_CONFIG_FILENAME {
            // Already resolved (defensive; shouldn't happen — argv is fresh
            // from `claude_argv` on every launch).
            return;
        }

        let session_dir = std::path::PathBuf::from(&self.config.data_root)
            .join("sessions")
            .join(container_name);
        let path = session_dir.join(WASL_MCP_CONFIG_FILENAME);
        let session_id = container_name
            .strip_prefix("marsad-")
            .unwrap_or(container_name);
        let config = serde_json::json!({
            "mcpServers": {
                "wasl": {
                    "command": "wasl-mcpd",
                    "args": [],
                    "env": {
                        "WASL_CONFIG": format!("{}/wasl.json", self.config.data_root),
                        "WASL_AGENT_ID": session_id,
                    }
                }
            }
        });

        let write_result = std::fs::create_dir_all(&session_dir)
            .and_then(|()| std::fs::write(&path, config.to_string()));
        match write_result {
            Ok(()) => *value = path.display().to_string(),
            Err(e) => {
                eprintln!(
                    "marsad-agent: failed to write wasl mcp config {}: {e} — leaving --mcp-config unresolved",
                    path.display()
                );
            }
        }
    }

    /// Look up a stored session's resolved remote target (for reconcile/kill of
    /// remote containers). Real-IO build only.
    #[cfg(not(coverage))]
    pub(crate) fn session_host_target(&mut self, session_id: &str) -> Option<String> {
        self.store
            .0
            .restore()
            .ok()?
            .sessions
            .into_iter()
            .find(|s| s.id == session_id)
            .and_then(|s| s.host_target)
    }

    /// WAVE 3b: look up a stored session's `(project, host_target)` for
    /// `PtyStart` — `project` seeds a local pty's working directory;
    /// `host_target` selects the pty's command (local shell vs. the
    /// session's existing SSH/WSL target, via `pty::pty_exec_argv`). Mirrors
    /// `session_host_target`/`session_transcript_project_uuid`'s lookup
    /// shape. Real-IO build only.
    #[cfg(not(coverage))]
    pub(crate) fn session_project_and_host_target(
        &mut self,
        session_id: &str,
    ) -> Option<(String, Option<String>)> {
        self.store
            .0
            .restore()
            .ok()?
            .sessions
            .into_iter()
            .find(|s| s.id == session_id)
            .map(|s| (s.project, s.host_target))
    }

    /// Look up a stored session's `(project, claude_uuid)` so the reader thread
    /// can fetch its remote transcript `.jsonl`. The uuid is `None` for a
    /// `--continue` session (no specific id) — the fetch then picks the most
    /// recent conversation in the project dir. Real-IO build only.
    #[cfg(not(coverage))]
    pub(crate) fn session_transcript_project_uuid(
        &mut self,
        session_id: &str,
    ) -> Option<(String, Option<String>)> {
        self.store
            .0
            .restore()
            .ok()?
            .sessions
            .into_iter()
            .find(|s| s.id == session_id)
            .map(|s| (s.project, s.session_id))
    }

    /// Kill a session's container — locally or on its remote host.
    #[cfg(coverage)]
    pub(crate) fn kill_container(&mut self, _session_id: &str, container_name: &str) {
        let _ = self.active_container().kill(container_name);
    }

    #[cfg(not(coverage))]
    pub(crate) fn kill_container(&mut self, session_id: &str, container_name: &str) {
        match exec_prefix_for(self.session_host_target(session_id).as_deref()) {
            None => {
                let _ = self.active_container().kill(container_name);
            }
            Some(_) => {
                // Durable remote session: the detached claude + keepalive holder
                // live independently of this agent process. Killing them requires
                // running a kill script on the remote host (reads the stored PID
                // files and sends SIGTERM, then removes the session dir).
                // Also drop + reap the local writer child (the `cat >> in.fifo`
                // connection) if it's still in the registry — its EOF on the remote
                // doesn't kill claude (the keepalive holds the write end open), but
                // we clean it up so the slot is freed.
                let host = self.session_host_target(session_id).unwrap_or_default();
                let script = remote_kill_script(session_id);
                if let Some(out) = run_remote_script(&host, &script) {
                    eprintln!(
                        "marsad-agent: session {session_id} remote kill on {host:?}: {}",
                        out.trim()
                    );
                } else {
                    eprintln!(
                        "marsad-agent: session {session_id} remote kill script failed (host unreachable?)"
                    );
                }
                // Reap the local writer child.
                let child = {
                    let mut children = self.session_children.0.lock().unwrap();
                    children.remove(session_id)
                };
                if let Some(mut child) = child {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
    }
}

// ── shell_quote ───────────────────────────────────────────────────────────────

/// Single-quote a string for POSIX shell.  Replaces `'` with `'\''`.
/// Gated out of the coverage build (only used by `build_spawn_cmd` in the
/// real-io path) and Windows-only builds (where the shim uses cmd.exe quoting).
#[cfg(all(unix, not(coverage)))]
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

// ── LaunchSpec → wasit SpawnSpec ────────────────────────────────────────────────

/// Build a `wasit_core::SpawnSpec` from Marsad's `LaunchSpec`.
///
/// This is the single mapping between Marsad's launch fields and the union
/// `SpawnSpec` the wasit pure argv builders consume. `cwd` is intentionally left
/// `None` — Marsad sets the working directory via the FIFO-wired
/// `cd <project> && <bin> …` shell command (`build_spawn_cmd`) / the remote run
/// command, NOT via an argv flag, so the wasit argv must not carry a `--cwd`.
///
/// `mcp_config` is populated from `spec.wasl` with the `wasl-mcp.json`
/// placeholder — `materialize_wasl_mcp_config` rewrites it to the true absolute
/// path (and writes the file) once the session id is known. `wasl:false`
/// produces `None`, keeping the argv byte-identical to a non-wasl launch.
///
/// For claude sessions the resolved path is forwarded via `--mcp-config`.
/// For direct-codex sessions from Marsad the placeholder is a filename (not
/// JSON), so `codex_argv` will silently skip it — codex sessions launched
/// through Marsad do not yet receive MCP servers this way. Wasl-managed codex
/// workers (launched by the Wasl runtime, not directly by Marsad) receive a
/// real JSON blob in `mcp_config` (set by `wasl_worker_mcp_config`), which
/// `wasit_codex::codex_argv` now translates into `-c mcp_servers.*` overrides.
fn spawn_spec_from_launch(spec: &marsad_proto::LaunchSpec) -> wasit_core::SpawnSpec {
    let resume = match &spec.mode {
        marsad_proto::LaunchMode::Fresh => wasit_core::ResumeMode::Fresh,
        marsad_proto::LaunchMode::Continue => wasit_core::ResumeMode::Continue,
        marsad_proto::LaunchMode::Resume(id) => wasit_core::ResumeMode::Resume(id.clone()),
    };
    wasit_core::SpawnSpec {
        // cwd handled by the shell `cd`, not an argv flag — leave None.
        cwd: None,
        model: if spec.model.is_empty() {
            None
        } else {
            Some(spec.model.clone())
        },
        mode: spec.session_mode.clone(),
        bin: spec.claude_bin.clone(),
        env: spec.env.clone(),
        setup: spec.setup.clone(),
        disallowed_tools: spec.disallowed_tools.clone(),
        mcp_config: if spec.wasl {
            Some(WASL_MCP_CONFIG_FILENAME.to_string())
        } else {
            None
        },
        resume,
        // first_prompt is delivered over the FIFO as a user message (claude) or
        // a turn/prompt request (codex/opencode), never as an argv positional.
        first_prompt: None,
    }
}

// ── claude_argv ───────────────────────────────────────────────────────────────

/// Build the exact `claude` invocation for a session launch, via the pure
/// `wasit_claude::claude_argv` builder.
///
/// `first_prompt` is intentionally NOT included — in `--input-format
/// stream-json` mode the prompt must be delivered via `in.fifo` as a stream-json
/// user message, not as a positional CLI argument (see reader.rs).
///
/// The trailing argument depends on `spec.mode` (Fresh → nothing; Continue →
/// `--continue`; Resume(id) → `--resume <id>`). The session's `cwd` is set by the
/// FIFO-wired `cd <project> && claude …` command (see `build_spawn_cmd`).
///
/// Signature kept `LaunchSpec`-shaped (frozen by tests/wasl_launch_attach.rs and
/// the lib re-export) — it wraps the wasit builder plus the wasl `--mcp-config`
/// placeholder (via `spawn_spec_from_launch`).
pub fn claude_argv(spec: &marsad_proto::LaunchSpec) -> Vec<String> {
    wasit_claude::claude_argv(&spawn_spec_from_launch(spec))
}

/// Placeholder `--mcp-config` filename `claude_argv` injects for a
/// `wasl:true` launch; `materialize_wasl_mcp_config` resolves it to an
/// absolute path once the session directory is known. Shared as a constant
/// so the two sides can never drift.
const WASL_MCP_CONFIG_FILENAME: &str = "wasl-mcp.json";

// ── opencode_argv ─────────────────────────────────────────────────────────────

/// Build the `opencode acp` invocation for a persistent session launch.
///
/// opencode is launched as a PERSISTENT process (bidirectional ACP channel via
/// stdin/stdout), NOT an eval-loop. Binary is resolved from
/// `LaunchSpec.claude_bin` (default `"opencode"`), same as codex reuses it.
///
/// The invocation is exactly `opencode acp` — no more. Verified against opencode
/// 1.18 (`opencode acp --help`): the `acp` subcommand accepts ONLY
/// --port/--hostname/--cwd/--log-level/--print-logs/--pure/--mdns/--cors, and
/// REJECTS `--model` (yargs strict: it prints usage and exits, in either flag
/// position). Appending `--model <model>` is why sessions never started — opencode
/// died at launch. In ACP mode opencode owns model selection (its configured
/// default); a per-session override would need a supported ACP path (session
/// config / modes), not a CLI flag. `spec.model` is intentionally unused here.
pub fn opencode_argv(spec: &marsad_proto::LaunchSpec) -> Vec<String> {
    wasit_opencode::opencode_argv(&spawn_spec_from_launch(spec))
}

/// Build the `codex app-server` invocation for a PERSISTENT session launch.
///
/// Like opencode's `opencode acp`: codex runs as a long-lived JSON-RPC process
/// (stdin/stdout, NDJSON — verified live against codex 0.142.5), NOT the one-shot
/// `codex exec` eval-loop. Model/approval/sandbox are NOT CLI flags here — they are
/// set per-thread via `thread/start`/`thread/settings/update` params (the reader
/// sends them), so `spec.model`/`spec.posture` are intentionally unused in the argv.
/// Binary from `LaunchSpec.claude_bin` (default `"codex"`).
pub fn codex_appserver_argv(spec: &marsad_proto::LaunchSpec) -> Vec<String> {
    wasit_codex::codex_argv(&spawn_spec_from_launch(spec))
}

/// Dispatch to the correct argv builder based on `spec.driver`.
///
/// - `"codex"`    → [`codex_appserver_argv`] (persistent app-server)
/// - `"opencode"` → [`opencode_argv`]
/// - anything else (including `"claude"`, the default) → [`claude_argv`]
pub fn session_argv(spec: &marsad_proto::LaunchSpec) -> Vec<String> {
    match spec.driver.as_str() {
        "codex" => codex_appserver_argv(spec),
        "opencode" => opencode_argv(spec),
        _ => claude_argv(spec),
    }
}

/// Build the run-command body for a REMOTE durable session, dispatched by driver.
///
/// - `"codex"`    → `codex app-server` wrapped by `build_remote_spawn_cmd`
/// - `"opencode"` → `opencode acp` wrapped by `build_remote_spawn_cmd`
/// - anything else (`"claude"`, default) → `claude_argv` wrapped by `build_remote_spawn_cmd`
///
/// Extracted from the launch flow so this per-driver dispatch is UNIT-TESTED: a
/// regression once shipped where every non-codex driver used `claude_argv`, so a
/// remote opencode session launched `claude` (with the opencode model) while the
/// reader spoke ACP — the process was claude, fed JSON-RPC it couldn't parse, and
/// nothing reached out.log ("connected, stuck on Loading Transcript").
pub fn build_remote_run_cmd(spec: &marsad_proto::LaunchSpec) -> String {
    let argv = match spec.driver.as_str() {
        "codex" => codex_appserver_argv(spec),
        "opencode" => opencode_argv(spec),
        _ => claude_argv(spec),
    };
    crate::remote::build_remote_spawn_cmd(&spec.project, &argv, spec.setup.as_deref(), &spec.env)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use marsad_proto::{LaunchMode, LaunchSpec};

    fn base_spec(session_mode: &str, mode: LaunchMode, driver: &str) -> LaunchSpec {
        LaunchSpec {
            project: "/proj".to_string(),
            host_id: "local".to_string(),
            model: "gpt-5-codex".to_string(),
            session_mode: session_mode.to_string(),
            first_prompt: None,
            mode,
            host_target: None,
            claude_bin: None,
            setup: None,
            env: vec![],
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: driver.to_string(),
        }
    }

    // ── session_argv dispatch ─────────────────────────────────────────────────

    #[test]
    fn session_argv_codex_driver_uses_app_server() {
        // Codex now launches the PERSISTENT app-server (not the one-shot exec
        // eval-loop) — the JSON-RPC session that gives it full Claude parity.
        let spec = base_spec("", LaunchMode::Fresh, "codex");
        let argv = session_argv(&spec);
        assert_eq!(argv[0], "codex");
        assert_eq!(argv[1], "app-server");
    }

    #[test]
    fn build_remote_run_cmd_codex_uses_app_server() {
        let spec = base_spec("", LaunchMode::Fresh, "codex");
        let cmd = build_remote_run_cmd(&spec);
        assert!(cmd.contains("app-server"), "remote codex launches app-server: {cmd}");
    }

    #[test]
    fn session_argv_claude_driver_uses_claude_argv() {
        let spec = base_spec("", LaunchMode::Fresh, "claude");
        let argv = session_argv(&spec);
        assert_eq!(argv[0], "claude");
        assert!(argv.contains(&"--print".to_string()));
    }

    #[test]
    fn session_argv_unknown_driver_falls_back_to_claude() {
        let spec = base_spec("", LaunchMode::Fresh, "unknown_driver");
        let argv = session_argv(&spec);
        assert_eq!(argv[0], "claude");
    }

    // ── claude_argv: session_mode → --permission-mode ─────────────────────────

    #[test]
    fn claude_argv_empty_session_mode_omits_permission_mode_flag() {
        // Empty mode = backend default → no --permission-mode flag in argv.
        let spec = base_spec("", LaunchMode::Fresh, "claude");
        let argv = claude_argv(&spec);
        assert!(
            !argv.contains(&"--permission-mode".to_string()),
            "empty session_mode must NOT emit --permission-mode; got {argv:?}"
        );
    }

    #[test]
    fn claude_argv_plan_mode_emits_permission_mode_plan() {
        let spec = base_spec("plan", LaunchMode::Fresh, "claude");
        let argv = claude_argv(&spec);
        let pm_pos = argv.iter().position(|a| a == "--permission-mode")
            .expect("--permission-mode must be present for non-empty session_mode");
        assert_eq!(argv[pm_pos + 1], "plan", "--permission-mode value must be the session_mode id");
    }

    #[test]
    fn claude_argv_bypass_mode_emits_permission_mode_bypass() {
        let spec = base_spec("bypassPermissions", LaunchMode::Fresh, "claude");
        let argv = claude_argv(&spec);
        let pm_pos = argv.iter().position(|a| a == "--permission-mode")
            .expect("--permission-mode must be present for bypassPermissions");
        assert_eq!(argv[pm_pos + 1], "bypassPermissions");
    }

    // codex mode→(approvalPolicy, sandbox) mapping now lives in wasit_codex
    // (`codex_mode_settings`) and is covered by that crate's own tests.
}

