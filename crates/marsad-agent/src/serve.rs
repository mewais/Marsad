//! `serve`, `bridge` and `stop`.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use marsad_proto::daemon::{DaemonInfo, Readiness};
use marsad_proto::diag::{
    AskpassOutcome, BridgeEndReason, BridgeOutcome, MarsadEvent, ReadinessFailure, ShutdownWhy,
    StartFailure, StartStage, StopOutcome,
};
use marsad_proto::hosts::{HostEntry, HostKind};
use marsad_proto::layout::Layout;
use marsad_proto::{methods, HostId, HOST_ROUTES, HUB_ROUTES};
use marsad_store::Store;
use tokio_util::sync::CancellationToken;
use wasit_control::{
    connector, BindError, ClientOptions, ConnConfig, ControlClient, ControlServer, Endpoint, Hello,
    Hub, HubConfig, InstanceLock, LockError, RoutingService, Service, ServiceMux, Supervised,
    SupervisedOptions, Uplink,
};
use wasit_core::DriverId;
use wasit_diag::{HomePath, Ident};
use wasit_runtime::environment::{CaptureOptions, HostEnv};
use wasit_runtime::wire::ROUTES;
use wasit_runtime::{
    Discovery, DiscoveryConfig, RuntimeConfig, RuntimeService, SessionManager, ShimLauncher,
};
use wasl_node::{Node, NodeConfig, NodeService};

use crate::cli::{default_host_id, Options};
use crate::diag::{self, emit};
use crate::host::{pty::Ptys, HostServices};
use crate::hub::auth::Prompts;
use crate::hub::export::job::{HubFacts, Limits};
use crate::hub::export::{ExportEnv, Exports};
use crate::hub::fleet::Fleet;
use crate::hub::gitwatch::GitWatch;
use crate::hub::hosts::{Registry, RegistryParts};
use crate::hub::provision::{AgentProvisioner, BootstrapConnector, BIN_DIR_ENV};
use crate::hub::workstreams::Workstreams;
use crate::hub::{AskUserMcp, HubServices, WaslMcp};
use crate::version;

/// Exit code when another daemon already serves the endpoint.
pub const EXIT_ALREADY_RUNNING: u8 = 3;

fn runtime_config(o: &Options) -> Result<RuntimeConfig, String> {
    let launcher = ShimLauncher::current_exe("shim").map_err(|e| e.to_string())?;
    let layout = o.layout();
    let mut cfg = RuntimeConfig::new(layout.data.clone(), launcher);
    cfg.derivation = version::build_id();
    cfg.run_dir = Some(layout.run.clone());
    for (d, p) in &o.driver_bin {
        let id = DriverId::new(d.clone());
        cfg.registry.get(&id).map_err(|e| e.to_string())?;
        cfg.overrides.entry(id).or_default().binary = Some(p.clone());
    }
    for (d, k, v) in &o.driver_env {
        let id = DriverId::new(d.clone());
        cfg.registry.get(&id).map_err(|e| e.to_string())?;
        cfg.overrides
            .entry(id)
            .or_default()
            .env
            .push((k.clone(), v.clone()));
    }
    if let Some(t) = o.handshake_timeout {
        cfg.timeouts.handshake = t;
    }
    if let Some(t) = o.stop_grace {
        cfg.timeouts.stop_grace = t;
    }
    if let Some(t) = o.command_timeout {
        cfg.timeouts.command = t;
    }
    Ok(cfg)
}

/// `msg` on stderr (the daemon's raw crash output) and the exit status.
fn fail(msg: impl std::fmt::Display, code: u8) -> ExitCode {
    eprintln!("marsad-agent: {msg}");
    ExitCode::from(code)
}

/// A startup step failed: the reason is recorded (`daemon.failed`, in the daemon's file once it
/// holds its lock, else in the shared one) and printed on stderr, i.e. in `marsad-agent.log`.
/// What is printed is what is recorded: the stage, the code and the OS's reason or the path of
/// Marsad's own files, never text that may quote stored data (the Wasl node's journal, the
/// hub's documents). With `MARSAD_LOG` set the full error is printed instead (`full`).
fn startup_failed(
    stage: StartStage,
    code: u8,
    reason: diag::Reason,
    full: impl std::fmt::Display,
) -> ExitCode {
    emit(MarsadEvent::DaemonFailed {
        stage,
        exit: i32::from(code),
        error: Some(reason.info.clone()),
    });
    if diag::developer() {
        fail(full, code)
    } else {
        fail(
            format!(
                "startup failed at {stage:?} (exit {code}): {}",
                reason.words()
            ),
            code,
        )
    }
}

