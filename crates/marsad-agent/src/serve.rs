//! The blocking real-IO event loop: bind the listener, reconnect sessions that
//! survived an agent restart, accept clients, and run the agent command loop.
//! Entirely `#[cfg(not(coverage))]` — see the doc comment on `Agent::serve`.

use marsad_lifecycle::Persisted;

use super::capabilities::{CapabilityProvider, RealAgyProbe, RealClaudeProbe, RealCodexProbe, RealOpencodeProbe};
use super::reader::spawn_session_reader;
use super::remote::exec_prefix_for;
use super::{Agent, AgentError, ListenSpec, SessionRecord};

/// Default `set_wasl_connector_factory` (docs/specs/wasl-connector.md §4.2):
/// a real `WaslOperatorClient` per operator entry. Only ever reached from
/// `serve()`'s real-IO block — `WaslOperatorClient`'s `WaslConnector` impl is
/// itself `cfg(not(coverage))` in marsad-wasl, so this function lives here
/// rather than in `wasl.rs` (which is compiled under coverage too).
fn default_wasl_connector_factory(
    entry: &marsad_wasl::WaslOperatorEntry,
) -> Box<dyn marsad_wasl::WaslConnector> {
    Box::new(marsad_wasl::WaslOperatorClient::new(
        marsad_wasl::WaslClientConfig {
            base_url: entry.url.clone(),
            initial_backoff_ms: 1000,
        },
    ))
}

