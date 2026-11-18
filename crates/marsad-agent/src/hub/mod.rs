//! Hub services (the GUI machine's daemon, `serve --hub`): hosts and their links,
//! provisioning, the fleet topic, the store, effective config and session launch.

pub mod fleet;
pub mod hosts;
pub mod provision;
pub mod transport;

use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::StreamExt;
use marsad_proto::agentconfig::{
    effective_config, AutomaticItem, BaseConfig, BaseRequest, EffectiveConfig, EffectiveRequest,
    LaunchReply, LaunchRequest, LayerScope, OverrideLayer, WorkstreamNodeRef,
};
use marsad_proto::hosts::{HostDraft, HostEntry, HostList, HostRef, LinkStates};
use marsad_proto::store::{
    LayerList, LayerSet, NotificationPrefs, RecentProject, RecentProjects, SeenMark, SeenMarkers,
    SettingSet, WorkstreamList, WorkstreamPut, WorkstreamRef,
};
use marsad_proto::{methods, topics, Ack, DriverId, HostId, MarsadError, SessionId};
use marsad_store::Store;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use wasit_control::{CallContext, CloseReason, Cursor, EventStream, RpcError, Service};
use wasit_core::DriverRegistry;
use wasit_runtime::wire::{methods as rt_methods, CreateSession, CreateSessionReply};

use crate::util::{now_ms, ok, params};
use fleet::Fleet;
use hosts::Registry;

/// What a launch is for, as automatic-item providers see it.
#[derive(Debug, Clone)]
pub struct LaunchContext {
    pub host: HostId,
    pub driver: DriverId,
    pub project: Option<String>,
    pub session: SessionId,
    pub workstream: Option<WorkstreamNodeRef>,
}

/// Supplies items Marsad injects into every launch (e.g. the orchestration MCP server). They
/// are applied after all override layers, locked, with provenance `automatic`.
pub trait AutomaticItems: Send + Sync {
    fn items(&self, ctx: &LaunchContext) -> Vec<AutomaticItem>;
}

/// See the module docs.
pub struct HubServices {
    pub registry: Registry,
    pub store: Store,
    pub fleet: Fleet,
    /// Serves host services on any host (local or routed); used for base layers and creates.
    pub routing: Arc<dyn Service>,
    pub drivers: DriverRegistry,
    pub automatic: Vec<Arc<dyn AutomaticItems>>,
}

impl HubServices {
    fn host_of(&self, h: Option<HostId>) -> Result<HostId, MarsadError> {
        let id = h.unwrap_or_else(|| self.registry.local().id.clone());
        self.registry
            .get(&id)
            .map(|e| e.id)
            .ok_or_else(|| MarsadError::NotFound {
                what: format!("host {id}"),
            })
    }

    /// Call a host service on `host`.
    async fn on_host<P: Serialize, R: DeserializeOwned>(
        &self,
        ctx: &CallContext,
        host: &HostId,
        method: &str,
        p: P,
    ) -> Result<R, MarsadError> {
        let mut c = ctx.clone();
        c.host = Some(host.clone());
        let v = self
            .routing
            .call(c, method, serde_json::to_value(p).map_err(MarsadError::io)?)
            .await
            .map_err(|e| MarsadError::Upstream {
                message: format!("{method} on {host}: {e}"),
                cause: e,
            })?;
        serde_json::from_value(v).map_err(|e| MarsadError::io(format!("{method} reply: {e}")))
    }

    #[allow(clippy::too_many_arguments)]
    async fn effective(
        &self,
        ctx: &CallContext,
        host: &HostId,
        driver: &DriverId,
        project: Option<String>,
        workstream: Option<WorkstreamNodeRef>,
        session: &SessionId,
        stored_session: bool,
        overrides: Option<OverrideLayer>,
    ) -> Result<EffectiveConfig, MarsadError> {
        let desc = self
            .drivers
            .get(driver)
            .map_err(|_| MarsadError::invalid(format!("unknown driver {driver}")))?;
        let base: BaseConfig = self
            .on_host(
                ctx,
                host,
                methods::AGENTCONFIG_BASE,
                BaseRequest {
                    driver: driver.clone(),
                    project: project.clone(),
                    refresh: false,
                },
            )
            .await?;
        let mut scopes = vec![
            LayerScope::Global,
            LayerScope::Host { host: host.clone() },
        ];
        if let Some(w) = &workstream {
            scopes.push(LayerScope::WorkstreamNode {
                workstream: w.workstream.clone(),
                node: w.node.clone(),
            });
        }
        let session_scope = LayerScope::Session {
            host: host.clone(),
            session: session.clone(),
        };
        if stored_session {
            scopes.push(session_scope.clone());
        }
        let mut layers = Vec::new();
        for s in scopes {
            let l = self.store.layer(s.clone()).await?;
            if !l.is_empty() {
                layers.push((s, l));
            }
        }
        if let Some(o) = overrides.filter(|o| !o.is_empty()) {
            layers.push((session_scope, o));
        }
        let lctx = LaunchContext {
            host: host.clone(),
            driver: driver.clone(),
            project,
            session: session.clone(),
            workstream,
        };
        let automatic: Vec<AutomaticItem> =
            self.automatic.iter().flat_map(|p| p.items(&lctx)).collect();
        Ok(effective_config(
            driver,
            &base.items,
            &layers,
            &automatic,
            desc.as_ref(),
        ))
    }

