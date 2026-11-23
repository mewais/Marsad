//! Services every daemon serves for its own host: `git.*`, `pty.*`, `fs.*`, `ask.user`,
//! `agentconfig.base`, `daemon.*`.

pub mod ask;
pub mod fs;
pub mod git;
pub mod pty;

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use marsad_proto::agentconfig::{BaseConfig, BaseRequest};
use marsad_proto::daemon::DaemonInfo;
use marsad_proto::fs::{FsHome, FsListRequest, FsPathRequest};
use marsad_proto::git::{
    GitCommitRequest, GitDiffRequest, GitDiscardRequest, GitStatusRequest, GitTarget,
};
use marsad_proto::pty::{PtyInputRequest, PtyList, PtyRef, PtyResizeRequest, PtyStartRequest};
use marsad_proto::{methods, topics, Ack, MarsadError, SessionId};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use wasit_control::{CallContext, Cursor, EventStream, RpcError, Service};
use wasit_core::DiscoveryKind;
use wasit_runtime::{Discovery, SessionManager};

use crate::util::{ok, params};

/// How long `agentconfig.base` waits for running probes.
const BASE_WAIT: Duration = Duration::from_secs(90);

/// See the module docs.
#[derive(Clone)]
pub struct HostServices {
    pub manager: SessionManager,
    pub discovery: Discovery,
    pub ptys: pty::Ptys,
    pub info: DaemonInfo,
    /// Cancelled by `daemon.shutdown`.
    pub shutdown: CancellationToken,
}

impl HostServices {
    /// The working directory a target names, and the session it belongs to.
    fn resolve(&self, t: &GitTarget) -> Result<(PathBuf, Option<String>), MarsadError> {
        match (&t.session, &t.cwd) {
            (Some(id), None) => {
                let snap = self
                    .manager
                    .snapshot(id)
                    .map_err(|_| MarsadError::NotFound {
                        what: format!("session {id}"),
                    })?;
                let cwd = snap.cwd.ok_or_else(|| {
                    MarsadError::invalid(format!("session {id} has no working directory"))
                })?;
                Ok((PathBuf::from(cwd), Some(id.to_string())))
            }
            (None, Some(cwd)) => Ok((crate::util::expand_tilde(cwd)?, None)),
            _ => Err(MarsadError::invalid(
                "exactly one of `session` and `cwd` must be given",
            )),
        }
    }

    async fn base(&self, r: BaseRequest) -> Result<BaseConfig, MarsadError> {
        let project = r.project.as_deref();
        if r.refresh {
            self.discovery
                .refresh(Some(&r.driver), project)
                .map_err(MarsadError::invalid)?;
        }
        let mut changes = self.discovery.changes();
        let deadline = tokio::time::Instant::now() + BASE_WAIT;
        loop {
            let inv = self.discovery.inventory(project);
            let e = inv
                .drivers
                .into_iter()
                .find(|d| d.driver == r.driver)
                .ok_or_else(|| MarsadError::invalid(format!("unknown driver {}", r.driver)))?;
            let done = !e.refreshing && e.refreshed_at_ms > 0;
            if done || tokio::time::Instant::now() >= deadline {
                let mut errors: Vec<String> = e
                    .probes
                    .iter()
                    .filter(|p| p.kind == DiscoveryKind::Config && !p.ok)
                    .filter_map(|p| p.error.clone())
                    .collect();
                if let Some(err) = &e.error {
                    errors.insert(0, err.clone());
                }
                return Ok(BaseConfig {
                    driver: e.driver,
                    project: r.project.clone(),
                    items: e.config,
                    refreshing: !done,
                    errors,
                    refreshed_at_ms: e.refreshed_at_ms,
                });
            }
            // Signalled by discovery on every inventory change; bounded by the deadline.
            if tokio::time::timeout_at(deadline, changes.changed())
                .await
                .is_err()
            {
                continue;
            }
        }
    }
}

