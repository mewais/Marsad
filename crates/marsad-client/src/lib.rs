//! marsad-client — the typed async client of the local Marsad daemon (the hub). This is what
//! the Tauri shell (and later phones) use.
//!
//! * [`MarsadClient::connect`] supervises one link to the daemon's local endpoint
//!   (`wasit_control::Supervised`: reconnect with backoff, durable subscriptions resume from
//!   their cursors). If nothing serves the endpoint, the daemon is started detached
//!   (`marsad-agent serve --hub …`) and its readiness line awaited.
//! * Every Marsad method and topic is a typed method here; the Wasit runtime's methods
//!   (sessions, commands, history, session events) are reached through
//!   [`MarsadClient::runtime`] for any host.

#![forbid(unsafe_code)]

use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use marsad_proto::agentconfig::{
    BaseConfig, BaseRequest, EffectiveConfig, EffectiveRequest, LaunchReply, LaunchRequest,
    LayerScope, OverrideLayer,
};
use marsad_proto::daemon::{DaemonInfo, Readiness};
use marsad_proto::fleet::FleetEvent;
use marsad_proto::fs::{FsExists, FsHome, FsListRequest, FsListing, FsPathRequest};
use marsad_proto::git::{
    DiffSide, GitCommitReply, GitCommitRequest, GitDiff, GitDiffRequest, GitDiscardRequest,
    GitStatus, GitStatusRequest, GitTarget,
};
use marsad_proto::hosts::{HostDraft, HostEntry, HostList, HostRef, ImportSuggestions, LinkStates};
use marsad_proto::pty::{
    PtyEvent, PtyInfo, PtyInputRequest, PtyList, PtyRef, PtyResizeRequest, PtyStartRequest,
};
use marsad_proto::store::{
    NotificationPrefs, RecentProject, RecentProjects, SeenMark, SeenMarkers, SettingSet, Settings,
    StoredLayer,
};
use marsad_proto::workstream::{
    AddNode, Connect, EdgeHistoryPage, EdgeHistoryRequest, EdgeRef, FromTemplate,
    LaunchWorkstreamReply, NodeRef, SaveTemplate, TemplateList, TemplateRef, Workstream,
    WorkstreamDraft, WorkstreamList, WorkstreamRef, WorkstreamTemplate, WorkstreamUpdate,
    WorkstreamsEvent,
};
use marsad_proto::{methods, topics, Ack, DriverId, HostId, MarsadError};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::AsyncBufReadExt;
use wasit_control::{
    connector, CallError, ClientOptions, ConnectError, ControlClient, Cursor, DurableOpts,
    DurableSubscription, Endpoint, Hello, RequestOpts, Supervised, SupervisedOptions,
};
use wasit_runtime::RuntimeClient;

/// A failed call.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ClientError {
    /// The daemon refused or failed the operation.
    #[error(transparent)]
    Marsad(MarsadError),
    /// Transport / protocol failure, or an error without typed data.
    #[error(transparent)]
    Call(CallError),
}

impl ClientError {
    fn from_call(e: CallError) -> Self {
        match &e {
            CallError::Rpc(rpc) => match MarsadError::from_rpc(rpc) {
                Some(m) => ClientError::Marsad(m),
                None => ClientError::Call(e),
            },
            _ => ClientError::Call(e),
        }
    }
}

/// How to reach (and if needed start) the local daemon.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub data_dir: PathBuf,
    /// Endpoint path (default `<data>/marsad.sock`).
    pub socket: Option<PathBuf>,
    /// Start the daemon when nothing serves the endpoint.
    pub start_daemon: bool,
    /// The daemon binary (default: `$MARSAD_AGENT_BIN`, else `marsad-agent` next to the
    /// running executable, else on `PATH`).
    pub agent_bin: Option<PathBuf>,
    /// Extra arguments for `serve` (e.g. `--driver-bin`).
    pub serve_args: Vec<String>,
    /// Environment set on a daemon this client starts (on top of its own environment).
    pub daemon_env: Vec<(String, String)>,
    pub hello: Hello,
    /// Per-call deadline.
    pub deadline: Duration,
    /// How long a call waits for a disconnected link.
    pub wait_connected: Duration,
    pub supervise: wasit_control::SupervisedConfig,
}

