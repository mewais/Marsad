//! Per-session reader thread: connect the session transport (local FIFO/pipe or
//! a remote durable FIFO bridge), pump its NDJSON stream, and broadcast decoded
//! events to every subscriber — plus the small pure-logic helpers
//! (`reconnect_backoff_next`, `assistant_has_file_modifying_tool_use`, the
//! `RECONNECT_*` constants) that drive its remote auto-reconnect behavior.

#[cfg(not(coverage))]
use std::collections::HashMap;

#[cfg(not(coverage))]
use super::remote::{
    exec_prefix_for, parse_remote_changes, remote_changes_script, remote_reader_argv,
    remote_resolve_pid_dir, remote_session_dir, remote_transcript_script, remote_usage_script,
    remote_writer_argv, resolve_remote_bin, run_remote_script,
};
#[cfg(not(coverage))]
use super::{PendingGate, SharedWriter};
#[cfg(not(coverage))]
use std::sync::atomic::{AtomicBool, Ordering};

// ── Per-session reader cancellation ──────────────────────────────────────────
// A reader may sit in a steady reconnect-probe backoff after a connection drop
// (an unreachable durable session is probed indefinitely, presumed alive). If the
// user pauses the session, or a NEW reader is spawned for the same session
// (resume / reconnect
// on a flapping host), the OLD reader MUST exit promptly — otherwise readers
// stack up (observed 14-for-one on a flaky etx host), multiplying gate broadcasts
// and stream events into churn. `spawn_session_reader` registers a fresh token
// (tripping any prior one for that sid); `cancel_reader` (StopSession) trips it.

#[cfg(not(coverage))]
fn reader_cancels() -> &'static std::sync::Mutex<HashMap<String, std::sync::Arc<AtomicBool>>> {
    static CANCELS: std::sync::OnceLock<
        std::sync::Mutex<HashMap<String, std::sync::Arc<AtomicBool>>>,
    > = std::sync::OnceLock::new();
    CANCELS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Trip the current reader thread's cancel token for `sid` (StopSession/pause),
/// so a lingering reconnect-probe loop exits at its next check instead of stacking
/// with the reader a later Resume spawns.
#[cfg(not(coverage))]
pub(crate) fn cancel_reader(sid: &str) {
    if let Some(tok) = reader_cancels().lock().unwrap().get(sid) {
        tok.store(true, Ordering::SeqCst);
    }
}

// ── Reconnect backoff constants and helper ───────────────────────────────────

/// Initial delay before the first liveness re-probe after a connection drop (seconds).
pub const RECONNECT_INITIAL_BACKOFF_SECS: u64 = 3;

/// Maximum delay between liveness probes (seconds).
pub const RECONNECT_MAX_BACKOFF_SECS: u64 = 30;

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

// ── Connection-health state machine (pure, unit-tested) ──────────────────────
//
// The reader emits a `ConnHealth` on real stream events. These two pure
// functions are the WHOLE decision logic — no flap counter driving state at
// attach, no verdict-string guessing. The reader just calls them and emits.

/// What to do after a liveness probe during a reconnect, plus the health to emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeStep {
    /// Connection health to broadcast now (None = leave unchanged).
    pub emit: Option<marsad_proto::ConnHealth>,
    pub action: ProbeAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeAction {
    /// Re-attach to the (live) session.
    Reattach,
    /// Process is confirmed gone → finalize the session to Paused.
    Pause,
    /// Couldn't confirm — keep probing (host presumed still hosting the session).
    KeepProbing,
}

/// Health to emit when the stream DROPS. A drop right after a bare re-attach that
/// never produced a line (`lines_seen == 0`) is a FLAP → `Unstable`; a drop from a
/// connection that was actually working → a plain `Reconnecting` blip.
pub fn conn_on_drop(lines_seen: u64) -> marsad_proto::ConnHealth {
    if lines_seen == 0 {
        marsad_proto::ConnHealth::Unstable
    } else {
        marsad_proto::ConnHealth::Reconnecting
    }
}

