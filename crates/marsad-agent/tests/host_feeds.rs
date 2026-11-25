//! A remote host's feeds across link drops and refusals: the fleet's `host.sessions` feed (the
//! host is live again with a snapshot after a reconnect; a refusal is shown and recovered) and the
//! workstreams' Wasl feeds (not subscribed while the host is down; subscribed again after a
//! reconnect).
//!
//! The remote daemon is in process: a hub link whose connector the test takes offline, and whose
//! connection the test drops by aborting its server side.

mod common;

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{stream, StreamExt};
use marsad_agent::hub::fleet::Fleet;
use marsad_agent::hub::workstreams::Workstreams;
use marsad_proto::fleet::{FleetEvent, FleetHost, FleetLink};
use marsad_proto::hosts::{HostEntry, HostKind};
use marsad_proto::{HostId, LinkState};
use marsad_store::Store;
use serde_json::Value;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use wasit_control::{
    connector, Attempt, CallContext, ClientOptions, CloseReason, ConnConfig, ConnectError,
    ControlClient, ControlServer, Cursor, EventStream, Hello, HostSpec, Hub, HubConfig,
    LinkConnector, NoService, RoutingService, RpcError, Service, Supervised, SupervisedOptions,
};
use wasit_runtime::wire::{topics as rt_topics, HostEvent};

use common::{link_is, TempDir, T};

const HOST: &str = "box";
const WASL_TASKS: &str = wasl_proto::topics::TASKS;

/// The remote daemon: the topics it refuses, the refusals it has answered, and whether it is
/// reachable.
struct Daemon {
    refused: Mutex<HashSet<String>>,
    /// Topics refused as overloaded (a retryable refusal).
    overloaded: Mutex<HashSet<String>>,
    refusals: Mutex<Vec<String>>,
    refusal_seen: Notify,
    /// Ends the current `host.sessions` feed with `Ended` (one permit: the next feed to be ended).
    end_feed: Notify,
    online: AtomicBool,
}

impl Daemon {
    fn new(refused: &[&str]) -> Arc<Daemon> {
        Arc::new(Daemon {
            refused: Mutex::new(refused.iter().map(|t| t.to_string()).collect()),
            overloaded: Mutex::new(HashSet::new()),
            refusals: Mutex::new(Vec::new()),
            refusal_seen: Notify::new(),
            end_feed: Notify::new(),
            online: AtomicBool::new(true),
        })
    }

    /// Wait until the daemon has refused `topic` once.
    async fn refused_once(&self, topic: &str) {
        tokio::time::timeout(T, async {
            loop {
                let seen = self.refusal_seen.notified();
                if self.refusals.lock().unwrap().iter().any(|t| t == topic) {
                    return;
                }
                seen.await;
            }
        })
        .await
        .expect("the daemon refused the topic");
    }
}

/// What the remote daemon serves: `host.sessions` (one snapshot, then nothing more) and the Wasl
/// topics (nothing, ever). A refused topic answers `topic_not_found`.
struct Served(Arc<Daemon>);

#[async_trait]
impl Service for Served {
    async fn call(
        &self,
        _ctx: CallContext,
        method: &str,
        _params: Value,
    ) -> Result<Value, RpcError> {
        Err(RpcError::method_not_found(method))
    }

