//! marsad-proto — every type that crosses a Marsad daemon's control surface, defined once.
//!
//! The daemon (`marsad-agent`) serves these over `wasit-control` next to the Wasit runtime's
//! own methods; every client (`marsad-client`, the Tauri shell, later phones) uses the same
//! types. With the `ts` feature every type a GUI needs (Marsad's and the re-exported Wasit
//! ones) derives `ts_rs::TS`; `marsad-ts-export` writes them out.
//!
//! # Methods (`Request.method` → params → result)
//!
//! Host services (served by every daemon; a request with `host = <id>` sent to the hub is
//! routed to that host's daemon):
//!
//! | method | params | result |
//! |---|---|---|
//! | Wasit runtime: `host.info`, `drivers.*`, `sessions.*`, `session.*` | see [`runtime`] | |
//! | [`methods::GIT_STATUS`] | [`git::GitStatusRequest`] | [`git::GitStatus`] |
//! | [`methods::GIT_DIFF`] | [`git::GitDiffRequest`] | [`git::GitDiff`] |
//! | [`methods::GIT_COMMIT`] | [`git::GitCommitRequest`] | [`git::GitCommitReply`] |
//! | [`methods::GIT_DISCARD`] | [`git::GitDiscardRequest`] | [`Ack`] |
//! | [`methods::PTY_START`] | [`pty::PtyStartRequest`] | [`pty::PtyInfo`] |
//! | [`methods::PTY_INPUT`] | [`pty::PtyInputRequest`] | [`Ack`] |
//! | [`methods::PTY_RESIZE`] | [`pty::PtyResizeRequest`] | [`Ack`] |
//! | [`methods::PTY_CLOSE`] | [`pty::PtyRef`] | [`Ack`] |
//! | [`methods::PTY_LIST`] | — | [`pty::PtyList`] |
//! | [`methods::FS_LIST`] | [`fs::FsListRequest`] | [`fs::FsListing`] |
//! | [`methods::FS_HOME`] | — | [`fs::FsHome`] |
//! | [`methods::FS_EXISTS`] | [`fs::FsPathRequest`] | [`fs::FsExists`] |
//! | [`methods::AGENTCONFIG_BASE`] | [`agentconfig::BaseRequest`] | [`agentconfig::BaseConfig`] |
//! | [`methods::DAEMON_INFO`] | — | [`daemon::DaemonInfo`] |
//! | [`methods::DAEMON_SHUTDOWN`] | — | [`Ack`] |
//!
//! Hub services (only on the GUI machine's daemon, started with `serve --hub`; never routed):
//!
//! | method | params | result |
//! |---|---|---|
//! | [`methods::HOSTS_LIST`] | — | [`hosts::HostList`] |
//! | [`methods::HOSTS_ADD`] | [`hosts::HostDraft`] | [`hosts::HostEntry`] |
//! | [`methods::HOSTS_UPDATE`] | [`hosts::HostEntry`] | [`hosts::HostEntry`] |
//! | [`methods::HOSTS_REMOVE`] | [`hosts::HostRef`] | [`Ack`] |
//! | [`methods::HOSTS_IMPORT_SUGGESTIONS`] | — | [`hosts::ImportSuggestions`] |
//! | [`methods::HOSTS_WSL_DISTROS`] | — | [`hosts::WslDistros`] |
//! | [`methods::HOSTS_RECONNECT`] | [`hosts::HostRef`] | [`Ack`] |
//! | [`methods::HOSTS_AUTH_ANSWER`] | [`hosts::AuthAnswer`] | [`Ack`] |
//! | [`methods::HOSTS_AUTH_CANCEL`] | [`hosts::AuthCancel`] | [`Ack`] |
//! | [`methods::HOSTS_ASKPASS`] (internal) | [`hosts::AskpassRequest`] | [`hosts::AskpassReply`] |
//! | [`methods::HOSTS_ENVIRONMENT_GET`] | [`hosts::HostRef`] | [`hosts::HostEnvSettings`] (secret values left out) |
//! | [`methods::HOSTS_ENVIRONMENT_SET`] | [`hosts::HostEnvSettings`] | [`hosts::HostEnvSaved`] (stored; applied when the host is connected) |
//! | [`methods::HOSTS_ENVIRONMENT_TEST`] | [`hosts::HostRef`] | `wasit_runtime::wire::EnvironmentTest` (setup re-run, agents re-detected) |
//! | [`methods::HOSTS_ENVIRONMENT_SECRETS`] | — | [`hosts::SecretsLocation`] (where secret values are kept) |
//! | [`methods::FLEET_GIT_REFRESH`] | [`fleet::GitRefresh`] | [`Ack`] |
//! | [`methods::AGENTCONFIG_EFFECTIVE`] | [`agentconfig::EffectiveRequest`] | [`agentconfig::EffectiveConfig`] |
//! | [`methods::SESSIONS_LAUNCH`] | [`agentconfig::LaunchRequest`] | [`agentconfig::LaunchReply`] |
//! | [`methods::STORE_SETTINGS_GET`] | — | [`store::Settings`] |
//! | [`methods::STORE_SETTINGS_SET`] | [`store::SettingSet`] | [`store::Settings`] |
//! | [`methods::STORE_NOTIFICATIONS_GET`] | — | [`store::NotificationPrefs`] |
//! | [`methods::STORE_NOTIFICATIONS_SET`] | [`store::NotificationPrefs`] | [`store::NotificationPrefs`] |
//! | [`methods::STORE_LAYERS_LIST`] | — | [`store::LayerList`] |
//! | [`methods::STORE_LAYERS_GET`] | [`agentconfig::LayerScope`] | [`agentconfig::OverrideLayer`] |
//! | [`methods::STORE_LAYERS_SET`] | [`store::LayerSet`] | [`Ack`] |
//! | [`methods::STORE_SEEN_LIST`] | — | [`store::SeenMarkers`] |
//! | [`methods::STORE_SEEN_MARK`] | [`store::SeenMark`] | [`Ack`] |
//! | [`methods::STORE_SEEN_MARK_ALL`] | — | [`store::SeenMarkers`] |
//! | [`methods::STORE_RECENT_LIST`] | — | [`store::RecentProjects`] |
//! | [`methods::STORE_RECENT_ADD`] | [`store::RecentProject`] | [`store::RecentProjects`] |
//!
//! Workstreams (hub; see [`workstream`] for the model):
//!
//! | method | params | result |
//! |---|---|---|
//! | [`methods::WS_LIST`] | — | [`workstream::WorkstreamList`] |
//! | [`methods::WS_GET`] | [`workstream::WorkstreamRef`] | [`workstream::Workstream`] |
//! | [`methods::WS_CREATE`] | [`workstream::WorkstreamDraft`] | [`workstream::Workstream`] |
//! | [`methods::WS_UPDATE`] | [`workstream::WorkstreamUpdate`] | [`workstream::Workstream`] |
//! | [`methods::WS_DELETE`] | [`workstream::WorkstreamRef`] | [`Ack`] |
//! | [`methods::WS_LAUNCH`] | [`workstream::WorkstreamRef`] | [`workstream::LaunchWorkstreamReply`] |
//! | [`methods::WS_ADD_NODE`] | [`workstream::AddNode`] | [`workstream::Workstream`] |
//! | [`methods::WS_REMOVE_NODE`] | [`workstream::NodeRef`] | [`workstream::Workstream`] |
//! | [`methods::WS_CONNECT`] | [`workstream::Connect`] | [`workstream::Workstream`] |
//! | [`methods::WS_DISCONNECT`] | [`workstream::EdgeRef`] | [`workstream::Workstream`] |
//! | [`methods::WS_SET_POLICY`] | [`workstream::Connect`] | [`workstream::Workstream`] |
//! | [`methods::WS_PAUSE`] / [`methods::WS_RESUME`] / [`methods::WS_ARCHIVE`] | [`workstream::WorkstreamRef`] | [`workstream::Workstream`] |
//! | [`methods::WS_SAVE_TEMPLATE`] | [`workstream::SaveTemplate`] | [`workstream::WorkstreamTemplate`] |
//! | [`methods::WS_FROM_TEMPLATE`] | [`workstream::FromTemplate`] | [`workstream::Workstream`] |
//! | [`methods::WS_TEMPLATES_LIST`] | — | [`workstream::TemplateList`] |
//! | [`methods::WS_TEMPLATES_DELETE`] | [`workstream::TemplateRef`] | [`Ack`] |
//! | [`methods::WS_HISTORY`] | [`workstream::EdgeHistoryRequest`] | [`workstream::EdgeHistoryPage`] |
//!
//! Wasl (`wasl.*`, see [`wasl`]) is served by every daemon's embedded node and routed by
//! `host` like the host services; clients normally need only `wasl.tasks.list` /
//! `wasl.task.cancel`. Handoff gates are answered with the runtime's `session.command`
//! (`answer_gate`) on the sending session, like any gate.
//!
//! # Topics (`Subscribe.topic` → payload, cursor)
//!
//! | topic | payload | cursor |
//! |---|---|---|
//! | Wasit runtime: `host.sessions`, `session.events:<id>`, `drivers.inventory` | see [`runtime`] | |
//! | `pty.output:<pty id>` ([`topics::pty_output`]) | [`pty::PtyEvent`] | byte offset after the event (decimal) |
//! | [`topics::FLEET`] (hub) | [`fleet::FleetEvent`] | `"<epoch>.<n>"`; unknown/expired ⇒ starts with `Reset` |
//! | [`topics::HOSTS_LINKS`] (hub) | [`hosts::LinkStates`] | `"<epoch>.<n>"`; every event is the full map |
//! | [`topics::WORKSTREAMS`] (hub) | [`workstream::WorkstreamsEvent`] | `"<epoch>.<n>"`; unknown/expired ⇒ starts with `Reset` |
//! | Wasl: `wasl.tasks`, `wasl.edges` (any host) | see [`wasl`] | journal sequence (decimal) |
//!
//! # Errors
//!
//! Marsad services answer failures with an `RpcError` whose `code` is [`MarsadError::code`]
//! and whose `data` is the serialized [`MarsadError`].

