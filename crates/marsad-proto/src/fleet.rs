//! `fleet` (hub): one subscription with everything a dashboard needs across all hosts.

use serde::{Deserialize, Serialize};
use wasit_control::{HostId, LinkState};
use wasit_runtime::wire::{SessionId, SessionSnapshot};

use crate::hosts::HostEntry;

/// The link of a host as the fleet reports it (the local host is always `connected`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "link", rename_all = "snake_case")]
pub enum FleetLink {
    Local,
    Remote { state: LinkState },
}

/// One host with its link and every session snapshot it reported last.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct FleetHost {
    pub host: HostEntry,
    pub link: FleetLink,
    /// `false` until the host's daemon delivered its session list over the current link
    /// (sessions are then the last known ones).
    pub sessions_live: bool,
    pub sessions: Vec<SessionSnapshot>,
}

/// Payload of the `fleet` topic. Apply in order; `reset` replaces everything.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FleetEvent {
    /// The whole fleet (first event of a fresh subscription or after an expired cursor).
    Reset {
        hosts: Vec<FleetHost>,
    },
    /// A host was added or its entry changed.
    HostUpserted {
        host: HostEntry,
    },
    HostRemoved {
        id: HostId,
    },
    /// A remote host's link state changed.
    Link {
        host: HostId,
        state: LinkState,
    },
    /// A host's daemon delivered its complete session list (after (re)connecting).
    Sessions {
        host: HostId,
        sessions: Vec<SessionSnapshot>,
    },
    /// A session was added or its snapshot changed (coalesced upstream: the latest snapshot
    /// is always delivered).
    SessionUpserted {
        host: HostId,
        session: Box<SessionSnapshot>,
    },
    SessionRemoved {
        host: HostId,
        id: SessionId,
    },
}