impl ClientConfig {
    pub fn new(data_dir: impl Into<PathBuf>, hello: Hello) -> Self {
        ClientConfig {
            data_dir: data_dir.into(),
            socket: None,
            start_daemon: true,
            agent_bin: None,
            serve_args: Vec::new(),
            daemon_env: Vec::new(),
            hello,
            deadline: Duration::from_secs(120),
            wait_connected: Duration::from_secs(30),
            supervise: wasit_control::SupervisedConfig::default(),
        }
    }

    pub fn socket_path(&self) -> PathBuf {
        self.socket
            .clone()
            .unwrap_or_else(|| self.data_dir.join("marsad.sock"))
    }

    pub fn endpoint(&self) -> Endpoint {
        wasit_control::local_endpoint(&self.socket_path())
    }

    fn agent(&self) -> PathBuf {
        if let Some(p) = &self.agent_bin {
            return p.clone();
        }
        if let Some(p) = std::env::var_os("MARSAD_AGENT_BIN") {
            return PathBuf::from(p);
        }
        let name = format!("marsad-agent{}", std::env::consts::EXE_SUFFIX);
        std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|d| d.join(&name)))
            .filter(|p| p.is_file())
            .unwrap_or_else(|| PathBuf::from(name))
    }
}

/// Start `marsad-agent serve --hub` detached and wait for its readiness line.
async fn start_daemon(cfg: &ClientConfig) -> Result<(), ConnectError> {
    use std::process::Stdio;
    tokio::fs::create_dir_all(&cfg.data_dir)
        .await
        .map_err(|e| ConnectError::Spawn(format!("{}: {e}", cfg.data_dir.display())))?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(cfg.data_dir.join("marsad-agent.log"))
        .map_err(|e| ConnectError::Spawn(format!("daemon log: {e}")))?;
    let bin = cfg.agent();
    let mut cmd = tokio::process::Command::new(&bin);
    cmd.arg("serve")
        .arg("--hub")
        .arg("--data")
        .arg(&cfg.data_dir);
    if let Some(s) = &cfg.socket {
        cmd.arg("--socket").arg(s);
    }
    cmd.args(&cfg.serve_args)
        .envs(cfg.daemon_env.iter().map(|(k, v)| (k, v)))
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
    let mut child = cmd
        .spawn()
        .map_err(|e| ConnectError::Spawn(format!("{}: {e}", bin.display())))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ConnectError::Spawn("daemon has no stdout".into()))?;
    let mut line = String::new();
    let mut reader = tokio::io::BufReader::new(stdout);
    let read = reader.read_line(&mut line);
    match tokio::time::timeout(Duration::from_secs(60), read).await {
        Err(_) => {
            return Err(ConnectError::Spawn(
                "the daemon did not become ready".into(),
            ))
        }
        Ok(Err(e)) => return Err(ConnectError::Spawn(format!("daemon readiness: {e}"))),
        Ok(Ok(_)) => {}
    }
    // An empty line means the daemon exited first, usually because another instance won the
    // lock; the connect that follows decides.
    if !line.trim().is_empty() {
        let r: Readiness = serde_json::from_str(line.trim())
            .map_err(|e| ConnectError::Spawn(format!("bad readiness line {line:?}: {e}")))?;
        tracing::info!(pid = r.pid, build = %r.build, "started the Marsad daemon");
    }
    drop(child);
    Ok(())
}