#![forbid(unsafe_code)]

pub mod agentconfig;
pub mod ask;
pub mod daemon;
pub mod diag;
pub mod fleet;
pub mod fs;
pub mod git;
pub mod hosts;
pub mod layout;
pub mod paths;
pub mod pty;
pub mod store;
pub mod workstream;

/// Wasl's wire types (tasks, topology, edge policies).
pub use wasl_proto as wasl;

use serde::{Deserialize, Serialize};

/// The Wasit runtime's wire types (sessions, snapshots, events, inventory).
pub use wasit_runtime::media;
pub use wasit_runtime::wire as runtime;

pub use wasit_control::{
    CloseReason, CommandFailureKind, Cursor, Hello, HostId, LinkState, ProtocolVersion,
    ProvisionReason, RpcError,
};
pub use wasit_core::{
    AgentConfig, AgentEvent, ChoiceText, Command, ConfigItem, ConfigItemKind, ConfigItemRef,
    ConfigSource, DriverId, GateChoice, GateChoiceKind, GateDecision, GateRequest, McpServerConfig,
    McpTransport, QuestionAnswer, Unsupported,
};
pub use wasit_core::{Attachment, ContentPart, FileInput, MediaSource, PromptInput};
pub use wasit_runtime::wire::{Ack, SessionId, SessionSnapshot};