pub async fn serve(o: Options) -> ExitCode {
    let began = Instant::now();
    let layout = o.layout().clone();
    if let Err(e) = tokio::fs::create_dir_all(&layout.data).await {
        return startup_failed(
            StartStage::DataDir,
            1,
            diag::Reason::io(format!("cannot create {}", layout.data.display()), &e),
            format!("cannot create {}: {e}", layout.data.display()),
        );
    }
    let cfg = match runtime_config(&o) {
        Ok(c) => c,
        Err(e) => {
            return startup_failed(StartStage::Config, 2, diag::Reason::ours("config", &e), e);
        }
    };
    let listener = match wasit_control::bind_local_exclusive(o.socket_path()).await {
        Ok(l) => l,
        Err(BindError::Lock(LockError::Held(p))) => {
            // Still on the shared file: a daemon that lost the race never writes `daemon.ndjson`.
            diag::announce();
            emit(MarsadEvent::DaemonLockHeld {
                lock: HomePath::new(&p),
            });
            return fail(
                format!("another daemon is running (lock {} is held)", p.display()),
                EXIT_ALREADY_RUNNING,
            );
        }
        Err(e) => {
            return startup_failed(
                StartStage::Listen,
                1,
                diag::Reason::ours("listen", &e),
                format!("cannot listen: {e}"),
            );
        }
    };
    // This process is the machine's daemon: its records move to `daemon.ndjson`.
    diag::promote_to_daemon();
    diag::announce();
    let build = version::build_id();
    let host_id = o.host_id.clone().unwrap_or_else(default_host_id);
    emit(MarsadEvent::DaemonStart {
        hub: o.hub,
        host_id: Ident::new(&host_id),
        build: Ident::new(&build),
        pid: std::process::id(),
        machine: Ident::new(&layout.machine),
        data: HomePath::new(&layout.data),
        run_dir: HomePath::new(&layout.run),
        socket: HomePath::new(&o.socket_path()),
        free_text_logging: diag::developer(),
    });
    #[cfg(feature = "panic-test")]
    if let Some(marker) = &o.panic_test {
        panic!("daemon panic test: {marker}");
    }
    // Holding this machine's lock: adopt what an earlier build left in the flat layout.
    if o.socket.is_none() {
        crate::migrate::run(&layout).await;
    }
    // The user's real environment (login shell), for everything this daemon starts.
    // The setup file a csh/tcsh login is sourced from goes in this daemon's own private runtime
    // directory, not in a generic one.
    let env = HostEnv::new(CaptureOptions {
        scratch_dir: Some(layout.run.clone()),
        ..CaptureOptions::default()
    });
    let phase = Instant::now();
    env.refresh().await;
    let env_ms = diag::ms(phase);
    // The host's agent commands (set with its environment settings) for discovery and launches.
    let mut cfg = cfg;
    cfg.commands = env.commands();
    let discovery = Discovery::new(DiscoveryConfig::from_runtime(&cfg));
    let drivers = cfg.registry.clone();
    let manager = match SessionManager::new(cfg).await {
        Ok(m) => m,
        Err(e) => {
            return startup_failed(
                StartStage::Sessions,
                1,
                diag::Reason::ours("sessions", &e),
                e,
            );
        }
    };
    let phase = Instant::now();
    match manager.adopt_all().await {
        Ok(r) => {
            emit(MarsadEvent::DaemonAdopted {
                running: u32::try_from(r.running.len()).unwrap_or(u32::MAX),
                finished: u32::try_from(r.finished.len()).unwrap_or(u32::MAX),
                errors: u32::try_from(r.errors.len()).unwrap_or(u32::MAX),
            });
            for (dir, e) in r.errors {
                emit(MarsadEvent::DaemonAdoptError {
                    session: Ident::new(
                        &dir.file_name()
                            .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
                    ),
                    error: diag::foreign("adopt", &e),
                });
            }
        }
        Err(e) => emit(MarsadEvent::DaemonScanFailed {
            error: diag::foreign("scan", &e.to_string()),
        }),
    }
    let adopt_ms = diag::ms(phase);
    let phase = Instant::now();
    let node = match Node::open(
        NodeConfig::new(host_id.clone(), layout.data.join("wasl")),
        manager.clone(),
    )
    .await
    {
        Ok(n) => n,
        Err(e) => {
            // The node's journal is Wasl's task data: only a code, a length and a fingerprint.
            return startup_failed(
                StartStage::Node,
                1,
                diag::Reason::foreign(&e.code, &e.message),
                format!("cannot open the Wasl node: {e}"),
            );
        }
    };
    let node_ms = diag::ms(phase);
    let shutdown = CancellationToken::new();
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            return startup_failed(
                StartStage::Executable,
                1,
                diag::Reason::io("cannot locate the daemon executable", &e),
                format!("cannot locate the daemon executable: {e}"),
            );
        }
    };
    let info = DaemonInfo {
        version: version::info(),
        host_id: host_id.clone(),
        hub: o.hub,
        data_dir: layout.data.display().to_string(),
        pid: std::process::id(),
        exe: exe.display().to_string(),
        socket: o.socket_path().display().to_string(),
    };
    let runtime = RuntimeService::new(
        manager.clone(),
        discovery.clone(),
        host_id.clone(),
        build.clone(),
    )
    .with_environment(env);
    // Terminals end with their session, and with this daemon (below).
    let ptys = Ptys::default();
    ptys.follow_sessions(manager.clone());
    let host = HostServices {
        manager: manager.clone(),
        discovery,
        ptys: ptys.clone(),
        info,
        shutdown: shutdown.clone(),
    };
    let mut local = ServiceMux::new();
    if let Err(e) = local
        .register(ROUTES.iter().copied(), Arc::new(runtime))
        .and_then(|_| local.register(HOST_ROUTES.iter().copied(), Arc::new(host)))
        .and_then(|_| {
            local.register(
                wasit_runtime::wire::DIAG_ROUTES.iter().copied(),
                Arc::new(wasit_runtime::DiagService::new(manager.clone())),
            )
        })
        .and_then(|_| {
            local.register(
                wasl_proto::ROUTES.iter().copied(),
                Arc::new(NodeService::new(node.clone())),
            )
        })
    {
        return startup_failed(
            StartStage::Services,
            1,
            diag::Reason::ours("services", &e),
            e,
        );
    }
    let local: Arc<dyn Service> = Arc::new(local);

    let mut hub_handle: Option<Hub> = None;
    let mut hub_ms = None;
    let top: Arc<dyn Service> = if o.hub {
        let phase = Instant::now();
        match start_hub(
            &o,
            &host_id,
            &build,
            local.clone(),
            manager.clone(),
            drivers,
            &node,
        )
        .await
        {
            Ok((svc, hub)) => {
                // The hub is the supervisor: handoffs always raise gates here, and other
                // hosts are reached over its own links.
                node.attach_uplink(Uplink::via_hub(hub.clone()));
                node.set_supervisor_probe(Arc::new(|| true));
                hub_handle = Some(hub);
                hub_ms = Some(diag::ms(phase));
                svc
            }
            Err(e) => {
                // Stored documents and the hub's other inputs may quote user data.
                return startup_failed(StartStage::Hub, 1, diag::Reason::foreign("hub", &e), e);
            }
        }
    } else {
        local
    };
    let role = if o.hub { "hub" } else { "daemon" };
    let server = ControlServer::new(
        top,
        Hello::new(build.clone(), host_id, role).with_machine(layout.machine.clone()),
        ConnConfig::default(),
    );
    if !o.hub {
        // The hub's link into this daemon is the uplink and the supervisor.
        node.attach_server(&server);
    }
    let ready = Readiness {
        ready: true,
        pid: std::process::id(),
        endpoint: format!("{:?}", o.endpoint()),
        build,
    };
    let line = match serde_json::to_string(&ready) {
        Ok(line) => line,
        Err(e) => {
            return startup_failed(
                StartStage::Readiness,
                1,
                diag::Reason::ours("readiness", &e),
                e,
            );
        }
    };
    // Whoever started this daemon may have given up and closed the pipe by now. The daemon is
    // healthy and serves anyway: that starter finds it by connecting.
    let line_written = match write_readiness(&line).await {
        Ok(()) => true,
        Err(e) => {
            let why = if e.kind() == std::io::ErrorKind::BrokenPipe {
                ReadinessFailure::PipeClosed
            } else {
                ReadinessFailure::WriteFailed
            };
            emit(MarsadEvent::DaemonReadinessUnwritten {
                why,
                error: diag::io(&e),
            });
            false
        }
    };
    emit(MarsadEvent::DaemonReady {
        total_ms: diag::ms(began),
        env_ms,
        adopt_ms,
        node_ms,
        hub_ms,
        line_written,
    });
    let mut signal = None;
    let mut error = None;
    let (code, why) = tokio::select! {
        r = server.serve(listener) => match r {
            Ok(()) => (ExitCode::SUCCESS, ShutdownWhy::ServeEnded),
            Err(e) => {
                error = Some(diag::ours("serve", &e));
                (fail(format!("serve failed: {e}"), 1), ShutdownWhy::ServeFailed)
            }
        },
        what = termination() => {
            signal = Some(wasit_diag::Code::new(what));
            (ExitCode::SUCCESS, ShutdownWhy::Signal)
        }
        _ = shutdown.cancelled() => (ExitCode::SUCCESS, ShutdownWhy::Request),
    };
    emit(MarsadEvent::DaemonShutdown { why, signal, error });
    server.shutdown(Duration::from_secs(2)).await;
    // No shell outlives the daemon that owns it.
    ptys.shutdown().await;
    if let Some(h) = hub_handle {
        h.shutdown().await;
    }
    code
}

