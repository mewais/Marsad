//! The `fleet` topic: one actor aggregates the local session feed, every remote host's
//! `host.sessions` feed (over its supervised link, resubscribing from its cursor across
//! reconnects) and the hub's link states into [`FleetEvent`]s.
//!
//! Replay and resume follow [`super::ring`].

use std::collections::BTreeMap;
use std::time::Duration;

use marsad_proto::fleet::{FleetEvent, FleetHost, FleetLink, GitSummary};
use marsad_proto::hosts::{AuthPrompt, HostEntry};
use marsad_proto::{HostId, LinkState};
use tokio::sync::{broadcast, mpsc, oneshot};
use wasit_control::{Cursor, DurableOpts, EventStream, Hub, RpcError, Supervised};
use wasit_runtime::wire::{topics as rt_topics, HostEvent, SessionSnapshot};
use wasit_runtime::{SessionManager, StreamEnd};

use super::gitwatch::GitWatch;
use super::ring::{self, Joined, Ring};

const INPUT_QUEUE: usize = 1024;

enum Input {
    HostUpserted(Box<HostEntry>, FleetLink),
    HostRemoved(HostId),
    Link(HostId, LinkState),
    Feed(HostId, HostEvent),
    Prompt(HostId, Option<AuthPrompt>),
    Git(HostId, GitSummary),
    SetGitWatch(GitWatch),
    Subscribe(Option<String>, oneshot::Sender<Joined<FleetEvent>>),
    Snapshot(oneshot::Sender<Vec<FleetHost>>),
}

/// Handle to the aggregator. Cheap to clone.
#[derive(Clone)]
pub struct Fleet {
    tx: mpsc::Sender<Input>,
}

struct State {
    hosts: BTreeMap<HostId, FleetHost>,
    ring: Ring<FleetEvent>,
    git: Option<GitWatch>,
}

/// A snapshot change that can change the working tree: a new session, a finished tool call or
/// turn, a phase change.
fn may_change_tree(old: Option<&SessionSnapshot>, new: &SessionSnapshot) -> bool {
    let Some(old) = old else { return true };
    old.phase.name() != new.phase.name()
        || old.turns_completed != new.turns_completed
        || old
            .turn
            .as_ref()
            .map(|t| (t.tool_calls, t.current_tool.is_some()))
            != new
                .turn
                .as_ref()
                .map(|t| (t.tool_calls, t.current_tool.is_some()))
}

impl State {
    fn emit(&mut self, ev: FleetEvent) {
        self.ring.emit(ev);
    }

