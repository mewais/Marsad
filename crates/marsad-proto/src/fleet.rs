//! `fleet` (hub): one subscription with everything a dashboard needs across all hosts.

use serde::{Deserialize, Serialize};
use wasit_control::{HostId, LinkState};
use wasit_runtime::wire::{SessionId, SessionSnapshot};

use crate::hosts::{AuthPrompt, HostEntry};

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
    /// A sign-in prompt waiting for the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_prompt: Option<AuthPrompt>,
    /// Git summaries of the sessions' working directories (one per directory).
    #[serde(default)]
    pub git: Vec<GitSummary>,
    /// Why the host's session feed was refused (the text says why); `None` once the host's
    /// sessions came through again. Until then `sessions` are the last known ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feed_error: Option<String>,
}

/// A cheap git summary of one working directory, kept fresh by the hub in the background:
/// refreshed (coalesced, rate-limited per directory, a bounded number at a time) when a
/// session in it starts, finishes a tool call, a turn or changes phase, and on
/// `fleet.git_refresh`. For list rows; the Changes view still asks `git.status` itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct GitSummary {
    pub cwd: String,
    /// `None` = not a git repository (see `error`) or not refreshed yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_root: Option<String>,
    /// `None` = detached HEAD (or not a repository).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// Changed paths (staged or not, untracked included).
    pub files: u32,
    pub added: u32,
    pub removed: u32,
    pub refreshed_at_ms: u64,
    /// Why there is no summary (not a repository, git missing, host unreachable…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `fleet.git_refresh` (hub): refresh one directory's [`GitSummary`] now (e.g. after a
/// commit); the result arrives as a `git` fleet event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct GitRefresh {
    pub host: HostId,
    pub cwd: String,
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
    /// A sign-in prompt appeared (`Some`) or was answered / cancelled / timed out (`None`).
    AuthPrompt {
        host: HostId,
        prompt: Option<AuthPrompt>,
    },
    /// A working directory's git summary was refreshed.
    Git {
        host: HostId,
        summary: GitSummary,
    },
    /// A host's session feed was refused (`Some`: why; the feed is retried when the host's link
    /// next connects) or its sessions came through again (`None`).
    FeedError {
        host: HostId,
        error: Option<String>,
    },
}