/// Writes the readiness line (one JSON line) to stdout. An error says the reader is gone (a
/// closed pipe) or stdout is unusable; it is the caller's to record, never a panic.
async fn write_readiness(line: &str) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut out = tokio::io::stdout();
    out.write_all(format!("{line}\n").as_bytes()).await?;
    out.flush().await
}

async fn start_hub(
    o: &Options,
    host_id: &str,
    build: &str,
    local: Arc<dyn Service>,
    manager: SessionManager,
    drivers: wasit_core::DriverRegistry,
    node: &Node,
) -> Result<(Arc<dyn Service>, Hub), String> {
    let store = Store::open(o.layout().data.join("store"))
        .await
        .map_err(|e| e.to_string())?;
    // Before any link starts: what a host's ssh prints may echo a stored secret.
    crate::hub::hostenv::register_stored_secrets(&store, &HostId::new(host_id)).await;
    let fleet = Fleet::spawn();
    let entries = Arc::new(RwLock::new(Default::default()));
    let (ltx, lrx) = tokio::sync::mpsc::channel(16);
    let prompt_timeout = o.auth_prompt_timeout.unwrap_or(Duration::from_secs(120));
    let prompts = Prompts::new(fleet.clone(), prompt_timeout);
    let restarts = Arc::new(std::sync::Mutex::new(Default::default()));
    // The shared ssh connections of the hub's ssh hosts live in `<run>/ssh` and outlive this
    // daemon (ssh owns them); this only finds them again.
    let masters = crate::hub::master::Masters::new(o.layout().ssh_dir());
    let connector = BootstrapConnector {
        entries: entries.clone(),
        learned: ltx,
        prompts: prompts.clone(),
        restarts: restarts.clone(),
        masters: masters.clone(),
        own: version::info(),
        bin_dir: std::env::var_os(BIN_DIR_ENV)
            .filter(|v| !v.is_empty())
            .map(|v| wasit_process::plain_path(&PathBuf::from(v))),
        socket: o.socket_path(),
        diag_dir: o.layout().diag(),
    };
    let provisioner = AgentProvisioner {
        restarts,
        prompts: prompts.clone(),
        entries: entries.clone(),
    };
    let mut hcfg =
        HubConfig::new(Hello::new(build, host_id, "hub").with_machine(o.layout().machine.clone()));
    // One attempt includes the user answering sign-in prompts and an upload.
    hcfg.supervise.connect_timeout = prompt_timeout * 3 + Duration::from_secs(120);
    let hub = Hub::new(hcfg, Arc::new(connector), Some(Arc::new(provisioner)));
    let routing: Arc<dyn Service> = Arc::new(RoutingService::new(hub.clone(), local));
    hub.set_inbound_service(routing.clone());
    let local_id = HostId::new(host_id);
    fleet.feed_links(hub.clone());
    let local_entry = HostEntry {
        id: local_id.clone(),
        name: host_id.to_string(),
        kind: HostKind::Local,
        // This machine's platform is known without probing.
        platform: Some(marsad_proto::hosts::HostPlatform {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            target: crate::version::target().to_string(),
        }),
        created_at_ms: 0,
        interactive_auth: false,
        local_files: true,
        shared_connection: false,
        signed_out: false,
        machine: Some(o.layout().machine.clone()),
    };
    let registry = Registry::start(
        RegistryParts {
            store: store.clone(),
            hub: hub.clone(),
            fleet: fleet.clone(),
            entries,
            local: local_entry,
            prompts: prompts.clone(),
            masters,
        },
        lrx,
    )
    .await
    .map_err(|e| e.to_string())?;
    // Only now: the registry has put the local host into the fleet, which drops the session
    // feed of a host it does not know (the feed's first event, the full session list, would
    // be lost and the fleet would show the host without sessions until one changed).
    fleet.feed_local(local_id.clone(), manager);
    let link = loopback(routing.clone(), build, host_id);
    let git = GitWatch::spawn(link.clone(), fleet.clone());
    let envs = crate::hub::hostenv::HostEnvs::start(
        store.clone(),
        link.clone(),
        drivers.clone(),
        &hub,
        local_id.clone(),
    );
    let mut limits = Limits::default();
    if let Some(cap) = o.export_cap {
        limits.cap = cap;
    }
    if let Some(budget) = o.export_host_budget {
        limits.host_budget = budget;
    }
    let exports = Exports::spawn(ExportEnv {
        fleet: fleet.clone(),
        link: link.clone(),
        diag_dir: o.layout().diag(),
        hub: HubFacts {
            host_id: host_id.to_string(),
            build: build.to_string(),
            target: version::target().to_string(),
            machine: o.layout().machine.clone(),
            pid: std::process::id(),
        },
        limits,
    });
    fleet.set_git_watch(git.clone()).await;
    let workstreams = Workstreams::start(store.clone(), link, fleet.clone(), local_id.clone())
        .await
        .map_err(|e| e.to_string())?;
    let gated: Arc<dyn Service> = Arc::new(crate::hub::hostenv::EnvGated {
        envs: envs.clone(),
        inner: routing.clone(),
        local: local_id.clone(),
    });
    let svc = HubServices {
        registry,
        prompts,
        store,
        fleet,
        routing: routing.clone(),
        drivers,
        automatic: vec![Arc::new(WaslMcp), Arc::new(AskUserMcp)],
        workstreams,
        git,
        envs,
        exports,
        spawn_lock: tokio::sync::Mutex::new(()),
    };
    let mut top = ServiceMux::new();
    top.register(
        ROUTES
            .iter()
            .chain(HOST_ROUTES)
            .chain(wasl_proto::ROUTES)
            .chain(wasit_runtime::wire::DIAG_ROUTES)
            .copied(),
        routing,
    )
    .and_then(|_| top.register(HUB_ROUTES.iter().copied(), Arc::new(svc)))
    .and_then(|_| top.register(crate::hub::hostenv::ENV_GATED.iter().copied(), gated))
    .map_err(|e| e.to_string())?;
    let top: Arc<dyn Service> = Arc::new(top);
    // The hub is the spawn authority: its node (and, through it, every other host's node)
    // asks the hub's `workstreams.spawn.*` methods.
    node.set_spawn_authority(Arc::new(crate::hub::spawn::HubSpawner::new(loopback(
        top.clone(),
        build,
        host_id,
    ))));
    Ok((top, hub))
}