#[async_trait]
impl Service for HostServices {
    async fn call(&self, _ctx: CallContext, method: &str, p: Value) -> Result<Value, RpcError> {
        let r: Result<Value, MarsadError> = async {
            Ok(match method {
                methods::GIT_STATUS => {
                    let r: GitStatusRequest = params(p).map_err(rpc_to)?;
                    let (dir, _) = self.resolve(&r.target)?;
                    ok(git::status(&dir).await?).map_err(rpc_to)?
                }
                methods::GIT_DIFF => {
                    let r: GitDiffRequest = params(p).map_err(rpc_to)?;
                    let (dir, _) = self.resolve(&r.target)?;
                    let max = r
                        .max_bytes
                        .map(|m| (m as usize).min(git::MAX_DIFF))
                        .unwrap_or(git::MAX_DIFF);
                    ok(git::diff(&dir, &r.path, r.side, max).await?).map_err(rpc_to)?
                }
                methods::GIT_COMMIT => {
                    let r: GitCommitRequest = params(p).map_err(rpc_to)?;
                    let (dir, _) = self.resolve(&r.target)?;
                    ok(git::commit(&dir, &r.message, &r.files).await?).map_err(rpc_to)?
                }
                methods::GIT_DISCARD => {
                    let r: GitDiscardRequest = params(p).map_err(rpc_to)?;
                    let (dir, _) = self.resolve(&r.target)?;
                    git::discard(&dir, &r.files).await?;
                    ok(Ack {}).map_err(rpc_to)?
                }
                methods::PTY_START => {
                    let r: PtyStartRequest = params(p).map_err(rpc_to)?;
                    let (dir, session) = self.resolve(&r.target)?;
                    let started = self
                        .ptys
                        .start(&dir, session.clone(), r.cols, r.rows, r.shell)
                        .await?;
                    // The session may have been deleted between resolving it and the terminal
                    // entering the table, after which no `Removed` event is coming for it: this
                    // check, made after the insertion, is what ends that terminal.
                    if let Some(id) = session {
                        if SessionId::parse(&id)
                            .is_none_or(|sid| self.manager.snapshot(&sid).is_err())
                        {
                            self.ptys.close_session(&id);
                            return Err(MarsadError::NotFound {
                                what: format!("session {id}"),
                            });
                        }
                    }
                    ok(started).map_err(rpc_to)?
                }
                methods::PTY_INPUT => {
                    let r: PtyInputRequest = params(p).map_err(rpc_to)?;
                    self.ptys.input(&r.id, r.data).await?;
                    ok(Ack {}).map_err(rpc_to)?
                }
                methods::PTY_RESIZE => {
                    let r: PtyResizeRequest = params(p).map_err(rpc_to)?;
                    self.ptys.resize(&r.id, r.cols, r.rows).await?;
                    ok(Ack {}).map_err(rpc_to)?
                }
                methods::PTY_CLOSE => {
                    let r: PtyRef = params(p).map_err(rpc_to)?;
                    self.ptys.close(&r.id).await?;
                    ok(Ack {}).map_err(rpc_to)?
                }
                methods::PTY_LIST => ok(PtyList {
                    ptys: self.ptys.list(),
                })
                .map_err(rpc_to)?,
                methods::FS_LIST => {
                    let r: FsListRequest = params(p).map_err(rpc_to)?;
                    ok(fs::list(&r.path, r.show_hidden, r.dirs_only).await?).map_err(rpc_to)?
                }
                methods::FS_HOME => ok(FsHome { path: fs::home()? }).map_err(rpc_to)?,
                methods::FS_EXISTS => {
                    let r: FsPathRequest = params(p).map_err(rpc_to)?;
                    ok(fs::exists(&r.path).await?).map_err(rpc_to)?
                }
                methods::ASK_USER => {
                    let r: marsad_proto::ask::AskUserRequest = params(p).map_err(rpc_to)?;
                    ok(ask::ask_user(&self.manager, r).await?).map_err(rpc_to)?
                }
                methods::AGENTCONFIG_BASE => {
                    let r: BaseRequest = params(p).map_err(rpc_to)?;
                    ok(self.base(r).await?).map_err(rpc_to)?
                }
                methods::DAEMON_INFO => ok(&self.info).map_err(rpc_to)?,
                methods::DAEMON_SHUTDOWN => {
                    self.shutdown.cancel();
                    ok(Ack {}).map_err(rpc_to)?
                }
                other => {
                    return Err(MarsadError::NotFound {
                        what: format!("method {other}"),
                    })
                }
            })
        }
        .await;
        r.map_err(|e| match e {
            MarsadError::NotFound { what } if what.starts_with("method ") => {
                RpcError::method_not_found(method)
            }
            e => e.into_rpc(),
        })
    }

    async fn subscribe(
        &self,
        _ctx: CallContext,
        topic: &str,
        _params: Value,
        from: Option<Cursor>,
    ) -> Result<EventStream, RpcError> {
        if let Some(id) = topic.strip_prefix(topics::PTY_OUTPUT_PREFIX) {
            let from =
                match &from {
                    None => None,
                    Some(c) => Some(c.as_u64().ok_or_else(|| {
                        MarsadError::invalid(format!("bad cursor {c}")).into_rpc()
                    })?),
                };
            return self.ptys.subscribe(id, from).map_err(MarsadError::into_rpc);
        }
        Err(RpcError::topic_not_found(topic))
    }
}

/// An `RpcError` from parameter decoding, carried through a `MarsadError` path.
fn rpc_to(e: RpcError) -> MarsadError {
    MarsadError::from_rpc(&e).unwrap_or_else(|| MarsadError::invalid(e.message))
}