    async fn subscribe(
        &self,
        _ctx: CallContext,
        topic: &str,
        _params: Value,
        from: Option<Cursor>,
    ) -> Result<EventStream, RpcError> {
        if self.0.overloaded.lock().unwrap().contains(topic) {
            self.0.refusals.lock().unwrap().push(topic.to_string());
            self.0.refusal_seen.notify_waiters();
            return Err(RpcError::new(wasit_control::codes::OVERLOADED, "busy"));
        }
        if self.0.refused.lock().unwrap().contains(topic) {
            self.0.refusals.lock().unwrap().push(topic.to_string());
            self.0.refusal_seen.notify_waiters();
            return Err(RpcError::topic_not_found(topic));
        }
        if topic == rt_topics::HOST_SESSIONS {
            // A fresh subscription starts with the snapshot; a resume from a cursor does not (the
            // events after it are all there is), as the host's own service behaves.
            let mut first: Vec<Result<(Cursor, Value), CloseReason>> = Vec::new();
            if from.is_none() {
                let snapshot = serde_json::to_value(HostEvent::Reset {
                    sessions: Vec::new(),
                })
                .expect("a host event");
                first.push(Ok((Cursor::new("1"), snapshot)));
            }
            // Then nothing, until the test ends the feed (the service's own verdict, link up).
            let daemon = self.0.clone();
            let ended = stream::once(async move {
                daemon.end_feed.notified().await;
                Err::<(Cursor, Value), _>(CloseReason::Ended)
            });
            return Ok(stream::iter(first).chain(ended).boxed());
        }
        if [
            WASL_TASKS,
            wasl_proto::topics::EDGES,
            wasl_proto::topics::SPAWNS,
        ]
        .contains(&topic)
        {
            return Ok(stream::pending().boxed());
        }
        Err(RpcError::topic_not_found(topic))
    }
}

/// The hub's connector to the remote daemon: refuses while the daemon is offline.
struct Link {
    daemon: Arc<Daemon>,
    server: ControlServer,
}

#[async_trait]
impl LinkConnector for Link {
    async fn connect(
        &self,
        _spec: &HostSpec,
        opts: ClientOptions,
        _attempt: Attempt,
    ) -> Result<ControlClient, ConnectError> {
        if !self.daemon.online.load(Ordering::SeqCst) {
            return Err(ConnectError::Io("the host is offline".into()));
        }
        let (a, b) = tokio::io::duplex(1 << 16);
        // Detached: the server closes the connection when the link is dropped (`Rig::drop_link`).
        let _detached = self.server.serve_connection(Box::new(a));
        ControlClient::connect(Box::new(b), opts).await
    }
}

/// The routing service the workstreams subscribe through, recording every subscribe that
/// reaches it as `host:topic`.
struct Counting {
    routing: RoutingService,
    subs: Mutex<Vec<String>>,
    notify: Notify,
}

impl Counting {
    /// Wait until `pred` holds for the subscribes seen so far.
    async fn until(&self, pred: impl Fn(&[String]) -> bool) {
        tokio::time::timeout(T, async {
            loop {
                // Created before the check: a subscribe after the check wakes it.
                let seen = self.notify.notified();
                let subs = self.subs.lock().unwrap().clone();
                if pred(&subs) {
                    return;
                }
                seen.await;
            }
        })
        .await
        .expect("the expected subscribes happened");
    }

    /// How many subscribes of `key` (`host:topic`) reached the routing service.
    fn count(&self, key: &str) -> usize {
        self.subs
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.as_str() == key)
            .count()
    }

    /// How many subscribes of any topic of `host` reached the routing service.
    fn count_host(&self, host: &str) -> usize {
        let prefix = format!("{host}:");
        self.subs
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.starts_with(&prefix))
            .count()
    }
}

#[async_trait]
impl Service for Counting {
    async fn call(&self, ctx: CallContext, method: &str, params: Value) -> Result<Value, RpcError> {
        self.routing.call(ctx, method, params).await
    }

    async fn subscribe(
        &self,
        ctx: CallContext,
        topic: &str,
        params: Value,
        from: Option<Cursor>,
    ) -> Result<EventStream, RpcError> {
        let host = ctx.host.as_ref().map_or("-", |h| h.as_str()).to_string();
        self.subs.lock().unwrap().push(format!("{host}:{topic}"));
        self.notify.notify_waiters();
        self.routing.subscribe(ctx, topic, params, from).await
    }
}

fn box_entry() -> HostEntry {
    HostEntry {
        id: HostId::new(HOST),
        name: HOST.into(),
        kind: HostKind::Local,
        platform: None,
        created_at_ms: 0,
        interactive_auth: false,
        local_files: false,
        shared_connection: false,
        signed_out: false,
        machine: None,
    }
}