async fn dial(
    cfg: Arc<ClientConfig>,
    starting: Arc<tokio::sync::Mutex<()>>,
) -> Result<ControlClient, ConnectError> {
    let ep = cfg.endpoint();
    let opts = ClientOptions::new(cfg.hello.clone());
    let attempt = |ep: Endpoint| async move {
        tokio::time::timeout(Duration::from_secs(5), ep.connect())
            .await
            .unwrap_or_else(|_| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "connect timed out",
                ))
            })
    };
    let io = match attempt(ep.clone()).await {
        Ok(io) => io,
        Err(e) if cfg.start_daemon && Endpoint::is_absent_error(&e) => {
            // One start at a time from this client; the daemon's own lock handles others.
            let _g = starting.lock().await;
            match attempt(ep.clone()).await {
                Ok(io) => io,
                Err(_) => {
                    start_daemon(&cfg).await?;
                    attempt(ep)
                        .await
                        .map_err(|e| ConnectError::Io(e.to_string()))?
                }
            }
        }
        Err(e) => return Err(ConnectError::Io(e.to_string())),
    };
    ControlClient::connect(io, opts).await
}

/// A durable, typed subscription (resubscribes from its last cursor across reconnects and
/// `Lagged` closes).
pub struct Subscription<T> {
    inner: DurableSubscription,
    _t: PhantomData<fn() -> T>,
}

impl<T: DeserializeOwned> Subscription<T> {
    /// Next event; `Some(Err)` once for a terminal error; `None` when the topic ended.
    pub async fn next(&mut self) -> Option<Result<(Cursor, T), ClientError>> {
        match self.inner.next().await? {
            Ok((c, v)) => Some(
                serde_json::from_value(v)
                    .map(|t| (c, t))
                    .map_err(|e| ClientError::Call(CallError::Decode(e.to_string()))),
            ),
            Err(e) => Some(Err(ClientError::from_call(CallError::Rpc(e)))),
        }
    }

    pub fn last_cursor(&self) -> Option<&Cursor> {
        self.inner.last_cursor()
    }

    pub fn subscribe_count(&self) -> u64 {
        self.inner.subscribe_count()
    }
}

/// See the crate docs. Cheap to clone.
#[derive(Clone)]
pub struct MarsadClient {
    link: Supervised,
    deadline: Duration,
    wait_connected: Duration,
}

impl MarsadClient {
    /// Supervise a link to the local daemon (starting it on demand).
    pub fn connect(cfg: ClientConfig) -> Self {
        let deadline = cfg.deadline;
        let wait_connected = cfg.wait_connected;
        let supervise = cfg.supervise.clone();
        let cfg = Arc::new(cfg);
        let starting = Arc::new(tokio::sync::Mutex::new(()));
        let conn = connector(move || dial(cfg.clone(), starting.clone()));
        MarsadClient {
            link: Supervised::spawn(
                conn,
                SupervisedOptions {
                    config: supervise,
                    ..SupervisedOptions::default()
                },
            ),
            deadline,
            wait_connected,
        }
    }

    pub fn link(&self) -> &Supervised {
        &self.link
    }

    /// The Wasit runtime client for `host` (`None` = the hub's own host).
    pub fn runtime(&self, host: Option<HostId>) -> RuntimeClient {
        let c = RuntimeClient::new(self.link.clone())
            .with_deadlines(self.deadline, self.wait_connected);
        match host {
            Some(h) => c.with_host(h),
            None => c,
        }
    }

    async fn call<P: Serialize, R: DeserializeOwned>(
        &self,
        host: Option<&HostId>,
        method: &str,
        params: P,
    ) -> Result<R, ClientError> {
        let mut opts = RequestOpts::new(self.deadline);
        opts.host = host.cloned();
        self.link
            .request(method, params, opts, self.wait_connected)
            .await
            .map_err(ClientError::from_call)
    }

    fn subscribe<T>(
        &self,
        host: Option<&HostId>,
        topic: String,
        from: Option<Cursor>,
    ) -> Subscription<T> {
        Subscription {
            inner: self.link.subscribe_durable(
                topic,
                DurableOpts {
                    host: host.cloned(),
                    from,
                    ..DurableOpts::default()
                },
            ),
            _t: PhantomData,
        }
    }

