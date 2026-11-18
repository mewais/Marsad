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

/// Which notifications the single notifier raises.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NotificationPrefs {
    pub needs_you: bool,
    pub failed: bool,
    pub finished: bool,
    pub host_lost: bool,
}

impl Default for NotificationPrefs {
    fn default() -> Self {
        NotificationPrefs {
            needs_you: true,
            failed: true,
            finished: true,
            host_lost: true,
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

/// A workstream definition: an opaque, versioned document (its schema belongs to the
/// orchestration layer, designed later).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WorkstreamDoc {
    pub id: String,
    /// Schema version of `doc`, chosen by its writer.
    pub schema: u32,
    /// Incremented by the store on every write.
    pub revision: u64,
    pub updated_at_ms: u64,
    pub doc: Value,
}

/// `store.workstreams.put`. With `expected_revision`, the write fails unless the stored
/// revision matches (optimistic concurrency between clients).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WorkstreamPut {
    pub id: String,
    pub schema: u32,
    pub doc: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
}

/// Names one workstream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WorkstreamRef {
    pub id: String,
}

/// Result of `store.workstreams.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WorkstreamList {
    pub workstreams: Vec<WorkstreamDoc>,
}
