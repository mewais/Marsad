//! `serve`, `bridge` and `stop`.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use marsad_proto::daemon::{DaemonInfo, Readiness};
use marsad_proto::hosts::{HostEntry, HostKind};
use marsad_proto::{methods, HostId, HOST_ROUTES, HUB_ROUTES};
use marsad_store::Store;
use tokio_util::sync::CancellationToken;
use wasit_control::{
    connector, BindError, ClientOptions, ConnConfig, ControlClient, ControlServer, Endpoint, Hello,
    Hub, HubConfig, InstanceLock, LockError, RoutingService, Service, ServiceMux, Supervised,
    SupervisedOptions, Uplink,
};
use wasit_core::DriverId;
use wasit_runtime::wire::ROUTES;
use wasit_runtime::{
    Discovery, DiscoveryConfig, RuntimeConfig, RuntimeService, SessionManager, ShimLauncher,
};
use wasl_node::{Node, NodeConfig, NodeService};

use crate::cli::{default_host_id, Options};
use crate::host::{pty::Ptys, HostServices};
use crate::hub::auth::Prompts;
use crate::hub::fleet::Fleet;
use crate::hub::gitwatch::GitWatch;
use crate::hub::hosts::Registry;
use crate::hub::provision::{AgentProvisioner, BootstrapConnector, BIN_DIR_ENV};
use crate::hub::workstreams::Workstreams;
use crate::hub::{AskUserMcp, HubServices, WaslMcp};
use crate::version;

/// Exit code when another daemon already serves the endpoint.
pub const EXIT_ALREADY_RUNNING: u8 = 3;

