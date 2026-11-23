//! `store.*` (hub): Marsad-level persistent data.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use wasit_control::HostId;
use wasit_runtime::wire::SessionId;

use crate::agentconfig::{LayerScope, OverrideLayer};

/// Free-form UI settings (`key → JSON value`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Settings {
    pub values: BTreeMap<String, Value>,
}

/// `store.settings.set` (`value: null` removes the key).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SettingSet {
    pub key: String,
    pub value: Value,
}

/// Which notifications the single notifier raises. A stored value without a key (written by
/// an older build) takes the key's default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default)]
pub struct NotificationPrefs {
    pub needs_you: bool,
    pub failed: bool,
    pub finished: bool,
    pub host_lost: bool,
    /// A host that asks for a sign-in at each connect (a code, a push approval) dropped, or the
    /// hub started without it connected: its link is parked, not retried, until the user
    /// connects it (a `link` fleet state `auth_needed` with `held`).
    pub sign_in_needed: bool,
}

impl Default for NotificationPrefs {
    fn default() -> Self {
        NotificationPrefs {
            needs_you: true,
            failed: true,
            finished: true,
            host_lost: true,
            sign_in_needed: true,
        }
    }
}

/// One stored override layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct StoredLayer {
    pub scope: LayerScope,
    pub layer: OverrideLayer,
}

/// Result of `store.layers.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct LayerList {
    pub layers: Vec<StoredLayer>,
}

/// `store.layers.set` (an empty layer removes it).
pub type LayerSet = StoredLayer;

/// When the user last looked at a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SeenMark {
    pub host: HostId,
    pub session: SessionId,
    /// The session event cursor seen up to.
    pub cursor: u64,
    pub at_ms: u64,
}

/// Result of `store.seen.list` / `store.seen.mark_all`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SeenMarkers {
    pub markers: Vec<SeenMark>,
}

/// A project directory used recently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RecentProject {
    pub host: HostId,
    pub path: String,
    /// Filled by the store when `0`.
    #[serde(default)]
    pub last_used_ms: u64,
}

/// Result of `store.recent.*` (most recent first, bounded to 50).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RecentProjects {
    pub projects: Vec<RecentProject>,
}