    async fn launch(&self, ctx: &CallContext, r: LaunchRequest) -> Result<LaunchReply, MarsadError> {
        let host = self.host_of(r.host.clone())?;
        // The id is chosen here so the session layer can be stored under it.
        let id = SessionId::random();
        let eff = self
            .effective(
                ctx,
                &host,
                &r.driver,
                r.cwd.clone(),
                r.workstream.clone(),
                &id,
                false,
                Some(r.overrides.clone()),
            )
            .await?;
        let mut create = CreateSession::new(r.driver.clone());
        create.id = Some(id.clone());
        create.cwd = r.cwd.clone();
        create.model = r.model;
        create.mode = r.mode;
        create.first_prompt = r.first_prompt;
        create.resume = r.resume;
        create.labels = r.labels;
        create.config = eff.agent_config();
        let reply: CreateSessionReply = self
            .on_host(ctx, &host, rt_methods::SESSIONS_CREATE, create)
            .await?;
        if !r.overrides.is_empty() {
            self.store
                .set_layer(
                    LayerScope::Session {
                        host: host.clone(),
                        session: reply.id.clone(),
                    },
                    r.overrides,
                )
                .await?;
        }
        if let Some(cwd) = r.cwd {
            self.store
                .add_recent(RecentProject {
                    host: host.clone(),
                    path: cwd,
                    last_used_ms: 0,
                })
                .await?;
        }
        Ok(LaunchReply {
            host,
            id: reply.id,
            effective: eff,
            unsupported: reply.unsupported,
        })
    }

    async fn mark_all_seen(&self) -> Result<SeenMarkers, MarsadError> {
        let now = now_ms();
        let marks: Vec<SeenMark> = self
            .fleet
            .snapshot()
            .await
            .into_iter()
            .flat_map(|h| {
                let host = h.host.id.clone();
                h.sessions.into_iter().map(move |s| SeenMark {
                    host: host.clone(),
                    session: s.id,
                    cursor: s.cursor,
                    at_ms: now,
                })
            })
            .collect();
        Ok(SeenMarkers {
            markers: self.store.mark_seen(marks).await?,
        })
    }