/// An in-process supervised connection to the hub's routing service: the workstream actor
/// talks to every host's node (local or remote) through it, with `host` on each request.
fn loopback(routing: Arc<dyn Service>, build: &str, host_id: &str) -> Supervised {
    let server = ControlServer::in_process(
        routing,
        Hello::new(build, host_id, "hub"),
        ConnConfig::default(),
    );
    let hello = Hello::new(build, host_id, "hub-internal");
    let conn = connector(move || {
        let server = server.clone();
        let hello = hello.clone();
        async move {
            let (a, b) = tokio::io::duplex(1 << 16);
            // The server side ends when the client side closes.
            drop(server.serve_connection(Box::new(a)));
            ControlClient::connect(Box::new(b), ClientOptions::new(hello)).await
        }
    });
    let mut opts = SupervisedOptions::default();
    opts.config.backoff.initial = Duration::from_millis(100);
    opts.config.backoff.max = Duration::from_secs(2);
    Supervised::spawn(conn, opts)
}

#[cfg(unix)]
async fn termination() -> &'static str {
    use tokio::signal::unix::{signal, SignalKind};
    match (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) {
        (Ok(mut t), Ok(mut i)) => tokio::select! {
            _ = t.recv() => "SIGTERM",
            _ = i.recv() => "SIGINT",
        },
        _ => {
            emit(MarsadEvent::DaemonSignalHandlersFailed {});
            std::future::pending().await
        }
    }
}

