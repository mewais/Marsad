//! The `fleet` topic: one actor aggregates the local session feed, every remote host's
//! `host.sessions` feed (over its supervised link, resubscribing from its cursor across
//! reconnects) and the hub's link states into [`FleetEvent`]s.
//!
//! Replay and resume follow [`super::ring`].
//!
//! Each host's session feed records its life (`fleet.feed`: started, lagged, undecodable event,
//! refused with the service's error code). A refused feed is not the end of the host's feed: the
//! host's sessions stay as they last were, marked with a [`FleetEvent::FeedError`] the app shows,
//! and the feed resubscribes (a fresh snapshot, so the sessions are live again) when the host's
//! link next connects.

use std::collections::BTreeMap;
use std::time::Duration;

use futures::{stream, Stream, StreamExt};
use marsad_proto::diag::{FeedEvent, MarsadEvent};
use marsad_proto::fleet::{FleetEvent, FleetHost, FleetLink, GitSummary};
use marsad_proto::hosts::{AuthPrompt, HostEntry};
use marsad_proto::{HostId, LinkState};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use wasit_control::{
    CloseReason, ControlClient, Cursor, EventStream, Hub, RpcError, SubscribeOpts, Subscription,
    Supervised,
};
use wasit_diag::{ErrorInfo, Ident};
use wasit_runtime::wire::{topics as rt_topics, HostEvent, SessionSnapshot};
use wasit_runtime::{SessionManager, StreamEnd};

use super::fleet_events;
use super::gitwatch::GitWatch;
use super::ring::{self, Joined, Ring};
use crate::diag::{self, emit};

const INPUT_QUEUE: usize = 1024;
/// Bound on each subscribe round trip of a [`Follow`].
const SUBSCRIBE_DEADLINE: Duration = Duration::from_secs(30);

/// `fleet.feed` for `host`.
fn feed_record(host: &HostId, event: FeedEvent, error: Option<ErrorInfo>) {
    emit(MarsadEvent::FleetFeed {
        host: Ident::new(host.as_str()),
        event,
        error,
    });
}

/// What a [`Follow`] delivers.
pub(super) enum Step {
    /// One event.
    Event(serde_json::Value),
    /// The feed failed while the link may stay up: the service refused the subscription (retryable
    /// or not), or ended the feed with the link up. The follow goes on: its next subscribe waits for
    /// the link's next change, and the link itself is never dropped on this account.
    Refused(RpcError),
}

/// Follows one topic over a supervised link for as long as the caller asks for events.
///
/// It subscribes only while the link is up (`up` says so for the state it watches, and a live
/// client is there): nothing is asked of a host that is down, so no request is made that can only
/// fail. A subscription ends with its connection, with the service's verdict, or with the feed
/// ending while the link stays up (reported as a [`Step::Refused`]); the next subscribe waits for
/// the next change of the link state (event-driven, no timers). A failed subscribe never reconnects
/// the link: the link is the host's, and its own supervisor decides when it is down. With
/// `reset_on_connect` the next subscribe starts from the beginning, so the service sends a full
/// snapshot first; otherwise it resumes from the last cursor.
pub(super) struct Follow<T> {
    link: Supervised,
    topic: &'static str,
    host: Option<HostId>,
    state: watch::Receiver<T>,
    up: fn(&T) -> bool,
    reset_on_connect: bool,
    last: Option<Cursor>,
    sub: Option<Subscription>,
    /// The next call waits for a change of the state before subscribing.
    await_change: bool,
    /// The next subscribe starts from the beginning.
    fresh: bool,
}

impl<T> Follow<T> {
    pub(super) fn new(
        link: Supervised,
        topic: &'static str,
        host: Option<HostId>,
        state: watch::Receiver<T>,
        up: fn(&T) -> bool,
        reset_on_connect: bool,
    ) -> Self {
        Follow {
            link,
            topic,
            host,
            state,
            up,
            reset_on_connect,
            last: None,
            sub: None,
            await_change: false,
            fresh: reset_on_connect,
        }
    }