/// The hub with the remote host's link, the fleet fed by the hub's link states and by the host's
/// `host.sessions` feed.
struct Rig {
    daemon: Arc<Daemon>,
    /// The remote daemon's server (its connections are the host's link).
    server: ControlServer,
    hub: Hub,
    fleet: Fleet,
    _feeds: Vec<JoinHandle<()>>,
}

impl Rig {
    async fn start(daemon: Arc<Daemon>) -> Rig {
        let server = ControlServer::in_process(
            Arc::new(Served(daemon.clone())),
            Hello::new("b", HOST, "daemon"),
            ConnConfig::default(),
        );
        let hub = Hub::new(
            HubConfig::new(Hello::new("b", "hub", "hub")),
            Arc::new(Link {
                daemon: daemon.clone(),
                server: server.clone(),
            }),
            None,
        );
        let fleet = Fleet::spawn();
        let links = fleet.feed_links(hub.clone());
        fleet
            .host_upserted(
                box_entry(),
                FleetLink::Remote {
                    state: LinkState::Connecting,
                },
            )
            .await;
        hub.add_link(HostSpec::new(HOST, ["unused"]))
            .expect("the link is added");
        let link = hub.link(&HostId::new(HOST)).expect("the link exists");
        let sessions = fleet.feed_remote(HostId::new(HOST), link);
        Rig {
            daemon,
            server,
            hub,
            fleet,
            _feeds: vec![links, sessions],
        }
    }

    /// Drop the host's link: its connection is closed on the daemon's side, the hub sees it lost.
    fn drop_link(&self) {
        for connection in self.server.connections() {
            connection.close();
        }
    }

    /// The loopback link the workstreams talk over: the hub's routing service, counted.
    fn loopback(&self) -> (Supervised, Arc<Counting>) {
        let counting = Arc::new(Counting {
            routing: RoutingService::new(self.hub.clone(), Arc::new(NoService)),
            subs: Mutex::new(Vec::new()),
            notify: Notify::new(),
        });
        let server = ControlServer::in_process(
            counting.clone(),
            Hello::new("b", "hub", "hub"),
            ConnConfig::default(),
        );
        let link = Supervised::spawn(
            connector(move || {
                let server = server.clone();
                async move {
                    let (a, b) = tokio::io::duplex(1 << 16);
                    let _detached = server.serve_connection(Box::new(a));
                    ControlClient::connect(
                        Box::new(b),
                        ClientOptions::new(Hello::new("b", "hub", "hub")),
                    )
                    .await
                }
            }),
            SupervisedOptions::default(),
        );
        (link, counting)
    }

    async fn host(&self) -> FleetHost {
        self.fleet
            .snapshot()
            .await
            .into_iter()
            .find(|h| h.host.id.as_str() == HOST)
            .expect("the host is in the fleet")
    }

    /// Wait until the host's state satisfies `done`: already, or by the next event `event`
    /// accepts. The subscription is made here, at the start of the wait, so it begins from the
    /// fleet's current state: an event from before this call (already reflected in the state, or
    /// awaited by an earlier call) is never taken for the one this call waits for.
    async fn settle(&self, done: impl Fn(&FleetHost) -> bool, event: impl Fn(&FleetEvent) -> bool) {
        let mut events = self.fleet.subscribe(None).await.expect("the fleet topic");
        if done(&self.host().await) {
            return;
        }
        tokio::time::timeout(T, async {
            loop {
                let (_, v) = events
                    .next()
                    .await
                    .expect("the fleet is open")
                    .expect("an event");
                let e: FleetEvent = serde_json::from_value(v).expect("a fleet event");
                if event(&e) {
                    return;
                }
            }
        })
        .await
        .expect("the fleet event came");
    }
}

fn link_up(h: &FleetHost) -> bool {
    matches!(&h.link, FleetLink::Remote { state } if state.is_connected())
}

fn link_event(e: &FleetEvent, connected: bool) -> bool {
    link_is(e, HOST, |s| s.is_connected() == connected)
}