#[cfg(windows)]
async fn termination() -> &'static str {
    match tokio::signal::ctrl_c().await {
        Ok(()) => "ctrl-c",
        Err(_) => std::future::pending().await,
    }
}

/// Where a bridge is in getting its daemon (set by the start closure, read when the bridge
/// returns).
const START_NOT_CALLED: u8 = 0;
const START_RUNNING: u8 = 1;
const START_DONE: u8 = 2;
const START_FAILED: u8 = 3;
/// Another daemon of this machine won the lock while the start was under way.
const START_ANOTHER: u8 = 4;

/// What a successful start came to.
enum Started {
    /// The daemon printed its readiness line.
    Ready,
    /// It exited because another daemon holds this machine's lock (the connect that follows
    /// decides whether that one serves this endpoint).
    AnotherWon,
}

pub async fn bridge(o: Options) -> ExitCode {
    let began = Instant::now();
    let ep = o.endpoint();
    let raw = o.raw.clone();
    let layout = o.layout().clone();
    emit(MarsadEvent::BridgeStart {
        host_id: Ident::new(&o.host_id.clone().unwrap_or_else(default_host_id)),
        pid: std::process::id(),
        data: HomePath::new(&layout.data),
    });
    let start_state = Arc::new(AtomicU8::new(START_NOT_CALLED));
    let state = start_state.clone();
    let result = wasit_control::bridge(
        tokio::io::stdin(),
        tokio::io::stdout(),
        &ep,
        move || async move {
            state.store(START_RUNNING, Ordering::Release);
            let started = Instant::now();
            match start_daemon_process(raw, &layout).await {
                Ok(how) => {
                    let (outcome, mark) = match how {
                        Started::Ready => (BridgeOutcome::Started, START_DONE),
                        Started::AnotherWon => (BridgeOutcome::Running, START_ANOTHER),
                    };
                    state.store(mark, Ordering::Release);
                    emit(MarsadEvent::BridgeDaemon {
                        outcome,
                        ms: Some(diag::ms(started)),
                        reason: None,
                        exit: None,
                        error: None,
                    });
                    Ok(())
                }
                Err(f) => {
                    state.store(START_FAILED, Ordering::Release);
                    emit(MarsadEvent::BridgeDaemon {
                        outcome: BridgeOutcome::StartFailed,
                        ms: Some(diag::ms(started)),
                        reason: Some(f.reason),
                        exit: f.exit,
                        error: f.error,
                    });
                    Err(f.cause)
                }
            }
        },
        wasit_control::BridgeOptions::default(),
    )
    .await;
    let state = start_state.load(Ordering::Acquire);
    match result {
        Ok(()) => {
            if state == START_NOT_CALLED {
                // The first connect found a daemon (nothing earlier says so; when a start was
                // tried, `bridge.daemon` already said how it came out).
                emit(MarsadEvent::BridgeDaemon {
                    outcome: BridgeOutcome::Running,
                    ms: None,
                    reason: None,
                    exit: None,
                    error: None,
                });
            }
            emit(MarsadEvent::BridgeEnd {
                reason: BridgeEndReason::Ended,
                error: None,
            });
            ExitCode::SUCCESS
        }
        Err(e) => {
            let (reason, error) = match &e {
                wasit_control::BridgeError::Io(io) => (BridgeEndReason::IoError, diag::io(io)),
                wasit_control::BridgeError::Unavailable(_) => {
                    (BridgeEndReason::NoDaemon, diag::ours("unavailable", &e))
                }
            };
            if matches!(e, wasit_control::BridgeError::Unavailable(_)) {
                match state {
                    // The start closure already recorded why it failed.
                    START_FAILED => {}
                    // Its bound ran out while it was starting the daemon.
                    START_RUNNING => emit(MarsadEvent::BridgeDaemon {
                        outcome: BridgeOutcome::StartFailed,
                        ms: Some(diag::ms(began)),
                        reason: Some(StartFailure::TimedOut),
                        exit: None,
                        error: None,
                    }),
                    // Not serving (or not reachable): the endpoint could not be connected to.
                    _ => emit(MarsadEvent::BridgeDaemon {
                        outcome: BridgeOutcome::Unavailable,
                        ms: Some(diag::ms(began)),
                        reason: None,
                        exit: None,
                        error: Some(error.clone()),
                    }),
                }
            } else if state == START_NOT_CALLED {
                emit(MarsadEvent::BridgeDaemon {
                    outcome: BridgeOutcome::Running,
                    ms: None,
                    reason: None,
                    exit: None,
                    error: None,
                });
            }
            emit(MarsadEvent::BridgeEnd {
                reason,
                error: Some(error),
            });
            fail(format!("bridge: {e}"), 1)
        }
    }
}