    /// The next event or refusal; `None` once the state can no longer change (its owner is gone).
    pub(super) async fn next(&mut self) -> Option<Step> {
        loop {
            if std::mem::take(&mut self.await_change) {
                self.state.changed().await.ok()?;
            }
            if self.sub.is_none() {
                let client = self.connected_client().await?;
                let from = if self.fresh { None } else { self.last.clone() };
                let mut opts = SubscribeOpts::new(SUBSCRIBE_DEADLINE).from(from);
                opts.host = self.host.clone();
                match client.subscribe_with(self.topic, opts).await {
                    Ok(sub) => {
                        self.sub = Some(sub);
                        self.fresh = false;
                    }
                    Err(e) => {
                        // Retryable or not, this is a feed error, not a reason to drop the link.
                        self.lost();
                        return Some(Step::Refused(e.into_rpc()));
                    }
                }
            }
            let Some(sub) = self.sub.as_mut() else {
                continue;
            };
            match sub.next().await {
                Some((c, v)) => {
                    self.last = Some(c.clone());
                    return Some(Step::Event(v));
                }
                None => {
                    let reason = sub.close_reason().cloned().unwrap_or(CloseReason::HostLost);
                    self.sub = None;
                    match reason {
                        // Resume from the last cursor at once: the link is still up.
                        CloseReason::Lagged => {}
                        // The link went with it: the next subscribe waits for the link.
                        CloseReason::HostLost => self.lost(),
                        // The feed ended while the link stayed up: the host's word that it has no
                        // more to say is a refusal, shown as one and recovered on the next link
                        // change (a reconnect starts with a snapshot).
                        CloseReason::Ended => {
                            self.lost();
                            return Some(Step::Refused(RpcError::new(
                                "ended",
                                "the feed ended while the link stayed up",
                            )));
                        }
                        CloseReason::Error { error } => {
                            self.lost();
                            return Some(Step::Refused(error));
                        }
                    }
                }
            }
        }
    }

    /// The subscription is over: the next one waits for a change of the link.
    fn lost(&mut self) {
        self.await_change = true;
        self.fresh = self.reset_on_connect;
    }

    /// A client of the link, once the link is up (waits for it otherwise).
    async fn connected_client(&mut self) -> Option<ControlClient> {
        loop {
            let up = (self.up)(&*self.state.borrow_and_update());
            if up {
                if let Some(client) = self.link.current() {
                    return Some(client);
                }
            }
            self.state.changed().await.ok()?;
        }
    }
}

enum Input {
    HostUpserted(Box<HostEntry>, FleetLink),
    HostRemoved(HostId),
    Link(HostId, LinkState),
    Feed(HostId, HostEvent),
    /// A host's session feed was refused (`Some`, why) or delivered its sessions again (`None`).
    FeedError(HostId, Option<String>),
    /// A prompt is shown for the host.
    PromptShown(HostId, Box<AuthPrompt>),
    /// The prompt with this id is over: taken off the host if it is the one showing (a newer
    /// prompt's is not).
    PromptEnded(HostId, u64),
    Git(HostId, GitSummary),
    SetGitWatch(GitWatch),
    Subscribe(Option<String>, oneshot::Sender<Joined<FleetEvent>>),
    Snapshot(oneshot::Sender<Vec<FleetHost>>),
}

/// Handle to the aggregator. Cheap to clone.
#[derive(Clone)]
pub struct Fleet {
    tx: mpsc::Sender<Input>,
    /// Bumped by the aggregator on every link and host change (see [`Fleet::changes`]).
    changes: watch::Sender<()>,
}