    fn apply(&mut self, input: Input) {
        match input {
            Input::HostUpserted(entry, link) => {
                let entry = *entry;
                let id = entry.id.clone();
                match self.hosts.get_mut(&id) {
                    Some(h) => h.host = entry.clone(),
                    None => {
                        let live = matches!(link, FleetLink::Local);
                        self.hosts.insert(
                            id,
                            FleetHost {
                                host: entry.clone(),
                                link,
                                sessions_live: live,
                                sessions: Vec::new(),
                                auth_prompt: None,
                                git: Vec::new(),
                            },
                        );
                    }
                }
                self.emit(FleetEvent::HostUpserted { host: entry });
            }
            Input::HostRemoved(id) => {
                if self.hosts.remove(&id).is_some() {
                    self.emit(FleetEvent::HostRemoved { id });
                }
            }
            Input::Link(id, state) => {
                let Some(h) = self.hosts.get_mut(&id) else {
                    return;
                };
                if !state.is_connected() {
                    h.sessions_live = false;
                }
                h.link = FleetLink::Remote {
                    state: state.clone(),
                };
                self.emit(FleetEvent::Link { host: id, state });
            }
            Input::Feed(id, ev) => {
                let Some(h) = self.hosts.get_mut(&id) else {
                    return;
                };
                let mut touch: Vec<String> = Vec::new();
                match &ev {
                    HostEvent::Reset { sessions } => {
                        touch.extend(sessions.iter().filter_map(|s| s.cwd.clone()));
                    }
                    HostEvent::Upsert { session } => {
                        let old = h.sessions.iter().find(|s| s.id == session.id);
                        if may_change_tree(old, session) {
                            touch.extend(session.cwd.clone());
                        }
                    }
                    HostEvent::Removed { .. } => {}
                }
                if let Some(g) = &self.git {
                    touch.sort();
                    touch.dedup();
                    for cwd in touch {
                        g.touch(id.clone(), cwd);
                    }
                }
                match ev {
                    HostEvent::Reset { sessions } => {
                        h.sessions = sessions.clone();
                        h.sessions_live = true;
                        self.emit(FleetEvent::Sessions { host: id, sessions });
                    }
                    HostEvent::Upsert { session } => {
                        match h.sessions.iter_mut().find(|s| s.id == session.id) {
                            Some(s) => *s = (*session).clone(),
                            None => h.sessions.push((*session).clone()),
                        }
                        self.emit(FleetEvent::SessionUpserted { host: id, session });
                    }
                    HostEvent::Removed { id: sid } => {
                        h.sessions.retain(|s| s.id != sid);
                        self.emit(FleetEvent::SessionRemoved { host: id, id: sid });
                    }
                }
            }
            Input::Prompt(id, prompt) => {
                let Some(h) = self.hosts.get_mut(&id) else {
                    return;
                };
                h.auth_prompt = prompt.clone();
                self.emit(FleetEvent::AuthPrompt { host: id, prompt });
            }
            Input::Git(id, summary) => {
                let Some(h) = self.hosts.get_mut(&id) else {
                    return;
                };
                // Summaries follow sessions: a folder no session uses is not published, and
                // folders no session uses any more are dropped.
                let used: std::collections::BTreeSet<String> =
                    h.sessions.iter().filter_map(|s| s.cwd.clone()).collect();
                h.git.retain(|g| used.contains(&g.cwd));
                if !used.contains(&summary.cwd) {
                    return;
                }
                let content = |g: &GitSummary| GitSummary {
                    refreshed_at_ms: 0,
                    ..g.clone()
                };
                match h.git.iter_mut().find(|g| g.cwd == summary.cwd) {
                    // Same content refreshed again: keep the row, publish nothing new.
                    Some(g) if content(g) == content(&summary) => {
                        *g = summary;
                        return;
                    }
                    Some(g) => *g = summary.clone(),
                    None => h.git.push(summary.clone()),
                }
                self.emit(FleetEvent::Git { host: id, summary });
            }
            Input::SetGitWatch(g) => self.git = Some(g),
            Input::Subscribe(from, reply) => {
                let hosts = &self.hosts;
                let joined = self.ring.join(from.as_deref(), || FleetEvent::Reset {
                    hosts: hosts.values().cloned().collect(),
                });
                // The subscriber may have gone away.
                let _ = reply.send(joined);
            }
            Input::Snapshot(reply) => {
                // The asker may have gone away.
                let _ = reply.send(self.hosts.values().cloned().collect());
            }
        }
    }
}

impl Fleet {
    pub fn spawn() -> Fleet {
        let (tx, mut rx) = mpsc::channel(INPUT_QUEUE);
        let mut st = State {
            hosts: BTreeMap::new(),
            ring: Ring::new(),
            git: None,
        };
        tokio::spawn(async move {
            while let Some(i) = rx.recv().await {
                st.apply(i);
            }
        });
        Fleet { tx }
    }

    async fn send(&self, i: Input) {
        if self.tx.send(i).await.is_err() {
            tracing::error!("the fleet aggregator is gone");
        }
    }

    pub async fn host_upserted(&self, entry: HostEntry, link: FleetLink) {
        self.send(Input::HostUpserted(Box::new(entry), link)).await
    }

