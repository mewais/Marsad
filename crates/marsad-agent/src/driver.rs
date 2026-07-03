//! Build the `claude` invocation (`claude_argv`/`posture_to_mode`) and drive the
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
                let mut prefix = String::new();
                if let Some(s) = setup.filter(|s| !s.is_empty()) {
                    prefix.push_str(s);
                    prefix.push('\n');
                }
                for (k, v) in env {
                    prefix.push_str(&format!("export {}={}\n", shell_quote(k), shell_quote(v)));
                }
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
                let _ = container_name; // suppress unused-variable on Windows
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
        let argv = claude_argv(spec);
        let cmd = self.build_spawn_cmd(
            container_name,
            &spec.project,
            &argv,
            spec.setup.as_deref(),
            &spec.env,
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
                let mut argv = claude_argv(spec);
                // X4a: resolve the `--mcp-config` placeholder (if `spec.wasl`
                // added one) to the true session directory now that
                // `container_name` — hence the session id — is known, and
                // write the file there. No-op when `wasl:false` (no
                // placeholder present).
                self.materialize_wasl_mcp_config(container_name, &mut argv);
                let cmd = self.build_spawn_cmd(
                    container_name,
                    &spec.project,
                    &argv,
                    spec.setup.as_deref(),
                    &spec.env,
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

// ── claude_argv ───────────────────────────────────────────────────────────────

/// Map a `Posture` to the `--permission-mode` value passed to `claude`.
fn posture_to_mode(posture: &marsad_proto::Posture) -> &'static str {
    match posture {
        marsad_proto::Posture::Ask => "default",
        marsad_proto::Posture::AutoApproveSafe => "acceptEdits",
        marsad_proto::Posture::FullAutonomy => "bypassPermissions",
    }
}

/// Build the exact `claude` invocation for a session launch.
///
/// `first_prompt` is intentionally NOT included here — in `--input-format
/// stream-json` mode the prompt must be delivered via `in.fifo` as a
/// stream-json user message, not as a positional CLI argument.
///
/// The trailing argument depends on `spec.mode`:
/// - `Fresh`    → nothing (new conversation; cwd is set by the spawn `cd`);
/// - `Continue` → `--continue` (resume the most recent conversation in the cwd);
/// - `Resume(id)` → `--resume <id>` (resume that specific conversation).
///
/// The session's `cwd` is set by the FIFO-wired `cd <project> && claude …`
/// command (see `build_spawn_cmd`), so `--continue`/`--resume` resolve against
/// the right project directory.
pub fn claude_argv(spec: &marsad_proto::LaunchSpec) -> Vec<String> {
    let mut argv = vec![
        // argv[0] is the host's configured claude executable (a wrapper like
        // `genie` or an absolute path), defaulting to the bare `claude`.
        spec.claude_bin
            .clone()
            .unwrap_or_else(|| "claude".to_string()),
        "--print".to_string(),
        "--input-format".to_string(),
        "stream-json".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--model".to_string(),
        spec.model.clone(),
        "--permission-mode".to_string(),
        posture_to_mode(&spec.posture).to_string(),
    ];

    // Route tool-permission prompts to the control-protocol stream so the GUI can
    // approve/deny them. `--permission-prompt-tool stdio` is the CLI sentinel that
    // makes claude emit `can_use_tool` control_requests on stdout (verified
    // against the 2.1.x binary: `mx9("stdio") => createCanUseTool`).
    //
    // ALWAYS included, for every posture (verified: claude accepts it alongside
    // `--permission-mode bypassPermissions` with no conflict). The flag only
    // *registers* a destination for approval requests; it never forces gating:
    // - Ask (`default`): every action gates.
    // - AutoApproveSafe (`acceptEdits`): edits auto-approve; risky tools gate.
    // - FullAutonomy (`bypassPermissions`): claude's OWN checks are skipped, but
    //   anything it can't bypass — notably MCP-server-sourced HITL (the MCP's own
    //   elicitation/approval, independent of claude's permission mode) — still
    //   routes here as a gate instead of silently hanging in `--print` mode.
    argv.push("--permission-prompt-tool".to_string());
    argv.push("stdio".to_string());

    match &spec.mode {
        // Fresh: nothing to add. The session's working directory is set by the
        // `cd <project>` in the remote run command (and by the local spawn cwd),
        // so claude starts a new conversation in the right place. A positional
        // here would be parsed as claude's *prompt* (e.g. a literal "~/"), which
        // is wrong — the first task is delivered as a stream-json user message.
        marsad_proto::LaunchMode::Fresh => {}
        marsad_proto::LaunchMode::Continue => argv.push("--continue".to_string()),
        marsad_proto::LaunchMode::Resume(id) => {
            argv.push("--resume".to_string());
            argv.push(id.clone());
        }
    }

    // Disabled tools: pass as a single comma-joined value to --disallowedTools.
    // Only added when the list is non-empty (verified: the flag is absent when
    // the list is empty so the absence/presence test holds trivially).
    if !spec.disallowed_tools.is_empty() {
        argv.push("--disallowedTools".to_string());
        argv.push(spec.disallowed_tools.join(","));
    }

    // X4a (docs/specs/wasl-connector.md §X4a): request claude launch the
    // host's `wasl-mcpd` as a stdio MCP-server subprocess (the session's SEND
    // node). ADDITIVE ONLY — `wasl:false`/absent produces byte-identical argv
    // to every pre-X4 launch (the load-bearing optionality proof).
    //
    // The VALUE here is a placeholder filename, not an absolute path: this fn
    // is a pure function of `LaunchSpec` alone (frozen by
    // `tests/wasl_launch_attach.rs`'s `claude_argv(&spec)` call shape), and
    // the true per-session directory isn't known until `Agent::launch`
    // assigns a session id (after this fn runs). `spawn_container`'s real-IO
    // path (`driver.rs`, `#[cfg(not(coverage))]`) rewrites this placeholder
    // to the true `<session_dir>/wasl-mcp.json` absolute path — and writes
    // that file — once the session id is known; see
    // `materialize_wasl_mcp_config`. UNSPECIFIED/flagged for the owner in the
    // test file's header: only the flag's presence/singularity and a
    // non-empty, "wasl"-identifying value are pinned here.
    if spec.wasl {
        argv.push("--mcp-config".to_string());
        argv.push(WASL_MCP_CONFIG_FILENAME.to_string());
    }

    argv
}

/// Placeholder `--mcp-config` filename `claude_argv` injects for a
/// `wasl:true` launch; `materialize_wasl_mcp_config` resolves it to an
/// absolute path once the session directory is known. Shared as a constant
/// so the two sides can never drift.
const WASL_MCP_CONFIG_FILENAME: &str = "wasl-mcp.json";
