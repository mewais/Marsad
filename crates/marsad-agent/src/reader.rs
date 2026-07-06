//! Per-session reader thread: connect the session transport (local FIFO/pipe or
//! a remote durable FIFO bridge), pump its NDJSON stream, and broadcast decoded
//! events to every subscriber — plus the small pure-logic helpers
//! (`reconnect_backoff_next`, `assistant_has_file_modifying_tool_use`, the
//! `RECONNECT_*` constants) that drive its remote auto-reconnect behavior.

#[cfg(not(coverage))]
use std::collections::HashMap;

#[cfg(not(coverage))]
use super::remote::{
    exec_prefix_for, parse_remote_changes, remote_changes_script, remote_liveness_script,
    remote_reader_argv, remote_resolve_pid_dir, remote_session_dir, remote_transcript_script,
    remote_usage_script, remote_writer_argv, resolve_remote_bin, run_remote_script,
    run_remote_script_bounded,
};
#[cfg(not(coverage))]
use super::{PendingGate, SharedWriter};

// ── Reconnect backoff constants and helper ───────────────────────────────────

/// Initial delay before the first liveness re-probe after a connection drop (seconds).
pub const RECONNECT_INITIAL_BACKOFF_SECS: u64 = 3;

/// Maximum delay between liveness probes (seconds).
pub const RECONNECT_MAX_BACKOFF_SECS: u64 = 30;

/// Maximum total time to spend retrying a dropped connection (seconds).
/// After this the reader thread gives up and the session is considered lost.
/// 30 minutes handles MFA re-auth windows; a permanently-gone host won't
/// leak the thread forever.
pub const RECONNECT_MAX_TOTAL_SECS: u64 = 1800;

/// Timeout for each bounded liveness probe while the connection is down (seconds).
/// Short so a host that is still unreachable doesn't stall the backoff loop.
pub const RECONNECT_PROBE_TIMEOUT_SECS: u64 = 8;

/// Compute the next backoff delay: double the current value, capped at
/// `RECONNECT_MAX_BACKOFF_SECS`.
///
/// Pure logic (no IO) — compiled in all build modes and unit-tested.
pub fn reconnect_backoff_next(current: u64) -> u64 {
    current.saturating_mul(2).min(RECONNECT_MAX_BACKOFF_SECS)
}

// ── assistant_has_file_modifying_tool_use ────────────────────────────────────

/// Return `true` if the given `assistant` stream event contains at least one
/// `tool_use` content block whose tool name is a known file-modifying tool.
///
/// Used by `spawn_session_reader` (#4) to decide whether to trigger a debounced
/// remote git Changes probe BEFORE the turn ends, so the Changes tab updates
/// while claude is mid-turn writing files rather than only at `result`.
///
/// Pure logic (no IO), compiled in all build modes — covered by unit tests.
pub fn assistant_has_file_modifying_tool_use(event: &serde_json::Value) -> bool {
    // File-modifying tools whose use signals a likely git-visible change.
    const FILE_TOOLS: &[&str] = &["Bash", "Edit", "Write", "NotebookEdit", "MultiEdit"];
    let content = match event
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    {
        Some(arr) => arr,
        None => return false,
    };
    content.iter().any(|block| {
        block.get("type").and_then(|t| t.as_str()) == Some("tool_use")
            && block
                .get("name")
                .and_then(|n| n.as_str())
                .map(|name| FILE_TOOLS.contains(&name))
                .unwrap_or(false)
    })
}