/// Decide the next step from a probe verdict. `backoff_at_max` means we've been
/// retrying long enough that the backoff has saturated — a still-Unreachable host
/// at that point is a SUSTAINED outage → surface `Disconnected` (honest "lost
/// contact", session presumed alive) rather than an endless "Reconnecting".
/// Crucially, `Unreachable` NEVER pauses — only a definitive `Dead` does.
pub fn conn_on_probe(verdict: crate::remote::Liveness, backoff_at_max: bool) -> ProbeStep {
    use marsad_proto::ConnHealth;
    match verdict {
        crate::remote::Liveness::Live => ProbeStep {
            emit: None, // the re-attach emits Connected
            action: ProbeAction::Reattach,
        },
        crate::remote::Liveness::Dead => ProbeStep {
            emit: None, // Paused (live=false) clears conn_health in core
            action: ProbeAction::Pause,
        },
        crate::remote::Liveness::Unreachable => ProbeStep {
            emit: Some(if backoff_at_max {
                ConnHealth::Disconnected
            } else {
                ConnHealth::Reconnecting
            }),
            action: ProbeAction::KeepProbing,
        },
    }
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
///     teardown (session finalized to Paused).
///   - Probe returns `Unreachable` → the host can't be reached; the durable session
///     is PRESUMED ALIVE, so the reader keeps probing at the steady backoff and
///     surfaces `Disconnected` (never a false Paused), flipping back to Connected
///     the moment the host answers LIVE. The thread exits only on cancel/stop or DEAD.
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
    // Prompts queued (by handle_send_prompt) while this session had no registered
    // writer — flushed in order right after the writer registers below.
    pending_prompts: super::PendingPrompts,
    pending_fifo: std::sync::Arc<
        std::sync::Mutex<HashMap<String, std::collections::VecDeque<PendingGate>>>,
    >,
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
    // opencode ACP: cached spec for the ACP handshake (initialize + session/new).
    // None for non-opencode sessions.
    opencode_spec: Option<crate::CachedOpencodeSpec>,
    // opencode ACP: shared map for storing the ACP sessionId from the session/new
    // (or session/load) result. Shared with handle_send_prompt / handle_interrupt /
    // handle_answer_gate so they can address subsequent ACP requests.
    acp_sessions: std::sync::Arc<std::sync::Mutex<HashMap<String, String>>>,
) {
    // `run_command` is only used when the caller signals that a launch script
    // was not yet run (i.e. not provided to spawn_container).  For the durable
    // model, it's not needed here — the launch script already ran.
    let _ = run_command;

    // Register a fresh cancellation token for this session, tripping any prior
    // reader's token so a stale reconnect-probe loop (from a flapping host or a
    // prior resume) exits promptly instead of stacking with this new reader.
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    {
        let mut m = reader_cancels().lock().unwrap();
        if let Some(old) = m.insert(sid.clone(), cancel.clone()) {
            old.store(true, Ordering::SeqCst);
        }
    }

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

        // Per-session CONNECTION health (distinct from liveness). Emitted on REAL
        // stream events via the pure state machine (`conn_on_drop`/`conn_on_probe`):
        // attach → Connected; drop → Reconnecting/Unstable; sustained unreachable →
        // Disconnected; DEAD → Paused. The UI uses it to show the true state and
        // HARD-BLOCK sending into a broken pipe.
        let emit_conn = |health: marsad_proto::ConnHealth| {
            let line = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::SessionConn {
                session_id: sid.clone(),
                health,
            });
            if let Ok(subs) = subs_fifo.lock() {
                for s in subs.iter() {
                    let _ = s.send(line.clone());
                }
            }
        };

        'reconnect: loop {
            // Superseded (a newer reader took over) or stopped (paused) → exit now,
            // don't attach/re-probe.
            if cancel.load(Ordering::SeqCst) {
                break 'reconnect;
            }
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
                    // Retry the resolve: on a host with heavy setup (e.g. `source
                    // ~/module.sh` on an HPC login node) claude can take many seconds
                    // to be up after the durable launch returns. A single resolve
                    // right after the launch's `sleep 0.3` loses that race and wrongly
                    // reports "claude not running" → the session sticks at Starting.
                    // Poll for a few seconds before giving up.
                    let resolved = {
                        let mut r = None;
                        for attempt in 0..12 {
                            if let Some(pd) = remote_resolve_pid_dir(&host, &sid) {
                                r = Some(pd);
                                break;
                            }
                            if attempt == 0 {
                                eprintln!(
                                    "marsad-agent: session {sid} claude not up yet — polling (slow remote setup?)"
                                );
                            }
                            std::thread::sleep(std::time::Duration::from_millis(600));
                        }
                        r
                    };
                    let (claude_pid, abs_dir) = match resolved {
                        Some(pd) => pd,
                        None => {
                            // Fetch the remote out.log tail so a genuine crash (claude
                            // not on PATH, setup error) is visible instead of a bare
                            // "not running".
                            let dir = remote_session_dir(&sid);
                            let log_tail = run_remote_script(
                                &host,
                                &format!("dir={dir}\ntail -c 4000 \"$dir/out.log\" 2>/dev/null"),
                            )
                            .unwrap_or_default();
                            eprintln!(
                                "marsad-agent: session {sid} remote resolve failed after retries (claude not running?) — not attaching. out.log tail:\n{log_tail}"
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
                    // Streams NEW out.log lines (tail -n0), EXCEPT an opencode ACP
                    // session's first attach, which must read from the start so the
                    // one-shot initialize/session/new responses (the ACP sessionId)
                    // are never missed by a late-attaching remote tail (see
                    // remote_reader_argv). Reconnects stay -n0 (no replay; sessionId
                    // already known).
                    let read_from_start = is_first_attach && driver == "opencode";
                    let reader_argv =
                        remote_reader_argv(&prefix, &claude_pid, &abs_dir, read_from_start);
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
                        if driver == "codex" || driver == "opencode" {
                            // Codex eval-loop transport AND opencode ACP persistent
                            // process: both write to out.log (a regular file) rather
                            // than out.fifo, so we use the same in.fifo + out.log
                            // transport. For opencode, the process is persistent and
                            // writes ACP JSON-RPC lines continuously to out.log.
                            //
                            // We cannot use ClaudeFifoTransport here because opening
                            // out.fifo for reading would block until opencode's first
                            // write — which can't happen until after the ACP handshake,
                            // creating a deadlock. A regular file never blocks on open().
                            //
                            // Open in.fifo for writing: blocks until the tmux shell opens
                            // it for reading (which it does immediately on the `cat in.fifo |`
                            // side). We wait for the session dir to appear (tmux creates
                            // it when the session starts).
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
                                    eprintln!("marsad-agent: {driver} session {sid}: in.fifo not created within 15s");
                                    break 'reconnect;
                                }
                                std::thread::sleep(std::time::Duration::from_millis(100));
                            }

                            // Open in.fifo for writing (non-blocking: the cat in the
                            // shell is already waiting as a reader).
                            let in_file = match std::fs::OpenOptions::new()
                                .write(true)
                                .open(&in_fifo)
                            {
                                Ok(f) => f,
                                Err(e) => {
                                    eprintln!("marsad-agent: {driver} session {sid}: open in.fifo: {e}");
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
                                    eprintln!("marsad-agent: {driver} session {sid}: open out.log: {e}");
                                    break 'reconnect;
                                }
                            };

                            eprintln!("marsad-agent: session {sid} {driver} transport connected (in.fifo + out.log)");
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
            // OPENCODE: ACP handshake is sent below (initialize + session/new|session/load).
            if send_init && is_first_attach && driver != "codex" && driver != "opencode" {
                use std::io::Write;
                let init = marsad_proto::encode_control_request_initialize("req_1_init");
                let mut w = writer.lock().unwrap();
                let _ = w.write_all(init.as_bytes());
                let _ = w.write_all(b"\n");
                let _ = w.flush();
                eprintln!("marsad-agent: session {sid} sent initialize handshake");
            }

            // ── opencode ACP handshake (LAUNCH only, first attach only) ───────
            // For a fresh opencode session: send `initialize` then `session/new`
            // (or `session/load` if we have a stored ACP sessionId for resume).
            // On reconnect: the remote process is already running and in-session;
            // DO NOT re-send the handshake — we just re-attach to the stream.
            // The ACP request IDs used here are the SAME ids the response-tracking
            // map below (`pending_acp_reqs`) uses to correlate responses.
            if driver == "opencode" && send_init && is_first_attach {
                use std::io::Write;
                use crate::acp;

                // Monotonic request-id counter: use a thread-local AtomicU64 so
                // multiple reader threads (parallel sessions) never collide.
                // We start at 1 and increment — the reader loop below also uses
                // this counter for per-turn session/prompt requests.
                // We use simple numeric IDs: 1 = initialize, 2 = session/new|load.
                let init_id = serde_json::json!(1u64);
                let session_id_for_new = serde_json::json!(2u64);

                // Send `initialize`.
                let init_params = acp::initialize_params("marsad", env!("CARGO_PKG_VERSION"));
                let init_line = acp::encode_request(init_id.clone(), "initialize", init_params);
                {
                    let mut w = writer.lock().unwrap();
                    let _ = w.write_all(init_line.as_bytes());
                    let _ = w.write_all(b"\n");
                    let _ = w.flush();
                }
                eprintln!("marsad-agent: session {sid} opencode sent initialize");

                // Send `session/new` or `session/load` depending on whether we have
                // a stored ACP sessionId (resume path).
                let (session_method, session_params) = if let Some(ref spec) = opencode_spec {
                    let cwd = &spec.project;
                    let mcp_servers = serde_json::json!([]);
                    if let Some(ref acp_sid) = spec.acp_session_id {
                        // Resume: session/load with the stored ACP sessionId.
                        (
                            "session/load",
                            acp::session_load_params(acp_sid, cwd, &mcp_servers),
                        )
                    } else {
                        // Fresh: session/new.
                        (
                            "session/new",
                            acp::session_new_params(cwd, &mcp_servers, None),
                        )
                    }
                } else {
                    // No spec — fall back to session/new with empty cwd.
                    ("session/new", acp::session_new_params(".", &serde_json::json!([]), None))
                };

                let session_line = acp::encode_request(
                    session_id_for_new.clone(),
                    session_method,
                    session_params,
                );
                {
                    let mut w = writer.lock().unwrap();
                    let _ = w.write_all(session_line.as_bytes());
                    let _ = w.write_all(b"\n");
                    let _ = w.flush();
                }
                eprintln!("marsad-agent: session {sid} opencode sent {session_method}");
            }

            // Deliver the first_prompt (if any) immediately so the process receives its
            // initial task.  Only on the very first attach — on reconnect the process is
            // already running (a prompt was already sent) and re-sending would be wrong.
            //
            // CLAUDE: delivers as a stream-json user message on stdin.
            // CODEX: delivers as a `codex_turn_cmd` eval-loop command on in.fifo;
            //        the eval-loop container spawns `codex exec …` as a subprocess.
            // OPENCODE: first_prompt is delivered as a `session/prompt` ACP request
            //           AFTER the session/new result arrives (so the ACP sessionId is
            //           known). The read loop below handles this delivery on the
            //           session/new response. We skip the first-prompt delivery here
            //           for opencode.
            if is_first_attach && driver != "opencode" {
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

            // Flush any prompts that arrived while the writer wasn't registered yet
            // (queued by handle_send_prompt during this (re)attach window). Deliver
            // in order; `drain_fresh_prompts` drops stale ones (TTL) so a prompt
            // typed long before a manual resume isn't fired out of context. Held
            // under the pending_prompts lock so a concurrent send_prompt either
            // queues before the drain (delivered here) or finds the writer already
            // registered (delivered directly) — never lost, never duplicated.
            {
                let queued = { pending_prompts.lock().unwrap().remove(&sid).unwrap_or_default() };
                if !queued.is_empty() {
                    use std::io::Write;
                    let fresh = super::drain_fresh_prompts(queued, std::time::Instant::now());
                    let mut w = writer.lock().unwrap();
                    let mut delivered = 0usize;
                    for bytes in &fresh {
                        if w.write_all(bytes).and_then(|_| w.flush()).is_ok() {
                            delivered += 1;
                        }
                    }
                    if delivered > 0 {
                        eprintln!(
                            "marsad-agent: session {sid} flushed {delivered} queued prompt(s) on (re)attach"
                        );
                    }
                }
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
            // Two-axis lifecycle: attaching to a running session ⇒ Resumed. Tell
            // core authoritatively (it starts restored sessions Paused until this
            // lands). Emitted on every (re)connect so a post-drop reconnect
            // re-asserts liveness.
            {
                eprintln!("marsad-agent: session {sid} liveness -> LIVE (broadcast)");
                let line = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::SessionLiveness {
                    session_id: sid.clone(),
                    live: true,
                });
                let subs = subs_fifo.lock().unwrap();
                for s in subs.iter() {
                    let _ = s.send(line.clone());
                }
            }
            // A successful (re)attach means we have a clean stream RIGHT NOW →
            // Connected. This always clears any prior Reconnecting/Unstable/
            // Disconnected — self-healing, so an idle-after-flap session can never
            // get stuck on "Unstable".
            emit_conn(marsad_proto::ConnHealth::Connected);
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
                        // For codex/opencode sessions reading from out.log (a regular
                        // file), EOF means "no more data right now" — the process is
                        // still running (between ACP messages / turns), so we poll-sleep
                        // rather than breaking. We detect session liveness by checking
                        // if in.fifo still exists (it disappears when the tmux session
                        // exits). For non-codex/opencode (FIFO/pipe) sessions, EOF
                        // always means done.
                        if driver == "codex" || driver == "opencode" {
                            let in_fifo = session_dir.join("in.fifo");
                            if in_fifo.exists() {
                                // Process still running, waiting for more output.
                                std::thread::sleep(std::time::Duration::from_millis(200));
                                continue;
                            }
                            // Process exited (in.fifo gone): real EOF.
                            eprintln!(
                                "marsad-agent: session {sid} {driver} reader done (process exited) after {lines_seen} line(s)"
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
                            let queue = gates.entry(sid.clone()).or_default();
                            // Deduplicate by request_id ACROSS the whole queue:
                            // a flapping remote replays the same control_request
                            // on each reconnect, so ignore any request_id we're
                            // already holding rather than enqueuing a duplicate
                            // that would later be answered against a gate claude
                            // is no longer blocked on. A genuinely NEW request_id
                            // is pushed to the BACK — claude can block on a burst
                            // of gates in one turn, and each must be preserved and
                            // answered against its own request_id (no overwrite).
                            let dup = queue.iter().any(|g| g.request_id == cr.request_id);
                            if dup {
                                eprintln!(
                                    "marsad-agent: session {sid} gate DUPLICATE ignored (control_request {})",
                                    cr.request_id
                                );
                            } else {
                                eprintln!(
                                    "marsad-agent: session {sid} gate captured (control_request {}, {} now queued)",
                                    cr.request_id,
                                    queue.len() + 1
                                );
                                // DIAG (temporary): log EXACTLY what claude offers for
                                // this tool so gate options are truth-based, not guessed.
                                eprintln!(
                                    "marsad-agent: session {sid} GATE_DIAG tool={} elicitation={} suggestions={}",
                                    cr.tool_name,
                                    cr.elicitation_message.is_some(),
                                    serde_json::to_string(&cr.permission_suggestions)
                                        .unwrap_or_else(|_| "[]".to_string())
                                );
                                queue.push_back(PendingGate {
                                    request_id: cr.request_id,
                                    input: cr.input,
                                    elicitation: cr.elicitation_message.is_some(),
                                });
                            }
                        }

                        let raw_value: serde_json::Value = serde_json::from_str(raw)
                            .unwrap_or_else(|_| serde_json::json!({"raw": raw}));

                        // Driver dispatch: translate raw output line into a
                        // claude-stream-json Value.
                        //
                        // - "opencode": parse ACP JSON-RPC; notifications →
                        //   translate via acp_translate; request_permission →
                        //   capture as pending gate + broadcast; session/new result
                        //   → stash ACP sessionId.
                        // - "codex": translate JSONL via codex_event_to_stream_json.
                        // - everything else ("claude"): raw stream-json, no translation.
                        if driver == "opencode" {
                            use crate::acp;
                            use crate::acp_translate;

                            let acp_msg = match acp::parse_acp_line(raw) {
                                Ok(m) => m,
                                Err(e) => {
                                    eprintln!(
                                        "marsad-agent: session {sid} opencode ACP parse error: {e} (line: {raw})"
                                    );
                                    continue;
                                }
                            };

                            match acp_msg {
                                // ── Notification ──────────────────────────────────────────
                                acp::AcpMessage::Notification { method, params } => {
                                    if method == "session/update" {
                                        if let Some(update) = acp::extract_update(&params) {
                                            // Compaction detection: bump the boundary event.
                                            if acp_translate::detect_compaction(update) {
                                                eprintln!(
                                                    "marsad-agent: session {sid} opencode compaction detected"
                                                );
                                            }
                                            // Translate to stream-json; skip None (unknown types).
                                            let model_str = opencode_spec
                                                .as_ref()
                                                .map(|s| s.model.as_str())
                                                .unwrap_or("");
                                            if let Some(translated) =
                                                acp_translate::acp_update_to_stream_json(
                                                    update, &sid, model_str,
                                                )
                                            {
                                                let ipc_msg = marsad_proto::IpcMsg::AgentEvent {
                                                    session_id: sid.clone(),
                                                    event: translated,
                                                };
                                                let encoded = marsad_proto::encode_ipc(&ipc_msg);
                                                let subs = subs_fifo.lock().unwrap();
                                                subs.iter().for_each(|s| {
                                                    let _ = s.send(encoded.clone());
                                                });
                                            }
                                        }
                                    } else {
                                        eprintln!(
                                            "marsad-agent: session {sid} opencode unknown notification: {method}"
                                        );
                                    }
                                }

                                // ── Request (agent→client) ─────────────────────────────────
                                acp::AcpMessage::Request { id, method, params } => {
                                    if acp::is_permission_request(&method) {
                                        // session/request_permission: capture as pending gate.
                                        // The ACP request `id` is stored as `request_id` in the
                                        // PendingGate so `handle_answer_gate` can reply to it.
                                        let acp_request_id = id.to_string();
                                        let gate = acp_translate::acp_permission_to_gate(&params);

                                        // The gate's own `request_id` field comes from acp_permission_to_gate
                                        // (uses toolUseId). We ALSO need to store the raw ACP JSON-RPC id
                                        // so we can write the response. We store it as the PendingGate's
                                        // request_id (overriding the toolUseId) so handle_answer_gate
                                        // can use it directly for the JSON-RPC reply.
                                        let gate_request_id = match &gate {
                                            marsad_gates::Gate::Permission { request_id, .. } => {
                                                request_id.clone()
                                            }
                                            _ => acp_request_id.clone(),
                                        };

                                        {
                                            let mut gates = pending_fifo.lock().unwrap();
                                            let queue = gates.entry(sid.clone()).or_default();
                                            let dup = queue
                                                .iter()
                                                .any(|g| g.request_id == gate_request_id);
                                            if dup {
                                                eprintln!(
                                                    "marsad-agent: session {sid} opencode gate DUPLICATE ignored ({})",
                                                    gate_request_id
                                                );
                                            } else {
                                                eprintln!(
                                                    "marsad-agent: session {sid} opencode gate captured (acp_id={acp_request_id}, gate_id={gate_request_id}, {} now queued)",
                                                    queue.len() + 1
                                                );
                                                // Store the raw ACP JSON-RPC id string as the
                                                // PendingGate.request_id so handle_answer_gate can
                                                // write `encode_response(id, ...)` to the writer.
                                                queue.push_back(PendingGate {
                                                    request_id: acp_request_id.clone(),
                                                    input: params.clone(),
                                                    elicitation: false,
                                                });
                                            }
                                        }

                                        // Broadcast the gate as an AgentEvent so the GUI renders it.
                                        // We encode the gate into the canonical control_request shape
                                        // that marsad_proto expects. For opencode, we synthesise a
                                        // stream-json `can_use_tool` control_request shape.
                                        let tool_call = params.get("toolCall").cloned().unwrap_or(serde_json::json!({}));
                                        let tool_name = tool_call.get("toolName").and_then(|v| v.as_str()).unwrap_or("unknown_tool");
                                        let tool_use_id = tool_call.get("toolUseId").and_then(|v| v.as_str()).unwrap_or("");
                                        let input_str = tool_call.get("input").and_then(|v| v.as_str()).unwrap_or("{}");
                                        let input_val: serde_json::Value = serde_json::from_str(input_str).unwrap_or(serde_json::json!({}));
                                        let options = params.get("options").cloned().unwrap_or(serde_json::json!([]));
                                        let permission_suggestions = acp_translate::acp_options_to_suggestions(&options);

                                        let gate_event = serde_json::json!({
                                            "type": "control_request",
                                            "request": {
                                                "type": "can_use_tool",
                                                "request_id": acp_request_id,
                                                "tool_name": tool_name,
                                                "tool_use_id": tool_use_id,
                                                "input": input_val,
                                                "permission_suggestions": permission_suggestions,
                                            }
                                        });
                                        let ipc_msg = marsad_proto::IpcMsg::AgentEvent {
                                            session_id: sid.clone(),
                                            event: gate_event,
                                        };
                                        let encoded = marsad_proto::encode_ipc(&ipc_msg);
                                        let subs = subs_fifo.lock().unwrap();
                                        subs.iter().for_each(|s| {
                                            let _ = s.send(encoded.clone());
                                        });
                                    } else {
                                        eprintln!(
                                            "marsad-agent: session {sid} opencode unknown agent request: {method} (id={id})"
                                        );
                                    }
                                }

                                // ── Response ──────────────────────────────────────────────
                                acp::AcpMessage::Response { id, result } => {
                                    // Correlate by id. id=1 → initialize result.
                                    // id=2 → session/new or session/load result.
                                    // id>=3 → session/prompt result (one per turn).
                                    let id_u64 = id.as_u64().unwrap_or(0);

                                    match result {
                                        Err(err_val) => {
                                            eprintln!(
                                                "marsad-agent: session {sid} opencode ACP error response id={id}: {err_val}"
                                            );
                                        }
                                        Ok(res) => {
                                            if id_u64 == 1 {
                                                // initialize result: agentCapabilities.
                                                // We just note it (no action needed beyond logging).
                                                eprintln!(
                                                    "marsad-agent: session {sid} opencode initialize OK (capabilities: {res})"
                                                );
                                            } else if id_u64 == 2 {
                                                // session/new or session/load result: extract sessionId.
                                                if let Some(acp_session_id) = acp::extract_session_id(&res) {
                                                    eprintln!(
                                                        "marsad-agent: session {sid} opencode ACP sessionId={acp_session_id}"
                                                    );
                                                    // Store in the registry so handle_send_prompt etc. can use it.
                                                    acp_sessions.lock().unwrap().insert(sid.clone(), acp_session_id.clone());

                                                    // Synthesize system/init event and broadcast.
                                                    let model_str = opencode_spec.as_ref().map(|s| s.model.as_str()).unwrap_or("");
                                                    let cwd_str = opencode_spec.as_ref().map(|s| s.project.as_str()).unwrap_or(".");
                                                    // Use a placeholder for init_result (we don't store it).
                                                    let init_result = serde_json::json!({});
                                                    let system_init = acp_translate::acp_init_to_system_init(
                                                        &init_result,
                                                        &res,
                                                        model_str,
                                                        cwd_str,
                                                        &sid,
                                                    );

                                                    // U1: observe model from system/init.
                                                    if let (Some(provider), Some(model)) = (
                                                        capability_provider.as_ref(),
                                                        system_init.get("model").and_then(|m| m.as_str()),
                                                    ) {
                                                        provider.observe_claude_model(&capability_host_id, model);
                                                    }

                                                    let ipc_msg = marsad_proto::IpcMsg::AgentEvent {
                                                        session_id: sid.clone(),
                                                        event: system_init,
                                                    };
                                                    let encoded = marsad_proto::encode_ipc(&ipc_msg);
                                                    {
                                                        let subs = subs_fifo.lock().unwrap();
                                                        subs.iter().for_each(|s| {
                                                            let _ = s.send(encoded.clone());
                                                        });
                                                    }

                                                    // Deliver first_prompt as session/prompt (if any).
                                                    if is_first_attach {
                                                        if let Some(ref prompt) = first_prompt {
                                                            use std::io::Write;
                                                            // Next request id: 3 (initialize=1, session/new=2).
                                                            let prompt_req_id = serde_json::json!(3u64);
                                                            let prompt_params = acp::session_prompt_params(&acp_session_id, prompt);
                                                            let prompt_line = acp::encode_request(
                                                                prompt_req_id,
                                                                "session/prompt",
                                                                prompt_params,
                                                            );
                                                            let mut w = writer.lock().unwrap();
                                                            let _ = w.write_all(prompt_line.as_bytes());
                                                            let _ = w.write_all(b"\n");
                                                            let _ = w.flush();
                                                            eprintln!(
                                                                "marsad-agent: session {sid} opencode sent first_prompt as session/prompt ({} chars)",
                                                                prompt.len()
                                                            );
                                                        }
                                                    }
                                                } else {
                                                    eprintln!(
                                                        "marsad-agent: session {sid} opencode session/new result missing sessionId: {res}"
                                                    );
                                                }
                                            } else {
                                                // session/prompt result: extract stopReason + usage, broadcast.
                                                // Use acp_prompt_result_to_result so per-turn usage is captured.
                                                let stop_reason = acp::extract_stop_reason(&res)
                                                    .unwrap_or_else(|| "completed".to_string());
                                                let result_event = acp_translate::acp_prompt_result_to_result(&res);
                                                let ipc_msg = marsad_proto::IpcMsg::AgentEvent {
                                                    session_id: sid.clone(),
                                                    event: result_event,
                                                };
                                                let encoded = marsad_proto::encode_ipc(&ipc_msg);
                                                let subs = subs_fifo.lock().unwrap();
                                                subs.iter().for_each(|s| {
                                                    let _ = s.send(encoded.clone());
                                                });
                                                eprintln!(
                                                    "marsad-agent: session {sid} opencode session/prompt result: {stop_reason}"
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                            // opencode lines are fully handled above; continue to next line.
                            continue;
                        }

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

            // A COMMANDED stop (pause / supersede) is NOT a connection blip. The
            // kill that ends a pause EOFs this stream too, so if the cancel token is
            // already set the drop we just saw is that intentional kill — exit
            // WITHOUT emitting Reconnecting/Unstable (which would paint a paused
            // session "reconnecting"). handle_stop_session sets the token before the
            // kill, so this check is deterministic; a real SSH drop leaves it unset.
            if cancel.load(Ordering::SeqCst) {
                break 'reconnect;
            }
            eprintln!("marsad-agent: session {sid} connection lost, probing liveness");
            // Emit the drop health via the pure state machine: a drop that never
            // restabilized this attach (no lines) is a FLAP → Unstable; otherwise a
            // plain Reconnecting blip. This is what HARD-BLOCKS the composer.
            emit_conn(conn_on_drop(lines_seen));

            // Probe + backoff loop. Exits ONLY on: cancel (pause/supersede), or a
            // definitive DEAD verdict (→ Paused). An Unreachable host NEVER pauses
            // the session — the durable process is presumed alive; we keep probing
            // at the steady backoff and surface Disconnected once it saturates,
            // flipping back to Connected the instant the host answers LIVE.
            loop {
                if cancel.load(Ordering::SeqCst) {
                    break 'reconnect;
                }

                // Sleep before the next probe (always, to avoid busy-looping), in
                // 1s increments so a cancel (pause/supersede) is honored within ~1s
                // instead of after the full backoff.
                eprintln!("marsad-agent: session {sid} still down, retrying in {backoff_secs}s");
                for _ in 0..backoff_secs {
                    if cancel.load(Ordering::SeqCst) {
                        break 'reconnect;
                    }
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
                // Once the backoff has grown to its steady max, a still-unreachable
                // host is a SUSTAINED outage (→ Disconnected, not endless Reconnecting).
                let backoff_at_max = backoff_secs >= RECONNECT_MAX_BACKOFF_SECS;
                backoff_secs = reconnect_backoff_next(backoff_secs);

                let verdict = crate::remote::probe_liveness(
                    host,
                    &sid,
                    std::time::Duration::from_secs(RECONNECT_PROBE_TIMEOUT_SECS),
                );
                let step = conn_on_probe(verdict, backoff_at_max);
                if let Some(h) = step.emit {
                    emit_conn(h);
                }
                match step.action {
                    ProbeAction::Reattach => {
                        eprintln!("marsad-agent: session {sid} reconnected live remote session");
                        backoff_secs = RECONNECT_INITIAL_BACKOFF_SECS;
                        continue 'reconnect;
                    }
                    ProbeAction::Pause => {
                        eprintln!("marsad-agent: session {sid} probe DEAD — process gone, finalizing");
                        break 'reconnect;
                    }
                    ProbeAction::KeepProbing => {
                        // Host unreachable — presume the durable session alive, keep going.
                    }
                }
            } // probe loop
        } // 'reconnect loop

        // Whether THIS reader was superseded/stopped (vs it giving up on its own).
        let was_cancelled = cancel.load(Ordering::SeqCst);
        // De-register our token if it's still the current one (a newer reader may
        // have already replaced it — don't clobber that).
        {
            let mut m = reader_cancels().lock().unwrap();
            if m.get(&sid)
                .map(|t| std::sync::Arc::ptr_eq(t, &cancel))
                .unwrap_or(false)
            {
                m.remove(&sid);
            }
        }

        // Two-axis lifecycle: this reader has given up (re)attaching. For a
        // REMOTE session that means the process is gone or unreachable — claude
        // exited, the host rebooted / OOM-killed / scheduler-killed it, or the
        // reconnect cap was hit. Tell core so the card flips to Paused instead of
        // lying with its last activity state (Gap B: host-death → Paused). Local
        // sessions manage liveness elsewhere; skip here to avoid a false Paused
        // on a still-live local session. Also SKIP when we were cancelled: a
        // supersede means a newer reader owns liveness (it will broadcast LIVE) and
        // a stop already set Paused in core — a stray PAUSED here would race and
        // wrongly flip the fresh session Paused.
        if exec_prefix_for(host_target.as_deref()).is_some() && !was_cancelled {
            eprintln!("marsad-agent: session {sid} liveness -> PAUSED (reader gave up; broadcast)");
            let line = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::SessionLiveness {
                session_id: sid.clone(),
                live: false,
            });
            let subs = subs_fifo.lock().unwrap();
            for s in subs.iter() {
                let _ = s.send(line.clone());
            }
        }

        // Final cleanup: ensure the writer registry slot is gone (it may already
        // be absent if the reconnect loop tore it down; the remove is idempotent).
        {
            let mut registry = writers_fifo.lock().unwrap();
            registry.remove(&sid);
        }
        // Do NOT wipe still-queued prompts here. This reader gave up, but the host
        // session is durable — a later resume spawns a fresh reader that will flush
        // this same (session-keyed) queue on attach, so anything the user sent must
        // survive. Wiping it here was a silent-loss bug: a prompt sent during a long
        // outage vanished when the reader hit its cap. The queue is bounded
        // (PENDING_PROMPT_MAX) so it can't grow without limit.
    });
}