/// Why starting a daemon failed: what is recorded, and the `io::Error` the bridge reports to
/// the hub (its text is part of the goodbye frame and is unchanged).
struct StartFailed {
    reason: StartFailure,
    exit: Option<i32>,
    error: Option<wasit_diag::ErrorInfo>,
    cause: std::io::Error,
}

impl StartFailed {
    fn io(reason: StartFailure, cause: std::io::Error) -> Self {
        StartFailed {
            reason,
            exit: None,
            error: Some(diag::io(&cause)),
            cause,
        }
    }
}

/// Start `serve` detached with `args` and wait for its readiness line. A daemon that exits
/// first is an error naming why (another instance holds this machine's lock, or see the log).
pub async fn start_detached(args: Vec<String>, layout: &Layout) -> std::io::Result<()> {
    start_daemon_process(args, layout)
        .await
        .map(|_| ())
        .map_err(|f| f.cause)
}

async fn start_daemon_process(args: Vec<String>, layout: &Layout) -> Result<Started, StartFailed> {
    use std::process::Stdio;
    use tokio::io::AsyncBufReadExt;
    let opener = layout.clone();
    // The daemon's stderr file: raw crash output, private, cut when large (`Layout::open_log`).
    let log = match tokio::task::spawn_blocking(move || opener.open_log()).await {
        Ok(Ok(f)) => f,
        Ok(Err(e)) => return Err(StartFailed::io(StartFailure::LogOpenFailed, e)),
        Err(e) => {
            return Err(StartFailed::io(
                StartFailure::LogOpenFailed,
                std::io::Error::other(e),
            ))
        }
    };
    let log_path = layout.log();
    let exe =
        std::env::current_exe().map_err(|e| StartFailed::io(StartFailure::NoExecutable, e))?;
    let mut cmd = wasit_process::detached_command(exe);
    cmd.arg("serve")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(log));
    let mut child = wasit_process::spawn_detached(&mut cmd)
        .map_err(|e| StartFailed::io(StartFailure::SpawnFailed, e))?;
    let stdout = child.stdout.take().ok_or_else(|| {
        StartFailed::io(StartFailure::NoStdout, std::io::Error::other("no stdout"))
    })?;
    let mut line = String::new();
    tokio::io::BufReader::new(stdout)
        .read_line(&mut line)
        .await
        .map_err(|e| StartFailed::io(StartFailure::ReadFailed, e))?;
    if !line.trim().is_empty() {
        drop(child);
        return Ok(Started::Ready);
    }
    // EOF without a readiness line: the daemon exited. Another instance on this machine that
    // won the lock is fine (the caller's connect decides); anything else is reported.
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait()).await;
    match status {
        Ok(Ok(s)) if s.code() == Some(EXIT_ALREADY_RUNNING as i32) => Ok(Started::AnotherWon),
        Ok(Ok(s)) => Err(StartFailed {
            reason: StartFailure::ExitedBeforeReady,
            exit: s.code(),
            error: None,
            cause: std::io::Error::other(format!(
                "the daemon exited ({s}) before it was ready; see {}",
                log_path.display()
            )),
        }),
        Ok(Err(e)) => Err(StartFailed::io(StartFailure::ReadFailed, e)),
        Err(_) => Err(StartFailed {
            reason: StartFailure::PipeClosedStillRunning,
            exit: None,
            error: None,
            cause: std::io::Error::other("the daemon closed its readiness pipe but did not exit"),
        }),
    }
}