/// Method names.
pub mod methods {
    pub const GIT_STATUS: &str = "git.status";
    pub const GIT_DIFF: &str = "git.diff";
    pub const GIT_COMMIT: &str = "git.commit";
    pub const GIT_DISCARD: &str = "git.discard";

    pub const PTY_START: &str = "pty.start";
    pub const PTY_INPUT: &str = "pty.input";
    pub const PTY_RESIZE: &str = "pty.resize";
    pub const PTY_CLOSE: &str = "pty.close";
    pub const PTY_LIST: &str = "pty.list";

    pub const FS_LIST: &str = "fs.list";
    pub const FS_HOME: &str = "fs.home";
    pub const FS_EXISTS: &str = "fs.exists";

    /// An agent asks the user a question (host; see [`crate::ask`]).
    pub const ASK_USER: &str = "ask.user";

    pub const AGENTCONFIG_BASE: &str = "agentconfig.base";
    pub const AGENTCONFIG_EFFECTIVE: &str = "agentconfig.effective";
    /// Marsad's session creation: computes the effective config, then calls the runtime's
    /// `sessions.create` on the target host. (An exact route next to the runtime's
    /// `sessions.` namespace.)
    pub const SESSIONS_LAUNCH: &str = "sessions.launch";

    /// Refresh one directory's git summary in the `fleet` topic (hub).
    pub const FLEET_GIT_REFRESH: &str = "fleet.git_refresh";

