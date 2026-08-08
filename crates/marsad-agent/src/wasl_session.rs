//! X6 — Wasl **Backend** (Wasl-managed session) agent wiring: the
//! `WaslSessionProvider`/`WaslSessionClient` registry + factory test seams, the
//! six inbound `*Wasl*` `handle_ipc` arms, and the two pure observe-thread
//! mapping helpers (`wasl_session_event_lines`/`wasl_session_status_line`).
//!
//! Authoritative design: `docs/design/wasl-backend-plan.md` (v2) §6.1, §7, §8,
//! §8.1. STRUCTURALLY IDENTICAL to the X1 operator wiring in `wasl.rs`: the
//! registry mirrors `wasl_connectors`, the factory/registration seams mirror
//! `set_wasl_connector_factory`/`register_wasl_connector`, and — critically —
//! every arm that calls a potentially-blocking `WaslSessionClient` method
//! follows `handle_answer_wasl_gate`'s lock-hygiene pattern EXACTLY: take the
//! map lock, clone the `Arc<dyn WaslSessionClient>` out, DROP the lock, THEN
//! call. No registry/map lock is ever held across a blocking client call (§8.1,
//! a numbered test requirement).

use marsad_proto::{encode_ipc, IpcMsg};
use marsad_wasl_session::{
    new_idempotency_key, WaslBusyPolicy, WaslCancelReason, WaslCausation, WaslGateDecision,
    WaslSessionClient, WaslSessionEvent, WaslSessionEventKind, WaslShutdownMode, WaslStreamItem,
    WaslTargetKey, WaslTargetStatus, WaslTurnId, WaslTurnRequest,
};

use super::{Agent, AgentError};

// ---------------------------------------------------------------------------
// §7 — the two pure observe-thread mapping helpers (crate-root re-exported)
// ---------------------------------------------------------------------------

/// Pure mapping (unit-testable without sockets / observe threads): one
/// normalized `WaslSessionEvent` -> the `WaslSessionEvent` IPC broadcast line
/// the observe thread emits (§7). Value-carried on purpose (the typed form lives
/// in `marsad-wasl-session`; `marsad-proto` must not depend on it), so core
/// re-parses the `event` Value back to the typed event when folding.
///
/// The analogue of X1's `wasl_event_lines`: the observe THREAD is spawned only
/// in `serve()` (real-IO, `#[cfg(not(coverage))]`), so this pure mapping is what
/// is proven, and the re-pump wiring is proven via the synchronous `handle_ipc`
/// arms.
pub fn wasl_session_event_lines(session_id: &str, seq: u64, ev: &WaslSessionEvent) -> Vec<String> {
    vec![encode_ipc(&IpcMsg::WaslSessionEvent {
        session_id: session_id.to_string(),
        seq,
        event: serde_json::to_value(ev).expect("WaslSessionEvent always serializes"),
    })]
}

/// Pure mapping: a Wasl advisory status -> its `WaslSessionStatus` broadcast line
/// (§7). Value-carried like the event mapping above; core projects the status to
/// `Phase` (§6.3).
pub fn wasl_session_status_line(session_id: &str, st: &WaslTargetStatus) -> String {
    encode_ipc(&IpcMsg::WaslSessionStatus {
        session_id: session_id.to_string(),
        status: serde_json::to_value(st).expect("WaslTargetStatus always serializes"),
    })
}

// ---------------------------------------------------------------------------
// §6.1 — the observe-thread pump (pure, always-compiled, unit-tested) + the
// reconnect-policy / reconnect-after_seq decisions it leans on.
//
// The spawned observe THREAD + real subscriber broadcast list + backoff sleep
// live in the `#[cfg(not(coverage))]` `serve()` layer (exactly like
// `reader.rs`'s `spawn_session_reader`); the LOAD-BEARING pump logic is
// extracted here as a pure, synchronous helper proven against the in-repo
// `FakeWaslSessionClient` (the analogue of `reconnect_backoff_next` /
// `conn_on_drop`). No socket, no real process — the fake IS the substrate.
// ---------------------------------------------------------------------------

/// The verdict `pump_wasl_observe` returns when its stream ends: the resume
/// cursor (highest emitted seq) and whether a TERMINAL event (`SessionExited`)
/// was seen before the end (§6.1). Drives `wasl_observe_should_reconnect`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaslObserveEnd {
    /// Highest event seq emitted — the resume cursor for a reconnect, and what a
    /// `WaslSessionBinding.last_seq` would persist (§6.1).
    pub last_seq: u64,
    /// True iff a TERMINAL event (`SessionExited`) was observed before the stream
    /// ended — the session is genuinely gone.
    pub terminal: bool,
}

/// Pump ONE observe subscription end to end (§6.1): call `client.attach(after_seq)`,
/// loop its `next()`, and for each item:
///
/// * `WaslStreamItem::Event(ev)` → emit every line `wasl_session_event_lines`
///   produces (so a launched session's reply / transcript / native-gate / status
///   change reaches core), advance the high-water seq to `ev.seq`, and note a
///   TERMINAL if the event is a `SessionExited`.
/// * `WaslStreamItem::Truncated{latest, ..}` → NOT emitted as an event line;
///   instead RE-ATTACH from `latest` (discarding the old stream) and keep
///   pumping — a retention-window truncation is transparently recovered (§4.4).
///   A stale truncation is never re-surfaced (the fake/real stream only yields a
///   truncation whose window is below the current cursor), so the recovery
///   converges.
/// * `None` (clean end-of-stream) → return the high-water seq + terminal verdict.
///
/// Pure + synchronous: the caller (the `serve()` observe thread) drives it in a
/// loop, broadcasting each `emit`ted line to subscribers and reconnecting per
/// `wasl_observe_should_reconnect` when it returns non-terminal.
pub fn pump_wasl_observe(
    session_id: &str,
    client: &dyn WaslSessionClient,
    after_seq: u64,
    emit: &mut dyn FnMut(String),
) -> WaslObserveEnd {
    let mut last_seq = after_seq;
    let mut terminal = false;
    let mut stream = client.attach(after_seq);
    loop {
        match stream.next() {
            Some(WaslStreamItem::Event(ev)) => {
                let seq = ev.seq;
                if matches!(ev.kind, WaslSessionEventKind::SessionExited { .. }) {
                    terminal = true;
                }
                for line in wasl_session_event_lines(session_id, seq, &ev) {
                    emit(line);
                }
                last_seq = seq;
            }
            // A truncation is a control signal, not an event: re-attach from
            // `latest` and keep delivering the events that live above it.
            Some(WaslStreamItem::Truncated { latest, .. }) => {
                stream = client.attach(latest);
            }
            None => {
                return WaslObserveEnd { last_seq, terminal };
            }
        }
    }
}

/// The reconnect-on-disconnect policy (§6.1): reconnect ⇔ the pump ended
/// NON-terminally (a bare transport disconnect while the Wasl session may still
/// be alive → re-`attach(last_seq)` to resume with no gap/dup). A stream that
/// ended AFTER a terminal `SessionExited` means the session is truly gone —
/// Marsad does not own the process, so reconnecting would attach to nothing.
/// The Wasl analogue of `reader.rs::conn_on_probe`'s "Dead never reconnects".
pub fn wasl_observe_should_reconnect(end: &WaslObserveEnd) -> bool {
    !end.terminal
}