    async fn dispatch(&self, ctx: &CallContext, method: &str, p: Value) -> Result<Value, MarsadError> {
        let enc = |v: Result<Value, RpcError>| v.map_err(|e| MarsadError::io(e.message));
        let dec = |e: RpcError| MarsadError::from_rpc(&e).unwrap_or_else(|| MarsadError::invalid(e.message));
        match method {
            methods::HOSTS_LIST => enc(ok(HostList {
                hosts: self.registry.list(),
            })),
            methods::HOSTS_ADD => {
                let d: HostDraft = params(p).map_err(dec)?;
                enc(ok(self.registry.add(d).await?))
            }
            methods::HOSTS_UPDATE => {
                let e: HostEntry = params(p).map_err(dec)?;
                enc(ok(self.registry.update(e).await?))
            }
            methods::HOSTS_REMOVE => {
                let r: HostRef = params(p).map_err(dec)?;
                self.registry.remove(&r.id).await?;
                enc(ok(Ack {}))
            }
            methods::HOSTS_RECONNECT => {
                let r: HostRef = params(p).map_err(dec)?;
                self.registry.reconnect(&r.id)?;
                enc(ok(Ack {}))
            }
            methods::HOSTS_IMPORT_SUGGESTIONS => enc(ok(self.registry.import_suggestions().await)),
            methods::AGENTCONFIG_EFFECTIVE => {
                let r: EffectiveRequest = params(p).map_err(dec)?;
                let host = self.host_of(r.host)?;
                let (session, stored) = match r.session {
                    Some(s) => (s, true),
                    None => (SessionId::random(), false),
                };
                enc(ok(self
                    .effective(ctx, &host, &r.driver, r.project, r.workstream, &session, stored, r.overrides)
                    .await?))
            }
            methods::SESSIONS_LAUNCH => {
                let r: LaunchRequest = params(p).map_err(dec)?;
                enc(ok(self.launch(ctx, r).await?))
            }
            methods::STORE_SETTINGS_GET => enc(ok(self.store.settings().await?)),
            methods::STORE_SETTINGS_SET => {
                let r: SettingSet = params(p).map_err(dec)?;
                enc(ok(self.store.set_setting(r.key, r.value).await?))
            }
            methods::STORE_NOTIFICATIONS_GET => enc(ok(self.store.notifications().await?)),
            methods::STORE_NOTIFICATIONS_SET => {
                let r: NotificationPrefs = params(p).map_err(dec)?;
                self.store.set_notifications(r).await?;
                enc(ok(r))
            }
            methods::STORE_LAYERS_LIST => enc(ok(LayerList {
                layers: self.store.layers().await?,
            })),
            methods::STORE_LAYERS_GET => {
                let s: LayerScope = params(p).map_err(dec)?;
                enc(ok(self.store.layer(s).await?))
            }
            methods::STORE_LAYERS_SET => {
                let r: LayerSet = params(p).map_err(dec)?;
                self.store.set_layer(r.scope, r.layer).await?;
                enc(ok(Ack {}))
            }
            methods::STORE_SEEN_LIST => enc(ok(SeenMarkers {
                markers: self.store.seen().await?,
            })),
            methods::STORE_SEEN_MARK => {
                let m: SeenMark = params(p).map_err(dec)?;
                self.store.mark_seen(vec![m]).await?;
                enc(ok(Ack {}))
            }
            methods::STORE_SEEN_MARK_ALL => enc(ok(self.mark_all_seen().await?)),
            methods::STORE_RECENT_LIST => enc(ok(RecentProjects {
                projects: self.store.recent().await?,
            })),
            methods::STORE_RECENT_ADD => {
                let r: RecentProject = params(p).map_err(dec)?;
                enc(ok(RecentProjects {
                    projects: self.store.add_recent(r).await?,
                }))
            }
            methods::STORE_WORKSTREAMS_LIST => enc(ok(WorkstreamList {
                workstreams: self.store.workstreams().await?,
            })),
            methods::STORE_WORKSTREAMS_PUT => {
                let r: WorkstreamPut = params(p).map_err(dec)?;
                enc(ok(self.store.put_workstream(r).await?))
            }
            methods::STORE_WORKSTREAMS_DELETE => {
                let r: WorkstreamRef = params(p).map_err(dec)?;
                self.store.delete_workstream(r.id).await?;
                enc(ok(Ack {}))
            }
            other => Err(MarsadError::NotFound {
                what: format!("method {other}"),
            }),
        }
    }

    /// `hosts.links`: the full map first, then again on every change.
    fn links(&self, from: Option<Cursor>) -> EventStream {
        let rx = self.registry.hub().states();
        let start = from
            .as_ref()
            .and_then(|c| c.as_str().rsplit('.').next())
            .and_then(|n| n.parse::<u64>().ok())
            .unwrap_or(0);
        futures::stream::unfold((rx, start, true), |(mut rx, n, first)| async move {
            if !first && rx.changed().await.is_err() {
                return None;
            }
            let states = rx.borrow_and_update().clone();
            let v = serde_json::to_value(LinkStates { states }).map_err(|e| CloseReason::Error {
                error: RpcError::internal(e.to_string()),
            });
            let n = n + 1;
            Some((v.map(|v| (Cursor::new(format!("links.{n}")), v)), (rx, n, false)))
        })
        .boxed()
    }
}

#[async_trait]
impl Service for HubServices {
    async fn call(&self, ctx: CallContext, method: &str, p: Value) -> Result<Value, RpcError> {
        self.dispatch(&ctx, method, p).await.map_err(|e| match e {
            MarsadError::NotFound { what } if what == format!("method {method}") => {
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
        match topic {
            topics::FLEET => self.fleet.subscribe(from).await,
            topics::HOSTS_LINKS => Ok(self.links(from)),
            _ => Err(RpcError::topic_not_found(topic)),
        }
    }
}