    pub const DAEMON_INFO: &str = "daemon.info";
    pub const DAEMON_SHUTDOWN: &str = "daemon.shutdown";

    pub const HOSTS_LIST: &str = "hosts.list";
    pub const HOSTS_ADD: &str = "hosts.add";
    pub const HOSTS_UPDATE: &str = "hosts.update";
    pub const HOSTS_REMOVE: &str = "hosts.remove";
    pub const HOSTS_IMPORT_SUGGESTIONS: &str = "hosts.import_suggestions";
    pub const HOSTS_WSL_DISTROS: &str = "hosts.wsl_distros";
    /// Connect now (also the explicit "Connect" for a host that signs in interactively).
    pub const HOSTS_RECONNECT: &str = "hosts.reconnect";
    pub const HOSTS_AUTH_ANSWER: &str = "hosts.auth_answer";
    pub const HOSTS_AUTH_CANCEL: &str = "hosts.auth_cancel";
    /// Internal: the `SSH_ASKPASS` helper waits here for the user's answer.
    pub const HOSTS_ASKPASS: &str = "hosts.askpass";
    pub const HOSTS_ENVIRONMENT_GET: &str = "hosts.environment.get";
    pub const HOSTS_ENVIRONMENT_SET: &str = "hosts.environment.set";
    pub const HOSTS_ENVIRONMENT_TEST: &str = "hosts.environment.test";
    pub const HOSTS_ENVIRONMENT_SECRETS: &str = "hosts.environment.secrets";

    pub const STORE_SETTINGS_GET: &str = "store.settings.get";
    pub const STORE_SETTINGS_SET: &str = "store.settings.set";
    pub const STORE_NOTIFICATIONS_GET: &str = "store.notifications.get";
    pub const STORE_NOTIFICATIONS_SET: &str = "store.notifications.set";
    pub const STORE_LAYERS_LIST: &str = "store.layers.list";
    pub const STORE_LAYERS_GET: &str = "store.layers.get";
    pub const STORE_LAYERS_SET: &str = "store.layers.set";
    pub const STORE_SEEN_LIST: &str = "store.seen.list";
    pub const STORE_SEEN_MARK: &str = "store.seen.mark";
    pub const STORE_SEEN_MARK_ALL: &str = "store.seen.mark_all";
    pub const STORE_RECENT_LIST: &str = "store.recent.list";
    pub const STORE_RECENT_ADD: &str = "store.recent.add";