/// Decide the `after_seq` the observe pump must (re)attach from on a §6.1
/// reconnect, given the `snapshot.last_seq` `get_or_start` returned and the seq
/// Marsad persisted for this binding. There is NO `created` flag in real Wasl:
///
/// * `snapshot_last_seq >= persisted` → the SAME still-alive session → resume
///   from `persisted` (Wasl's `events(after_seq)` replay is gap/dup-free);
/// * `snapshot_last_seq  < persisted` → the target was REPLACED/restarted (its
///   journal started over below what Marsad had folded) → treat it as fresh:
///   reset local seq tracking to `0` and re-attach from `0`.
pub fn wasl_reconnect_after_seq(snapshot_last_seq: u64, persisted_last_seq: u64) -> u64 {
    if snapshot_last_seq >= persisted_last_seq {
        persisted_last_seq
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// §6.1/§7 — provider registry + factory test seams (mirror wasl.rs's X1 seams)
// ---------------------------------------------------------------------------

impl Agent {
    /// Additive test seam (also the direct registration seam used by `serve()`'s
    /// real-IO wiring): register the `WaslSessionProvider` the agent uses to
    /// `get_or_start` clients on `LaunchWaslSession`. The analogue of
    /// `register_wasl_connector`. Registering a provider does NOT start any target
    /// or hand out a client — that only happens on a `LaunchWaslSession`.
    pub fn register_wasl_session_provider(
        &mut self,
        provider: Box<dyn marsad_wasl_session::WaslSessionProvider>,
    ) {
        self.wasl_session_provider.0 = Some(provider);
    }

    /// Additive test seam: set the zero-arg factory the agent uses to build its
    /// `WaslSessionProvider` on first launch. The direct analogue of
    /// `set_wasl_connector_factory`. Setter so no constructor changes. Setting a
    /// factory alone starts nothing — the provider is built lazily on the first
    /// `LaunchWaslSession` that needs it.
    pub fn set_wasl_session_provider_factory(&mut self, f: super::WaslSessionProviderFactoryFn) {
        self.wasl_session_provider_factory.0 = Some(f);
    }

    /// Obtain the session's registered client handle (an `Arc` clone), dropping
    /// the map lock before returning it — so callers NEVER hold the map lock
    /// across a blocking client call (§8.1). Absent session -> `Protocol` error
    /// (the whole no-session-launched surface: a fresh agent has an empty map).
    fn wasl_session_client(
        &self,
        session_id: &str,
    ) -> Result<std::sync::Arc<dyn WaslSessionClient>, AgentError> {
        let clients = self.wasl_session_clients.0.lock().unwrap();
        clients
            .get(session_id)
            .cloned()
            .ok_or_else(|| AgentError::Protocol(format!("no wasl-managed session '{session_id}'")))
    }

    /// Obtain an INDEPENDENTLY-OWNED client `Box` (which is `Send`, unlike the
    /// registry's `Arc<dyn WaslSessionClient>`) for the per-session observe thread,
    /// plus the `after_seq` that thread's first `pump_wasl_observe` must attach
    /// from. Create-or-reuse: `get_or_start` reattaches to the SAME target the
    /// preceding `LaunchWaslSession` registered (never a second competing target),
    /// so the observe thread pumps the same live session the synchronous IPC arms
    /// drive. The start seq is the §6.1 decision over the returned
    /// `snapshot.last_seq` and the seq core persisted for this binding.
    ///
    /// `serve()`-only (real IO): the observe thread it feeds is spawned from
    /// `serve()`, which is `#[cfg(not(coverage))]`. The pump/reconnect LOGIC it
    /// runs (`pump_wasl_observe` / `wasl_observe_should_reconnect` /
    /// `wasl_reconnect_after_seq`) is the pure, always-compiled, unit-tested core.
    #[cfg(not(coverage))]
    pub(crate) fn wasl_observe_client_and_after_seq(
        &self,
        wasl_target_key: serde_json::Value,
        persisted_last_seq: u64,
    ) -> Result<(Box<dyn WaslSessionClient>, u64), AgentError> {
        let key = WaslTargetKey(wasl_target_key);
        let (client, snapshot) = self
            .wasl_session_provider
            .0
            .as_ref()
            .ok_or_else(|| {
                AgentError::Protocol("no wasl session provider for observe".to_string())
            })?
            .get_or_start(&key)
            .map_err(|e| AgentError::Protocol(format!("observe get_or_start failed: {e}")))?;
        let after_seq = wasl_reconnect_after_seq(snapshot.last_seq, persisted_last_seq);
        Ok((client, after_seq))
    }

    // -----------------------------------------------------------------------
    // §7 — the six inbound `*Wasl*` handle_ipc arms
    // -----------------------------------------------------------------------

    /// B1 — Adopt-or-spawn the host's managed-runtime `wasl-mcpd`, then
    /// register the provider factory pointing at `http://<control_bind>/`.
    ///
    /// Idempotent: if a provider OR a provider-factory is ALREADY registered
    /// (the test-seam case — fake seams pre-set these to short-circuit spawn IO),
    /// returns `Ok(wasl_control_bind(host_id))` immediately without overwriting
    /// the pre-set seam.
    ///
    /// Spawn path (no seam pre-set):
    ///   1. Compute the deterministic `control_bind` via `wasl_control_bind(host_id)`.
    ///   2. Render + write the TOML config to `<data_root>/wasl-node-<host_id>.toml`.
    ///   3. Probe/adopt-or-spawn `wasl-mcpd-runtime-<host_id>` via the Container
    ///      seam (same pattern as `adopt_or_spawn_wasl_mcpd`).
    ///   4. Register a `set_wasl_session_provider_factory` factory that builds a
    ///      `RealWaslSessionProvider` pointed at `http://<control_bind>/`.
    ///   5. Return `Ok(base_url)`.
    ///
    /// Called at the top of `handle_launch_wasl_session` before `get_or_start`.
    ///
    /// `host_target` selects LOCAL vs REMOTE:
    ///
    /// * `None` (or a local sentinel — `"local"`/`"localhost"`/empty, i.e. any
    ///   value `exec_prefix_for` treats as local): the node is spawned on THIS
    ///   machine via the Container seam and the daemon dials the node's own
    ///   loopback control_bind directly. Byte-identical to the pre-remote
    ///   behavior.
    /// * `Some(ssh_target)` (a real SSH/WSL target): the node is spawned on the
    ///   REMOTE host over that connection (durable `wasl-mcpd` holding stdin
    ///   open), `ssh -L` tunnels are opened to its control/operator/a2a loopback
    ///   binds, and the provider + operator connector are registered at the LOCAL
    ///   tunnel endpoints — see [`Self::ensure_remote_wasl_runtime_node`].
    pub fn ensure_wasl_runtime_node(
        &mut self,
        host_id: &str,
        host_target: Option<&str>,
        agent_commands: &[(String, String)],
        claude_bin: Option<&str>,
    ) -> Result<String, AgentError> {
        // The endpoint a caller dials to reach this node's control API — needed
        // both for the idempotent early-return and (local branch) the spawn path.
        // For a LOCAL node that's the node's own loopback `control_bind`; for a
        // REMOTE node the node binds on the remote host and Marsad reaches it via
        // an `ssh -L` tunnel, so the caller must dial the LOCAL tunnel port. The
        // early-return below (factory already registered) MUST hand back the SAME
        // endpoint the first call returned — otherwise a repeat launch/adopt of a
        // remote host returns the remote port and the client dials an unreachable
        // address (i15/S2 regression).
        let is_remote = host_target
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| crate::remote::exec_prefix_for(Some(t)).is_some())
            .unwrap_or(false);
        // A plain `wsl:` host is reached via WSL2 localhost forwarding: Marsad dials
        // the node's OWN loopback control bind directly (no ssh -L tunnel). Any
        // other remote is a real SSH host: dial the LOCAL tunnel endpoint.
        let is_wsl_direct = host_target
            .map(crate::wasl::is_wsl_direct_target)
            .unwrap_or(false);
        let base_url = if is_wsl_direct {
            format!(
                "http://{}",
                crate::wasl::wasl_remote_bind(host_id, crate::wasl::WaslBindKind::Control)
            )
        } else if is_remote {
            format!(
                "http://127.0.0.1:{}",
                crate::wasl::wasl_local_tunnel_port(host_id, crate::wasl::WaslBindKind::Control)
            )
        } else {
            crate::wasl::wasl_control_bind(host_id)
        };

        // Test-seam / serve()-pre-set precedence: if a provider or factory is
        // already registered, return immediately with the real base_url — do NOT
        // overwrite the pre-set seam with a new factory. (Identical for local and
        // remote: once a node is up and its factory registered, a repeat launch is
        // a cheap no-op regardless of where the node lives.)
        if self.wasl_session_provider.0.is_some()
            || self.wasl_session_provider_factory.0.is_some()
        {
            return Ok(base_url);
        }

        // REMOTE branch: a real SSH/WSL target. The Container seam only manages
        // LOCAL processes, so a remote node is spawned + tunneled over the SAME
        // remote seams solo remote sessions use (run_remote_script + ssh children).
        // A local sentinel target falls through to the local branch below.
        #[cfg(not(coverage))]
        if let Some(target) = host_target
            .filter(|t| crate::remote::exec_prefix_for(Some(t)).is_some())
        {
            return self.ensure_remote_wasl_runtime_node(
                host_id,
                target,
                agent_commands,
                claude_bin,
            );
        }
        #[cfg(coverage)]
        let _ = host_target;

        // Resolve per-protocol bins from agent_commands (prefer the explicit
        // claude_bin arg, then fall back to agent_commands lookup).
        let claude_bin_resolved = claude_bin.or_else(|| {
            agent_commands
                .iter()
                .find(|(k, _)| k == "claude")
                .map(|(_, v)| v.as_str())
        });
        let opencode_bin_resolved = agent_commands
            .iter()
            .find(|(k, _)| k == "opencode")
            .map(|(_, v)| v.as_str());
        let codex_bin_resolved = agent_commands
            .iter()
            .find(|(k, _)| k == "codex")
            .map(|(_, v)| v.as_str());

        // Compute the deterministic A2A + operator bind addresses and agent_id.
        let agent_id = format!("marsad-{host_id}");
        let a2a_bind = crate::wasl::wasl_a2a_bind(host_id);
        let operator_bind = crate::wasl::wasl_operator_bind(host_id);

        eprintln!(
            "marsad-agent: wasl-node: host={host_id} control_bind={base_url} \
             a2a_bind={a2a_bind} operator_bind={operator_bind} agent_id={agent_id}"
        );

        // Render the TOML config and write it to data_root.
        let toml_cfg = crate::wasl::WaslNodeConfig {
            agent_id: &agent_id,
            host_id,
            a2a_bind: &a2a_bind,
            operator_bind: &operator_bind,
            control_bind: &base_url,
            // Protocol selection: codex when only codex_bin is present (no claude, no
            // opencode); opencode when only opencode_bin is present (no claude);
            // claude otherwise (default — always has a fallback to PATH).
            default_protocol: crate::wasl::wasl_node_protocol(
                if codex_bin_resolved.is_some()
                    && claude_bin_resolved.is_none()
                    && opencode_bin_resolved.is_none()
                {
                    "codex"
                } else if opencode_bin_resolved.is_some() && claude_bin_resolved.is_none() {
                    "opencode"
                } else {
                    "claude"
                },
            ),
            claude_bin: claude_bin_resolved,
            opencode_bin: opencode_bin_resolved,
            codex_bin: codex_bin_resolved,
        };
        let toml_str = crate::wasl::render_wasl_node_toml(&toml_cfg);
        let config_path = std::path::Path::new(&self.config.data_root)
            .join(format!("wasl-node-{host_id}.toml"));
        let log_path = std::path::Path::new(&self.config.data_root)
            .join(format!("wasl-node-{host_id}.log"));
        // Best-effort write: if the data_root doesn't exist yet, create it.
        // Failure to write is logged but not fatal here — the node can still be
        // adopted if already running (the Container probe determines liveness).
        let _ = std::fs::create_dir_all(&self.config.data_root);
        let write_ok = std::fs::write(&config_path, &toml_str).is_ok();

        eprintln!(
            "marsad-agent: wasl-node: config={} write_ok={write_ok} node_log={}",
            config_path.display(),
            log_path.display()
        );

        // Probe / adopt-or-spawn the managed-runtime node via the Container seam.
        // Container call-log entries produced (matching adopt_or_spawn_wasl_mcpd):
        //   exists:wasl-mcpd-runtime-<host_id>
        //   attach:wasl-mcpd-runtime-<host_id>   (if exists)
        //   spawn:wasl-mcpd-runtime-<host_id> wasl-mcpd --config <path>   (if absent)
        let node_name = format!("wasl-mcpd-runtime-{host_id}");
        if self.active_container().exists(&node_name) {
            eprintln!("marsad-agent: wasl-node: adopting existing node {node_name}");
            let _ = self.active_container().attach(&node_name);
        } else {
            // Spawn the managed node EXACTLY like the proven remote recipe
            // (`remote_wasl_node_launch_script`, live-verified by i15/i16):
            //   sleep 100000 | WASL_CONFIG=<toml> wasl-mcpd 2>> <log>
            // Two load-bearing details (both were wrong before → a local managed
            // session hung forever because the node never served its control API):
            //   1. wasl-mcpd reads its config from the WASL_CONFIG **env var** — it
            //      does NOT accept a `--config` flag. An unknown flag is ignored and
            //      the daemon falls back to its DEFAULT config, which has no
            //      `[runtime] kind=managed`/`control_bind`, so the HTTP control /
            //      operator / a2a servers never start and `get_or_start`
            //      (POST /v1/targets) can never reach the node.
            //   2. `sleep 100000 |` holds stdin open as a non-TTY pipe so the
            //      stdio-MCP task never sees EOF (which tears the whole process down)
            //      and the tmux pane's PTY isn't handed to it as stdin.
            // Stderr → a durable log for post-mortem even if it exits.
            let spawn_cmd = if write_ok {
                format!(
                    "sleep 100000 | WASL_CONFIG=\"{}\" wasl-mcpd 2>> \"{}\"",
                    config_path.to_string_lossy(),
                    log_path.to_string_lossy()
                )
            } else {
                "sleep 100000 | wasl-mcpd".to_string()
            };
            eprintln!("marsad-agent: wasl-node: spawning node {node_name}: {spawn_cmd}");
            let _ = self.active_container().spawn(&node_name, &spawn_cmd);
        }

        // Real-IO only: the just-spawned node's HTTP control server is NOT listening
        // the instant `spawn` returns — the tmux child needs a moment to bind. Poll
        // the control bind until it accepts a TCP connection (bounded), so the
        // immediately-following `get_or_start` (POST /v1/targets) does not RACE the
        // bind and fail with a spurious `ECONNREFUSED` — which would leave the launch
        // with no target and the session hung forever waiting for a Done that never
        // comes (the exact managed-wasl bug this closes). On timeout we proceed
        // anyway so `get_or_start` surfaces a real typed error rather than a false
        // "up". Gated to real-IO: a FakeContainer unit test never spawns a real node,
        // so it must NOT wait — it expects the immediate `ECONNREFUSED`. Excluded
        // from the coverage build (no real node exists there).
        #[cfg(not(coverage))]
        if self.real_container.0.is_some() {
            if let Some(addr) = base_url
                .strip_prefix("http://")
                .and_then(|hp| hp.parse::<std::net::SocketAddr>().ok())
            {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
                let mut waited_ms = 0u64;
                loop {
                    if std::net::TcpStream::connect_timeout(
                        &addr,
                        std::time::Duration::from_millis(300),
                    )
                    .is_ok()
                    {
                        if waited_ms > 0 {
                            eprintln!(
                                "marsad-agent: wasl-node: control API up at {base_url} after {waited_ms}ms"
                            );
                        }
                        break;
                    }
                    if std::time::Instant::now() >= deadline {
                        eprintln!(
                            "marsad-agent: wasl-node: control API at {base_url} not up within 20s — proceeding (launch will surface a real error if still down)"
                        );
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    waited_ms += 200;
                }
            }
        }

        // Register a RealWaslSessionProvider factory pointed at the real
        // control_bind URL. In the unit-test environment (FakeContainer — no real
        // wasl-mcpd answering HTTP), `get_or_start` will hit the loopback port,
        // get ECONNREFUSED immediately, and return a graceful typed `Err` —
        // exactly what the node-less tests expect (`is_err()` + spawn logged).
        // In production the just-spawned node answers and the launch succeeds.
        let factory_base_url = base_url.clone();
        self.wasl_session_provider_factory.0 = Some(Box::new(move || {
            Box::new(marsad_wasl_session::RealWaslSessionProvider::new(
                marsad_wasl_session::WaslSessionClientConfig {
                    base_url: factory_base_url.clone(),
                },
            )) as Box<dyn marsad_wasl_session::WaslSessionProvider>
        }));

        eprintln!(
            "marsad-agent: wasl-node: factory registered for host {host_id} at {base_url}"
        );

        // Wire the X1 operator connector for this node so GateRequested/GateDecided
        // events from the node's operator API flow through wasl_event_lines into
        // the broadcast path. Uses the registered connector factory if set; otherwise
        // skips (serve() wires from load_wasl_config; tests pre-register via
        // register_wasl_connector). Best-effort: a missing factory never fails a
        // LaunchWaslSession.
        let operator_url = format!("http://{operator_bind}");
        self.ensure_wasl_operator_connector(host_id, &operator_url);

        Ok(base_url)
    }

    /// Resolve a logical `host_id` to its real SSH/WSL target from the persisted
    /// host registry (`PersistedHost.target`). A local host (`is_local`, or a
    /// `local`/`localhost`/empty target), an unknown `host_id`, or a store that
    /// won't restore all resolve to `None` — the caller then takes the LOCAL node
    /// spawn path. Always compiled (the launch path must resolve the target under
    /// coverage too; the store is present in every build). `pub` so the resolution
    /// (all three branches: local host, remote host, unknown host) is directly
    /// unit-tested via a store-seeded `Agent`.
    pub fn wasl_host_target(&mut self, host_id: &str) -> Option<String> {
        // The store's `restore()` can only fail on a backend IO error; every test
        // (and every real launch that reached this point) drives it against a live
        // backend, so the Err arm is unreachable from the launch path. Folded under
        // coverage with `.expect()` — the crate's established residual convention
        // for a call that never fails in the tested regime (see `real.rs`).
        #[cfg(not(coverage))]
        let state = self.store.0.restore().ok()?;
        #[cfg(coverage)]
        let state = self
            .store
            .0
            .restore()
            .expect("coverage-mode: the test backend always restores");
        let host = state.hosts.into_iter().find(|h| h.id == host_id)?;
        if host.is_local {
            return None;
        }
        let t = host.target.trim();
        if t.is_empty() || t == "local" || t == "localhost" {
            None
        } else {
            Some(t.to_string())
        }
    }

    /// REMOTE node spawn + tunnel wiring (real-IO only). Spawns a durable
    /// `wasl-mcpd` on `host_target` (holding stdin open so the stdio-MCP control
    /// task survives EOF — the crux recipe), opens per-host `ssh -L` tunnels to
    /// its control/operator/a2a loopback binds, and registers the provider factory
    /// + operator connector at the LOCAL tunnel endpoints (`http://127.0.0.1:<lport>`),
    /// NOT the remote bind.
    ///
    /// Reuses the existing remote seams — `run_remote_script` (config write +
    /// durable spawn over the host's connection) and `exec_prefix_for` (the tunnel
    /// argv), the SAME machinery solo remote sessions use — never a parallel SSH
    /// layer. Honest errors: an unreachable host or a missing `wasl-mcpd` on the
    /// remote surfaces a typed `Spawn` error, never a panic.
    ///
    /// Returns the LOCAL control tunnel endpoint (`http://127.0.0.1:<lport>`) as
    /// the base_url — the address the factory dials.
    #[cfg(not(coverage))]
    fn ensure_remote_wasl_runtime_node(
        &mut self,
        host_id: &str,
        host_target: &str,
        agent_commands: &[(String, String)],
        claude_bin: Option<&str>,
    ) -> Result<String, AgentError> {
        use crate::wasl::WaslBindKind;

        // Resolve per-protocol bins (prefer the explicit arg, then agent_commands).
        let claude_bin_resolved = claude_bin.or_else(|| {
            agent_commands
                .iter()
                .find(|(k, _)| k == "claude")
                .map(|(_, v)| v.as_str())
        });
        let opencode_bin_resolved = agent_commands
            .iter()
            .find(|(k, _)| k == "opencode")
            .map(|(_, v)| v.as_str());
        let codex_bin_resolved = agent_commands
            .iter()
            .find(|(k, _)| k == "codex")
            .map(|(_, v)| v.as_str());

        // The node's OWN loopback binds live on the REMOTE host (bare ip:port).
        let agent_id = format!("marsad-{host_id}");
        let remote_control = crate::wasl::wasl_remote_bind(host_id, WaslBindKind::Control);
        let remote_operator = crate::wasl::wasl_remote_bind(host_id, WaslBindKind::Operator);
        let remote_a2a = crate::wasl::wasl_remote_bind(host_id, WaslBindKind::A2a);

        // Reach strategy: a plain `wsl:` host is dialed DIRECTLY at the node's own
        // loopback binds — WSL2 forwards the guest's `127.0.0.1:<port>` to the same
        // address on the Windows host, so no `ssh -L` tunnel is opened (the
        // ssh-shaped tunnel does not fit the `wsl.exe` transport). Any other remote
        // is a real SSH host: open tunnels and dial the LOCAL tunnel endpoints.
        let is_wsl_direct = crate::wasl::is_wsl_direct_target(host_target);
        let l_control = crate::wasl::wasl_local_tunnel_port(host_id, WaslBindKind::Control);
        let l_operator = crate::wasl::wasl_local_tunnel_port(host_id, WaslBindKind::Operator);
        let l_a2a = crate::wasl::wasl_local_tunnel_port(host_id, WaslBindKind::A2a);
        let (base_url, operator_local_url) = if is_wsl_direct {
            (
                format!("http://{remote_control}"),
                format!("http://{remote_operator}"),
            )
        } else {
            (
                format!("http://127.0.0.1:{l_control}"),
                format!("http://127.0.0.1:{l_operator}"),
            )
        };

        eprintln!(
            "marsad-agent: wasl-node[remote {host_target:?}]: host={host_id} wsl_direct={is_wsl_direct} \
             remote control={remote_control} operator={remote_operator} a2a={remote_a2a} \
             dial control={base_url} operator={operator_local_url}"
        );

        // Render the node TOML with the REMOTE binds (bare ip:port).
        let toml_cfg = crate::wasl::WaslNodeConfig {
            agent_id: &agent_id,
            host_id,
            a2a_bind: &remote_a2a,
            operator_bind: &remote_operator,
            control_bind: &remote_control,
            // Mirror the local branch's protocol selection logic exactly.
            default_protocol: crate::wasl::wasl_node_protocol(
                if codex_bin_resolved.is_some()
                    && claude_bin_resolved.is_none()
                    && opencode_bin_resolved.is_none()
                {
                    "codex"
                } else if opencode_bin_resolved.is_some() && claude_bin_resolved.is_none() {
                    "opencode"
                } else {
                    "claude"
                },
            ),
            claude_bin: claude_bin_resolved,
            opencode_bin: opencode_bin_resolved,
            codex_bin: codex_bin_resolved,
        };
        let toml_str = crate::wasl::render_wasl_node_toml(&toml_cfg);

        // Write the config + durably spawn the node over the host's connection.
        let script = crate::wasl::remote_wasl_node_launch_script(host_id, &toml_str);
        let out = crate::remote::run_remote_script(host_target, &script);
        match crate::wasl::parse_wasl_node_spawn(out.as_deref()) {
            crate::wasl::WaslNodeSpawn::Spawned(pid)
            | crate::wasl::WaslNodeSpawn::Adopted(pid) => {
                eprintln!(
                    "marsad-agent: wasl-node[remote {host_target:?}]: node up (pid {pid})"
                );
            }
            crate::wasl::WaslNodeSpawn::NoBinary => {
                return Err(AgentError::Spawn(format!(
                    "wasl-mcpd is not installed on remote host {host_target:?} \
                     (expected on the login PATH, like the agent CLIs) — install it to \
                     launch Wasl-managed sessions on this host"
                )));
            }
            crate::wasl::WaslNodeSpawn::Unreachable => {
                return Err(AgentError::Spawn(format!(
                    "could not reach remote host {host_target:?} to spawn the Wasl node \
                     (connection failed / no sentinel)"
                )));
            }
        }

        // Open the three ssh -L tunnels (control + operator + a2a) and track the
        // children so node/session teardown can kill them. Idempotent per host:
        // if tunnels are already tracked for this host, they are left in place.
        // Skipped entirely for a `wsl:` host — WSL2 localhost forwarding makes the
        // node's own loopback binds directly reachable, so there is nothing to
        // tunnel (and no ssh child to track/teardown).
        if is_wsl_direct {
            eprintln!(
                "marsad-agent: wasl-node[wsl {host_target:?}]: no ssh tunnel — reaching the \
                 node's loopback directly via WSL2 localhost forwarding ({base_url})"
            );
        } else {
            self.open_wasl_tunnels(
                host_id,
                host_target,
                &[
                    (l_control, remote_port(&remote_control)),
                    (l_operator, remote_port(&remote_operator)),
                    (l_a2a, remote_port(&remote_a2a)),
                ],
            );
        }

        // Register the provider factory at the LOCAL control tunnel endpoint.
        let factory_base_url = base_url.clone();
        self.wasl_session_provider_factory.0 = Some(Box::new(move || {
            Box::new(marsad_wasl_session::RealWaslSessionProvider::new(
                marsad_wasl_session::WaslSessionClientConfig {
                    base_url: factory_base_url.clone(),
                },
            )) as Box<dyn marsad_wasl_session::WaslSessionProvider>
        }));

        eprintln!(
            "marsad-agent: wasl-node[remote {host_target:?}]: factory registered at {base_url}"
        );

        // Wire the operator connector at the LOCAL operator tunnel endpoint.
        self.ensure_wasl_operator_connector(host_id, &operator_local_url);

        Ok(base_url)
    }

    /// Open (once) the `ssh -L` tunnel children for a remote node and track them
    /// under `host_id` so [`Self::teardown_wasl_tunnels`] can reap them. Idempotent:
    /// if tunnels are already tracked for this host, this is a no-op (a second
    /// launch on the same host reuses the live tunnels). Real-IO only.
    #[cfg(not(coverage))]
    fn open_wasl_tunnels(
        &mut self,
        host_id: &str,
        host_target: &str,
        forwards: &[(u16, u16)],
    ) {
        use std::process::{Command, Stdio};
        {
            let map = self.wasl_tunnels.0.lock().unwrap();
            if map.contains_key(host_id) {
                eprintln!(
                    "marsad-agent: wasl-node[remote {host_target:?}]: tunnels already open for {host_id} — reuse"
                );
                return;
            }
        }
        let mut children = Vec::new();
        for (lport, rport) in forwards {
            let Some(argv) = crate::wasl::wasl_tunnel_ssh_argv(host_target, *lport, *rport) else {
                continue;
            };
            let bin = crate::remote::resolve_remote_bin(&argv[0]);
            let mut command = Command::new(&bin);
            command
                .args(&argv[1..])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            }
            match command.spawn() {
                Ok(child) => {
                    eprintln!(
                        "marsad-agent: wasl-node[remote {host_target:?}]: tunnel 127.0.0.1:{lport} -> :{rport} (pid {})",
                        child.id()
                    );
                    children.push(child);
                }
                Err(e) => {
                    eprintln!(
                        "marsad-agent: wasl-node[remote {host_target:?}]: tunnel spawn failed for :{lport}: {e}"
                    );
                }
            }
        }
        if !children.is_empty() {
            self.wasl_tunnels
                .0
                .lock()
                .unwrap()
                .insert(host_id.to_string(), children);
            // Give the tunnels a moment to establish before the first factory dial.
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    }

    /// Reap all `ssh -L` tunnel children tracked for `host_id` (SIGKILL + wait).
    /// Idempotent — an unknown host is a no-op. Real-IO only. Called on node/session
    /// teardown so the local tunnel ports are freed and no ssh children leak.
    #[cfg(not(coverage))]
    pub(crate) fn teardown_wasl_tunnels(&mut self, host_id: &str) {
        let children = self.wasl_tunnels.0.lock().unwrap().remove(host_id);
        if let Some(children) = children {
            for mut child in children {
                let _ = child.kill();
                let _ = child.wait();
            }
            eprintln!("marsad-agent: wasl-node: tunnels torn down for host {host_id}");
        }
    }

    /// Wire the X1 operator connector for a node's operator endpoint, if not
    /// already registered. Idempotent (deduped by host_id). Uses the registered
    /// connector factory if set; otherwise logs and skips (serve() wires from
    /// load_wasl_config for static config, tests pre-register via
    /// register_wasl_connector). Best-effort: never panics, never fails a
    /// LaunchWaslSession.
    pub fn ensure_wasl_operator_connector(&mut self, host_id: &str, operator_url: &str) {
        // Dedup: already registered → skip (the early-return path for a second
        // LaunchWaslSession on the same host or a pre-registered test seam).
        let already_registered = self
            .wasl_connectors
            .0
            .lock()
            .unwrap()
            .contains_key(host_id);
        if already_registered {
            eprintln!(
                "marsad-agent: wasl-operator: connector for host {host_id} already registered — skip"
            );
            return;
        }

        // Build via the connector factory if one is set. In real-IO mode serve()
        // supplies the factory; in test mode no factory is set (tests pre-register
        // via register_wasl_connector before calling ensure_wasl_runtime_node).
        let entry = marsad_wasl::WaslOperatorEntry {
            host_id: host_id.to_string(),
            url: operator_url.to_string(),
        };
        if let Some(factory) = self.wasl_connector_factory.0.as_ref() {
            let connector = factory(&entry);
            self.register_wasl_connector(host_id, connector);
            eprintln!(
                "marsad-agent: wasl-operator: connector registered for host {host_id} at {operator_url}"
            );
        } else {
            // No factory — serve() will wire the connector from load_wasl_config,
            // or the caller pre-registered a test seam. Log for observability.
            eprintln!(
                "marsad-agent: wasl-operator: no connector factory for host {host_id} — skipping operator connector registration"
            );
        }
    }

    /// `LaunchWaslSession`: build the provider (via the registered one, or lazily
    /// via the factory), `get_or_start` a client for `wasl_target_key`, and
    /// register it under `session_id`. No argv, no container — the process is
    /// Wasl's. Idempotent per (session, key): a duplicate launch reuses the
    /// existing target rather than spawning a second competing one (the reconnect
    /// guarantee, §4.3/§6.1) — `get_or_start` is create-or-reuse (it returns a
    /// `WaslTargetSnapshot`, never a `created` bool), and the map is keyed by
    /// `session_id` so no second entry appears. The per-session observe THREAD
    /// that pumps `attach(after_seq)` and broadcasts the mapped event/status
    /// lines is spawned by `serve()`'s real-IO layer (which owns the subscriber
    /// broadcast list); it decides its start seq via
    /// [`crate::wasl_reconnect_after_seq`]`(snapshot.last_seq, persisted)` and
    /// loops [`crate::pump_wasl_observe`] with [`crate::wasl_observe_should_reconnect`].
    pub(crate) fn handle_launch_wasl_session(
        &mut self,
        session_id: String,
        host_id: String,
        wasl_target_key: serde_json::Value,
        name: Option<String>,
    ) -> Result<Vec<String>, AgentError> {
        // B1: ensure the host's managed-runtime wasl-mcpd is up and its provider
        // factory is registered. When a factory/provider is already set (serve()
        // pre-set, test seam, or a prior call), this is a cheap early-return.
        // On the spawn path it registers a RealWaslSessionProvider factory; in
        // the unit-test environment (no live node) the subsequent get_or_start
        // hits ECONNREFUSED and returns a graceful Err — no short-circuit needed.
        //
        // Resolve the logical `host_id` to its real SSH/WSL target from the
        // persisted host registry so a REMOTE node is spawned + tunneled on the
        // right host. A local host (or an unknown host_id, or the coverage build
        // with no store) resolves to `None` → the local Container spawn path,
        // byte-identical to the pre-remote behavior.
        let host_target = self.wasl_host_target(&host_id);
        self.ensure_wasl_runtime_node(&host_id, host_target.as_deref(), &[], None)?;

        let _ = name;

        // Phase D — worker-send: inject the Wasl MCP config into the target key's
        // context BEFORE `get_or_start`. The worker's inner claude/opencode binary
        // is launched by the Wasl runtime with `--mcp-config <path>`, wiring
        // `wasl-mcpd` as the "wasl" MCP server so the worker can send A2A messages
        // to its peers. This is unconditional: even with zero peers the worker gets
        // the wasl send-tools MCP (peers may be added later).
        let mut key_val = wasl_target_key;

        // Determine worker identity from the key if present, else use session_id.
        let worker_agent_id = key_val
            .get("key")
            .and_then(|v| v.as_str())
            .unwrap_or(&session_id)
            .to_string();
        let worker_host = host_id.clone();

        // Build the peer list from context.peers (tolerate missing/non-array).
        let peers: Vec<crate::wasl::WaslWorkerPeer> = key_val
            .get("context")
            .and_then(|ctx| ctx.get("peers"))
            .and_then(|p| p.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|entry| {
                        let agent_id = entry.get("agent_id")?.as_str()?.to_string();
                        let peer_host = entry.get("host_id")?.as_str()?.to_string();
                        let url = format!(
                            "http://{}",
                            crate::wasl::wasl_a2a_bind(&peer_host)
                        );
                        Some(crate::wasl::WaslWorkerPeer {
                            agent_id,
                            host_id: peer_host,
                            url,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Render the TOML and write it to <data_root>/wasl-worker-<session_id>.toml.
        let toml_content =
            crate::wasl::render_wasl_worker_toml(&worker_agent_id, &worker_host, &peers);
        let config_path = std::path::Path::new(&self.config.data_root)
            .join(format!("wasl-worker-{session_id}.toml"));
        let _ = std::fs::create_dir_all(&self.config.data_root);
        let write_ok = std::fs::write(&config_path, &toml_content).is_ok();

        eprintln!(
            "marsad-agent: wasl-worker: session {session_id} host {worker_host} \
             peers {} config {} write_ok {write_ok}",
            peers.len(),
            config_path.display()
        );

        // Build the mcp_config JSON string and inject it into context.
        let config_path_str = config_path.to_string_lossy().into_owned();
        let mcp_config = crate::wasl::wasl_worker_mcp_config(&config_path_str);

        // Ensure context is an object, then set mcp_config.
        if key_val.get("context").map(|v| v.is_object()).unwrap_or(false) {
            key_val["context"]["mcp_config"] = serde_json::Value::String(mcp_config);
        } else {
            key_val["context"] = serde_json::json!({ "mcp_config": mcp_config });
        }

        // Ensure a provider exists: prefer the directly-registered one, else build
        // one from the factory (once) and keep it. Absent both -> Protocol error.
        if self.wasl_session_provider.0.is_none() {
            match self.wasl_session_provider_factory.0.as_ref() {
                Some(factory) => self.wasl_session_provider.0 = Some(factory()),
                None => {
                    return Err(AgentError::Protocol(format!(
                        "LaunchWaslSession {session_id}: no wasl session provider registered"
                    )))
                }
            }
        }

        // Resolve (or reattach to) the client. `get_or_start` is the potentially
        // blocking call, but the provider is `&mut self`-borrowed here (not under
        // any registry lock), so lock hygiene holds trivially.
        // Speak the Wasl node's canonical protocol name (opencode → opencode-acp)
        // before create, so the node's router finds the sub-provider. claude/codex
        // are identical on both sides. This is the one Marsad→Wasl boundary mapping.
        if let Some(p) = key_val.get("protocol").and_then(|v| v.as_str()) {
            let mapped = crate::wasl::wasl_node_protocol(p).to_string();
            key_val["protocol"] = serde_json::Value::String(mapped);
        }
        let key = WaslTargetKey(key_val);
        let (client, _snapshot) = self
            .wasl_session_provider
            .0
            .as_ref()
            .expect("provider present after the guard above")
            .get_or_start(&key)
            .map_err(|e| AgentError::Protocol(format!("get_or_start failed: {e}")))?;

        // Box<dyn WaslSessionClient> -> Arc<dyn WaslSessionClient> at the registry
        // boundary (mirrors `register_wasl_connector`'s `Arc::from`) so later arms
        // can clone the handle out and drop the map lock before the blocking call.
        let client: std::sync::Arc<dyn WaslSessionClient> = std::sync::Arc::from(client);
        self.wasl_session_clients
            .0
            .lock()
            .unwrap()
            .insert(session_id.clone(), client);

        Ok(vec![encode_ipc(&IpcMsg::SessionLaunched { session_id })])
    }

    /// `SendWaslPrompt`: build a `WaslTurnRequest` + `WaslBusyPolicy` and relay it
    /// to the session's client via `prompt`; reply with the `TurnReceipt` as a
    /// `WaslPromptReceipt` (the atomic admission decision; the reply itself
    /// arrives later as a `WaslSessionEvent::TurnCompleted`). Lock hygiene: the
    /// client `Arc` is cloned out and the map lock dropped BEFORE `prompt`.
    pub(crate) fn handle_send_wasl_prompt(
        &mut self,
        session_id: String,
        text: String,
        idempotency_key: String,
        busy_policy: String,
        max_wait_ms: u64,
    ) -> Result<Vec<String>, AgentError> {
        let client = self.wasl_session_client(&session_id)?;
        // The agent mints the fresh single-hop `WaslCausation` (§7): it is NOT
        // carried on the IPC, since Marsad ALWAYS originates a fresh ROOT
        // `LocalHuman` turn (no chaining, no parent, empty path, hop 0). A fresh
        // `root_request` per send — distinct across sends.
        let req = WaslTurnRequest {
            idempotency_key,
            text,
            causation: WaslCausation::fresh(new_idempotency_key()),
        };
        let policy = busy_policy_from_wire(&busy_policy, max_wait_ms);
        let receipt = client.prompt(req, policy);
        Ok(vec![encode_ipc(&IpcMsg::WaslPromptReceipt {
            session_id,
            receipt: serde_json::to_value(&receipt).expect("WaslTurnReceipt always serializes"),
        })])
    }

    /// `AttachWaslSession`: (re)subscribe the observe stream from `after_seq`.
    /// The per-session observe THREAD is spawned in `serve()` (real IO); here the
    /// synchronous path just acks — the observe events themselves broadcast
    /// through `wasl_session_event_lines` on the thread.
    ///
    /// CRITICAL (relaunch): do NOT require a pre-existing client here. On a cold
    /// agent restart the client does NOT exist yet — the observe re-pump in
    /// serve.rs re-adopts the node from THIS message's `wasl_target_key` via
    /// `wasl_observe_client_and_after_seq` → `get_or_start`, which CREATES it. A
    /// hard `wasl_session_client(..)?` here returned `Err` on relaunch, making
    /// `out_lines` an error, which silently gated out the serve.rs re-pump (it runs
    /// only `if out_lines.is_ok() && …`). That is exactly why a relaunched Wasl
    /// session never replayed its transcript. The ack is only the routing signal;
    /// the real (re)attach + client creation happens in serve.rs from the key.
    pub(crate) fn handle_attach_wasl_session(
        &mut self,
        session_id: String,
        after_seq: u64,
    ) -> Result<Vec<String>, AgentError> {
        let _ = after_seq;
        Ok(vec![encode_ipc(&IpcMsg::SessionAttached { session_id })])
    }

    /// `AnswerWaslNativeGate`: relay the human's decision to the session's client
    /// via `resolve_native_gate`, then ack with `WaslNativeGateAnswered`
    /// REGARDLESS of the client ack (the follow-on status/event is authoritative,
    /// mirroring X1's `WaslGateAnswered` semantics). A scripted client error
    /// surfaces as a `Protocol` agent error. Lock hygiene (§8.1): the client
    /// `Arc` is cloned out and the map lock DROPPED before `resolve_native_gate`
    /// — a wedged/panicking client cannot poison the registry lock.
    pub(crate) fn handle_answer_wasl_native_gate(
        &mut self,
        session_id: String,
        gate_id: String,
        outcome: String,
        reason: Option<String>,
    ) -> Result<Vec<String>, AgentError> {
        let client = self.wasl_session_client(&session_id)?;
        let decision = gate_decision_from_wire(&outcome, reason);
        // v3: the wire `gate_id` field carries core's per-occurrence
        // `native_gate_key(turn_id, seq)` (format `"{turn_id}#{seq}"`), Marsad's
        // ledger identity. Wasl's `resolve_native_gate` is keyed by the bare
        // `turn_id` (the real gate has no gate id), so decompose the key back to
        // the turn_id before calling the client — split on the LAST `#` so a
        // turn_id that itself contains a `#` is preserved.
        let turn_id = turn_id_from_gate_key(&gate_id);
        client
            .resolve_native_gate(&WaslTurnId(turn_id), decision)
            .map_err(|e| {
                AgentError::Protocol(format!(
                    "resolve_native_gate failed for {session_id}/{gate_id}: {e}"
                ))
            })?;
        Ok(vec![encode_ipc(&IpcMsg::WaslNativeGateAnswered {
            session_id,
            gate_id,
        })])
    }

    /// `InterruptWaslSession`: best-effort `cancel_turn` via the client. Lock
    /// hygiene: client `Arc` cloned out, map lock dropped before `interrupt`.
    pub(crate) fn handle_interrupt_wasl_session(
        &mut self,
        session_id: String,
    ) -> Result<Vec<String>, AgentError> {
        let client = self.wasl_session_client(&session_id)?;
        client
            .interrupt(WaslCancelReason::Human)
            .map_err(|e| AgentError::Protocol(format!("interrupt failed for {session_id}: {e}")))?;
        Ok(vec![encode_ipc(&IpcMsg::Interrupted { session_id })])
    }

    /// `StopWaslSession`: `shutdown(Drain|Immediate)` via the client and drop the
    /// registry entry. Lock hygiene: client `Arc` cloned out, map lock dropped
    /// before `shutdown`.
    pub(crate) fn handle_stop_wasl_session(
        &mut self,
        session_id: String,
        drain: bool,
    ) -> Result<Vec<String>, AgentError> {
        let client = self.wasl_session_client(&session_id)?;
        let mode = if drain {
            WaslShutdownMode::Drain
        } else {
            WaslShutdownMode::Immediate
        };
        client
            .shutdown(mode)
            .map_err(|e| AgentError::Protocol(format!("shutdown failed for {session_id}: {e}")))?;
        self.wasl_session_clients
            .0
            .lock()
            .unwrap()
            .remove(&session_id);
        Ok(vec![encode_ipc(&IpcMsg::SessionStopped { session_id })])
    }
}

/// Reap every REMOTE Wasl node's `ssh -L` tunnel children on agent shutdown so no
/// ssh child leaks past the daemon's lifetime. The durable remote `wasl-mcpd`
/// itself is intentionally LEFT RUNNING (it is per-host and survives agent
/// restarts, exactly like a durable remote solo session — a fresh agent
/// re-adopts it and re-opens the tunnels); only the local-side tunnel children,
/// which are meaningless without this agent, are killed here. Real-IO only (the
/// tunnels map only exists in the non-coverage build).
#[cfg(not(coverage))]
impl Drop for Agent {
    fn drop(&mut self) {
        let hosts: Vec<String> = {
            let map = self.wasl_tunnels.0.lock().unwrap();
            map.keys().cloned().collect()
        };
        for host_id in hosts {
            self.teardown_wasl_tunnels(&host_id);
        }
    }
}

/// Parse the port out of a bare `ip:port` socket address. Used to derive the
/// REMOTE side of each `ssh -L` forward from the node's bare bind addresses.
/// `0` on an unparseable value (a malformed bind never forwards to a real port —
/// the tunnel simply won't connect, surfaced later as a factory dial failure).
#[cfg(not(coverage))]
fn remote_port(bind: &str) -> u16 {
    bind.rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(0)
}

/// Map the wire busy-policy string (+ `max_wait_ms`) to a typed `WaslBusyPolicy`
/// (§7). "reject" -> `Reject`; "queue_related" -> `QueueRelated`; anything else
/// (incl. the default "queue" and empty) -> `Queue` (§12 Q4: a human's late
/// prompt is usually still wanted).
fn busy_policy_from_wire(policy: &str, max_wait_ms: u64) -> WaslBusyPolicy {
    match policy {
        "reject" => WaslBusyPolicy::Reject,
        "queue_related" => WaslBusyPolicy::QueueRelated { max_wait_ms },
        _ => WaslBusyPolicy::Queue { max_wait_ms },
    }
}

/// Decompose core's per-occurrence gate key `native_gate_key(turn_id, seq)`
/// (format `"{turn_id}#{seq}"`, `marsad-wasl-session`'s projection module) back
/// into the bare Wasl `turn_id` that `resolve_native_gate` is keyed by (§8, v3).
/// Splits on the LAST `#` so a turn_id containing a `#` survives; a key with no
/// `#` (e.g. a raw turn_id, or an unrecognized shape) is returned verbatim.
fn turn_id_from_gate_key(gate_key: &str) -> String {
    match gate_key.rsplit_once('#') {
        Some((turn_id, _seq)) => turn_id.to_string(),
        None => gate_key.to_string(),
    }
}

/// Map the wire outcome string (+ optional reason) to a typed `WaslGateDecision`
/// (§7/§8). "denied" -> `Deny { reason }`; anything else (incl. "approved") ->
/// `Approve`.
fn gate_decision_from_wire(outcome: &str, reason: Option<String>) -> WaslGateDecision {
    if outcome == "denied" {
        WaslGateDecision::Deny {
            reason: reason.unwrap_or_default(),
        }
    } else {
        WaslGateDecision::Approve
    }
}

#[cfg(test)]
mod attach_relaunch_tests {
    use super::*;
    use crate::{AgentConfig, ListenSpec};
    use marsad_lifecycle::FakeContainer;
    use marsad_notify::CaptureNotifier;
    use marsad_store::{MemoryBackend, Store};

    fn agent() -> Agent {
        Agent::with_fakes(
            AgentConfig {
                listen: ListenSpec::Stdio,
                data_root: "/tmp/marsad-agent-test-attach-relaunch".to_string(),
            },
            Box::new(FakeContainer::new()),
            Box::new(CaptureNotifier::new()),
            Box::new(Store::new(Box::new(MemoryBackend::new()))),
        )
        .expect("Agent::with_fakes")
    }

    /// Relaunch regression: on a cold agent restart the client registry is EMPTY
    /// (no prior `LaunchWaslSession`). `handle_attach_wasl_session` MUST still
    /// return `Ok(SessionAttached)`. A prior version hard-failed here on the
    /// missing client, which made `out_lines` an `Err` and silently gated out the
    /// observe re-pump in `serve.rs` (it runs only `if out_lines.is_ok() && …`) —
    /// so a relaunched Wasl session never replayed its transcript. The real node
    /// re-adopt + client creation happens in `serve.rs` from the message's
    /// `wasl_target_key`; this handler only produces the routing ack.
    #[test]
    fn attach_wasl_session_acks_even_with_no_prior_client() {
        let mut a = agent();
        let out = a
            .handle_attach_wasl_session("session-cold-1".to_string(), 0)
            .expect("attach must ack even when no client exists yet (relaunch)");
        assert_eq!(out.len(), 1, "exactly one ack line");
        match marsad_proto::parse_ipc(&out[0]) {
            Ok(IpcMsg::SessionAttached { session_id }) => {
                assert_eq!(session_id, "session-cold-1")
            }
            other => panic!("expected SessionAttached ack, got {other:?}"),
        }
    }
}