    pub async fn auth_prompt(&self, id: HostId, prompt: Option<AuthPrompt>) {
        self.send(Input::Prompt(id, prompt)).await
    }

    /// Report a refreshed git summary.
    pub async fn git_summary(&self, host: HostId, summary: GitSummary) {
        self.send(Input::Git(host, summary)).await
    }

    /// Start touching `watch` on session changes.
    pub async fn set_git_watch(&self, watch: GitWatch) {
        self.send(Input::SetGitWatch(watch)).await
    }

    pub async fn host_removed(&self, id: HostId) {
        self.send(Input::HostRemoved(id)).await
    }

    pub async fn snapshot(&self) -> Vec<FleetHost> {
        let (tx, rx) = oneshot::channel();
        self.send(Input::Snapshot(tx)).await;
        rx.await.unwrap_or_default()
    }

    /// The `fleet` topic.
    pub async fn subscribe(&self, from: Option<Cursor>) -> Result<EventStream, RpcError> {
        let (tx, rx) = oneshot::channel();
        self.send(Input::Subscribe(from.map(|c| c.0), tx)).await;
        let joined = rx
            .await
            .map_err(|_| RpcError::internal("the fleet aggregator is gone"))?;
        Ok(ring::stream(joined))
    }

    /// Feed the local session manager's `host.sessions` into the fleet (forever; resumes from
    /// its cursor after a lag).
    pub fn feed_local(&self, host: HostId, manager: SessionManager) -> tokio::task::JoinHandle<()> {
        let fleet = self.clone();
        tokio::spawn(async move {
            let mut last: Option<String> = None;
            let mut failures = 0u32;
            loop {
                let mut s = manager.subscribe_host(last.as_deref());
                while let Some(item) = s.next().await {
                    match item {
                        Ok((c, ev)) => {
                            failures = 0;
                            last = Some(c);
                            fleet.send(Input::Feed(host.clone(), ev)).await;
                        }
                        Err(StreamEnd::Lagged) => break,
                        Err(StreamEnd::Error(e)) => {
                            tracing::warn!(error = %e, "local session feed failed");
                            break;
                        }
                    }
                }
                failures = failures.saturating_add(1);
                // Backoff schedule for a feed that keeps failing.
                tokio::time::sleep(Duration::from_millis(10u64 << failures.min(10))).await;
            }
        })
    }

    /// Feed a remote host's `host.sessions` over its supervised link.
    pub fn feed_remote(&self, host: HostId, link: Supervised) -> tokio::task::JoinHandle<()> {
        let fleet = self.clone();
        tokio::spawn(async move {
            let mut sub = link.subscribe_durable(
                rt_topics::HOST_SESSIONS,
                DurableOpts {
                    subscribe_deadline: Duration::from_secs(30),
                    ..DurableOpts::default()
                },
            );
            while let Some(item) = sub.next().await {
                match item {
                    Ok((_, v)) => match serde_json::from_value::<HostEvent>(v) {
                        Ok(ev) => fleet.send(Input::Feed(host.clone(), ev)).await,
                        Err(e) => {
                            tracing::warn!(host = %host, error = %e, "undecodable host event")
                        }
                    },
                    Err(e) => {
                        tracing::warn!(host = %host, error = %e, "remote session feed failed");
                        return;
                    }
                }
            }
        })
    }

    /// Forward the hub's link state changes (re-reading all states after a lag).
    pub fn feed_links(&self, hub: Hub) -> tokio::task::JoinHandle<()> {
        let fleet = self.clone();
        tokio::spawn(async move {
            let mut rx = hub.events();
            loop {
                match rx.recv().await {
                    Ok(ev) => {
                        if let Some(state) = ev.state {
                            fleet.send(Input::Link(ev.host, state)).await;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let states = hub.states().borrow().clone();
                        for (h, s) in states {
                            fleet.send(Input::Link(h, s)).await;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        })
    }
}