    pub const WS_LIST: &str = "workstreams.list";
    pub const WS_GET: &str = "workstreams.get";
    pub const WS_CREATE: &str = "workstreams.create";
    pub const WS_UPDATE: &str = "workstreams.update";
    pub const WS_DELETE: &str = "workstreams.delete";
    pub const WS_LAUNCH: &str = "workstreams.launch";
    pub const WS_ADD_NODE: &str = "workstreams.add_node";
    pub const WS_REMOVE_NODE: &str = "workstreams.remove_node";
    pub const WS_CONNECT: &str = "workstreams.connect";
    pub const WS_DISCONNECT: &str = "workstreams.disconnect";
    pub const WS_SET_POLICY: &str = "workstreams.set_policy";
    pub const WS_PAUSE: &str = "workstreams.pause";
    pub const WS_RESUME: &str = "workstreams.resume";
    pub const WS_ARCHIVE: &str = "workstreams.archive";
    pub const WS_SAVE_TEMPLATE: &str = "workstreams.save_template";
    pub const WS_FROM_TEMPLATE: &str = "workstreams.from_template";
    pub const WS_TEMPLATES_LIST: &str = "workstreams.templates.list";
    pub const WS_TEMPLATES_DELETE: &str = "workstreams.templates.delete";
    pub const WS_HISTORY: &str = "workstreams.history";
    /// The spawn authority (internal: the hub's Wasl node calls these; see
    /// `wasl_proto::spawn`).
    pub const WS_SPAWN_PLAN: &str = "workstreams.spawn.plan";
    pub const WS_SPAWN_LAUNCH: &str = "workstreams.spawn.launch";
    pub const WS_SPAWN_DISMISS: &str = "workstreams.spawn.dismiss";
    pub const WS_SPAWNABLE: &str = "workstreams.spawn.spawnable";
}

/// Topic names.
pub mod topics {
    pub const PTY_OUTPUT_PREFIX: &str = "pty.output:";
    pub const FLEET: &str = "fleet";
    pub const HOSTS_LINKS: &str = "hosts.links";
    pub const WORKSTREAMS: &str = "workstreams";

    /// The output topic of one terminal.
    pub fn pty_output(id: &str) -> String {
        format!("{PTY_OUTPUT_PREFIX}{id}")
    }
}

/// Routes of the services every daemon serves (and the hub routes by `host`).
pub const HOST_ROUTES: &[&str] = &["git.", "pty.", "fs.", "ask.", "daemon.", "agentconfig.base"];

/// Routes of the hub-only services.
pub const HUB_ROUTES: &[&str] = &[
    "hosts.",
    "store.",
    "fleet",
    methods::FLEET_GIT_REFRESH,
    "agentconfig.effective",
    "sessions.launch",
    "workstreams",
    "workstreams.",
];

/// Error codes of [`MarsadError`].
pub mod codes {
    pub const INVALID_REQUEST: &str = "invalid_params";
    pub const NOT_FOUND: &str = "not_found";
    pub const GIT_FAILED: &str = "git_failed";
    pub const NOT_A_REPO: &str = "not_a_repo";
    pub const TOOL_MISSING: &str = "tool_missing";
    pub const IO: &str = "io";
    pub const TIMEOUT: &str = "timeout";
    pub const UNSUPPORTED: &str = "unsupported";
    pub const NOT_HUB: &str = "not_hub";
    pub const UPSTREAM: &str = "upstream";
    pub const STORE: &str = "store";
    pub const CONFLICT: &str = "conflict";
}

/// A failed Marsad operation (carried as `RpcError.data`).
///
/// Tagged `marsad_error` so it can never be mistaken for the runtime's own error data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "marsad_error", rename_all = "snake_case")]
pub enum MarsadError {
    #[error("invalid request: {message}")]
    InvalidRequest { message: String },
    #[error("not found: {what}")]
    NotFound { what: String },
    #[error("not a git repository: {path}")]
    NotARepo { path: String },
    #[error("git failed: {message}")]
    GitFailed { message: String, stderr: String },
    #[error("`{tool}` is not installed on this host")]
    ToolMissing { tool: String },
    #[error("i/o error: {message}")]
    Io { message: String },
    #[error("timed out: {what}")]
    Timeout { what: String },
    #[error("not supported: {message}")]
    Unsupported { message: String },
    #[error("this daemon is not the hub (start it with `serve --hub`)")]
    NotHub,
    /// A call this operation made to another service failed.
    #[error("{message}")]
    Upstream {
        message: String,
        cause: wasit_control::RpcError,
    },
    #[error("store error: {message}")]
    Store { message: String },
    /// The object changed since the caller read it (`expected_revision` did not match).
    #[error("conflict: {message}")]
    Conflict { message: String },
    /// A spawn request the hub refuses (policy, limits, not the caller's worker); the
    /// message is for the asking agent.
    #[error("{message}")]
    Spawn { code: SpawnError, message: String },
}