    // ── daemon ──────────────────────────────────────────────────────────────

    pub async fn daemon_info(&self, host: Option<&HostId>) -> Result<DaemonInfo, ClientError> {
        self.call(host, methods::DAEMON_INFO, ()).await
    }

    // ── git ─────────────────────────────────────────────────────────────────

    pub async fn git_status(
        &self,
        host: Option<&HostId>,
        target: GitTarget,
    ) -> Result<GitStatus, ClientError> {
        self.call(host, methods::GIT_STATUS, GitStatusRequest { target })
            .await
    }

    pub async fn git_diff(
        &self,
        host: Option<&HostId>,
        target: GitTarget,
        path: &str,
        side: DiffSide,
        max_bytes: Option<u32>,
    ) -> Result<GitDiff, ClientError> {
        self.call(
            host,
            methods::GIT_DIFF,
            GitDiffRequest {
                target,
                path: path.into(),
                side,
                max_bytes,
            },
        )
        .await
    }

    pub async fn git_commit(
        &self,
        host: Option<&HostId>,
        target: GitTarget,
        message: &str,
        files: Vec<String>,
    ) -> Result<GitCommitReply, ClientError> {
        self.call(
            host,
            methods::GIT_COMMIT,
            GitCommitRequest {
                target,
                message: message.into(),
                files,
            },
        )
        .await
    }

    pub async fn git_discard(
        &self,
        host: Option<&HostId>,
        target: GitTarget,
        files: Vec<String>,
    ) -> Result<(), ClientError> {
        let _: Ack = self
            .call(
                host,
                methods::GIT_DISCARD,
                GitDiscardRequest { target, files },
            )
            .await?;
        Ok(())
    }

    // ── pty ─────────────────────────────────────────────────────────────────

    pub async fn pty_start(
        &self,
        host: Option<&HostId>,
        req: PtyStartRequest,
    ) -> Result<PtyInfo, ClientError> {
        self.call(host, methods::PTY_START, req).await
    }

    pub async fn pty_input(
        &self,
        host: Option<&HostId>,
        id: &str,
        data: &str,
    ) -> Result<(), ClientError> {
        let _: Ack = self
            .call(
                host,
                methods::PTY_INPUT,
                PtyInputRequest {
                    id: id.into(),
                    data: data.into(),
                },
            )
            .await?;
        Ok(())
    }