struct State {
    hosts: BTreeMap<HostId, FleetHost>,
    ring: Ring<FleetEvent>,
    git: Option<GitWatch>,
    changes: watch::Sender<()>,
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
        // A link or host change: a feed that waits for one may open its subscription again.
        if matches!(
            input,
            Input::HostUpserted(..) | Input::HostRemoved(_) | Input::Link(..)
        ) {
            self.changes.send_replace(());
        }
        match input {
            Input::HostUpserted(entry, link) => {
                let entry = *entry;
                let id = entry.id.clone();
                // The link a host is first seen with is announced right after it: a link event
                // that came before the host was known was not kept.
                let announce = match self.hosts.get_mut(&id) {
                    Some(h) => {
                        h.host = entry.clone();
                        None
                    }
                    None => {
                        let live = matches!(link, FleetLink::Local);
                        let announce = match &link {
                            FleetLink::Local => None,
                            FleetLink::Remote { state } => Some(state.clone()),
                        };
                        self.hosts.insert(
                            id.clone(),
                            FleetHost {
                                host: entry.clone(),
                                link,
                                sessions_live: live,
                                sessions: Vec::new(),
                                auth_prompt: None,
                                git: Vec::new(),
                                feed_error: None,
                            },
                        );
                        announce
                    }
                };
                self.emit(FleetEvent::HostUpserted { host: entry });
                if let Some(state) = announce {
                    self.emit(FleetEvent::Link { host: id, state });
                }
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
                    // A feed is started only after its host was upserted; this is the tail of
                    // a host just removed.
                    fleet_events::feed_unknown_host(&id);
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
                        // A snapshot is what a refused feed gets back: the refusal is over.
                        let recovered = h.feed_error.take().is_some();
                        self.emit(FleetEvent::Sessions {
                            host: id.clone(),
                            sessions,
                        });
                        if recovered {
                            self.emit(FleetEvent::FeedError {
                                host: id,
                                error: None,
                            });
                        }
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
            Input::FeedError(id, error) => {
                let Some(h) = self.hosts.get_mut(&id) else {
                    return;
                };
                if h.feed_error == error {
                    return;
                }
                if error.is_some() {
                    h.sessions_live = false;
                }
                h.feed_error = error.clone();
                self.emit(FleetEvent::FeedError { host: id, error });
            }
            Input::PromptShown(id, prompt) => {
                let Some(h) = self.hosts.get_mut(&id) else {
                    return;
                };
                h.auth_prompt = Some((*prompt).clone());
                self.emit(FleetEvent::AuthPrompt {
                    host: id,
                    prompt: Some(*prompt),
                });
            }
            Input::PromptEnded(id, prompt) => {
                let Some(h) = self.hosts.get_mut(&id) else {
                    return;
                };
                // Whoever ends a prompt may do so after a newer one was shown (the order of
                // their messages is not the order of their prompts): only the prompt that is
                // showing can be taken down.
                if h.auth_prompt.as_ref().is_some_and(|p| p.id == prompt) {
                    h.auth_prompt = None;
                    self.emit(FleetEvent::AuthPrompt {
                        host: id,
                        prompt: None,
                    });
                }
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

/// What a feed does once its subscription ended.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Resume {
    /// Open the subscription again at once.
    Now,
    /// Record the failure, then wait for the next link, host or fleet state change
    /// ([`Fleet::changes`]) before opening it again.
    AfterChange,
}

/// When a feed whose subscription ended opens it again.
///
/// A lag resumes at once, from the last cursor, and so does an end after events (the subscription
/// worked). An end that delivered nothing is a failure to deliver: the feed records it and waits
/// for the next state change before it asks again. There is no timer. A feed that keeps failing
/// waits for a change: it does not spin, and it is never given up on.
#[derive(Debug, Default)]
pub(super) struct Resubscribe {
    /// The subscription that just ended delivered an event.
    delivered: bool,
}

impl Resubscribe {
    /// An event arrived: the subscription works.
    pub(super) fn delivered(&mut self) {
        self.delivered = true;
    }

    /// Decides how the subscription that ended is opened again (`lagged`: it ended because it fell
    /// behind). The next subscription is counted from no events.
    pub(super) fn after_end(&mut self, lagged: bool) -> Resume {
        let delivered = std::mem::take(&mut self.delivered);
        if lagged || delivered {
            Resume::Now
        } else {
            Resume::AfterChange
        }
    }
}

/// The feed error recorded when a subscription ends without delivering any event.
pub(super) const NO_EVENTS: &str = "the feed ended without delivering events";

/// The local feed's loop: every event of `host.sessions` goes into the fleet, from the
/// subscription `subscribe` opens (given the last cursor delivered). Runs until the process
/// ends. Generic over the stream so the policy can be tested with scripted subscriptions.
async fn run_local_feed<S>(
    fleet: Fleet,
    host: HostId,
    mut subscribe: impl FnMut(Option<String>) -> S,
) where
    S: Stream<Item = Result<(String, HostEvent), StreamEnd>> + Unpin,
{
    let mut last: Option<String> = None;
    let mut resume = Resubscribe::default();
    loop {
        // Taken before the subscription opens, so a change from here on is never missed.
        let mut changes = fleet.changes();
        let mut s = subscribe(last.clone());
        let mut lagged = false;
        let mut errored = false;
        while let Some(item) = s.next().await {
            match item {
                Ok((c, ev)) => {
                    resume.delivered();
                    last = Some(c);
                    fleet.send(Input::Feed(host.clone(), ev)).await;
                }
                Err(StreamEnd::Lagged) => {
                    feed_record(&host, FeedEvent::Lagged, None);
                    lagged = true;
                    break;
                }
                Err(StreamEnd::Error(e)) => {
                    // The feed resubscribes; the text is the runtime's.
                    feed_record(&host, FeedEvent::Ended, Some(diag::foreign("stream", &e)));
                    errored = true;
                    break;
                }
            }
        }
        if resume.after_end(lagged) == Resume::AfterChange {
            // The error is recorded here only when the end was not already recorded above.
            if !errored {
                feed_record(
                    &host,
                    FeedEvent::Ended,
                    Some(diag::foreign("no_events", NO_EVENTS)),
                );
            }
            fleet
                .send(Input::FeedError(host.clone(), Some(NO_EVENTS.to_string())))
                .await;
            // The link, host or fleet state changes: the feed opens its subscription again. The
            // fleet is gone when this errs, and the feed ends with it.
            if changes.changed().await.is_err() {
                return;
            }
        }
    }
}

impl Fleet {
    pub fn spawn() -> Fleet {
        let (tx, mut rx) = mpsc::channel(INPUT_QUEUE);
        let (changes, _) = watch::channel(());
        let mut st = State {
            hosts: BTreeMap::new(),
            ring: Ring::new(),
            git: None,
            changes: changes.clone(),
        };
        tokio::spawn(async move {
            while let Some(i) = rx.recv().await {
                st.apply(i);
            }
        });
        Fleet { tx, changes }
    }

    /// Resolves each time a link state or a host changes, from the moment this receiver was taken
    /// (`changed`; it errs once the aggregator is gone). The feeds wait on it before they open a
    /// subscription that failed without delivering anything.
    pub(super) fn changes(&self) -> watch::Receiver<()> {
        self.changes.subscribe()
    }

    async fn send(&self, i: Input) {
        if self.tx.send(i).await.is_err() {
            fleet_events::aggregator_gone();
        }
    }

    pub async fn host_upserted(&self, entry: HostEntry, link: FleetLink) {
        self.send(Input::HostUpserted(Box::new(entry), link)).await
    }

    /// Show `prompt` to the user (it replaces the host's previous one).
    pub async fn show_prompt(&self, id: HostId, prompt: AuthPrompt) {
        self.send(Input::PromptShown(id, Box::new(prompt))).await
    }

    /// The prompt `prompt` of `id` is over (answered, cancelled, timed out, withdrawn): take it
    /// off, unless a newer prompt has replaced it.
    pub async fn end_prompt(&self, id: HostId, prompt: u64) {
        self.send(Input::PromptEnded(id, prompt)).await
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
            feed_record(&host, FeedEvent::Started, None);
            let subscribe = move |from: Option<String>| {
                stream::unfold(
                    manager.subscribe_host(from.as_deref()),
                    |mut s| async move {
                        let item = s.next().await?;
                        Some((item, s))
                    },
                )
                .boxed()
            };
            run_local_feed(fleet, host, subscribe).await
        })
    }

    /// Feed a remote host's `host.sessions` over its supervised link, for as long as the host is
    /// known (see [`Follow`]). A refusal is recorded and shown; the feed resubscribes when the
    /// link next connects, and that subscription starts with a snapshot.
    pub fn feed_remote(&self, host: HostId, link: Supervised) -> tokio::task::JoinHandle<()> {
        let fleet = self.clone();
        let mut follow = Follow::new(
            link.clone(),
            rt_topics::HOST_SESSIONS,
            None,
            link.state(),
            LinkState::is_connected,
            true,
        );
        tokio::spawn(async move {
            feed_record(&host, FeedEvent::Started, None);
            while let Some(step) = follow.next().await {
                match step {
                    Step::Event(v) => match serde_json::from_value::<HostEvent>(v) {
                        Ok(ev) => fleet.send(Input::Feed(host.clone(), ev)).await,
                        // The event is dropped; the text quotes the host's JSON.
                        Err(e) => feed_record(
                            &host,
                            FeedEvent::Undecodable,
                            Some(diag::foreign("decode", &e.to_string())),
                        ),
                    },
                    Step::Refused(e) => {
                        feed_record(
                            &host,
                            FeedEvent::Ended,
                            Some(diag::foreign(&e.code, &e.message)),
                        );
                        fleet
                            .send(Input::FeedError(
                                host.clone(),
                                Some(format!("{}: {}", e.code, e.message)),
                            ))
                            .await;
                    }
                }
            }
            // The link's state can no longer change (its supervisor is gone).
            feed_record(&host, FeedEvent::Ended, None);
        })
    }

    /// Forward the hub's link state changes (re-reading all states after a lag).
    pub fn feed_links(&self, hub: Hub) -> tokio::task::JoinHandle<()> {
        let fleet = self.clone();
        // Subscribed here, not in the task: no change between this call and the task's start is
        // missed.
        let mut rx = hub.events();
        tokio::spawn(async move {
            // The states as they are now (a link may have connected before this feed started).
            let states = hub.states().borrow().clone();
            for (h, s) in states {
                fleet.send(Input::Link(h, s)).await;
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream::BoxStream;
    use marsad_proto::hosts::HostKind;
    use std::sync::Arc;
    use std::sync::Mutex;

    type Item = Result<(String, HostEvent), StreamEnd>;

    #[test]
    fn a_lag_and_an_end_after_events_resume_at_once() {
        let mut r = Resubscribe::default();
        assert_eq!(r.after_end(true), Resume::Now);
        assert_eq!(r.after_end(true), Resume::Now);
        r.delivered();
        assert_eq!(r.after_end(false), Resume::Now);
    }

    /// An end that delivered nothing waits for a state change, and each subscription is judged on
    /// its own: an end after events does not carry over to the next one.
    #[test]
    fn an_end_that_delivered_nothing_waits_for_a_change() {
        let mut r = Resubscribe::default();
        assert_eq!(
            r.after_end(false),
            Resume::AfterChange,
            "an empty end waits"
        );
        r.delivered();
        assert_eq!(r.after_end(false), Resume::Now, "an end after events");
        assert_eq!(r.after_end(false), Resume::AfterChange, "counted from none");
    }

    /// How long a feed must stay closed after an empty end before the test takes that as "it did
    /// not reopen". A reopen without a state change would come at once (the policy has no timer), so
    /// this only bounds how long a correct feed is observed.
    const QUIET: Duration = Duration::from_millis(300);
    /// The bound on any step the feed is expected to reach.
    const REACHED: Duration = Duration::from_secs(5);

    /// A feed whose subscription ends without delivering anything records a feed error for its
    /// host and does not open the subscription again until a link state changes.
    #[tokio::test]
    async fn a_feed_that_ends_empty_waits_for_a_state_change() {
        let fleet = Fleet::spawn();
        let host = HostId::new("h");
        fleet
            .host_upserted(
                HostEntry {
                    id: host.clone(),
                    name: "h".to_string(),
                    kind: HostKind::Local,
                    platform: None,
                    created_at_ms: 0,
                    interactive_auth: false,
                    local_files: true,
                    shared_connection: false,
                    signed_out: false,
                    machine: None,
                },
                FleetLink::Local,
            )
            .await;
        let mut events = fleet.subscribe(None).await.expect("the fleet subscribes");
        let (opened_tx, mut opened) = tokio::sync::mpsc::unbounded_channel::<()>();
        let mut first = true;
        let subscribe = move |_from: Option<String>| -> BoxStream<'static, Item> {
            let _ = opened_tx.send(());
            if std::mem::replace(&mut first, false) {
                stream::empty().boxed()
            } else {
                stream::pending().boxed()
            }
        };
        let feed = tokio::spawn(run_local_feed(fleet.clone(), host.clone(), subscribe));

        assert_eq!(
            tokio::time::timeout(REACHED, opened.recv()).await,
            Ok(Some(())),
            "the feed opens its subscription"
        );
        // The failure is on record for the host (the fleet shows it).
        let recorded = tokio::time::timeout(REACHED, async {
            while let Some(item) = events.next().await {
                let (_, v) = item.expect("the fleet feed");
                if let Ok(FleetEvent::FeedError {
                    host: h,
                    error: Some(_),
                }) = serde_json::from_value(v)
                {
                    if h == host {
                        return true;
                    }
                }
            }
            false
        })
        .await;
        assert_eq!(
            recorded,
            Ok(true),
            "the empty end is recorded as a feed error"
        );

        assert!(
            tokio::time::timeout(QUIET, opened.recv()).await.is_err(),
            "the feed reopened without a state change"
        );

        // A link state change is what reopens it.
        fleet
            .send(Input::Link(host.clone(), LinkState::Connecting))
            .await;
        assert_eq!(
            tokio::time::timeout(REACHED, opened.recv()).await,
            Ok(Some(())),
            "the feed reopens after a link state change"
        );
        feed.abort();
    }

    /// Every subscription lags at once, 50 times over; the 51st waits for an event. Each lag must
    /// reopen the subscription without a pause: a pause per lag (doubling) would take far longer
    /// than the bound.
    #[tokio::test]
    async fn lags_resubscribe_without_sleeping() {
        const LAGS: usize = 50;
        let (tx, mut opened) = tokio::sync::mpsc::unbounded_channel::<usize>();
        let count = Arc::new(Mutex::new(0usize));
        let subscribe = move |_from: Option<String>| -> BoxStream<'static, Item> {
            let mut n = count.lock().unwrap();
            *n += 1;
            let _ = tx.send(*n);
            if *n <= LAGS {
                stream::iter(vec![Err(StreamEnd::Lagged)]).boxed()
            } else {
                stream::pending().boxed()
            }
        };
        let feed = tokio::spawn(run_local_feed(Fleet::spawn(), HostId::new("h"), subscribe));
        let reached = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(n) = opened.recv().await {
                if n == LAGS + 1 {
                    return n;
                }
            }
            0
        })
        .await;
        feed.abort();
        assert_eq!(reached, Ok(LAGS + 1), "resubscribes after lags were paused");
    }
}