/// How `stop` ended, for the exit status and the record.
struct Stopped {
    code: ExitCode,
    outcome: StopOutcome,
    legacy: bool,
    error: Option<wasit_diag::ErrorInfo>,
}

impl Stopped {
    fn new(code: ExitCode, outcome: StopOutcome, legacy: bool) -> Self {
        Stopped {
            code,
            outcome,
            legacy,
            error: None,
        }
    }
}

/// `stop`: ask the daemon to exit and wait until its single-instance lock is free.
pub async fn stop(o: Options) -> ExitCode {
    let began = Instant::now();
    let done = stop_daemon(&o).await;
    emit(MarsadEvent::StopEnd {
        outcome: done.outcome,
        ms: diag::ms(began),
        legacy: done.legacy,
        error: done.error,
    });
    done.code
}

async fn stop_daemon(o: &Options) -> Stopped {
    let mut legacy_stopped = false;
    if o.socket.is_none() {
        // A daemon of a build before the per-machine layout, on this machine.
        let legacy = wasit_control::local_endpoint(&o.layout().legacy_socket());
        if crate::migrate::stop_endpoint(&legacy).await {
            println!("stopped a daemon of an earlier build");
            legacy_stopped = true;
        }
    }
    let ep: Endpoint = o.endpoint();
    let opts = ClientOptions::new(Hello::new(version::build_id(), "marsad-agent-stop", "cli"));
    let client = match ControlClient::connect_endpoint(&ep, opts).await {
        Ok(c) => c,
        Err(wasit_control::ConnectError::Io(m)) => {
            println!("no daemon running ({m})");
            return Stopped::new(ExitCode::SUCCESS, StopOutcome::NotRunning, legacy_stopped);
        }
        Err(e) => {
            // What a peer wrote in a refusal is not recorded: code, length, fingerprint.
            return Stopped {
                error: Some(diag::foreign("connect", &e.to_string())),
                ..Stopped::new(
                    fail(format!("cannot reach the daemon: {e}"), 1),
                    StopOutcome::Unreachable,
                    legacy_stopped,
                )
            };
        }
    };
    // The daemon may close the connection before answering; both mean it is going down.
    let _ = client
        .request::<marsad_proto::Ack>(methods::DAEMON_SHUTDOWN, (), Duration::from_secs(10))
        .await;
    if tokio::time::timeout(Duration::from_secs(10), client.closed())
        .await
        .is_err()
    {
        return Stopped::new(
            fail("the daemon did not close the connection", 1),
            StopOutcome::NotClosed,
            legacy_stopped,
        );
    }
    let lock = wasit_control::lock_path_for(&o.socket_path());
    let mut delay = Duration::from_millis(2);
    for _ in 0..14 {
        match InstanceLock::try_acquire(&lock).await {
            Ok(l) => {
                drop(l);
                println!("stopped");
                return Stopped::new(ExitCode::SUCCESS, StopOutcome::Stopped, legacy_stopped);
            }
            Err(LockError::Held(_)) => {
                // Backoff schedule while the process exits.
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_millis(500));
            }
            Err(e) => {
                return Stopped {
                    error: Some(diag::ours("lock", &e)),
                    ..Stopped::new(fail(&e, 1), StopOutcome::LockError, legacy_stopped)
                };
            }
        }
    }
    Stopped::new(
        fail("the daemon did not exit", 1),
        StopOutcome::NotExited,
        legacy_stopped,
    )
}