    pub async fn pty_resize(
        &self,
        host: Option<&HostId>,
        id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(), ClientError> {
        let _: Ack = self
            .call(
                host,
                methods::PTY_RESIZE,
                PtyResizeRequest {
                    id: id.into(),
                    cols,
                    rows,
                },
            )
            .await?;
        Ok(())
    }

    pub async fn pty_close(&self, host: Option<&HostId>, id: &str) -> Result<(), ClientError> {
        let _: Ack = self
            .call(host, methods::PTY_CLOSE, PtyRef { id: id.into() })
            .await?;
        Ok(())
    }

    pub async fn pty_list(&self, host: Option<&HostId>) -> Result<Vec<PtyInfo>, ClientError> {
        let l: PtyList = self.call(host, methods::PTY_LIST, ()).await?;
        Ok(l.ptys)
    }

    /// Terminal output after byte offset `from` (from the scrollback start when `None`).
    pub fn pty_output(
        &self,
        host: Option<&HostId>,
        id: &str,
        from: Option<u64>,
    ) -> Subscription<PtyEvent> {
        self.subscribe(host, topics::pty_output(id), from.map(Cursor::from_u64))
    }

    // ── fs ──────────────────────────────────────────────────────────────────

    pub async fn fs_list(
        &self,
        host: Option<&HostId>,
        req: FsListRequest,
    ) -> Result<FsListing, ClientError> {
        self.call(host, methods::FS_LIST, req).await
    }

    pub async fn fs_home(&self, host: Option<&HostId>) -> Result<String, ClientError> {
        let h: FsHome = self.call(host, methods::FS_HOME, ()).await?;
        Ok(h.path)
    }

    pub async fn fs_exists(
        &self,
        host: Option<&HostId>,
        path: &str,
    ) -> Result<FsExists, ClientError> {
        self.call(
            host,
            methods::FS_EXISTS,
            FsPathRequest { path: path.into() },
        )
        .await
    }

    // ── agent config / launch ───────────────────────────────────────────────

    pub async fn agentconfig_base(
        &self,
        host: Option<&HostId>,
        driver: impl Into<DriverId>,
        project: Option<String>,
        refresh: bool,
    ) -> Result<BaseConfig, ClientError> {
        self.call(
            host,
            methods::AGENTCONFIG_BASE,
            BaseRequest {
                driver: driver.into(),
                project,
                refresh,
            },
        )
        .await
    }

    pub async fn agentconfig_effective(
        &self,
        req: EffectiveRequest,
    ) -> Result<EffectiveConfig, ClientError> {
        self.call(None, methods::AGENTCONFIG_EFFECTIVE, req).await
    }

    pub async fn launch(&self, req: LaunchRequest) -> Result<LaunchReply, ClientError> {
        self.call(None, methods::SESSIONS_LAUNCH, req).await
    }

    /// Refresh one directory's git summary in the `fleet` topic now.
    pub async fn git_refresh(
        &self,
        host: HostId,
        cwd: impl Into<String>,
    ) -> Result<(), ClientError> {
        let _: Ack = self
            .call(
                None,
                methods::FLEET_GIT_REFRESH,
                marsad_proto::fleet::GitRefresh {
                    host,
                    cwd: cwd.into(),
                },
            )
            .await?;
        Ok(())
    }

    // ── hosts ───────────────────────────────────────────────────────────────

    pub async fn hosts(&self) -> Result<Vec<HostEntry>, ClientError> {
        let l: HostList = self.call(None, methods::HOSTS_LIST, ()).await?;
        Ok(l.hosts)
    }

    pub async fn add_host(&self, draft: HostDraft) -> Result<HostEntry, ClientError> {
        self.call(None, methods::HOSTS_ADD, draft).await
    }

    pub async fn update_host(&self, entry: HostEntry) -> Result<HostEntry, ClientError> {
        self.call(None, methods::HOSTS_UPDATE, entry).await
    }

    pub async fn remove_host(&self, id: &HostId) -> Result<(), ClientError> {
        let _: Ack = self
            .call(None, methods::HOSTS_REMOVE, HostRef { id: id.clone() })
            .await?;
        Ok(())
    }

    pub async fn reconnect_host(&self, id: &HostId) -> Result<(), ClientError> {
        let _: Ack = self
            .call(None, methods::HOSTS_RECONNECT, HostRef { id: id.clone() })
            .await?;
        Ok(())
    }

    /// Answer a sign-in prompt (the answer goes to ssh only; never logged or stored).
    pub async fn answer_auth(
        &self,
        host: &HostId,
        id: u64,
        answer: &str,
    ) -> Result<(), ClientError> {
        let _: Ack = self
            .call(
                None,
                methods::HOSTS_AUTH_ANSWER,
                marsad_proto::hosts::AuthAnswer {
                    host: host.clone(),
                    id,
                    answer: answer.into(),
                },
            )
            .await?;
        Ok(())
    }

    /// Give up a sign-in prompt (the link goes `auth_needed`).
    pub async fn cancel_auth(&self, host: &HostId, id: u64) -> Result<(), ClientError> {
        let _: Ack = self
            .call(
                None,
                methods::HOSTS_AUTH_CANCEL,
                marsad_proto::hosts::AuthCancel {
                    host: host.clone(),
                    id,
                },
            )
            .await?;
        Ok(())
    }

    pub async fn import_suggestions(&self) -> Result<ImportSuggestions, ClientError> {
        self.call(None, methods::HOSTS_IMPORT_SUGGESTIONS, ()).await
    }

    pub fn link_states(&self) -> Subscription<LinkStates> {
        self.subscribe(None, topics::HOSTS_LINKS.into(), None)
    }

    /// Everything a dashboard needs across all hosts.
    pub fn fleet(&self, from: Option<Cursor>) -> Subscription<FleetEvent> {
        self.subscribe(None, topics::FLEET.into(), from)
    }

    // ── store ───────────────────────────────────────────────────────────────

    pub async fn settings(&self) -> Result<Settings, ClientError> {
        self.call(None, methods::STORE_SETTINGS_GET, ()).await
    }

    pub async fn set_setting(
        &self,
        key: &str,
        value: serde_json::Value,
    ) -> Result<Settings, ClientError> {
        self.call(
            None,
            methods::STORE_SETTINGS_SET,
            SettingSet {
                key: key.into(),
                value,
            },
        )
        .await
    }

    pub async fn notification_prefs(&self) -> Result<NotificationPrefs, ClientError> {
        self.call(None, methods::STORE_NOTIFICATIONS_GET, ()).await
    }

    pub async fn set_notification_prefs(
        &self,
        p: NotificationPrefs,
    ) -> Result<NotificationPrefs, ClientError> {
        self.call(None, methods::STORE_NOTIFICATIONS_SET, p).await
    }

    pub async fn layers(&self) -> Result<Vec<StoredLayer>, ClientError> {
        let l: marsad_proto::store::LayerList =
            self.call(None, methods::STORE_LAYERS_LIST, ()).await?;
        Ok(l.layers)
    }

    pub async fn layer(&self, scope: LayerScope) -> Result<OverrideLayer, ClientError> {
        self.call(None, methods::STORE_LAYERS_GET, scope).await
    }

    pub async fn set_layer(
        &self,
        scope: LayerScope,
        layer: OverrideLayer,
    ) -> Result<(), ClientError> {
        let _: Ack = self
            .call(
                None,
                methods::STORE_LAYERS_SET,
                StoredLayer { scope, layer },
            )
            .await?;
        Ok(())
    }

    pub async fn seen(&self) -> Result<Vec<SeenMark>, ClientError> {
        let m: SeenMarkers = self.call(None, methods::STORE_SEEN_LIST, ()).await?;
        Ok(m.markers)
    }

    pub async fn mark_seen(&self, mark: SeenMark) -> Result<(), ClientError> {
        let _: Ack = self.call(None, methods::STORE_SEEN_MARK, mark).await?;
        Ok(())
    }

    pub async fn mark_all_seen(&self) -> Result<Vec<SeenMark>, ClientError> {
        let m: SeenMarkers = self.call(None, methods::STORE_SEEN_MARK_ALL, ()).await?;
        Ok(m.markers)
    }

    pub async fn recent_projects(&self) -> Result<Vec<RecentProject>, ClientError> {
        let r: RecentProjects = self.call(None, methods::STORE_RECENT_LIST, ()).await?;
        Ok(r.projects)
    }

    pub async fn add_recent_project(
        &self,
        p: RecentProject,
    ) -> Result<Vec<RecentProject>, ClientError> {
        let r: RecentProjects = self.call(None, methods::STORE_RECENT_ADD, p).await?;
        Ok(r.projects)
    }

    // ── generic (for bridges such as the desktop shell) ─────────────────────

    /// Any method, untyped (`host` routes it like the typed calls).
    pub async fn call_raw(
        &self,
        host: Option<&HostId>,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, ClientError> {
        self.call(host, method, params).await
    }

    /// Any topic, untyped and durable.
    pub fn subscribe_raw(
        &self,
        host: Option<&HostId>,
        topic: impl Into<String>,
        from: Option<Cursor>,
    ) -> Subscription<serde_json::Value> {
        self.subscribe(host, topic.into(), from)
    }

    // ── workstreams ─────────────────────────────────────────────────────────

    /// The `workstreams` topic.
    pub fn workstreams_topic(&self, from: Option<Cursor>) -> Subscription<WorkstreamsEvent> {
        self.subscribe(None, topics::WORKSTREAMS.into(), from)
    }

    pub async fn workstreams(&self) -> Result<Vec<Workstream>, ClientError> {
        let l: WorkstreamList = self.call(None, methods::WS_LIST, ()).await?;
        Ok(l.workstreams)
    }

    pub async fn workstream(&self, id: &str) -> Result<Workstream, ClientError> {
        self.call(None, methods::WS_GET, WorkstreamRef { id: id.into() })
            .await
    }

    pub async fn create_workstream(&self, d: WorkstreamDraft) -> Result<Workstream, ClientError> {
        self.call(None, methods::WS_CREATE, d).await
    }

    pub async fn update_workstream(&self, u: WorkstreamUpdate) -> Result<Workstream, ClientError> {
        self.call(None, methods::WS_UPDATE, u).await
    }

    pub async fn delete_workstream(&self, id: &str) -> Result<(), ClientError> {
        let _: Ack = self
            .call(None, methods::WS_DELETE, WorkstreamRef { id: id.into() })
            .await?;
        Ok(())
    }

    pub async fn launch_workstream(&self, id: &str) -> Result<LaunchWorkstreamReply, ClientError> {
        self.call(None, methods::WS_LAUNCH, WorkstreamRef { id: id.into() })
            .await
    }

    pub async fn add_node(&self, a: AddNode) -> Result<Workstream, ClientError> {
        self.call(None, methods::WS_ADD_NODE, a).await
    }

    pub async fn remove_node(&self, r: NodeRef) -> Result<Workstream, ClientError> {
        self.call(None, methods::WS_REMOVE_NODE, r).await
    }

    pub async fn connect_nodes(&self, c: Connect) -> Result<Workstream, ClientError> {
        self.call(None, methods::WS_CONNECT, c).await
    }

    pub async fn set_edge_policy(&self, c: Connect) -> Result<Workstream, ClientError> {
        self.call(None, methods::WS_SET_POLICY, c).await
    }

    pub async fn disconnect_nodes(&self, r: EdgeRef) -> Result<Workstream, ClientError> {
        self.call(None, methods::WS_DISCONNECT, r).await
    }

    pub async fn pause_workstream(&self, id: &str) -> Result<Workstream, ClientError> {
        self.call(None, methods::WS_PAUSE, WorkstreamRef { id: id.into() })
            .await
    }

    pub async fn resume_workstream(&self, id: &str) -> Result<Workstream, ClientError> {
        self.call(None, methods::WS_RESUME, WorkstreamRef { id: id.into() })
            .await
    }

    pub async fn archive_workstream(&self, id: &str) -> Result<Workstream, ClientError> {
        self.call(None, methods::WS_ARCHIVE, WorkstreamRef { id: id.into() })
            .await
    }

    pub async fn save_template(&self, s: SaveTemplate) -> Result<WorkstreamTemplate, ClientError> {
        self.call(None, methods::WS_SAVE_TEMPLATE, s).await
    }

    pub async fn from_template(&self, f: FromTemplate) -> Result<Workstream, ClientError> {
        self.call(None, methods::WS_FROM_TEMPLATE, f).await
    }

    pub async fn templates(&self) -> Result<Vec<WorkstreamTemplate>, ClientError> {
        let l: TemplateList = self.call(None, methods::WS_TEMPLATES_LIST, ()).await?;
        Ok(l.templates)
    }

    pub async fn delete_template(&self, id: &str) -> Result<(), ClientError> {
        let _: Ack = self
            .call(
                None,
                methods::WS_TEMPLATES_DELETE,
                TemplateRef { id: id.into() },
            )
            .await?;
        Ok(())
    }

    pub async fn edge_history(
        &self,
        r: EdgeHistoryRequest,
    ) -> Result<EdgeHistoryPage, ClientError> {
        self.call(None, methods::WS_HISTORY, r).await
    }
}
