//! Per-session reader thread: connect the session transport (local FIFO/pipe or
//! a remote durable FIFO bridge), pump its NDJSON stream, and broadcast decoded
//! events to every subscriber — plus the small pure-logic helpers
//! (`reconnect_backoff_next`, `assistant_has_file_modifying_tool_use`, the
//! `RECONNECT_*` constants) that drive its remote auto-reconnect behavior.

mod util;
pub use util::*;

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
    // codex app-server: cached spec (model/project/posture/bin) for the reader's
    // and translate-and-broadcast). None for claude sessions.
    codex_spec: Option<crate::CachedCodexSpec>,
    // codex app-server: shared thread/turn state (thread_id from thread/start,
    // turn_id from turn/started). Shared with handle_send_prompt/handle_interrupt/
    // handle_set_session_config_option to address JSON-RPC requests.
    codex_threads: std::sync::Arc<std::sync::Mutex<HashMap<String, crate::CodexThread>>>,
    // opencode ACP: cached spec for the ACP handshake (initialize + session/new).
    // None for non-opencode sessions.
    opencode_spec: Option<crate::CachedOpencodeSpec>,
    // opencode ACP: shared map for storing the ACP sessionId from the session/new
    // (or session/load) result. Shared with handle_send_prompt / handle_interrupt /
    // handle_answer_gate so they can address subsequent ACP requests.
    acp_sessions: std::sync::Arc<std::sync::Mutex<HashMap<String, String>>>,
    // Reader-liveness registry (see `LiveReaders`). This reader inserts its sid
    // here on spawn and removes it when it gives up (unless superseded), so the
    // resume guard knows whether a live reader owns the session.
    live_readers: super::LiveReaders,
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
    // Mark this session as having a live reader (the resume guard's signal).
    // Idempotent with the synchronous insert in `launch_with_id`; also covers the
    // startup-reconnect path, which spawns readers without going through
    // `launch_with_id`. Removed on give-up below (unless superseded).
    live_readers.lock().unwrap().insert(sid.clone());

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
        // stream events (`conn_on_drop` + the signal-driven `reconnect_action`):
        // attach → Connected; drop → Reconnecting/Unstable; sustained unreachable →
        // Disconnected; process gone → Paused. The UI uses it to show the true state
        // and HARD-BLOCK sending into a broken pipe.
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

        // Emit an arbitrary translated stream-json event to subscribers.
        let emit_event = |event: serde_json::Value| {
            let line = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::AgentEvent {
                session_id: sid.clone(),
                event,
            });
            if let Ok(subs) = subs_fifo.lock() {
                for s in subs.iter() {
                    let _ = s.send(line.clone());
                }
            }
        };

        // opencode streams ONE assistant reply as many `agent_message_chunk`
        // deltas that share a message id. We ACCUMULATE them here and emit a single
        // complete assistant message (on the next message id, any non-text update,
        // a request/result, or stream end) — otherwise each tiny delta became its
        // own transcript bubble. `flush_acp_text!()` emits + clears the buffer.
        let mut acp_text_buf: Option<(String, String)> = None;
        macro_rules! flush_acp_text {
            () => {
                if let Some((mid, txt)) = acp_text_buf.take() {
                    if !txt.is_empty() {
                        emit_event(crate::acp_translate::acp_assistant_message(&mid, &txt));
                    }
                }
            };
        }

        // opencode spawns subagents via its `task` tool, surfaced as a
        // tool_call/tool_call_update lifecycle. We re-emit that as claude's
        // canonical `Task` tool_use + `tool_result` so marsad_events groups it
        // like a claude subagent. These sets keep create/complete idempotent
        // across reconnect replays (declared outside the reconnect loop on
        // purpose — a subagent can span a reconnect).
        let mut acp_sub_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut acp_sub_created: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut acp_sub_done: std::collections::HashSet<String> = std::collections::HashSet::new();

        'reconnect: loop {
            // Superseded (a newer reader took over) or stopped (paused) → exit now,
            // don't attach/re-probe.
            if cancel.load(Ordering::SeqCst) {
                break 'reconnect;
            }
            // Per-attempt flag: set by the reader child's stderr thread when it sees
            // a transport error. Read at stream-end to distinguish a network drop
            // (reattach) from a dead process (pause). Reset each iteration.
            let saw_conn_error =
                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
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
                    // Stream reader's stderr to agent.log for diagnosis, AND flag a
                    // transport error so the post-drop classifier can tell a network
                    // drop from a dead process even in the rare case ssh exits 0.
                    // (The reader child's exit code is the primary signal; this is
                    // corroboration — see `reconnect_action`.)
                    if let Some(errpipe) = reader_child.stderr.take() {
                        let esid = sid.clone();
                        let flag = saw_conn_error.clone();
                        std::thread::spawn(move || {
                            use std::io::BufRead;
                            let r = std::io::BufReader::new(errpipe);
                            for line in r.lines().map_while(Result::ok) {
                                if !line.trim().is_empty() {
                                    let l = line.to_ascii_lowercase();
                                    if l.contains("connection reset")
                                        || l.contains("closed by remote host")
                                        || l.contains("connection to")
                                        || l.contains("client_loop")
                                        || l.contains("broken pipe")
                                        || l.contains("timed out")
                                        || l.contains("connection refused")
                                    {
                                        flag.store(true, std::sync::atomic::Ordering::SeqCst);
                                    }
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

            // ── codex app-server handshake (LAUNCH only, first attach only) ───
            // Persistent JSON-RPC: `initialize` (id 1) → `initialized`
            // (notification) → `thread/start` (id 2). thread/start carries the
            // project cwd + model + posture-derived approvalPolicy/sandbox so the
            // very first turn runs under the right settings. The thread id comes
            // back on the id==2 response (handled in the read loop), which then
            // delivers the first_prompt as a `turn/start`.
            if driver == "codex" && send_init && is_first_attach {
                use crate::acp;
                use crate::codex_appserver as cx;
                use std::io::Write;
                let (model, approval, sandbox) = codex_spec
                    .as_ref()
                    .map(|cs| {
                        let (a, s) = codex_posture_settings(&cs.posture);
                        (cs.model.clone(), a, s)
                    })
                    .unwrap_or_else(|| (String::new(), "on-request", "read-only"));
                let cwd = codex_spec.as_ref().map(|cs| cs.project.clone()).unwrap_or_else(|| ".".into());
                let init_line = acp::encode_request(
                    serde_json::json!(1u64),
                    cx::INITIALIZE,
                    cx::initialize_params("marsad", env!("CARGO_PKG_VERSION")),
                );
                let started_line =
                    acp::encode_notification(cx::INITIALIZED, serde_json::Value::Null);
                let ts_params = cx::thread_start_params(
                    &cwd,
                    Some(model.as_str()),
                    Some(approval),
                    Some(sandbox),
                );
                let ts_line = acp::encode_request(serde_json::json!(2u64), cx::THREAD_START, ts_params);
                {
                    let mut w = writer.lock().unwrap();
                    let _ = w.write_all(init_line.as_bytes());
                    let _ = w.write_all(b"\n");
                    let _ = w.write_all(started_line.as_bytes());
                    let _ = w.write_all(b"\n");
                    let _ = w.write_all(ts_line.as_bytes());
                    let _ = w.write_all(b"\n");
                    let _ = w.flush();
                }
                eprintln!("marsad-agent: session {sid} codex sent initialize + thread/start");
            }

            // Deliver the first_prompt (if any) immediately so the process receives its
            // initial task.  Only on the very first attach — on reconnect the process is
            // already running (a prompt was already sent) and re-sending would be wrong.
            //
            // CLAUDE: delivers as a stream-json user message on stdin.
            // CODEX: delivers first_prompt as a `turn/start` after thread/start;
            //        the eval-loop container spawns `codex exec …` as a subprocess.
            // OPENCODE: first_prompt is delivered as a `session/prompt` ACP request
            //           AFTER the session/new result arrives (so the ACP sessionId is
            //           known). The read loop below handles this delivery on the
            //           session/new response. We skip the first-prompt delivery here
            //           for opencode.
            // CODEX: like opencode, the first_prompt is delivered as a `turn/start`
            // AFTER the thread/start result arrives (so the threadId is known); the
            // read loop handles that on the id==2 response. Skip here.
            if is_first_attach && driver != "opencode" && driver != "codex" {
                if let Some(ref prompt) = first_prompt {
                    use std::io::Write;
                    let plen = prompt.len();
                    {
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
            //
            // OPENCODE: skip this entirely. This backfill reads claude's on-disk
            // transcript (`~/.claude/projects/<project>/<uuid>.jsonl`), which an
            // opencode session does NOT own — and when a claude AND an opencode
            // session share a project dir, it would splice the CLAUDE session's
            // transcript under the opencode session (opencode header + claude
            // transcript — a real cross-wire bug). opencode history comes from the
            // ACP stream (session/load replay on resume, live session/update events).
            if is_first_attach && driver != "opencode" {
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
                        // Emit any assistant text still buffered before we stop.
                        flush_acp_text!();
                        break;
                    }
                    Err(e) => {
                        eprintln!("marsad-agent: session {sid} reader ERROR after {lines_seen} line(s): {e}");
                        // Emit any assistant text still buffered before we stop.
                        flush_acp_text!();
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
                                    acp_response_id: None,
                                    codex_method: None,
                                    // The claude gate event IS this raw stream-json
                                    // line (broadcast verbatim below) — store it so a
                                    // reconnecting client can have the gate replayed.
                                    event: serde_json::from_str(raw)
                                        .unwrap_or_else(|_| serde_json::json!({ "raw": raw })),
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
                        // - "codex": parse app-server JSON-RPC; translate via codex_translate.
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
                                            // A streamed text delta → accumulate into
                                            // the current assistant message instead of
                                            // emitting one bubble per delta.
                                            if let Some((mid, txt)) =
                                                acp_translate::acp_text_chunk(update)
                                            {
                                                match &mut acp_text_buf {
                                                    Some((cur, buf)) if *cur == mid => {
                                                        buf.push_str(&txt)
                                                    }
                                                    _ => {
                                                        flush_acp_text!();
                                                        acp_text_buf = Some((mid, txt));
                                                    }
                                                }
                                                continue;
                                            }
                                            // Any OTHER update (tool_call, plan, mode,
                                            // usage…): the current text message is done,
                                            // flush it first so ordering is preserved.
                                            flush_acp_text!();

                                            // opencode subagents: re-emit the `task`
                                            // tool lifecycle as claude's canonical Task
                                            // tool_use + tool_result so the core groups
                                            // it identically. The raw `task` rows are
                                            // suppressed (we `continue`) — one clean
                                            // Task row + the subagent group instead.
                                            if let Some(id) =
                                                acp_translate::acp_subagent_pending_id(update)
                                            {
                                                // The bare `task` tool_call has no
                                                // description yet; just mark the id and
                                                // emit the create from the first enriched
                                                // update below.
                                                acp_sub_ids.insert(id);
                                                continue;
                                            }
                                            if let Some(su) =
                                                acp_translate::acp_subagent_parse_update(update)
                                            {
                                                if acp_sub_ids.contains(&su.tool_call_id) {
                                                    // Emit the Task tool_use once, as soon
                                                    // as we have a name for the subagent.
                                                    if !acp_sub_created.contains(&su.tool_call_id) {
                                                        if let Some(name) = &su.name {
                                                            emit_event(
                                                                acp_translate::acp_subagent_task_tool_use(
                                                                    &su.tool_call_id,
                                                                    name,
                                                                    &su.raw_input,
                                                                ),
                                                            );
                                                            acp_sub_created
                                                                .insert(su.tool_call_id.clone());
                                                        }
                                                    }
                                                    // On completion, emit the tool_result
                                                    // once (marks the subagent Done).
                                                    if su.status == "completed"
                                                        && !acp_sub_done.contains(&su.tool_call_id)
                                                    {
                                                        // Guarantee a create even if no
                                                        // named update was seen first.
                                                        if !acp_sub_created
                                                            .contains(&su.tool_call_id)
                                                        {
                                                            let name = su
                                                                .name
                                                                .clone()
                                                                .unwrap_or_else(|| {
                                                                    "subagent".to_string()
                                                                });
                                                            emit_event(
                                                                acp_translate::acp_subagent_task_tool_use(
                                                                    &su.tool_call_id,
                                                                    &name,
                                                                    &su.raw_input,
                                                                ),
                                                            );
                                                            acp_sub_created
                                                                .insert(su.tool_call_id.clone());
                                                        }
                                                        let text =
                                                            su.result_text.clone().unwrap_or_default();
                                                        emit_event(
                                                            acp_translate::acp_subagent_tool_result(
                                                                &su.tool_call_id,
                                                                &text,
                                                            ),
                                                        );
                                                        acp_sub_done.insert(su.tool_call_id.clone());
                                                    }
                                                    // Suppress the raw subagent tool rows.
                                                    continue;
                                                }
                                            }
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
                                                emit_event(translated);
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
                                    // A gate interrupts the reply — flush any buffered
                                    // assistant text before the gate event.
                                    flush_acp_text!();
                                    if acp::is_permission_request(&method) {
                                        // session/request_permission: capture as pending gate.
                                        // The ACP request `id` is stored as `request_id` in the
                                        // PendingGate so `handle_answer_gate` can reply to it.
                                        // Raw ACP JSON-RPC id (e.g. 0) — needed to reply to
                                        // opencode. It is NOT unique across sessions (the first
                                        // permission of every session is id 0), so it must NOT be
                                        // the gate's routing id.
                                        let acp_request_id = id.to_string();
                                        // Routing id = opencode's toolCallId (unique per tool call),
                                        // falling back to a session-scoped composite. This becomes the
                                        // frontend decision_id AND the core ledger dedup key, so it
                                        // MUST be unique — using the raw ACP id "0" made the core
                                        // ledger treat every session's first gate as "already seen"
                                        // and silently drop the answer (gate never sent, never cleared).
                                        let unique_id = acp_translate::acp_gate_routing_id(
                                            &sid,
                                            &acp_request_id,
                                            &params,
                                        );

                                        {
                                            let mut gates = pending_fifo.lock().unwrap();
                                            let queue = gates.entry(sid.clone()).or_default();
                                            let dup = queue.iter().any(|g| g.request_id == unique_id);
                                            if dup {
                                                eprintln!(
                                                    "marsad-agent: session {sid} opencode gate DUPLICATE ignored ({unique_id})"
                                                );
                                            } else {
                                                eprintln!(
                                                    "marsad-agent: session {sid} opencode gate captured (acp_id={acp_request_id}, gate_id={unique_id}, {} now queued)",
                                                    queue.len() + 1
                                                );
                                                // request_id = the UNIQUE routing id; the raw ACP id is
                                                // kept in acp_response_id so handle_answer_gate replies
                                                // to opencode with the correct JSON-RPC id.
                                                queue.push_back(PendingGate {
                                                    request_id: unique_id.clone(),
                                                    input: params.clone(),
                                                    elicitation: false,
                                                    acp_response_id: Some(acp_request_id.clone()),
                                                    codex_method: None,
                                                    // The translated control_request broadcast just
                                                    // below — stored for replay on re-subscribe.
                                                    event: acp_translate::acp_permission_to_control_request_event(
                                                        &unique_id,
                                                        &params,
                                                    ),
                                                });
                                            }
                                        }

                                        // Surface the gate as a stream-json control_request that
                                        // parse_control_request accepts (request_id top-level,
                                        // inner subtype=can_use_tool). Uses the SAME unique_id so the
                                        // frontend decision_id matches the pending gate for answering.
                                        let gate_event = acp_translate::acp_permission_to_control_request_event(
                                            &unique_id,
                                            &params,
                                        );
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
                                    // A prompt result (end_turn) ends the turn — flush
                                    // any buffered assistant text before the result.
                                    flush_acp_text!();
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

                                                    // Surface opencode's mode list + current from the
                                                    // session/new (or session/load) configOptions.
                                                    if let Some(mu) = acp_translate::acp_config_modes_update(&sid, &res) {
                                                        emit_event(mu);
                                                    }

                                                    // Apply the New-Session model pick — opencode acp
                                                    // rejects --model, so the supported path is
                                                    // set_config_option AFTER session/new. Truthful:
                                                    // only when the pick is a real option AND differs
                                                    // from the current. Done BEFORE the first prompt so
                                                    // the first turn runs on the chosen model.
                                                    if is_first_attach {
                                                        use std::io::Write;
                                                        if let Some(spec) = opencode_spec.as_ref() {
                                                            let want = spec.model.trim();
                                                            if !want.is_empty() {
                                                                let (current, valid) = acp_translate::acp_config_model_values(&res);
                                                                if valid.iter().any(|v| v == want) && current.as_deref() != Some(want) {
                                                                    let params = serde_json::json!({
                                                                        "sessionId": acp_session_id,
                                                                        "configId": "model",
                                                                        "value": want,
                                                                    });
                                                                    let line = acp::encode_request(serde_json::json!(3u64), "session/set_config_option", params);
                                                                    let mut w = writer.lock().unwrap();
                                                                    let _ = w.write_all(line.as_bytes());
                                                                    let _ = w.write_all(b"\n");
                                                                    let _ = w.flush();
                                                                    eprintln!("marsad-agent: session {sid} opencode applied launch model {want}");
                                                                } else if !valid.is_empty() && !valid.iter().any(|v| v == want) {
                                                                    eprintln!("marsad-agent: session {sid} opencode launch model {want:?} not in options — left at {current:?}");
                                                                }
                                                            }
                                                        }

                                                        // Deliver first_prompt as session/prompt (id 4,
                                                        // after the optional model-set at id 3).
                                                        if let Some(ref prompt) = first_prompt {
                                                            let prompt_params = acp::session_prompt_params(&acp_session_id, prompt);
                                                            let prompt_line = acp::encode_request(
                                                                serde_json::json!(4u64),
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
                                            } else if res.get("configOptions").is_some() {
                                                // A set_config_option result (model/mode changed) —
                                                // NOT a prompt turn. It echoes the authoritative new
                                                // configOptions; surface the mode list + current.
                                                if let Some(mu) = acp_translate::acp_config_modes_update(&sid, &res) {
                                                    emit_event(mu);
                                                }
                                                eprintln!(
                                                    "marsad-agent: session {sid} opencode set_config_option result applied"
                                                );
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

                        // ── codex app-server (persistent JSON-RPC) ───────────────
                        if driver == "codex" {
                            use crate::acp;
                            use crate::codex_appserver as cx;
                            use crate::codex_translate;
                            let msg = match cx::parse_line(raw) {
                                Ok(m) => m,
                                Err(e) => {
                                    eprintln!("marsad-agent: session {sid} codex parse error: {e} (line: {raw})");
                                    continue;
                                }
                            };
                            match msg {
                                acp::AcpMessage::Notification { method, params } => {
                                    // Track the current turnId (for turn/interrupt) on
                                    // the session's thread state. Set on turn/started,
                                    // cleared on turn/completed (which codex also emits
                                    // for interrupted/failed turns — see codex_turn_result).
                                    if method == "turn/started" {
                                        if let (Some(tid), Ok(mut m)) = (cx::extract_turn_id(&params), codex_threads.lock()) {
                                            if let Some(entry) = m.get_mut(&sid) {
                                                entry.turn_id = Some(tid);
                                            }
                                        }
                                    } else if method == "turn/completed" {
                                        if let Ok(mut m) = codex_threads.lock() {
                                            if let Some(entry) = m.get_mut(&sid) {
                                                entry.turn_id = None;
                                            }
                                        }
                                    }
                                    if let Some(sj) = codex_translate::codex_notification_to_stream_json(&method, &params, &sid) {
                                        emit_event(sj);
                                    }
                                }
                                acp::AcpMessage::Request { id, method, params } => {
                                    use crate::codex_translate as ct;
                                    // Every codex server request that needs a human maps
                                    // to a Marsad gate: approvals (command/file/permissions
                                    // + legacy) → can_use_tool; requestUserInput →
                                    // AskUserQuestion; elicitation → elicitation. The
                                    // codex `method` is stored on the PendingGate so the
                                    // answer path replies in the right shape.
                                    let raw_id = id.to_string();
                                    let gate_id = ct::codex_gate_routing_id(&sid, &raw_id, &params);
                                    let gate_event: Option<(serde_json::Value, bool)> =
                                        if cx::is_command_approval(&method) || cx::is_legacy_approval(&method) {
                                            Some((ct::codex_approval_to_control_request(&gate_id, &method, &params), false))
                                        } else if method == cx::REQ_USER_INPUT {
                                            Some((ct::codex_question_to_control_request(&gate_id, &params), false))
                                        } else if method == cx::REQ_ELICITATION {
                                            Some((ct::codex_elicitation_to_control_request(&gate_id, &params), true))
                                        } else {
                                            None
                                        };
                                    if let Some((event, is_elicit)) = gate_event {
                                        {
                                            let mut gates = pending_fifo.lock().unwrap();
                                            let queue = gates.entry(sid.clone()).or_default();
                                            if queue.iter().any(|g| g.request_id == gate_id) {
                                                eprintln!("marsad-agent: session {sid} codex gate DUPLICATE ignored ({gate_id})");
                                            } else {
                                                eprintln!("marsad-agent: session {sid} codex gate captured ({method}, gate_id={gate_id})");
                                                queue.push_back(PendingGate {
                                                    request_id: gate_id.clone(),
                                                    input: params.clone(),
                                                    elicitation: is_elicit,
                                                    acp_response_id: Some(raw_id.clone()),
                                                    codex_method: Some(method.clone()),
                                                    // The translated control_request broadcast just
                                                    // below — stored for replay on re-subscribe.
                                                    event: event.clone(),
                                                });
                                            }
                                        }
                                        emit_event(event);
                                    } else {
                                        // An unrecognized server request → decline so the
                                        // turn never hangs.
                                        eprintln!("marsad-agent: session {sid} codex unhandled server request {method}; declining");
                                        let resp = acp::encode_response(id, cx::approval_decision(false, false));
                                        let mut w = writer.lock().unwrap();
                                        let _ = w.write_all(resp.as_bytes());
                                        let _ = w.write_all(b"\n");
                                        let _ = w.flush();
                                    }
                                }
                                acp::AcpMessage::Response { id, result } => {
                                    let id_u = id.as_u64().unwrap_or(0);
                                    match result {
                                        Err(err_val) => eprintln!(
                                            "marsad-agent: session {sid} codex error response id={id}: {err_val}"
                                        ),
                                        Ok(res) => {
                                            if id_u == 2 {
                                                // thread/start result: capture threadId,
                                                // synthesize system/init, deliver the
                                                // first turn as turn/start.
                                                if let Some(tid) = cx::extract_thread_id(&res) {
                                                    codex_threads.lock().unwrap().insert(sid.clone(), crate::CodexThread { thread_id: tid.clone(), turn_id: None });
                                                    eprintln!("marsad-agent: session {sid} codex threadId={tid}");
                                                    let model_str = codex_spec.as_ref().map(|c| c.model.clone()).unwrap_or_default();
                                                    let cwd_str = codex_spec.as_ref().map(|c| c.project.clone()).unwrap_or_else(|| ".".to_string());
                                                    emit_event(crate::codex_translate::codex_system_init(&sid, &model_str, &cwd_str));
                                                    // Query the available collaboration
                                                    // modes (id 5) so the Config "Session
                                                    // mode" UI can render + live-switch.
                                                    {
                                                        let ml = acp::encode_request(serde_json::json!(5u64), cx::COLLABORATION_MODE_LIST, serde_json::json!({}));
                                                        let mut w = writer.lock().unwrap();
                                                        let _ = w.write_all(ml.as_bytes());
                                                        let _ = w.write_all(b"\n");
                                                        let _ = w.flush();
                                                    }
                                                    if is_first_attach {
                                                        if let Some(ref prompt) = first_prompt {
                                                            let tp = cx::turn_start_params(&tid, prompt);
                                                            let line = acp::encode_request(serde_json::json!(3u64), cx::TURN_START, tp);
                                                            let mut w = writer.lock().unwrap();
                                                            let _ = w.write_all(line.as_bytes());
                                                            let _ = w.write_all(b"\n");
                                                            let _ = w.flush();
                                                            eprintln!("marsad-agent: session {sid} codex sent first turn/start ({} chars)", prompt.len());
                                                        }
                                                    }
                                                } else {
                                                    eprintln!("marsad-agent: session {sid} codex thread/start missing thread.id: {res}");
                                                }
                                            } else if id_u == 5 {
                                                // collaborationMode/list result → surface as modes_update.
                                                emit_event(crate::codex_translate::codex_modes_from_collab_list(&res, &sid, None));
                                            }
                                            // id==1 (initialize) / id>=3 (turn/start) →
                                            // no action; the turn/completed notification
                                            // carries the result.
                                        }
                                    }
                                }
                            }
                            continue;
                        }

                        // claude: raw stream-json, no translation. (codex uses the
                        // app-server branch above and never reaches here; opencode
                        // is fully handled above.)
                        let event: serde_json::Value = raw_value;

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
            // Reap the reader child (remote sessions: the `ssh … tail --pid -f`
            // child) stored under the `"reader:<sid>"` key. Capture its exit code
            // FIRST — it's the authoritative signal for why the stream ended (ssh
            // 255 = transport drop; the remote `tail --pid` exiting 0 = the watched
            // process died). The stream already EOF'd, so the child has normally
            // exited on its own; `try_wait` reads that. Only if it's somehow still
            // running (e.g. a half-open connection) do we kill it — and a kill means
            // it did NOT exit cleanly, i.e. treat as a transport drop, not death.
            let reader_exit_code: Option<i32> = {
                let reader_key = format!("reader:{sid}");
                let child = children.lock().unwrap().remove(&reader_key);
                if let Some(mut rchild) = child {
                    match rchild.try_wait() {
                        Ok(Some(status)) => status.code(),
                        _ => {
                            // Still running (or unwaitable): kill it. A killed reader
                            // is NOT a clean exit → don't let it read as "process
                            // died"; None routes to Reattach in `reconnect_action`.
                            let _ = rchild.kill();
                            let _ = rchild.wait();
                            None
                        }
                    }
                } else {
                    None
                }
            };

            // ── Remote reconnect: classify the stream-end, then reattach or stop ─
            // Only for REMOTE sessions. Local sessions (host_target None) stop here.
            if !matches!(host_target.as_deref(), Some(h) if exec_prefix_for(Some(h)).is_some()) {
                break 'reconnect;
            }

            // A COMMANDED stop (pause / supersede) is NOT a connection blip. The
            // kill that ends a pause EOFs this stream too, so if the cancel token is
            // already set the drop we just saw is that intentional kill — exit
            // WITHOUT emitting Reconnecting/Unstable (which would paint a paused
            // session "reconnecting"). handle_stop_session sets the token before the
            // kill, so this check is deterministic; a real SSH drop leaves it unset.
            if cancel.load(Ordering::SeqCst) {
                break 'reconnect;
            }

            // Decide from the reader child's OWN exit signal — NOT a fixed-timeout
            // probe. A clean `tail --pid` exit (code 0, no connection error) means
            // the watched process died → finalize (Pause). Any transport error
            // (ssh 255 / a captured "connection reset" / "closed by remote host")
            // means the connection dropped, not the process → reattach. This is the
            // fix for slow hosts (ETX: 4–26s SSH round-trip) where an 8s probe timed
            // out and stranded a LIVE session on Disconnected for ~25 min.
            let saw_err = saw_conn_error.load(Ordering::SeqCst);
            match reconnect_action(reader_exit_code, saw_err) {
                ReconnectAction::Pause => {
                    eprintln!(
                        "marsad-agent: session {sid} reader exited cleanly (code {reader_exit_code:?}) — process gone, finalizing"
                    );
                    break 'reconnect;
                }
                ReconnectAction::Reattach => {
                    // A connection that was actually working (produced lines) then
                    // dropped is a fresh blip → reset the backoff so we retry fast. A
                    // reattach that never streamed keeps the backoff growing, so a
                    // genuinely-unreachable host escalates Reconnecting → Disconnected
                    // (via the SAME state machine) — but is NEVER falsely Paused.
                    if lines_seen > 0 {
                        backoff_secs = RECONNECT_INITIAL_BACKOFF_SECS;
                    }
                    let backoff_at_max = backoff_secs >= RECONNECT_MAX_BACKOFF_SECS;
                    let health = if backoff_at_max {
                        marsad_proto::ConnHealth::Disconnected
                    } else {
                        conn_on_drop(lines_seen)
                    };
                    // Cancel-guard: if a pause/supersede raced in during teardown, a
                    // fresh reader may have already emitted Connected — don't clobber
                    // it with a stale Reconnecting/Disconnected.
                    if !cancel.load(Ordering::SeqCst) {
                        emit_conn(health);
                    }
                    eprintln!(
                        "marsad-agent: session {sid} connection dropped (exit {reader_exit_code:?}, conn_err={saw_err}) — reattaching in {backoff_secs}s"
                    );
                    // Backoff before the reattach, in 1s increments so a cancel
                    // (pause/supersede) is honored within ~1s.
                    for _ in 0..backoff_secs {
                        if cancel.load(Ordering::SeqCst) {
                            break 'reconnect;
                        }
                        std::thread::sleep(std::time::Duration::from_secs(1));
                    }
                    backoff_secs = reconnect_backoff_next(backoff_secs);
                    continue 'reconnect; // re-open the transport + reader
                }
            }
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
                // This reader is still the current one for the session (NOT
                // superseded by a fresh reader that took over) and it has given up
                // (re)attaching. Clear the liveness marker so a subsequent resume
                // relaunches (re-spawning a reader, which drains any queued
                // prompts) instead of no-op'ing forever. A superseded reader must
                // NOT clear it — the newer reader owns the session's liveness.
                live_readers.lock().unwrap().remove(&sid);
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
