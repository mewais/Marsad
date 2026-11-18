//! The `fleet` topic: one actor aggregates the local session feed, every remote host's
//! `host.sessions` feed (over its supervised link, resubscribing from its cursor across
//! reconnects) and the hub's link states into [`FleetEvent`]s.
//!
//! Replay: the actor keeps the last [`RING`] events. A subscriber resuming from a cursor still
//! in the ring gets exactly the events after it; any other cursor (expired, another daemon
//! run) starts with a `reset`. Subscribing is a message to the actor, so the replay/reset and
//! the live receiver join with no gap. A subscriber that lags is closed with `Lagged` and
//! resumes from its cursor.

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use futures::stream::StreamExt;
use marsad_proto::fleet::{FleetEvent, FleetHost, FleetLink};
use marsad_proto::hosts::{AuthPrompt, HostEntry};
use marsad_proto::{HostId, LinkState};
use tokio::sync::{broadcast, mpsc, oneshot};
use wasit_control::{CloseReason, Cursor, DurableOpts, EventStream, Hub, RpcError, Supervised};
use wasit_runtime::wire::{topics as rt_topics, HostEvent};
use wasit_runtime::{SessionManager, StreamEnd};

/// Events kept for replay.
pub const RING: usize = 4096;
const INPUT_QUEUE: usize = 1024;
const LIVE_QUEUE: usize = 1024;

type Item = (u64, FleetEvent);
type Subscribed = (Vec<(String, FleetEvent)>, broadcast::Receiver<Item>, String);

enum Input {
    HostUpserted(HostEntry, FleetLink),
    HostRemoved(HostId),
    Link(HostId, LinkState),
    Feed(HostId, HostEvent),
    Prompt(HostId, Option<AuthPrompt>),
    Subscribe(Option<String>, oneshot::Sender<Subscribed>),
    Snapshot(oneshot::Sender<Vec<FleetHost>>),
}

/// Handle to the aggregator. Cheap to clone.
#[derive(Clone)]
pub struct Fleet {
    tx: mpsc::Sender<Input>,
}

struct State {
    epoch: String,
    hosts: BTreeMap<HostId, FleetHost>,
    ring: VecDeque<Item>,
    next: u64,
    live: broadcast::Sender<Item>,
}

impl State {
    fn emit(&mut self, ev: FleetEvent) {
        self.next += 1;
        let item = (self.next, ev);
        if self.ring.len() == RING {
            self.ring.pop_front();
        }
        self.ring.push_back(item.clone());
        // No subscribers is fine.
        let _ = self.live.send(item);
    }

    fn cursor(&self, n: u64) -> String {
        format!("{}.{n}", self.epoch)
    }

    fn apply(&mut self, input: Input) {
        match input {
            Input::HostUpserted(entry, link) => {
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
            Input::Subscribe(from, reply) => {
                let first = self.ring.front().map(|(n, _)| *n).unwrap_or(self.next + 1);
                let resume = from.as_deref().and_then(|c| {
                    let (e, n) = c.split_once('.')?;
                    let n: u64 = n.parse().ok()?;
                    (e == self.epoch && n + 1 >= first && n <= self.next).then_some(n)
                });
                let initial = match resume {
                    Some(n) => self
                        .ring
                        .iter()
                        .filter(|(m, _)| *m > n)
                        .map(|(m, e)| (self.cursor(*m), e.clone()))
                        .collect(),
                    None => vec![(
                        self.cursor(self.next),
                        FleetEvent::Reset {
                            hosts: self.hosts.values().cloned().collect(),
                        },
                    )],
                };
                // The subscriber may have gone away.
                let _ = reply.send((initial, self.live.subscribe(), self.epoch.clone()));
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
            epoch: format!("{:x}", fastrand::u64(..) >> 16),
            hosts: BTreeMap::new(),
            ring: VecDeque::new(),
            next: 0,
            live: broadcast::channel(LIVE_QUEUE).0,
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
        self.send(Input::HostUpserted(entry, link)).await
    }

    pub async fn auth_prompt(&self, id: HostId, prompt: Option<AuthPrompt>) {
        self.send(Input::Prompt(id, prompt)).await
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
        let (initial, live, epoch) = rx
            .await
            .map_err(|_| RpcError::internal("the fleet aggregator is gone"))?;
        let enc = |c: String, e: &FleetEvent| {
            serde_json::to_value(e)
                .map(|v| (Cursor::new(c), v))
                .map_err(|e| CloseReason::Error {
                    error: RpcError::internal(e.to_string()),
                })
        };
        let head: Vec<_> = initial.iter().map(|(c, e)| enc(c.clone(), e)).collect();
        let tail = futures::stream::unfold(live, move |mut rx| {
            let epoch = epoch.clone();
            async move {
                match rx.recv().await {
                    Ok((n, e)) => Some((enc(format!("{epoch}.{n}"), &e), rx)),
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        Some((Err(CloseReason::Lagged), rx))
                    }
                    Err(broadcast::error::RecvError::Closed) => None,
                }
            }
        })
        .scan(false, |ended, item| {
            // Stop after the first error (a `Lagged` close ends the stream).
            if *ended {
                return futures::future::ready(None);
            }
            *ended = item.is_err();
            futures::future::ready(Some(item))
        });
        Ok(futures::stream::iter(head).chain(tail).boxed())
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