fn runtime_config(o: &Options) -> Result<RuntimeConfig, String> {
    let launcher = ShimLauncher::current_exe("shim").map_err(|e| e.to_string())?;
    let mut cfg = RuntimeConfig::new(o.data.clone(), launcher);
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

fn fail(msg: impl std::fmt::Display, code: u8) -> ExitCode {
    eprintln!("marsad-agent: {msg}");
    ExitCode::from(code)
}

pub async fn serve(o: Options) -> ExitCode {
    if let Err(e) = tokio::fs::create_dir_all(&o.data).await {
        return fail(format!("cannot create {}: {e}", o.data.display()), 1);
    }
    let cfg = match runtime_config(&o) {
        Ok(c) => c,
        Err(e) => return fail(e, 2),
    };
    let listener = match wasit_control::bind_local_exclusive(o.socket_path()).await {
        Ok(l) => l,
        Err(BindError::Lock(LockError::Held(p))) => {
            return fail(
                format!("another daemon is running (lock {} is held)", p.display()),
                EXIT_ALREADY_RUNNING,
            )
        }
        Err(e) => return fail(format!("cannot listen: {e}"), 1),
    };
    let build = version::build_id();
    let host_id = o.host_id.clone().unwrap_or_else(default_host_id);
    let discovery = Discovery::new(DiscoveryConfig::from_runtime(&cfg));
    let drivers = cfg.registry.clone();
    let manager = match SessionManager::new(cfg).await {
        Ok(m) => m,
        Err(e) => return fail(e, 1),
    };
    match manager.adopt_all().await {
        Ok(r) => {
            tracing::info!(
                running = r.running.len(),
                finished = r.finished.len(),
                "adopted sessions"
            );
            for (dir, e) in r.errors {
                tracing::warn!(dir = %dir.display(), error = %e, "session not adopted");
            }
        }
        Err(e) => tracing::error!(error = %e, "cannot scan sessions"),
    }
    let node = match Node::open(
        NodeConfig::new(host_id.clone(), o.data.join("wasl")),
        manager.clone(),
    )
    .await
    {
        Ok(n) => n,
        Err(e) => return fail(format!("cannot open the Wasl node: {e}"), 1),
    };
    let shutdown = CancellationToken::new();
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => return fail(format!("cannot locate the daemon executable: {e}"), 1),
    };
    let info = DaemonInfo {
        version: version::info(),
        host_id: host_id.clone(),
        hub: o.hub,
        data_dir: o.data.display().to_string(),
        pid: std::process::id(),
        exe: exe.display().to_string(),
        socket: o.socket_path().display().to_string(),
    };
    let runtime = RuntimeService::new(
        manager.clone(),
        discovery.clone(),
        host_id.clone(),
        build.clone(),
    );
    let host = HostServices {
        manager: manager.clone(),
        discovery,
        ptys: Ptys::default(),
        info,
        shutdown: shutdown.clone(),
    };
    let mut local = ServiceMux::new();
    if let Err(e) = local
        .register(ROUTES.iter().copied(), Arc::new(runtime))
        .and_then(|_| local.register(HOST_ROUTES.iter().copied(), Arc::new(host)))
        .and_then(|_| {
            local.register(
                wasl_proto::ROUTES.iter().copied(),
                Arc::new(NodeService::new(node.clone())),
            )
        })
    {
        return fail(e, 1);
    }
    let local: Arc<dyn Service> = Arc::new(local);

    let mut hub_handle: Option<Hub> = None;
    let top: Arc<dyn Service> = if o.hub {
        match start_hub(
            &o,
            &host_id,
            &build,
            local.clone(),
            manager.clone(),
            drivers,
        )
        .await
        {
            Ok((svc, hub)) => {
                // The hub is the supervisor: handoffs always raise gates here, and other
                // hosts are reached over its own links.
                node.attach_uplink(Uplink::via_hub(hub.clone()));
                node.set_supervisor_probe(Arc::new(|| true));
                hub_handle = Some(hub);
                svc
            }
            Err(e) => return fail(e, 1),
        }
    } else {
        local
    };
    let role = if o.hub { "hub" } else { "daemon" };
    let server = ControlServer::new(
        top,
        Hello::new(build.clone(), host_id, role),
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
    match serde_json::to_string(&ready) {
        Ok(line) => println!("{line}"),
        Err(e) => return fail(e, 1),
    }
    let code = tokio::select! {
        r = server.serve(listener) => match r {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => fail(format!("serve failed: {e}"), 1),
        },
        what = termination() => {
            tracing::info!("{what}: shutting down (sessions keep running)");
            ExitCode::SUCCESS
        }
        _ = shutdown.cancelled() => ExitCode::SUCCESS,
    };
    server.shutdown(Duration::from_secs(2)).await;
    if let Some(h) = hub_handle {
        h.shutdown().await;
    }
    code
}

async fn start_hub(
    o: &Options,
    host_id: &str,
    build: &str,
    local: Arc<dyn Service>,
    manager: SessionManager,
    drivers: wasit_core::DriverRegistry,
) -> Result<(Arc<dyn Service>, Hub), String> {
    let store = Store::open(o.data.join("store"))
        .await
        .map_err(|e| e.to_string())?;
    let fleet = Fleet::spawn();
    let entries = Arc::new(RwLock::new(Default::default()));
    let (ltx, lrx) = tokio::sync::mpsc::channel(16);
    let prompt_timeout = o.auth_prompt_timeout.unwrap_or(Duration::from_secs(120));
    let prompts = Prompts::new(fleet.clone(), prompt_timeout);
    let restarts = Arc::new(std::sync::Mutex::new(Default::default()));
    let connector = BootstrapConnector {
        entries: entries.clone(),
        learned: ltx,
        prompts: prompts.clone(),
        restarts: restarts.clone(),
        own: version::info(),
        bin_dir: std::env::var_os(BIN_DIR_ENV).map(PathBuf::from),
        socket: o.socket_path(),
    };
    let provisioner = AgentProvisioner {
        restarts,
        prompts: prompts.clone(),
    };
    let mut hcfg = HubConfig::new(Hello::new(build, host_id, "hub"));
    // One attempt includes the user answering sign-in prompts and an upload.
    hcfg.supervise.connect_timeout = prompt_timeout * 3 + Duration::from_secs(120);
    let hub = Hub::new(hcfg, Arc::new(connector), Some(Arc::new(provisioner)));
    let routing: Arc<dyn Service> = Arc::new(RoutingService::new(hub.clone(), local));
    hub.set_inbound_service(routing.clone());
    let local_id = HostId::new(host_id);
    fleet.feed_local(local_id.clone(), manager);
    fleet.feed_links(hub.clone());
    let local_entry = HostEntry {
        id: local_id.clone(),
        name: host_id.to_string(),
        kind: HostKind::Local,
        platform: None,
        created_at_ms: 0,
        interactive_auth: false,
    };
    let registry = Registry::start(
        store.clone(),
        hub.clone(),
        fleet.clone(),
        entries,
        local_entry,
        prompts.clone(),
        lrx,
    )
    .await
    .map_err(|e| e.to_string())?;
    let link = loopback(routing.clone(), build, host_id);
    let git = GitWatch::spawn(link.clone(), fleet.clone());
    fleet.set_git_watch(git.clone()).await;
    let workstreams = Workstreams::start(store.clone(), link, fleet.clone(), local_id.clone())
        .await
        .map_err(|e| e.to_string())?;
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
    };
    let mut top = ServiceMux::new();
    top.register(
        ROUTES
            .iter()
            .chain(HOST_ROUTES)
            .chain(wasl_proto::ROUTES)
            .copied(),
        routing,
    )
    .and_then(|_| top.register(HUB_ROUTES.iter().copied(), Arc::new(svc)))
    .map_err(|e| e.to_string())?;
    Ok((Arc::new(top), hub))
}