impl Agent {
    /// Blocking event loop — real socket/pipe IO, excluded from the coverage
    /// build because it requires a live socket/pipe and loops forever.
    /// Called only from `src/main.rs`; since the binary target is not compiled
    /// during `cargo llvm-cov` (autobins = false), this function is absent from
    /// the coverage surface entirely.
    ///
    /// # Concurrency model
    ///
    /// `Agent` owns non-`Send` trait objects, so it cannot be wrapped in
    /// `Arc<Mutex<Agent>>` across threads.  Instead we use a **command channel**
    /// pattern:
    ///
    /// - This serve thread is the single owner of `Agent` and processes IPC
    ///   commands sequentially via `mpsc::Receiver<AgentCmd>`.
    /// - One short-lived reader thread is spawned per accepted client socket.
    ///   It reads inbound lines and forwards them as `AgentCmd` values on the
    ///   shared command sender, blocking on a one-shot reply channel for the
    ///   response.  While waiting for the reply it holds no locks and does no
    ///   IO on the agent.
    /// - Broadcast messages (AgentEvent lines from session FIFO readers) bypass
    ///   the agent thread and go directly to each client's `mpsc::Sender<String>`
    ///   in the subscriber list `Arc<Mutex<Vec<mpsc::Sender<String>>>>`.
    ///
    /// Deadlock analysis:
    ///  - The agent thread holds no subscriber-list lock when processing a command.
    ///  - Session FIFO reader threads lock only the subscriber list (briefly) per
    ///    line — never the agent command channel.
    ///  - Client reader threads lock only the subscriber list on disconnect prune,
    ///    which is also brief.
    ///  - No lock is held while blocking on socket or FIFO reads.
    #[cfg(not(coverage))]
    pub fn serve(mut self) -> Result<(), AgentError> {
        use marsad_transport::LocalIpcListener;
        use std::sync::{mpsc, Arc, Mutex};

        // ── Bind ──────────────────────────────────────────────────────────────
        let socket_path = match &self.config.listen {
            ListenSpec::UnixSocket(path) => path.clone(),
            other => {
                return Err(AgentError::Listen(format!(
                    "unsupported listen spec: {other:?}"
                )));
            }
        };

        let listener = LocalIpcListener::bind(&socket_path)
            .map_err(|e| AgentError::Listen(format!("bind {socket_path}: {e}")))?;

        eprintln!("marsad-agent: listening on {socket_path}");

        // ── Command channel ────────────────────────────────────────────────
        // Each `AgentCmd` carries one inbound IPC line plus a one-shot reply
        // sender.  The serve thread processes commands sequentially and sends
        // the reply back through the one-shot channel.
        //
        // `AgentReply` carries either the list of outbound lines (success) or
        // an error string, plus an optional session_id that signals the serve
        // thread to open a FIFO reader for that session, plus an optional
        // first_prompt to deliver to claude's stdin immediately after launch.
        struct AgentCmd {
            line: String,
            reply_tx: mpsc::SyncSender<AgentReply>,
        }
        struct AgentReply {
            out_lines: Result<Vec<String>, String>,
            /// Set when a LaunchSession/ResumeSession produced a session_id and
            /// a new FIFO reader should be started.
            new_session_id: Option<String>,
            /// The first prompt to write to in.fifo right after the FIFO
            /// connects, so claude receives its initial task without hanging.
            first_prompt: Option<String>,
            /// The session's resolved remote target (`None` = local). Tells the
            /// reader thread whether to open local FIFOs or a remote transport.
            host_target: Option<String>,
            /// For a REMOTE session: the run-command body (setup + env + `cd …
            /// && exec <claude> <args>`) to run as the single piped child. `None`
            /// for local sessions (which use the platform container).
            run_command: Option<String>,
            /// For a REMOTE resume/continue: `(project, claude_uuid)` so the
            /// reader thread fetches the remote `.jsonl` over the session's own
            /// connection and broadcasts a TranscriptBackfill. `None` for local
            /// sessions (core reads their JSONL directly) and fresh sessions
            /// (no prior history to fetch).
            transcript_hint: Option<(String, Option<String>)>,
            /// WAVE 3b: set when this command was a `PtyStart` that
            /// `handle_ipc` accepted (known session) — `(session_id, cols,
            /// rows, project, host_target)`, resolved from the raw inbound
            /// line + a store lookup (only this — the serve — thread has
            /// `self`). The accept-thread's reply handling spawns the real
            /// pty from this, mirroring `new_session_id`'s FIFO-reader
            /// handoff (see `pty::spawn_pty_session`'s doc comment for why
            /// the spawn can't happen inside `handle_ipc` itself).
            pty_start: Option<(String, String, u16, u16, String, Option<String>)>,
            /// Stage 2 codex: driver string for the new session ("codex"/"claude").
            #[allow(dead_code)]
            driver: String,
            /// Stage 2 codex: cached spec for the reader thread's first-prompt delivery.
            #[allow(dead_code)]
            codex_spec: Option<super::CachedCodexSpec>,
            /// Stage 2 codex: shared thread/turn state map (Arc clone from Agent).
            #[allow(dead_code)]
            codex_threads: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, crate::CodexThread>>>,
            /// opencode ACP: cached spec for the reader thread's ACP handshake.
            #[allow(dead_code)]
            opencode_spec: Option<super::CachedOpencodeSpec>,
            /// opencode ACP: shared ACP sessionId map (Arc clone from Agent).
            #[allow(dead_code)]
            acp_sessions: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>,
        }

        // The command sender is cloned into each client thread.
        let (cmd_tx, cmd_rx) = mpsc::channel::<AgentCmd>();

        // ── Broadcast list ─────────────────────────────────────────────────
        // Arc<Mutex<Vec<Sender>>> — every connected client has one slot.
        // Session FIFO reader threads and the client reader thread on disconnect
        // both hold a clone of this Arc.
        let subscribers: Arc<Mutex<Vec<mpsc::Sender<String>>>> = Arc::new(Mutex::new(Vec::new()));

        // ── Session writers registry ───────────────────────────────────────
        // Shared across the serve thread (handle_send_prompt via self) and the
        // FIFO threads (which register the writer after connect).
        // `session_writers_arc` is a clone of the same Arc stored in
        // `self.session_writers.0`, so handle_send_prompt can look up writers
        // by session_id via `self.session_writers.0` while FIFO threads
        // insert via `session_writers_for_fifo`.
        let session_writers_for_fifo = Arc::clone(&self.session_writers.0);

        // ── Pending-prompt queue ───────────────────────────────────────────
        // Shared across the serve thread (handle_send_prompt enqueues when no
        // writer is registered) and the reader threads (which flush on register).
        let session_pending_prompts_for_fifo = Arc::clone(&self.pending_prompts);

        // ── Session children registry ──────────────────────────────────────
        // Shared across the serve thread (kill_container via self) and the
        // reader threads (which insert the spawned remote child after spawn).
        let session_children_for_reader = Arc::clone(&self.session_children.0);

        // ── Pending-gate registry ──────────────────────────────────────────
        // Shared across the serve thread (handle_answer_gate via self) and the
        // FIFO threads (which insert when claude emits a `can_use_tool`).
        let pending_gates_for_fifo = Arc::clone(&self.pending_gates.0);

        // ── Stage 2 codex thread_id registry ───────────────────────────────
        // Shared across the serve thread (handle_send_prompt, which reads
        // thread_id to build resume commands) and the FIFO reader threads
        // (which insert thread_id when thread.started arrives from codex).
        let codex_threads_for_reader = Arc::clone(&self.codex_threads.0);

        // ── opencode ACP sessionId registry ────────────────────────────────
        // Shared across the serve thread (handle_send_prompt, handle_interrupt,
        // handle_answer_gate — all read the ACP sessionId) and the FIFO reader
        // threads (which insert the sessionId when the session/new result arrives).
        let acp_sessions_for_reader = Arc::clone(&self.acp_sessions.0);

        // ── WAVE 3b pty registry ────────────────────────────────────────────
        // Shared across the serve thread (handle_pty_input/handle_pty_resize
        // via self, handle_stop_session's teardown via self) and the pty
        // spawn/pump thread (`pty::spawn_pty_session`, which inserts on
        // spawn and removes on exit — see `pty.rs`).
        let pty_entries_for_reader = Arc::clone(&self.pty_entries.0);

        let data_root = self.config.data_root.clone();

        // ── U1 dynamic capability discovery ──────────────────────────────────
        // One provider for the whole daemon lifetime: loads any persisted
        // <data_root>/capabilities/<host>.json synchronously, then spawns a
        // background full refresh (§3.2 trigger 1). "local" is the only known
        // host wired here; remote hosts arrive via explicit RefreshCapabilities
        // requests (host_id from the registry) once the GUI asks for them.
        let capability_provider = Arc::new(CapabilityProvider::new(
            vec![
                Box::new(RealClaudeProbe),
                Box::new(RealCodexProbe),
                Box::new(RealAgyProbe),
                Box::new(RealOpencodeProbe),
            ],
            vec!["local".to_string()],
            std::path::Path::new(&data_root),
        ));

        // ── Reconnect sessions that survived an agent restart ──────────────────
        // The agent is a detached daemon; if it died (e.g. killed on reinstall) but
        // a session container is still alive, re-attach a reader to each live
        // session and re-track it so prompt/stop work and the conversation CONTINUES.
        // `first_prompt: None` and `send_init: false` — the claude is already
        // running and initialized; we only re-attach to its stream.
        //
        // LOCAL sessions: reconcile via tmux/shim container as before.
        // REMOTE sessions: durable (FIFO + setsid) — probe liveness via
        // `remote_liveness_script`, then re-attach with writer + tail children.
        {
            use marsad_lifecycle::Action;
            let restored = self
                .store
                .0
                .restore()
                .map(|r| r.sessions)
                .unwrap_or_default();

            // Probe ALL remote sessions' liveness in PARALLEL with a bounded wait,
            // BEFORE touching `self`. The probes are blocking remote calls; doing
            // them sequentially on this (the serve) thread — as the code used to —
            // froze the whole agent if any persisted session lived on a host that
            // blocks (a Duo/MFA host with no live ControlMaster sits at the prompt
            // forever). That manifested as "no sessions start, nothing appears at
            // all": the command loop below never ran. Now each probe runs in its own
            // thread with a hard timeout; any that don't answer in time are treated
            // DEAD and skipped, so startup completes regardless of one bad host.
            let mut live_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
            {
                let remote: Vec<_> = restored
                    .iter()
                    .filter(|s| {
                        s.host_target
                            .as_deref()
                            .map(|h| exec_prefix_for(Some(h)).is_some())
                            .unwrap_or(false)
                    })
                    .cloned()
                    .collect();
                let n = remote.len();
                if n > 0 {
                    let (tx, rx) = mpsc::channel::<(String, bool)>();
                    for s in &remote {
                        let host = s.host_target.clone().unwrap_or_default();
                        let id = s.id.clone();
                        let tx = tx.clone();
                        std::thread::spawn(move || {
                            // Startup reconcile: only a definitive LIVE counts as
                            // Resumed. Unreachable/Dead both start the session
                            // Paused here; the reader's state machine corrects an
                            // Unreachable host to Disconnected once it attaches.
                            let live = crate::remote::probe_liveness(
                                &host,
                                &id,
                                std::time::Duration::from_secs(6),
                            ) == crate::remote::Liveness::Live;
                            let _ = tx.send((id, live));
                        });
                    }
                    drop(tx);
                    // Each bounded probe self-terminates within ~6s, so the whole
                    // collect finishes in ~6s (parallel) even with many hosts; the
                    // 8s per-recv cap is a belt-and-suspenders backstop.
                    for _ in 0..n {
                        match rx.recv_timeout(std::time::Duration::from_secs(8)) {
                            Ok((id, live)) => {
                                eprintln!(
                                    "marsad-agent: remote session {id} liveness: {}",
                                    if live { "LIVE" } else { "DEAD" }
                                );
                                if live {
                                    live_ids.insert(id);
                                }
                            }
                            Err(_) => break,
                        }
                    }
                }
            }

            for s in restored {
                if let Some(host) = s.host_target.as_deref() {
                    if exec_prefix_for(Some(host)).is_some() {
                        // REMOTE session: reconnect if the parallel probe found it live.
                        let live = live_ids.contains(&s.id);
                        if live {
                            self.sessions.insert(
                                s.id.clone(),
                                SessionRecord {
                                    marsad_id: s.id.clone(),
                                },
                            );
                            // On reconnect: no run_command (already running), no
                            // first_prompt (already initialized), no send_init.
                            // transcript_hint: supply for resume so backfill fires.
                            let transcript_hint = s
                                .session_id
                                .as_ref()
                                .filter(|_| true) // always try backfill on reconnect
                                .map(|uuid| (s.project.clone(), Some(uuid.clone())))
                                .or_else(|| Some((s.project.clone(), None)));
                            // For a reconnected CODEX session, rebuild the
                            // `CachedCodexSpec` from the persisted record (the
                            // in-memory `codex_specs` map is empty after a
                            // restart) so a follow-up prompt can build a
                            // resume turn command. Insert it into `self.codex_specs`
                            // (read by `handle_send_prompt`) AND pass it to the
                            // reader. Claude reconnect is unchanged (spec = None).
                            let reconnect_codex_spec = if s.driver == "codex" {
                                let spec = super::cached_codex_spec_from_persisted(
                                    s.model.clone(),
                                    s.project.clone(),
                                    s.claude_bin.clone(),
                                    s.posture.as_deref(),
                                );
                                self.codex_specs
                                    .0
                                    .lock()
                                    .unwrap()
                                    .insert(s.id.clone(), spec.clone());
                                Some(spec)
                            } else {
                                None
                            };
                            spawn_session_reader(
                                s.id.clone(),
                                s.host_target.clone(),
                                None,
                                false, // already initialized
                                None,  // run_command: already running
                                transcript_hint,
                                data_root.clone(),
                                Arc::clone(&subscribers),
                                Arc::clone(&self.session_writers.0),
                                Arc::clone(&self.pending_prompts),
                                Arc::clone(&self.pending_gates.0),
                                Arc::clone(&self.session_children.0),
                                Some(Arc::clone(&capability_provider)),
                                s.driver.clone(),
                                reconnect_codex_spec, // rebuilt for codex, None for claude
                                Arc::clone(&codex_threads_for_reader),
                                None, // opencode_spec: not re-sent on reconnect
                                Arc::clone(&acp_sessions_for_reader),
                            );
                            eprintln!("marsad-agent: reconnected live remote session {}", s.id);
                        }
                        continue;
                    }
                }

                // LOCAL session: reconcile via tmux/shim container.
                let known = Persisted {
                    session_id: s.session_id.clone(),
                    container_name: format!("marsad-{}", s.id),
                    project: s.project.clone(),
                    model: s.model.clone(),
                };
                let action = self.reconcile_one(&known, s.host_target.as_deref()).action;
                eprintln!(
                    "marsad-agent: reconcile {} on {:?} -> {action:?}",
                    s.id, s.host_target
                );
                if matches!(action, Action::Adopt) {
                    self.sessions.insert(
                        s.id.clone(),
                        SessionRecord {
                            marsad_id: s.id.clone(),
                        },
                    );
                    // Reconnected CODEX session: rebuild + register the cached
                    // spec from the persisted record so `handle_send_prompt` can
                    // build a resume turn command for the next prompt (the
                    // in-memory map is empty after a restart). Claude unchanged.
                    let reconnect_codex_spec = if s.driver == "codex" {
                        let spec = super::cached_codex_spec_from_persisted(
                            s.model.clone(),
                            s.project.clone(),
                            s.claude_bin.clone(),
                            s.posture.as_deref(),
                        );
                        self.codex_specs
                            .0
                            .lock()
                            .unwrap()
                            .insert(s.id.clone(), spec.clone());
                        Some(spec)
                    } else {
                        None
                    };
                    spawn_session_reader(
                        s.id.clone(),
                        s.host_target.clone(),
                        None,
                        false,
                        None,
                        None, // reconnect path is local-only; no remote backfill
                        data_root.clone(),
                        Arc::clone(&subscribers),
                        Arc::clone(&self.session_writers.0),
                        Arc::clone(&self.pending_prompts),
                        Arc::clone(&self.pending_gates.0),
                        Arc::clone(&self.session_children.0),
                        Some(Arc::clone(&capability_provider)),
                        s.driver.clone(),
                        reconnect_codex_spec, // rebuilt for codex, None for claude
                        Arc::clone(&codex_threads_for_reader),
                        None, // opencode_spec: not re-sent on reconnect
                        Arc::clone(&acp_sessions_for_reader),
                    );
                    eprintln!("marsad-agent: reconnected live session {}", s.id);
                }
            }
        }

        // ── X2 Wasl connector wiring (docs/specs/wasl-connector.md §4.2) ────
        // Strictly optional (§6): absent `<data_root>/wasl.json` means zero
        // threads, zero connections, empty `wasl_connectors` map — the whole
        // block below is guarded by `Some(cfg)`.
        if let Some(wasl_cfg) = crate::load_wasl_config(&data_root) {
            for entry in wasl_cfg.operators {
                let mut connector: Box<dyn marsad_wasl::WaslConnector> =
                    match self.wasl_connector_factory.0.as_deref() {
                        Some(f) => f(&entry),
                        None => default_wasl_connector_factory(&entry),
                    };

                // `start()` needs `&mut` on the connector — call it while we
                // still exclusively own the `Box`, BEFORE registering it.
                // (Review finding #1: the registry stores `Arc<dyn
                // WaslConnector>` so `handle_answer_wasl_gate` can clone a
                // handle out and drop the map lock before the blocking
                // `decide()` call; `Arc` gives no safe way to get `&mut`
                // back out once shared, so this ordering — not a
                // lock-and-`get_mut` dance after insertion — is how `start`
                // gets its one-time mutable access.)
                let (wasl_tx, wasl_rx) = mpsc::channel::<marsad_wasl::WaslEvent>();
                connector.start(wasl_tx);
                self.register_wasl_connector(&entry.host_id, connector);

                // Forwarder thread: drains the connector's event channel and
                // pushes the mapped IPC lines into the subscriber broadcast
                // list — byte-identical mechanics to the session FIFO reader
                // threads (reader.rs's "Broadcast to all subscribers").
                let host_id = entry.host_id.clone();
                let subs_for_wasl = Arc::clone(&subscribers);
                std::thread::spawn(move || {
                    for event in wasl_rx {
                        for line in crate::wasl_event_lines(&host_id, &event) {
                            let subs = subs_for_wasl.lock().unwrap();
                            subs.iter().for_each(|s| {
                                let _ = s.send(line.clone());
                            });
                        }
                    }
                });

                eprintln!(
                    "marsad-agent: wasl connector started for host {} ({})",
                    entry.host_id, entry.url
                );
            }
        }

        // ── Accept thread ──────────────────────────────────────────────────
        // We spin off the accept loop in a separate thread so that the current
        // thread can run the agent command loop below.  The accept thread
        // never touches the `Agent`.
        let subs_accept = Arc::clone(&subscribers);
        let cmd_tx_accept = cmd_tx.clone();
        let capability_provider_accept = Arc::clone(&capability_provider);

        std::thread::spawn(move || {
            loop {
                let client = match listener.accept() {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("marsad-agent: accept error: {e}");
                        break;
                    }
                };

                // Per-client broadcast channel.
                // bcast_tx_for_reader — given to the reader thread so it can
                //   route ack/reply lines through the writer thread.
                // bcast_tx_for_subs   — pushed into the subscriber list so that
                //   FIFO reader threads can broadcast AgentEvents to this client.
                // Both are Sender clones of the same channel; bcast_rx is drained
                // exclusively by the writer thread.
                let (bcast_tx_for_reader, bcast_rx) = mpsc::channel::<String>();
                let bcast_tx_for_subs = bcast_tx_for_reader.clone();
                {
                    let mut subs = subs_accept.lock().unwrap();
                    subs.push(bcast_tx_for_subs);
                }

                // §3.2 trigger 2: replay every cached CapabilitiesUpdate to this
                // NEW subscriber immediately, then stream live broadcasts —
                // routed through the same writer-thread channel as every other
                // outbound line for this client.
                {
                    let cap_rx = capability_provider_accept.subscribe();
                    let cap_tx = bcast_tx_for_reader.clone();
                    std::thread::spawn(move || {
                        for line in cap_rx {
                            let _ = cap_tx.send(line);
                        }
                    });
                }

                let cmd_tx_client = cmd_tx_accept.clone();
                let subs_client = Arc::clone(&subs_accept);
                let data_root_client = data_root.clone();
                let subs_for_fifo = Arc::clone(&subs_accept);
                let writers_for_fifo = Arc::clone(&session_writers_for_fifo);
                let pending_prompts_for_fifo = Arc::clone(&session_pending_prompts_for_fifo);
                let pending_for_fifo = Arc::clone(&pending_gates_for_fifo);
                let children_for_reader = Arc::clone(&session_children_for_reader);
                let capability_provider_for_reader = Arc::clone(&capability_provider_accept);
                let pty_entries_for_client = Arc::clone(&pty_entries_for_reader);
                let codex_threads_for_client = Arc::clone(&codex_threads_for_reader);
                let acp_sessions_for_client = Arc::clone(&acp_sessions_for_reader);

                // Split the client into independent write and read halves so
                // each thread has exclusive ownership — no Mutex required.
                // All outbound bytes (acks, SessionLaunched, AgentEvents) flow
                // through bcast_tx_for_reader → bcast_rx → writer thread, which
                // is the sole owner of write_half.
                let (write_half, read_half) = match client.into_split() {
                    Some(pair) => pair,
                    None => {
                        eprintln!("marsad-agent: client socket not connected after accept");
                        continue;
                    }
                };

                // Writer thread: sole owner of write_half; drains bcast_rx.
                std::thread::spawn(move || {
                    use std::io::Write;
                    let mut w = write_half;
                    for line in bcast_rx {
                        // Skip empty probe lines used for subscriber-list pruning.
                        if line.is_empty() {
                            continue;
                        }
                        if w.write_all(line.as_bytes()).is_err()
                            || w.write_all(b"\n").is_err()
                            || w.flush().is_err()
                        {
                            break;
                        }
                    }
                });

                // Reader thread: sole owner of read_half; pushes all outbound
                // reply lines via bcast_tx_for_reader so the writer thread
                // serialises them to the socket.
                std::thread::spawn(move || {
                    use std::io::BufRead;
                    let mut r = read_half;
                    loop {
                        let mut raw = String::new();
                        let line_opt = r.read_line(&mut raw);

                        match line_opt {
                            Ok(0) | Err(_) => {
                                // Clean EOF or IO error — disconnect.
                                // Sessions/tmux are intentionally NOT killed.
                                break;
                            }
                            Ok(_) => {
                                // Trim the trailing newline.
                                let line = raw.trim_end_matches('\n').to_string();
                                if line.is_empty() {
                                    continue;
                                }

                                // Send the command to the agent thread and await reply.
                                let (reply_tx, reply_rx) = mpsc::sync_channel::<AgentReply>(0);
                                if cmd_tx_client.send(AgentCmd { line, reply_tx }).is_err() {
                                    // Agent thread gone — daemon shutting down.
                                    break;
                                }
                                let reply = match reply_rx.recv() {
                                    Ok(r) => r,
                                    Err(_) => break,
                                };

                                match reply.out_lines {
                                    Err(msg) => {
                                        let err_line = marsad_proto::encode_ipc(
                                            &marsad_proto::IpcMsg::Error { message: msg },
                                        );
                                        // Route error reply through writer thread.
                                        let _ = bcast_tx_for_reader.send(err_line);
                                    }
                                    Ok(out_lines) => {
                                        // Route ack/reply lines through writer thread.
                                        for out_line in &out_lines {
                                            let _ = bcast_tx_for_reader.send(out_line.clone());
                                        }

                                        // If a new session was launched, start a
                                        // FIFO reader thread for it. Defensive
                                        // dedup: never spawn a second reader onto a
                                        // session that already has a writer
                                        // registered (a reader is attached). The
                                        // agent-loop `resume_already_live` guard
                                        // already suppresses the common case
                                        // synchronously; this closes the narrow race
                                        // where a reader spawned moments earlier is
                                        // still connecting.
                                        let already_attached = reply
                                            .new_session_id
                                            .as_deref()
                                            .map(|sid| {
                                                writers_for_fifo.lock().unwrap().contains_key(sid)
                                            })
                                            .unwrap_or(false);
                                        if let Some(session_id) =
                                            reply.new_session_id.filter(|_| !already_attached)
                                        {
                                            spawn_session_reader(
                                                session_id,
                                                reply.host_target,
                                                reply.first_prompt,
                                                true, // launch: send the initialize handshake
                                                reply.run_command,
                                                reply.transcript_hint,
                                                data_root_client.clone(),
                                                Arc::clone(&subs_for_fifo),
                                                Arc::clone(&writers_for_fifo),
                                                Arc::clone(&pending_prompts_for_fifo),
                                                Arc::clone(&pending_for_fifo),
                                                Arc::clone(&children_for_reader),
                                                Some(Arc::clone(&capability_provider_for_reader)),
                                                reply.driver,
                                                reply.codex_spec,
                                                Arc::clone(&codex_threads_for_client),
                                                reply.opencode_spec,
                                                Arc::clone(&acp_sessions_for_client),
                                            );
                                        }

                                        // WAVE 3b: a `PtyStart` accepted by
                                        // `handle_ipc` — spawn the real pty +
                                        // its broadcast-pump thread now that
                                        // we're on a scope that owns
                                        // `subscribers` (see
                                        // `pty::spawn_pty_session`'s doc
                                        // comment).
                                        if let Some((sid, terminal_id, cols, rows, project, host_target)) =
                                            reply.pty_start
                                        {
                                            crate::pty::spawn_pty_session(
                                                sid,
                                                terminal_id,
                                                cols,
                                                rows,
                                                project,
                                                host_target,
                                                Arc::clone(&subs_for_fifo),
                                                Arc::clone(&pty_entries_for_client),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // On disconnect: prune dead subscriber senders.
                    // Sessions/tmux are intentionally left alive (liveness).
                    let mut subs = subs_client.lock().unwrap();
                    subs.retain(|s| s.send(String::new()).is_ok());
                });
            }
        });

        // ── Agent command loop (runs on the serve thread) ──────────────────
        // Owns `self` (the Agent).  Processes one command at a time.
        for cmd in cmd_rx {
            // U1: RefreshCapabilities is handled entirely by the capability
            // provider, off `self` — the ack returns immediately and matching
            // probes run on their own background threads (§4), never blocking
            // this command loop (which every other session's launch/prompt/gate
            // traffic also depends on).
            if cmd.line.contains("\"RefreshCapabilities\"") {
                let v: serde_json::Value =
                    serde_json::from_str(&cmd.line).unwrap_or(serde_json::Value::Null);
                let host_id = v
                    .get("host_id")
                    .and_then(|s| s.as_str())
                    .map(str::to_string);
                let driver = v.get("driver").and_then(|s| s.as_str()).map(str::to_string);
                let project = v
                    .get("project")
                    .and_then(|s| s.as_str())
                    .map(str::to_string);
                // Host launch config (deliverable #1): the remote target + the
                // pre-command + the per-driver command overrides, so the probe
                // runs setup-first over the RIGHT connection with the EXACT
                // command a launch would use. All optional (absent → local,
                // no-setup, bare command names).
                let host_target = v
                    .get("host_target")
                    .and_then(|s| s.as_str())
                    .map(str::to_string);
                let setup = v.get("setup").and_then(|s| s.as_str()).map(str::to_string);
                let claude_bin = v
                    .get("claude_bin")
                    .and_then(|s| s.as_str())
                    .map(str::to_string);
                let agent_commands: Vec<(String, String)> = v
                    .get("agent_commands")
                    .and_then(|a| a.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|pair| {
                                let p = pair.as_array()?;
                                Some((p.first()?.as_str()?.to_string(), p.get(1)?.as_str()?.to_string()))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let cfg = crate::capabilities::HostLaunchCfg {
                    host_target,
                    setup,
                    agent_commands,
                    claude_bin,
                };
                let ack = capability_provider.handle_refresh(host_id, driver, project, cfg);
                let _ = cmd.reply_tx.send(AgentReply {
                    out_lines: Ok(vec![ack]),
                    new_session_id: None,
                    first_prompt: None,
                    host_target: None,
                    run_command: None,
                    transcript_hint: None,
                    pty_start: None,
                    driver: "claude".to_string(),
                    codex_spec: None,
                    codex_threads: Arc::clone(&self.codex_threads.0),
                    opencode_spec: None,
                    acp_sessions: Arc::clone(&self.acp_sessions.0),
                });
                continue;
            }

            let is_launch = cmd.line.contains("\"LaunchSession\"");
            let is_resume = cmd.line.contains("\"ResumeSession\"");

            // Double-launch guard (see handle_resume_session). Capture BEFORE
            // handle_ipc whether the resume target is already tracked live: if so,
            // the handler no-ops (no relaunch) and we must NOT extract a
            // new_session_id below — otherwise the client thread would spawn a
            // second reader onto the already-attached session. `self.sessions` is
            // the authoritative synchronous signal (populated by startup reconnect
            // before this loop runs).
            let resume_already_live = is_resume
                && serde_json::from_str::<serde_json::Value>(&cmd.line)
                    .ok()
                    .and_then(|v| v["session_id"].as_str().map(|s| s.to_string()))
                    .map(|sid| self.sessions.contains_key(&sid))
                    .unwrap_or(false);

            // For a launch OR resume, capture the first_prompt BEFORE handle_ipc
            // consumes the JSON — we pass it to the FIFO thread via AgentReply so
            // a resumed/continued session can be handed a new prompt on reconnect.
            let launch_first_prompt: Option<String> = if is_launch || is_resume {
                serde_json::from_str::<serde_json::Value>(&cmd.line)
                    .ok()
                    .and_then(|v| v["first_prompt"].as_str().map(|s| s.to_string()))
            } else {
                None
            };

            let result = self.handle_ipc(&cmd.line);

            let (out_lines, new_session_id, first_prompt) = match result {
                Err(e) => (Err(format!("{e}")), None, None),
                Ok(lines) => {
                    // Extract a new session_id from the response when the
                    // command was a launch or resume — so the accept thread
                    // can open the FIFO reader.
                    let sid = if (is_launch || is_resume) && !resume_already_live {
                        lines.first().and_then(|l| {
                            serde_json::from_str::<serde_json::Value>(l)
                                .ok()
                                .and_then(|v| v["session_id"].as_str().map(|s| s.to_string()))
                        })
                    } else {
                        // Already-live resume: handler no-op'd (no relaunch), so do
                        // not spawn a duplicate reader for the still-attached session.
                        None
                    };

                    // Register the session in the writers registry now that we
                    // know its session_id.  The actual File handle is added by
                    // the FIFO thread once it connects.  We pass first_prompt
                    // via AgentReply so the FIFO thread writes it immediately
                    // after connecting — before the reader loop starts.
                    // The registry entry itself is created by the FIFO thread
                    // once it connects, using the Arc clone passed to it.
                    let prompt = if sid.is_some() {
                        launch_first_prompt
                    } else {
                        None
                    };
                    (Ok(lines), sid, prompt)
                }
            };

            // Resolve the session's remote target (if any) so the reader thread
            // picks the right transport. The session was just persisted by
            // launch_with_id, so the store has its host_target.
            let host_target = new_session_id
                .as_deref()
                .and_then(|s| self.session_host_target(s));

            // Pop the remote run command stashed by launch_with_id (None for a
            // local session). The reader thread spawns it as the single piped
            // child. Serve processes commands sequentially, so this never races.
            let run_command = new_session_id
                .as_deref()
                .and_then(|s| self.session_run_cmds.0.lock().unwrap().remove(s));

            // For a REMOTE resume/continue, tell the reader thread to backfill the
            // transcript from the remote host. `wants_history` is true for a
            // ResumeSession IPC, or a LaunchSession carrying resume_session_id /
            // continue (NOT a fresh launch — that has no prior conversation).
            let wants_history = is_resume
                || serde_json::from_str::<serde_json::Value>(&cmd.line)
                    .ok()
                    .map(|v| {
                        v["resume_session_id"]
                            .as_str()
                            .map(|s| !s.is_empty())
                            .unwrap_or(false)
                            || v["continue"].as_bool().unwrap_or(false)
                    })
                    .unwrap_or(false);
            let transcript_hint = if wants_history && host_target.is_some() {
                new_session_id
                    .as_deref()
                    .and_then(|s| self.session_transcript_project_uuid(s))
            } else {
                None
            };

            // WAVE 3b: a `PtyStart` that `handle_ipc` just accepted (known
            // session — an error reply here means `out_lines` is `Err`, so
            // `is_ok()` gates spawning a pty for an unknown session). Parsed
            // straight off the raw inbound line (mirrors `launch_first_prompt`
            // above) since `handle_ipc`'s typed match arm drops `cols`/`rows`
            // (see `ipc.rs::handle_pty_start`'s doc comment) — the project +
            // host_target are resolved here, the only scope with `self`.
            let pty_start: Option<(String, String, u16, u16, String, Option<String>)> =
                if cmd.line.contains("\"PtyStart\"") && out_lines.is_ok() {
                    serde_json::from_str::<serde_json::Value>(&cmd.line)
                        .ok()
                        .and_then(|v| {
                            let sid = v.get("session_id")?.as_str()?.to_string();
                            // v2: terminal_id is optional (empty → session keys the pty).
                            let terminal_id = v
                                .get("terminal_id")
                                .and_then(|t| t.as_str())
                                .unwrap_or("")
                                .to_string();
                            let cols = v.get("cols")?.as_u64()? as u16;
                            let rows = v.get("rows")?.as_u64()? as u16;
                            let (project, host_target) =
                                self.session_project_and_host_target(&sid)?;
                            Some((sid, terminal_id, cols, rows, project, host_target))
                        })
                } else {
                    None
                };

            // Stage 2 codex: look up driver + cached spec for the new session
            // so the reader thread can build first-turn commands + translate events.
            let (session_driver, session_codex_spec, session_opencode_spec) =
                if let Some(ref sid) = new_session_id {
                    let session_rec = self
                        .store
                        .0
                        .restore()
                        .ok()
                        .and_then(|r| r.sessions.into_iter().find(|s| &s.id == sid));
                    let drv = session_rec
                        .as_ref()
                        .map(|s| s.driver.clone())
                        .unwrap_or_else(|| "claude".to_string());
                    let cs = if drv == "codex" {
                        self.codex_specs.0.lock().unwrap().get(sid.as_str()).cloned()
                    } else {
                        None
                    };
                    let os = if drv == "opencode" {
                        session_rec.map(|s| super::CachedOpencodeSpec {
                            model: s.model.clone(),
                            project: s.project.clone(),
                            // For a Resume(uuid) launch, the stored session_id is the
                            // ACP sessionId to use for session/load. For Fresh/Continue,
                            // it is None → reader sends session/new.
                            acp_session_id: s.session_id.clone(),
                        })
                    } else {
                        None
                    };
                    (drv, cs, os)
                } else {
                    ("claude".to_string(), None, None)
                };

            // Send reply; if the client thread is gone, ignore the error.
            let _ = cmd.reply_tx.send(AgentReply {
                out_lines,
                new_session_id,
                first_prompt,
                host_target,
                run_command,
                transcript_hint,
                pty_start,
                driver: session_driver,
                codex_spec: session_codex_spec,
                codex_threads: Arc::clone(&self.codex_threads.0),
                opencode_spec: session_opencode_spec,
                acp_sessions: Arc::clone(&self.acp_sessions.0),
            });
        }

        Ok(())
    }
}
