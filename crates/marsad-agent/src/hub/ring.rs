//! Replayable event topics owned by one actor (`fleet`, `workstreams`).
//!
//! The owning actor keeps the last [`RING`] events. A subscriber resuming from a cursor still
//! in the ring gets exactly the events after it; any other cursor (expired, another daemon
//! run) starts with the actor's `reset` event. Subscribing is a message to the actor, so the
//! replay/reset and the live receiver join with no gap. A subscriber that lags is closed with
//! `Lagged` and resumes from its cursor. Cursors are `"<epoch>.<n>"`.

use std::collections::VecDeque;

use futures::stream::StreamExt;
use serde::Serialize;
use tokio::sync::broadcast;
use wasit_control::{CloseReason, Cursor, EventStream, RpcError};

/// Events kept for replay.
pub const RING: usize = 4096;
const LIVE_QUEUE: usize = 1024;

/// What a subscriber starts with: the initial events (replay or reset), the live receiver
/// and the epoch.
pub type Joined<E> = (Vec<(String, E)>, broadcast::Receiver<(u64, E)>, String);

/// See the module docs.
pub struct Ring<E> {
    epoch: String,
    ring: VecDeque<(u64, E)>,
    next: u64,
    live: broadcast::Sender<(u64, E)>,
}

impl<E: Clone + Serialize + Send + 'static> Ring<E> {
    pub fn new() -> Self {
        Ring {
            epoch: format!("{:x}", fastrand::u64(..) >> 16),
            ring: VecDeque::new(),
            next: 0,
            live: broadcast::channel(LIVE_QUEUE).0,
        }
    }

    pub fn emit(&mut self, ev: E) {
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

    /// Join a subscriber resuming after `from`; `reset` builds the full-state event.
    pub fn join(&self, from: Option<&str>, reset: impl FnOnce() -> E) -> Joined<E> {
        let first = self.ring.front().map(|(n, _)| *n).unwrap_or(self.next + 1);
        let resume = from.and_then(|c| {
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
            None => vec![(self.cursor(self.next), reset())],
        };
        (initial, self.live.subscribe(), self.epoch.clone())
    }
}

impl<E: Clone + Serialize + Send + 'static> Default for Ring<E> {
    fn default() -> Self {
        Self::new()
    }
}

/// The topic stream of a joined subscriber (ends after a `Lagged` close).
pub fn stream<E: Clone + Serialize + Send + 'static>(joined: Joined<E>) -> EventStream {
    let (initial, live, epoch) = joined;
    let enc = |c: String, e: &E| {
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
                Err(broadcast::error::RecvError::Lagged(_)) => Some((Err(CloseReason::Lagged), rx)),
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
    futures::stream::iter(head).chain(tail).boxed()
}