/// An in-process supervised connection to the hub's routing service: the workstream actor
/// talks to every host's node (local or remote) through it, with `host` on each request.
fn loopback(routing: Arc<dyn Service>, build: &str, host_id: &str) -> Supervised {
    let server = ControlServer::new(
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
            tracing::error!("cannot install signal handlers");
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

pub async fn bridge(o: Options) -> ExitCode {
    let ep = o.endpoint();
    let raw = o.raw.clone();
    let data = o.data.clone();
    match wasit_control::bridge(
        tokio::io::stdin(),
        tokio::io::stdout(),
        &ep,
        move || start_detached(raw, data),
        wasit_control::BridgeOptions::default(),
    )
    .await
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(format!("bridge: {e}"), 1),
    }
}

/// Start `serve` detached with `args` and wait for its readiness line (or its exit).
pub async fn start_detached(args: Vec<String>, data: PathBuf) -> std::io::Result<()> {
    use std::process::Stdio;
    use tokio::io::AsyncBufReadExt;
    tokio::fs::create_dir_all(&data).await?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(data.join("marsad-agent.log"))?;
    let exe = std::env::current_exe()?;
    let mut cmd = tokio::process::Command::new(exe);
    cmd.arg("serve")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(log))
        .kill_on_drop(false);
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    {
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    let mut child = cmd.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("no stdout"))?;
    let mut line = String::new();
    tokio::io::BufReader::new(stdout)
        .read_line(&mut line)
        .await?;
    // EOF without a readiness line usually means another instance won the lock; the caller's
    // connect decides.
    drop(child);
    Ok(())
}

/// `stop`: ask the daemon to exit and wait until its single-instance lock is free.
pub async fn stop(o: Options) -> ExitCode {
    let ep: Endpoint = o.endpoint();
    let opts = ClientOptions::new(Hello::new(version::build_id(), "marsad-agent-stop", "cli"));
    let client = match ControlClient::connect_endpoint(&ep, opts).await {
        Ok(c) => c,
        Err(wasit_control::ConnectError::Io(m)) => {
            println!("no daemon running ({m})");
            return ExitCode::SUCCESS;
        }
        Err(e) => return fail(format!("cannot reach the daemon: {e}"), 1),
    };
    // The daemon may close the connection before answering; both mean it is going down.
    let _ = client
        .request::<marsad_proto::Ack>(methods::DAEMON_SHUTDOWN, (), Duration::from_secs(10))
        .await;
    if tokio::time::timeout(Duration::from_secs(10), client.closed())
        .await
        .is_err()
    {
        return fail("the daemon did not close the connection", 1);
    }
    let lock = wasit_control::lock_path_for(&o.socket_path());
    let mut delay = Duration::from_millis(2);
    for _ in 0..14 {
        match InstanceLock::try_acquire(&lock).await {
            Ok(l) => {
                drop(l);
                println!("stopped");
                return ExitCode::SUCCESS;
            }
            Err(LockError::Held(_)) => {
                // Backoff schedule while the process exits.
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_millis(500));
            }
            Err(e) => return fail(e, 1),
        }
    }
    fail("the daemon did not exit", 1)
}

/// The `SSH_ASKPASS` helper: forward `prompt` to the hub (which shows it to the user) and
/// print the answer. Exit 1 on cancel, timeout or any failure (ssh then gives up that prompt).
pub async fn askpass(prompt: String) -> ExitCode {
    use crate::hub::provision::{ASKPASS_HOST_ENV, ASKPASS_SOCKET_ENV};
    let (Ok(host), Some(socket)) = (
        std::env::var(ASKPASS_HOST_ENV),
        std::env::var_os(ASKPASS_SOCKET_ENV),
    ) else {
        return fail("askpass: not started by a Marsad hub link", 1);
    };
    let ep = wasit_control::local_endpoint(std::path::Path::new(&socket));
    let opts = ClientOptions::new(Hello::new(version::build_id(), "marsad-askpass", "askpass"));
    let client = match ControlClient::connect_endpoint(&ep, opts).await {
        Ok(c) => c,
        Err(e) => return fail(format!("askpass: cannot reach the hub: {e}"), 1),
    };
    let r: Result<marsad_proto::hosts::AskpassReply, _> = client
        .request(
            methods::HOSTS_ASKPASS,
            marsad_proto::hosts::AskpassRequest {
                host: HostId::new(host),
                prompt,
            },
            Duration::from_secs(24 * 3600),
        )
        .await;
    match r {
        Ok(a) => {
            println!("{}", a.answer);
            ExitCode::SUCCESS
        }
        Err(e) => fail(format!("askpass: {e}"), 1),
    }
}