fn sessions_event(e: &FleetEvent) -> bool {
    matches!(e, FleetEvent::Sessions { host, .. } if host.as_str() == HOST)
}

/// A `FeedError` of the host (`Some(None)` = the refusal is over).
fn feed_error_event(e: &FleetEvent) -> Option<Option<String>> {
    match e {
        FleetEvent::FeedError { host, error } if host.as_str() == HOST => Some(error.clone()),
        _ => None,
    }
}

/// A store for the workstreams, in a directory that lives as long as the returned guard.
async fn workstreams_store() -> (TempDir, Store) {
    let dir = TempDir::new();
    let store = Store::open(dir.path().to_path_buf())
        .await
        .expect("the store opens");
    (dir, store)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_host_feed_is_live_again_with_a_snapshot_after_its_link_drops() {
    let rig = Rig::start(Daemon::new(&[])).await;
    rig.settle(link_up, |e| link_event(e, true)).await;
    rig.settle(|h| h.sessions_live, sessions_event).await;

    rig.drop_link();
    rig.settle(|h| !link_up(h), |e| link_event(e, false)).await;
    rig.settle(link_up, |e| link_event(e, true)).await;
    // The resubscribe starts with a snapshot: the host's sessions are live again.
    rig.settle(|h| h.sessions_live, sessions_event).await;

    let host = rig.host().await;
    assert!(host.sessions_live, "the host is live after the reconnect");
    assert_eq!(host.feed_error, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_host_feed_is_shown_and_recovered_on_reconnect() {
    let rig = Rig::start(Daemon::new(&[rt_topics::HOST_SESSIONS])).await;
    rig.settle(
        |h| h.feed_error.is_some(),
        |e| feed_error_event(e).is_some_and(|e| e.is_some()),
    )
    .await;
    let host = rig.host().await;
    let text = host.feed_error.clone().expect("the refusal is shown");
    assert!(text.starts_with("topic_not_found"), "{text}");
    assert!(!host.sessions_live, "no snapshot came through");

    // The host serves the topic now; its link drops and comes back.
    rig.daemon.refused.lock().unwrap().clear();
    rig.drop_link();
    rig.settle(|h| !link_up(h), |e| link_event(e, false)).await;
    rig.settle(link_up, |e| link_event(e, true)).await;
    rig.settle(
        |h| h.feed_error.is_none(),
        |e| feed_error_event(e) == Some(None),
    )
    .await;

    let host = rig.host().await;
    assert!(host.sessions_live, "the snapshot made the host live");
    assert_eq!(host.feed_error, None, "the refusal is cleared");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_wasl_subscribe_is_attempted_while_the_host_is_disconnected() {
    let rig = Rig::start(Daemon::new(&[])).await;
    let (loopback, counting) = rig.loopback();
    let (_dir, store) = workstreams_store().await;
    let _ws = Workstreams::start(store, loopback, rig.fleet.clone(), HostId::new("hub"))
        .await
        .expect("the workstreams start");
    rig.settle(link_up, |e| link_event(e, true)).await;
    let wasl = format!("{HOST}:{WASL_TASKS}");
    counting.until(|s| s.contains(&wasl)).await;

    // The host goes away and stays away for now: nothing is asked of it.
    rig.daemon.online.store(false, Ordering::SeqCst);
    rig.drop_link();
    rig.settle(|h| !link_up(h), |e| link_event(e, false)).await;
    let before = counting.count_host(HOST);
    let wasl_before = counting.count(&wasl);
    // The window in which a feed that still asks would ask again (the supervisor's own connection
    // attempts are not subscribes).
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        counting.count_host(HOST),
        before,
        "no subscribe reaches a disconnected host"
    );

    // Back: the workstreams subscribe again.
    rig.daemon.online.store(true, Ordering::SeqCst);
    rig.hub.reconnect_now(&HostId::new(HOST));
    rig.settle(link_up, |e| link_event(e, true)).await;
    counting
        .until(|_| counting.count(&wasl) > wasl_before)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_workstream_feed_refused_for_good_is_subscribed_again_on_reconnect() {
    let rig = Rig::start(Daemon::new(&[WASL_TASKS])).await;
    let (loopback, counting) = rig.loopback();
    let (_dir, store) = workstreams_store().await;
    let _ws = Workstreams::start(store, loopback, rig.fleet.clone(), HostId::new("hub"))
        .await
        .expect("the workstreams start");
    let wasl = format!("{HOST}:{WASL_TASKS}");
    counting.until(|s| s.contains(&wasl)).await;
    // The first subscribe is refused by the daemon (not merely sent) before the topic is accepted.
    rig.daemon.refused_once(WASL_TASKS).await;
    let refused_attempts = counting.count(&wasl);

    // The service accepts the topic from now on; the link drops and comes back.
    rig.daemon.refused.lock().unwrap().clear();
    rig.drop_link();
    rig.settle(|h| !link_up(h), |e| link_event(e, false)).await;
    rig.settle(link_up, |e| link_event(e, true)).await;

    // The feed was not left dead: it subscribes again.
    counting
        .until(|_| counting.count(&wasl) > refused_attempts)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retryable_refusal_is_a_feed_error_and_does_not_drop_the_link() {
    let daemon = Daemon::new(&[]);
    daemon
        .overloaded
        .lock()
        .unwrap()
        .insert(rt_topics::HOST_SESSIONS.to_string());
    let rig = Rig::start(daemon).await;
    rig.settle(link_up, |e| link_event(e, true)).await;
    rig.settle(
        |h| h.feed_error.is_some(),
        |e| feed_error_event(e).is_some_and(|e| e.is_some()),
    )
    .await;
    let text = rig.host().await.feed_error.expect("the refusal is shown");
    assert!(text.starts_with("overloaded"), "{text}");

    // The refusal does not drop the link: no link-down event follows it (a window in which the
    // link would be dropped and reported; the link itself is the only thing that drops it).
    let dropped = tokio::time::timeout(
        Duration::from_secs(1),
        rig.settle(|h| !link_up(h), |e| link_event(e, false)),
    )
    .await;
    assert!(
        dropped.is_err(),
        "the link was dropped on account of the refusal"
    );
    assert!(link_up(&rig.host().await), "the link is still up");

    // The host accepts the feed now; the next link event brings it back with a snapshot.
    rig.daemon.overloaded.lock().unwrap().clear();
    rig.drop_link();
    rig.settle(|h| !link_up(h), |e| link_event(e, false)).await;
    rig.settle(link_up, |e| link_event(e, true)).await;
    rig.settle(
        |h| h.feed_error.is_none(),
        |e| feed_error_event(e) == Some(None),
    )
    .await;
    assert!(
        rig.host().await.sessions_live,
        "the snapshot made the host live"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_feed_that_ends_while_the_link_is_up_is_a_feed_error_and_recovers_on_reconnect() {
    let rig = Rig::start(Daemon::new(&[])).await;
    rig.settle(link_up, |e| link_event(e, true)).await;
    rig.settle(|h| h.sessions_live, sessions_event).await;

    // The host ends its feed; the link stays up.
    rig.daemon.end_feed.notify_one();
    rig.settle(
        |h| h.feed_error.is_some(),
        |e| feed_error_event(e).is_some_and(|e| e.is_some()),
    )
    .await;
    let host = rig.host().await;
    let text = host.feed_error.clone().expect("the end is shown");
    assert!(text.starts_with("ended"), "{text}");
    assert!(!host.sessions_live, "the sessions are no longer live");
    assert!(link_up(&host), "the link stayed up");

    // The next link connection starts the feed again, with a snapshot.
    rig.drop_link();
    rig.settle(|h| !link_up(h), |e| link_event(e, false)).await;
    rig.settle(link_up, |e| link_event(e, true)).await;
    rig.settle(
        |h| h.feed_error.is_none(),
        |e| feed_error_event(e) == Some(None),
    )
    .await;
    let host = rig.host().await;
    assert!(host.sessions_live, "the snapshot made the host live");
    assert_eq!(host.feed_error, None);
}