/// The `SSH_ASKPASS` helper: forward `prompt` to the hub (which shows it to the user) and
/// print the answer. Exit 1 on cancel, timeout or any failure (ssh then gives up that prompt).
/// Its record names the host's id and the outcome; never the prompt, never the answer.
pub async fn askpass(prompt: String) -> ExitCode {
    use crate::hub::provision::{ASKPASS_ATTEMPT_ENV, ASKPASS_HOST_ENV, ASKPASS_SOCKET_ENV};
    let began = Instant::now();
    let (Ok(host), Some(socket), Some(attempt)) = (
        std::env::var(ASKPASS_HOST_ENV),
        std::env::var_os(ASKPASS_SOCKET_ENV),
        std::env::var(ASKPASS_ATTEMPT_ENV)
            .ok()
            .and_then(|a| a.parse::<u64>().ok()),
    ) else {
        emit(MarsadEvent::AskpassEnd {
            host: None,
            outcome: AskpassOutcome::NotAHubLink,
            ms: diag::ms(began),
            error: None,
        });
        return fail("askpass: not started by a Marsad hub link", 1);
    };
    let host_ident = Ident::new(&host);
    let ep = wasit_control::local_endpoint(std::path::Path::new(&socket));
    let opts = ClientOptions::new(Hello::new(version::build_id(), "marsad-askpass", "askpass"));
    let client = match ControlClient::connect_endpoint(&ep, opts).await {
        Ok(c) => c,
        Err(e) => {
            emit(MarsadEvent::AskpassEnd {
                host: Some(host_ident),
                outcome: AskpassOutcome::Unreachable,
                ms: diag::ms(began),
                error: Some(diag::foreign("connect", &e.to_string())),
            });
            return fail(format!("askpass: cannot reach the hub: {e}"), 1);
        }
    };
    let r: Result<marsad_proto::hosts::AskpassReply, _> = client
        .request(
            methods::HOSTS_ASKPASS,
            marsad_proto::hosts::AskpassRequest {
                host: HostId::new(host),
                attempt,
                prompt,
            },
            Duration::from_secs(24 * 3600),
        )
        .await;
    match r {
        Ok(a) => {
            emit(MarsadEvent::AskpassEnd {
                host: Some(host_ident),
                outcome: AskpassOutcome::Answered,
                ms: diag::ms(began),
                error: None,
            });
            println!("{}", a.answer);
            ExitCode::SUCCESS
        }
        Err(e) => {
            emit(MarsadEvent::AskpassEnd {
                host: Some(host_ident),
                outcome: AskpassOutcome::Refused,
                ms: diag::ms(began),
                error: Some(diag::foreign(call_code(&e), &e.to_string())),
            });
            fail(format!("askpass: {e}"), 1)
        }
    }
}

/// The machine word of a failed call (an RPC error's code, else the kind of failure).
fn call_code(e: &wasit_control::CallError) -> &str {
    use wasit_control::CallError;
    match e {
        CallError::Rpc(r) => &r.code,
        CallError::Timeout => "timeout",
        CallError::NotConnected => "not_connected",
        CallError::ConnectionLost(_) => "connection_lost",
        CallError::Encode(_) => "encode",
        CallError::Decode(_) => "decode",
    }
}