/// Spawn the per-session reader thread: connect the transport (local FIFO/pipe, or a
/// remote durable FIFO bridge), send the control `initialize` handshake + an optional
/// first prompt, register the writer, then stream claude's stdout to every subscriber
/// until the session ends. Used both when a session is launched AND when reconnecting
/// a session that survived an agent restart (pass `first_prompt: None` for reconnect).
///
/// # Remote durable model (design 2B)
///
/// A durable remote session uses two independent ssh/wsl children per agent connection:
///   - **Writer child**: runs `tee -a $HOME/.marsad/<sid>/in.fifo` with its stdin piped
///     from the agent.  The agent writes prompts + gate responses to it.  When the agent
///     drops the child's stdin pipe the `tee` exits, but the remote claude keeps running
///     because the keepalive holder (`sleep 999999 > in.fifo`, started at launch) still
///     holds the FIFO write-end open.
///   - **Reader child**: runs `tail -n0 --pid=<claude> -f $HOME/.marsad/<sid>/out.log`
///     with its stdout piped to the agent.  Streams only NEW output (lines written after
///     connect time) so reconnects don't replay historical output.  Historical
///     conversation is supplied by the `TranscriptBackfill` mechanism over a separate
///     `run_remote_script` call.
///
/// The writer child is stored in `children` so it can be reaped on thread exit.
/// Killing the session runs `remote_kill_script` over ssh (SIGTERM to ka+claude, dir
/// removal) rather than killing a child process.
///
/// # Auto-reconnect (connection-drop recovery)
///
/// When the reader stream ends for a REMOTE session, the thread probes liveness via
/// `remote_liveness_script` with `run_remote_script_bounded`.  If the probe itself
/// fails or times out (the connection/ControlMaster is still down), the thread retries
/// with exponential backoff.  When the connection comes back:
///   - Probe returns `LIVE` → old children are reaped, pid+dir are re-resolved, new
///     writer+reader children are spawned, TranscriptBackfill is re-run, and the read
///     loop resumes — all without any user action.
///   - Probe returns `DEAD` → claude truly exited (turn done / crashed) → normal
///     teardown, same as before.
///
/// Total retry time is capped at `RECONNECT_MAX_TOTAL_SECS` so a permanently-gone
/// host doesn't leak a thread forever.
#[cfg(not(coverage))]
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_session_reader(
    sid: String,
    host_target: Option<String>,
    first_prompt: Option<String>,
    send_init: bool,
    run_command: Option<String>,
    transcript_hint: Option<(String, Option<String>)>,
    data_root: String,
    subs_fifo: std::sync::Arc<std::sync::Mutex<Vec<std::sync::mpsc::Sender<String>>>>,
    writers_fifo: std::sync::Arc<std::sync::Mutex<HashMap<String, SharedWriter>>>,
    pending_fifo: std::sync::Arc<std::sync::Mutex<HashMap<String, PendingGate>>>,
    children: std::sync::Arc<std::sync::Mutex<HashMap<String, std::process::Child>>>,
    // U1 (§3.2 trigger 4): passive claude-model observation. `None` for
    // reconnect paths that don't carry the Arc (never actually passed as
    // None today — kept Option so a future caller without a provider handy
    // degrades to a no-op rather than a forced dependency).
    capability_provider: Option<std::sync::Arc<crate::capabilities::CapabilityProvider>>,
    // Stage 2 codex: driver discriminator ("codex" or "claude").
    driver: String,
    // Stage 2 codex: cached spec for building codex_turn_cmd (first_prompt delivery
    // and translate-and-broadcast). None for claude sessions.
    codex_spec: Option<crate::CachedCodexSpec>,
    // Stage 2 codex: shared map for storing the codex thread_id from thread.started.
    // Shared with handle_send_prompt so follow-up turns use codex exec resume.
    codex_threads: std::sync::Arc<std::sync::Mutex<HashMap<String, String>>>,
) {
    // `run_command` is only used when the caller signals that a launch script
    // was not yet run (i.e. not provided to spawn_container).  For the durable
    // model, it's not needed here — the launch script already ran.
    let _ = run_command;

    let session_dir = std::path::PathBuf::from(&data_root)
        .join("sessions")
        .join(format!("marsad-{sid}"));
    std::thread::spawn(move || {
        use marsad_transport::Transport;

        // For REMOTE sessions: save the project path so the read loop can probe
        // git after each result event. We extract it from transcript_hint when
        // available; otherwise we have no project path to probe (skip silently).
        // U1 (§3.2 trigger 4): the capability-provider host key for this
        // session — "local" when host_target is unset, mirroring the
        // provider's own local/remote key convention (capabilities.rs).
        let capability_host_id = host_target.clone().unwrap_or_else(|| "local".to_string());

        let remote_project: Option<String> = if host_target.is_some() {
            transcript_hint.as_ref().map(|(p, _)| p.clone())
        } else {
            None
        };

        // ── Supervising reconnect loop ────────────────────────────────────────
        // For LOCAL sessions this loop runs exactly once (no reconnect logic).
        // For REMOTE sessions it may run multiple times: after the stream ends
        // we probe liveness and re-attach if the connection dropped but claude
        // is still alive on the remote.
        let mut is_first_attach = true;

        // Backoff state (remote reconnect only).
        let mut backoff_secs = RECONNECT_INITIAL_BACKOFF_SECS;
        let mut total_waited_secs: u64 = 0;

        'reconnect: loop {
            // ── Open and connect the transport ─────────────
            // Local session → platform FIFO/pipe transport
            // (tmux+FIFO on Unix, named pipe on Windows), yielding
            // a `SplitIo` (boxed writer + buffered boxed reader).
            //
            // Remote session (host_target resolves to an exec prefix) → DURABLE model:
            //   • Writer child: ssh/wsl running `tee -a in.fifo` (stdin piped from agent).
            //   • Reader child: ssh/wsl running `tail -n0 --pid=<cl> -f out.log` (stdout piped).
            // The writer child is stored in `children` so it can be reaped on exit.
            // The reader child is owned entirely by this thread.
            let (writer, mut reader): marsad_transport::SplitIo = match exec_prefix_for(
                host_target.as_deref(),
            ) {
                Some(prefix) => {
                    use std::process::{Command, Stdio};

                    // Resolve the session's ABSOLUTE dir + detached claude pid over
                    // the reliable stdin-pipe path, so the writer/reader commands can
                    // be built from literals (no $HOME/$()/quotes that Rust→wsl.exe/
                    // ssh.exe would mangle). If claude isn't running, bail — the
                    // session reader has nothing to attach to.
                    let host = host_target.clone().unwrap_or_default();
                    let (claude_pid, abs_dir) = match remote_resolve_pid_dir(&host, &sid) {
                        Some(pd) => pd,
                        None => {
                            eprintln!(
                                    "marsad-agent: session {sid} remote resolve failed (claude not running?) — not attaching"
                                );
                            break 'reconnect;
                        }
                    };

                    // ── Spawn writer child ─────────────────────────────────────
                    // Appends everything written to its stdin into the remote FIFO.
                    let writer_argv = remote_writer_argv(&prefix, &abs_dir);
                    let w_bin = resolve_remote_bin(&writer_argv[0]);
                    let mut w_cmd = Command::new(&w_bin);
                    w_cmd
                        .args(&writer_argv[1..])
                        .stdin(Stdio::piped())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null());
                    #[cfg(windows)]
                    {
                        use std::os::windows::process::CommandExt;
                        w_cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
                    }
                    let mut writer_child = match w_cmd.spawn() {
                        Ok(c) => c,
                        Err(e) => {
                            eprintln!("marsad-agent: remote writer spawn {sid}: {e}");
                            break 'reconnect;
                        }
                    };
                    let child_stdin = match writer_child.stdin.take() {
                        Some(s) => s,
                        None => {
                            eprintln!("marsad-agent: remote writer child {sid} has no stdin");
                            let _ = writer_child.kill();
                            let _ = writer_child.wait();
                            break 'reconnect;
                        }
                    };
                    // Store writer child so it is reaped on thread exit.
                    children.lock().unwrap().insert(sid.clone(), writer_child);

                    // ── Spawn reader child ─────────────────────────────────────
                    // Streams only NEW out.log lines (tail -n0 = skip historical).
                    let reader_argv = remote_reader_argv(&prefix, &claude_pid, &abs_dir);
                    let r_bin = resolve_remote_bin(&reader_argv[0]);
                    let mut r_cmd = Command::new(&r_bin);
                    r_cmd
                        .args(&reader_argv[1..])
                        .stdin(Stdio::null())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped());
                    #[cfg(windows)]
                    {
                        use std::os::windows::process::CommandExt;
                        r_cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
                    }
                    let mut reader_child = match r_cmd.spawn() {
                        Ok(c) => c,
                        Err(e) => {
                            eprintln!("marsad-agent: remote reader spawn {sid}: {e}");
                            // Clean up writer child we already stored.
                            if let Some(mut wc) = children.lock().unwrap().remove(&sid) {
                                let _ = wc.kill();
                                let _ = wc.wait();
                            }
                            break 'reconnect;
                        }
                    };
                    // Stream reader's stderr to agent.log for diagnosis.
                    if let Some(errpipe) = reader_child.stderr.take() {
                        let esid = sid.clone();
                        std::thread::spawn(move || {
                            use std::io::BufRead;
                            let r = std::io::BufReader::new(errpipe);
                            for line in r.lines().map_while(Result::ok) {
                                if !line.trim().is_empty() {
                                    eprintln!("marsad-agent: session {esid} reader-stderr: {line}");
                                }
                            }
                        });
                    }
                    let child_stdout = match reader_child.stdout.take() {
                        Some(s) => s,
                        None => {
                            eprintln!("marsad-agent: remote reader child {sid} has no stdout");
                            let _ = reader_child.kill();
                            let _ = reader_child.wait();
                            if let Some(mut wc) = children.lock().unwrap().remove(&sid) {
                                let _ = wc.kill();
                                let _ = wc.wait();
                            }
                            break 'reconnect;
                        }
                    };
                    // Reader child: store in the session_children registry under a
                    // distinct key so the read-loop cleanup can reap it.  We use a
                    // `"reader:<sid>"` key (never used by other code) so the normal
                    // kill path (keyed by plain sid) is unaffected.
                    let reader_key = format!("reader:{sid}");
                    children.lock().unwrap().insert(reader_key, reader_child);

                    let w: Box<dyn std::io::Write + Send> = Box::new(child_stdin);
                    let r: std::io::BufReader<Box<dyn std::io::Read + Send>> =
                        std::io::BufReader::new(Box::new(child_stdout));
                    (w, r)
                }
                None => {
                    #[cfg(unix)]
                    {
                        if driver == "codex" {
                            // Codex eval-loop transport: open in.fifo for writing
                            // (prompt delivery) and out.log (a regular file) for
                            // reading (codex JSONL output).
                            //
                            // We cannot use ClaudeFifoTransport here because the
                            // eval-loop writes to out.log (a regular file) only
                            // when a command runs — so opening out.fifo for reading
                            // would deadlock (no writer appears until the first
                            // codex command, which requires connect() to have
                            // already completed). Using a regular file avoids
                            // the circular wait entirely.
                            //
                            // Open in.fifo for writing: blocks until the tmux eval-loop
                            // shell opens it for reading (which it does immediately on
                            // the `cat in.fifo |` side). We wait for the session dir
                            // to appear (tmux creates it when the session starts).
                            let in_fifo = session_dir.join("in.fifo");
                            let out_log = session_dir.join("out.log");

                            // Wait up to 15s for in.fifo to appear (tmux creates it
                            // when the session starts).
                            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
                            loop {
                                if in_fifo.exists() {
                                    break;
                                }
                                if std::time::Instant::now() >= deadline {
                                    eprintln!("marsad-agent: codex session {sid}: in.fifo not created within 15s");
                                    break 'reconnect;
                                }
                                std::thread::sleep(std::time::Duration::from_millis(100));
                            }

                            // Open in.fifo for writing (non-blocking: the cat in the
                            // eval-loop is already waiting as a reader).
                            let in_file = match std::fs::OpenOptions::new()
                                .write(true)
                                .open(&in_fifo)
                            {
                                Ok(f) => f,
                                Err(e) => {
                                    eprintln!("marsad-agent: codex session {sid}: open in.fifo: {e}");
                                    break 'reconnect;
                                }
                            };

                            // Ensure out.log exists (create it if not yet).
                            if !out_log.exists() {
                                let _ = std::fs::File::create(&out_log);
                            }

                            // Open out.log for reading. Since it's a regular file, this
                            // never blocks.
                            let out_file = match std::fs::File::open(&out_log) {
                                Ok(f) => f,
                                Err(e) => {
                                    eprintln!("marsad-agent: codex session {sid}: open out.log: {e}");
                                    break 'reconnect;
                                }
                            };

                            eprintln!("marsad-agent: session {sid} codex transport connected (in.fifo + out.log)");
                            let w: Box<dyn std::io::Write + Send> = Box::new(in_file);
                            let r: std::io::BufReader<Box<dyn std::io::Read + Send>> =
                                std::io::BufReader::new(Box::new(out_file));
                            (w, r)
                        } else {
                            use marsad_transport::ClaudeFifoTransport;
                            let mut fifo = match ClaudeFifoTransport::open(&session_dir) {
                                Ok(f) => f,
                                Err(e) => {
                                    eprintln!("marsad-agent: fifo open {sid}: {e}");
                                    break 'reconnect;
                                }
                            };
                            if let Err(e) = fifo.connect() {
                                eprintln!("marsad-agent: fifo connect {sid}: {e}");
                                break 'reconnect;
                            }
                            match fifo.into_split() {
                                Some(pair) => pair,
                                None => {
                                    eprintln!("marsad-agent: fifo not connected after connect {sid}");
                                    break 'reconnect;
                                }
                            }
                        }
                    }
                    #[cfg(windows)]
                    {
                        let _ = &session_dir;
                        use marsad_transport::ClaudeNamedPipeTransport;
                        let container_name = format!("marsad-{sid}");
                        let mut transport = ClaudeNamedPipeTransport::for_session(&container_name);
                        if let Err(e) = transport.connect() {
                            eprintln!("marsad-agent: pipe connect {sid}: {e}");
                            break 'reconnect;
                        }
                        match transport.into_split() {
                            Some(pair) => pair,
                            None => {
                                eprintln!("marsad-agent: pipe not connected after connect {sid}");
                                break 'reconnect;
                            }
                        }
                    }
                }
            };

            // The writer is shared (Arc<Mutex>) between this
            // thread (initialize handshake + first prompt) and
            // the serve thread (handle_send_prompt / answer_gate)
            // — boxed writers can't be try_clone'd like a File.
            let writer: SharedWriter = std::sync::Arc::new(std::sync::Mutex::new(writer));

            // Send the control-protocol `initialize` handshake first (LAUNCH only,
            // first attach only, CLAUDE ONLY). It pairs with `--permission-prompt-tool stdio` so
            // claude routes tool-permission prompts back as `can_use_tool`
            // control_requests. On RECONNECT the remote claude is already running +
            // initialized, so re-sending it would be wrong — hence `send_init` and
            // `is_first_attach`. Fire-and-forget: the CLI's ack is ignored.
            // CODEX: no initialize handshake — codex exec uses no control protocol.
            if send_init && is_first_attach && driver != "codex" {
                use std::io::Write;
                let init = marsad_proto::encode_control_request_initialize("req_1_init");
                let mut w = writer.lock().unwrap();
                let _ = w.write_all(init.as_bytes());
                let _ = w.write_all(b"\n");
                let _ = w.flush();
                eprintln!("marsad-agent: session {sid} sent initialize handshake");
            }

            // Deliver the first_prompt (if any) immediately so the process receives its
            // initial task.  Only on the very first attach — on reconnect the process is
            // already running (a prompt was already sent) and re-sending would be wrong.
            //
            // CLAUDE: delivers as a stream-json user message on stdin.
            // CODEX: delivers as a `codex_turn_cmd` eval-loop command on in.fifo;
            //        the eval-loop container spawns `codex exec …` as a subprocess.
            if is_first_attach {
                if let Some(ref prompt) = first_prompt {
                    use std::io::Write;
                    let plen = prompt.len();

                    if driver == "codex" {
                        // Codex first turn: build the eval-loop command and write it.
                        if let Some(ref cs) = codex_spec {
                            // Build a LaunchSpec-like value for codex_turn_cmd.
                            // For first_prompt, thread_id is None (fresh turn).
                            let spec_for_cmd = marsad_proto::LaunchSpec {
                                project: cs.project.clone(),
                                host_id: "local".to_string(),
                                host_target: None,
                                claude_bin: cs.claude_bin.clone(),
                                setup: None,
                                env: vec![],
                                model: cs.model.clone(),
                                posture: cs.posture.clone(),
                                first_prompt: None,
                                mode: marsad_proto::LaunchMode::Fresh,
                                name: None,
                                disallowed_tools: vec![],
                                wasl: false,
                                driver: "codex".to_string(),
                            };
                            let cmd = crate::driver::codex_turn_cmd(&spec_for_cmd, None, prompt);
                            let mut w = writer.lock().unwrap();
                            let _ = w.write_all(cmd.as_bytes());
                            let _ = w.write_all(b"\n");
                            let _ = w.flush();
                            eprintln!(
                                "marsad-agent: session {sid} codex delivered first turn cmd ({plen} chars)"
                            );
                        } else {
                            eprintln!(
                                "marsad-agent: session {sid} codex has no cached spec — first_prompt dropped"
                            );
                        }
                    } else {
                        // Claude: stream-json user message.
                        use marsad_proto::{encode_user_input, ContentBlock, UserInput};
                        let encoded = encode_user_input(&UserInput {
                            content: vec![ContentBlock::Text {
                                text: prompt.clone(),
                            }],
                        });
                        let mut w = writer.lock().unwrap();
                        let _ = w.write_all(encoded.as_bytes());
                        let _ = w.write_all(b"\n");
                        let _ = w.flush();
                        eprintln!("marsad-agent: session {sid} delivered first_prompt ({plen} chars) to claude stdin");
                    }
                } else {
                    eprintln!("marsad-agent: session {sid} no first_prompt — process idle until a prompt is sent (phase stays Starting)");
                }
            }

            // Register the shared writer so handle_send_prompt
            // / handle_answer_gate can deliver future prompts and
            // gate responses to this session's stdin. Both sides
            // hold an Arc to the same Mutex<Box<dyn Write>>.
            {
                let mut registry = writers_fifo.lock().unwrap();
                registry.insert(sid.clone(), std::sync::Arc::clone(&writer));
            }

            // For a REMOTE resume/reconnect, fetch the prior conversation from the
            // remote host's `.jsonl` over a fresh connection (which reuses the
            // session's ControlMaster / plink-share, so no extra MFA), and broadcast
            // it as a `TranscriptBackfill` so core can show the history. The remote
            // file is unreachable by core's local JSONL reader, so this is the only
            // way a resumed WSL/SSH session shows its past turns. Done before the read
            // loop so the backfill precedes any live events.
            // On reconnect after a connection drop (is_first_attach == false) we do
            // NOT re-run the backfill: the transcript already streamed; re-running
            // it would duplicate all previous turns in the UI.
            if is_first_attach {
                if let Some((project, uuid)) = &transcript_hint {
                    if let Some(target) = host_target.as_deref() {
                        let script = remote_transcript_script(project, uuid.as_deref());
                        // run_remote_script pipes the script via stdin to `bash -l -s` —
                        // login shell (nested wsl.exe keeps $HOME), no argv quoting, and
                        // CREATE_NO_WINDOW so ssh.exe doesn't flash a console.
                        if let Some(jsonl) = run_remote_script(target, &script) {
                            if !jsonl.trim().is_empty() {
                                let line = marsad_proto::encode_ipc(
                                    &marsad_proto::IpcMsg::TranscriptBackfill {
                                        session_id: sid.clone(),
                                        jsonl: jsonl.clone(),
                                    },
                                );
                                let subs = subs_fifo.lock().unwrap();
                                for s in subs.iter() {
                                    let _ = s.send(line.clone());
                                }
                                eprintln!(
                                    "marsad-agent: session {sid} transcript backfill ({} bytes) broadcast",
                                    jsonl.len()
                                );
                            } else {
                                eprintln!("marsad-agent: session {sid} transcript backfill: remote .jsonl empty/absent");
                            }
                        }
                    }
                }
            }

            // For a REMOTE first-attach, probe full-project usage AFTER the transcript
            // backfill so the result supersedes the partial per-session value the
            // backfill set.  Best-effort: if the output is empty or the script fails,
            // broadcast nothing (do not send an empty UsageUpdate — the empty-guard in
            // core would no-op it, but skipping the broadcast is cleaner and avoids
            // the network round-trip).
            if is_first_attach {
                if let (Some(target), Some(project)) =
                    (host_target.as_deref(), remote_project.as_deref())
                {
                    let usage_script = remote_usage_script(project);
                    if let Some(usage_out) = run_remote_script(target, &usage_script) {
                        let trimmed = usage_out.trim();
                        if !trimmed.is_empty() && trimmed != "{}" {
                            if let Ok(usage_val) =
                                serde_json::from_str::<serde_json::Value>(trimmed)
                            {
                                let uu_line =
                                    marsad_proto::encode_ipc(&marsad_proto::IpcMsg::UsageUpdate {
                                        session_id: sid.clone(),
                                        usage: usage_val,
                                    });
                                let subs = subs_fifo.lock().unwrap();
                                subs.iter().for_each(|s| {
                                    let _ = s.send(uu_line.clone());
                                });
                                eprintln!(
                                    "marsad-agent: session {sid} usage update broadcast (attach)"
                                );
                            }
                        }
                    }
                }
            }

            is_first_attach = false;

            // Read out.log tail lines and broadcast to all subscribers. The reader
            // blocks here; the writer clone is in the session_writers registry for
            // handle_send_prompt — no lock contention.
            eprintln!("marsad-agent: session {sid} stream connected");
            use std::io::BufRead;
            let mut lines_seen = 0u64;
            // Debounce tracker for the tool_use-triggered Changes probe (#4).
            // Stores the Instant of the last probe so rapid edits don't spam ssh
            // (at most one probe per CHANGES_PROBE_DEBOUNCE_SECS per session).
            let mut last_changes_probe: Option<std::time::Instant> = None;
            const CHANGES_PROBE_DEBOUNCE_SECS: u64 = 3;
            loop {
                let mut raw = String::new();
                match reader.read_line(&mut raw) {
                    Ok(0) => {
                        // For codex sessions reading from out.log (a regular file),
                        // EOF means "no more data right now" — the eval-loop may be
                        // between turns, so we poll-sleep rather than breaking.
                        // We detect session liveness by checking if in.fifo still exists
                        // (it disappears when the tmux session exits/eval-loop ends).
                        // For non-codex (FIFO/pipe) sessions, EOF always means done.
                        if driver == "codex" {
                            let in_fifo = session_dir.join("in.fifo");
                            if in_fifo.exists() {
                                // Eval-loop still running, waiting for next turn.
                                std::thread::sleep(std::time::Duration::from_millis(200));
                                continue;
                            }
                            // Eval-loop exited (in.fifo gone): real EOF.
                            eprintln!(
                                "marsad-agent: session {sid} codex reader done (eval-loop exited) after {lines_seen} line(s)"
                            );
                        } else {
                            eprintln!(
                                "marsad-agent: session {sid} reader EOF after {lines_seen} line(s)"
                            );
                        }
                        break;
                    }
                    Err(e) => {
                        eprintln!("marsad-agent: session {sid} reader ERROR after {lines_seen} line(s): {e}");
                        break;
                    }
                    Ok(_) => {
                        lines_seen += 1;
                        let raw = raw.trim_end_matches('\n');

                        // Capture a `can_use_tool` control
                        // request as a pending gate so a later
                        // AnswerGate can write the matching
                        // control_response. The event is still
                        // broadcast (below) so the GUI can
                        // render the gate and answer it.
                        if let Ok(marsad_proto::StreamMsg::ControlRequest(cr)) =
                            marsad_proto::parse_stream_line(raw)
                        {
                            let mut gates = pending_fifo.lock().unwrap();
                            // Deduplicate: if the SAME request_id already
                            // sits in the registry (claude re-emitted it),
                            // ignore the duplicate so we don't clobber the
                            // existing entry and misroute a later AnswerGate.
                            // If it's a genuinely NEW request_id while one
                            // is already pending, replace it (the first one
                            // was either answered or abandoned) and warn.
                            let dup = gates
                                .get(&sid)
                                .map(|existing| existing.request_id == cr.request_id)
                                .unwrap_or(false);
                            if dup {
                                eprintln!(
                                    "marsad-agent: session {sid} gate DUPLICATE ignored (control_request {})",
                                    cr.request_id
                                );
                            } else {
                                if gates.contains_key(&sid) {
                                    eprintln!(
                                        "marsad-agent: session {sid} gate REPLACED — new control_request {} (previous unanswered)",
                                        cr.request_id
                                    );
                                } else {
                                    eprintln!(
                                        "marsad-agent: session {sid} gate captured (control_request {})",
                                        cr.request_id
                                    );
                                }
                                gates.insert(
                                    sid.clone(),
                                    PendingGate {
                                        request_id: cr.request_id,
                                        input: cr.input,
                                        elicitation: cr.elicitation_message.is_some(),
                                    },
                                );
                            }
                        }

                        let raw_value: serde_json::Value = serde_json::from_str(raw)
                            .unwrap_or_else(|_| serde_json::json!({"raw": raw}));

                        // Stage 2 codex: translate the raw codex JSONL event into
                        // a claude-stream-json Value. If translation yields None
                        // (skip), do NOT broadcast this line (e.g. turn.started,
                        // reasoning items). For thread.started, also stash the
                        // thread_id in codex_threads so follow-up turns resume.
                        let event: serde_json::Value = if driver == "codex" {
                            // Stash thread_id for follow-up turns.
                            if raw_value.get("type").and_then(|t| t.as_str()) == Some("thread.started") {
                                if let Some(tid) = raw_value.get("thread_id").and_then(|v| v.as_str()) {
                                    codex_threads.lock().unwrap().insert(sid.clone(), tid.to_string());
                                    eprintln!(
                                        "marsad-agent: session {sid} codex thread_id={tid} stored"
                                    );
                                }
                            }
                            // Translate to claude-stream-json shape.
                            if let Some(cs) = codex_spec.as_ref() {
                                match crate::codex::codex_event_to_stream_json(
                                    &raw_value,
                                    &cs.model,
                                    &cs.project,
                                    &cs.posture_mode,
                                ) {
                                    Some(translated) => translated,
                                    None => continue, // skip — do not broadcast
                                }
                            } else {
                                // No spec cached — broadcast raw (fallback, shouldn't happen).
                                raw_value
                            }
                        } else {
                            raw_value
                        };

                        // U1 (§3.2 trigger 4): a `system:init` line reports the
                        // model claude actually launched with — the only signal
                        // that exists for claude's model catalog (§1.1: "no list
                        // command"). Accumulate it into the host's observed set.
                        // Also triggered for codex sessions (the synthesized system/init
                        // from thread.started carries the configured model name).
                        if event.get("type").and_then(|t| t.as_str()) == Some("system")
                            && event.get("subtype").and_then(|t| t.as_str()) == Some("init")
                        {
                            if let (Some(provider), Some(model)) = (
                                capability_provider.as_ref(),
                                event.get("model").and_then(|m| m.as_str()),
                            ) {
                                provider.observe_claude_model(&capability_host_id, model);
                            }
                        }

                        // Determine whether this event should trigger a
                        // remote git Changes probe.
                        //
                        // Trigger 1 (existing): `result` event — turn ended.
                        // Trigger 2 (new, #4): `assistant` event containing a
                        // `tool_use` block for a file-modifying tool (Bash,
                        // Edit, Write, NotebookEdit, MultiEdit).  Debounced:
                        // at most one probe per CHANGES_PROBE_DEBOUNCE_SECS.
                        let is_result =
                            event.get("type").and_then(|t| t.as_str()) == Some("result");
                        let is_file_modifying_tool_use = !is_result
                            && event.get("type").and_then(|t| t.as_str()) == Some("assistant")
                            && assistant_has_file_modifying_tool_use(&event);
                        let should_probe_changes = is_result
                            || (is_file_modifying_tool_use && {
                                let now = std::time::Instant::now();
                                let elapsed_ok = last_changes_probe
                                    .map(|t| {
                                        now.duration_since(t).as_secs()
                                            >= CHANGES_PROBE_DEBOUNCE_SECS
                                    })
                                    .unwrap_or(true);
                                if elapsed_ok {
                                    last_changes_probe = Some(now);
                                }
                                elapsed_ok
                            });
                        if should_probe_changes {
                            if let (Some(target), Some(project)) =
                                (host_target.as_deref(), remote_project.as_deref())
                            {
                                let script = remote_changes_script(project);
                                if let Some(stdout) = run_remote_script(target, &script) {
                                    let changes = parse_remote_changes(&stdout);
                                    let cu_line = marsad_proto::encode_ipc(
                                        &marsad_proto::IpcMsg::ChangesUpdate {
                                            session_id: sid.clone(),
                                            changes,
                                        },
                                    );
                                    let subs2 = subs_fifo.lock().unwrap();
                                    subs2.iter().for_each(|s| {
                                        let _ = s.send(cu_line.clone());
                                    });
                                    eprintln!(
                                        "marsad-agent: session {sid} changes update broadcast ({})",
                                        if is_result { "result" } else { "tool_use" }
                                    );
                                }
                                // Project-wide usage aggregation: run on result events
                                // only (not on every tool_use — it reads all *.jsonl,
                                // so it's more expensive than the git probe).
                                if is_result {
                                    let usage_script = remote_usage_script(project);
                                    if let Some(usage_out) =
                                        run_remote_script(target, &usage_script)
                                    {
                                        let trimmed = usage_out.trim();
                                        if !trimmed.is_empty() && trimmed != "{}" {
                                            if let Ok(usage_val) =
                                                serde_json::from_str::<serde_json::Value>(trimmed)
                                            {
                                                let uu_line = marsad_proto::encode_ipc(
                                                    &marsad_proto::IpcMsg::UsageUpdate {
                                                        session_id: sid.clone(),
                                                        usage: usage_val,
                                                    },
                                                );
                                                let subs3 = subs_fifo.lock().unwrap();
                                                subs3.iter().for_each(|s| {
                                                    let _ = s.send(uu_line.clone());
                                                });
                                                eprintln!(
                                                    "marsad-agent: session {sid} usage update broadcast"
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        let ipc_msg = marsad_proto::IpcMsg::AgentEvent {
                            session_id: sid.clone(),
                            event,
                        };
                        let encoded = marsad_proto::encode_ipc(&ipc_msg);
                        // Broadcast to all subscribers.
                        let subs = subs_fifo.lock().unwrap();
                        subs.iter().for_each(|s| {
                            let _ = s.send(encoded.clone());
                        });
                    }
                }
            }

            eprintln!("marsad-agent: session {sid} stream ended after {lines_seen} line(s)");
            // If claude produced NO output on a remote session, run a diagnosis so the
            // agent.log gives a clue (PATH, module, etc.).
            if lines_seen == 0 {
                if let Some(target) = host_target.as_deref() {
                    // Dump claude's actual out.log — claude's stderr/error is redirected
                    // there (start.sh: `>> out.log 2>&1`), but the reader's `tail -n0`
                    // skips pre-existing content, so a startup error (bad flag, crash,
                    // `--continue` with no session, etc.) never reaches the stream. This
                    // surfaces the REAL reason claude exited, plus a PATH check.
                    let dir = remote_session_dir(&sid);
                    let script = format!(
                        "echo '== claude =='; command -v claude 2>/dev/null || echo 'claude NOT on PATH'; \
                         echo '== out.log (claude startup output/error) =='; \
                         tail -c 2000 {dir}/out.log 2>/dev/null"
                    );
                    if let Some(out) = run_remote_script(target, &script) {
                        eprintln!(
                            "marsad-agent: session {sid} remote diagnosis:\n{}",
                            out.trim()
                        );
                    }
                }
            }

            // ── Teardown for this connection attempt ─────────────────────────
            // Remove writer from registry and reap children.  For remote sessions
            // we may re-attach below (reconnect loop), so the registry slot will be
            // re-inserted on the next iteration; for local sessions and definitive
            // exits the slot stays absent (correct).
            {
                let mut registry = writers_fifo.lock().unwrap();
                registry.remove(&sid);
            }
            drop(writer);
            // Reap the writer child (remote sessions: the `tee -a in.fifo` child).
            // Dropping the writer above closes the child's stdin; `wait()` reaps it.
            // Local sessions never insert into `children`, so this is a no-op for them.
            if let Some(mut child) = children.lock().unwrap().remove(&sid) {
                let _ = child.wait();
            }
            // Reap the reader child (remote sessions: the `tail -n0 -f out.log` child)
            // stored under the `"reader:<sid>"` key.
            {
                let reader_key = format!("reader:{sid}");
                if let Some(mut rchild) = children.lock().unwrap().remove(&reader_key) {
                    let _ = rchild.kill(); // tail -f never exits on its own; kill it.
                    let _ = rchild.wait();
                }
            }

            // ── Remote reconnect: probe liveness, retry or stop ──────────────
            // Only for REMOTE sessions.  Local sessions (host_target None) stop here.
            let host = match host_target.as_deref() {
                Some(h) if exec_prefix_for(Some(h)).is_some() => h,
                _ => break 'reconnect, // local session or no exec prefix → stop
            };

            eprintln!("marsad-agent: session {sid} connection lost, probing liveness");

            // Probe + backoff loop: keep probing until the connection comes back
            // (probe succeeds) or we exceed the total retry cap.
            loop {
                if total_waited_secs >= RECONNECT_MAX_TOTAL_SECS {
                    eprintln!(
                        "marsad-agent: session {sid} reconnect cap ({RECONNECT_MAX_TOTAL_SECS}s) reached — giving up"
                    );
                    break 'reconnect;
                }

                // Sleep before the next probe (always, to avoid busy-looping).
                eprintln!("marsad-agent: session {sid} still down, retrying in {backoff_secs}s");
                std::thread::sleep(std::time::Duration::from_secs(backoff_secs));
                total_waited_secs = total_waited_secs.saturating_add(backoff_secs);
                backoff_secs = reconnect_backoff_next(backoff_secs);

                // Bounded probe: if the connection is still down, the remote
                // `bash -s` can't even start → the spawn or read fails quickly,
                // returning None.  We treat None as "still down".
                let probe_result = run_remote_script_bounded(
                    host,
                    &remote_liveness_script(&sid),
                    std::time::Duration::from_secs(RECONNECT_PROBE_TIMEOUT_SECS),
                );

                match probe_result {
                    None => {
                        // Connection still down (probe failed / timed out).
                        // Continue the probe loop.
                    }
                    Some(out) => {
                        // Connection is back.  Interpret the result.
                        if out.trim() == "LIVE" {
                            eprintln!(
                                "marsad-agent: session {sid} reconnected live remote session"
                            );
                            // Reset backoff for next potential drop.
                            backoff_secs = RECONNECT_INITIAL_BACKOFF_SECS;
                            total_waited_secs = 0;
                            // Re-attach: continue the outer reconnect loop.
                            continue 'reconnect;
                        } else {
                            eprintln!("marsad-agent: session {sid} claude exited, stopping");
                            break 'reconnect;
                        }
                    }
                }
            } // probe loop
        } // 'reconnect loop

        // Final cleanup: ensure the writer registry slot is gone (it may already
        // be absent if the reconnect loop tore it down; the remove is idempotent).
        {
            let mut registry = writers_fifo.lock().unwrap();
            registry.remove(&sid);
        }
    });
}