/// Why the hub refused a spawn request (as the Wasl error codes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum SpawnError {
    /// The policy ("never"), the host, the driver or the request itself.
    Refused,
    /// A limit: count, depth, budget.
    Limit,
    /// The agent named was not spawned by the caller.
    NotSpawnedByCaller,
}

impl MarsadError {
    pub fn code(&self) -> &'static str {
        match self {
            MarsadError::InvalidRequest { .. } => codes::INVALID_REQUEST,
            MarsadError::NotFound { .. } => codes::NOT_FOUND,
            MarsadError::NotARepo { .. } => codes::NOT_A_REPO,
            MarsadError::GitFailed { .. } => codes::GIT_FAILED,
            MarsadError::ToolMissing { .. } => codes::TOOL_MISSING,
            MarsadError::Io { .. } => codes::IO,
            MarsadError::Timeout { .. } => codes::TIMEOUT,
            MarsadError::Unsupported { .. } => codes::UNSUPPORTED,
            MarsadError::NotHub => codes::NOT_HUB,
            MarsadError::Upstream { .. } => codes::UPSTREAM,
            MarsadError::Store { .. } => codes::STORE,
            MarsadError::Conflict { .. } => codes::CONFLICT,
            MarsadError::Spawn { code, .. } => match code {
                SpawnError::Refused => wasl_proto::codes::SPAWN_REFUSED,
                SpawnError::Limit => wasl_proto::codes::SPAWN_LIMIT,
                SpawnError::NotSpawnedByCaller => wasl_proto::codes::NOT_SPAWNED_BY_CALLER,
            },
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        MarsadError::InvalidRequest {
            message: message.into(),
        }
    }

    pub fn io(message: impl std::fmt::Display) -> Self {
        MarsadError::Io {
            message: message.to_string(),
        }
    }

    /// As the `RpcError` a service returns.
    pub fn into_rpc(self) -> RpcError {
        let data = serde_json::to_value(&self).ok();
        let mut e = RpcError::new(self.code(), self.to_string());
        e.data = data;
        e
    }

    /// The typed error carried by an `RpcError`, if it is one.
    pub fn from_rpc(e: &RpcError) -> Option<Self> {
        serde_json::from_value(e.data.clone()?).ok()
    }
}

impl From<MarsadError> for RpcError {
    fn from(e: MarsadError) -> Self {
        e.into_rpc()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_round_trip_through_rpc() {
        let e = MarsadError::GitFailed {
            message: "commit".into(),
            stderr: "nothing to commit".into(),
        };
        let rpc = e.clone().into_rpc();
        assert_eq!(rpc.code, codes::GIT_FAILED);
        assert_eq!(MarsadError::from_rpc(&rpc), Some(e));
        assert_eq!(MarsadError::from_rpc(&RpcError::internal("x")), None);
    }

    #[test]
    fn routes_do_not_overlap() {
        for h in HOST_ROUTES {
            for u in HUB_ROUTES {
                assert!(!h.starts_with(u) && !u.starts_with(h), "{h} vs {u}");
            }
        }
        for r in wasit_runtime::wire::ROUTES.iter().chain(wasl_proto::ROUTES) {
            for m in HOST_ROUTES.iter().chain(HUB_ROUTES) {
                assert!(
                    !r.starts_with(m) && !(m.starts_with(r) && *m != methods::SESSIONS_LAUNCH),
                    "{r} vs {m}"
                );
            }
        }
    }
}
