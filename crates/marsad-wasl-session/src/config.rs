//! The agent's production `wasl.json` config shape for the Wasl **Backend**
//! (Wasl-managed session) side — the direct structural analogue of X1
//! `marsad_wasl::{WaslOperatorEntry, WaslOperatorsConfig}` for the
//! runtime-control endpoint.
//!
//! Authoritative design: `docs/design/wasl-backend-plan.md` (v3) §6.1/§7. These
//! live here (the L1 crate) exactly as `marsad_wasl::WaslOperatorEntry`/
//! `WaslOperatorsConfig` live in the X1 L1 crate `marsad-wasl`. A daemon reaches
//! ONE runtime-control endpoint per host; `base_url` is the `http://ip:port`
//! shape [`crate::WaslSessionClientConfig::base_url`] (and
//! [`crate::RealWaslSessionProvider::new`]) consume.
//!
//! `#[serde(default)]` on `session_runtimes` and no `deny_unknown_fields`
//! (forward-compat, the X1 §2.1 convention): a `wasl.json` carrying only the X1
//! `operators` key (and none of `session_runtimes`) parses to an EMPTY runtimes
//! list — the backend is inert, not an error — and an unknown extra field on an
//! entry is tolerated.

use serde::{Deserialize, Serialize};

/// The `session_runtimes` block of the agent's `<data_root>/wasl.json`. The
/// direct analogue of `marsad_wasl::WaslOperatorsConfig` for the
/// runtime-control API. Fully independent of the X1 `operators` key: a file
/// with only `operators` deserializes to an empty `session_runtimes` (the inert
/// path), and vice-versa.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaslSessionRuntimesConfig {
    /// One runtime-control endpoint per host this daemon can drive Wasl-managed
    /// sessions on. `#[serde(default)]` so an absent key (e.g. an X1-only
    /// `wasl.json`) yields an empty list rather than a parse error.
    #[serde(default)]
    pub session_runtimes: Vec<WaslSessionRuntimeEntry>,
}

/// One Wasl runtime-control endpoint, keyed by the Marsad host-registry id. The
/// direct analogue of `marsad_wasl::WaslOperatorEntry` — but `base_url` reaches
/// the **runtime-control** API (very likely a different port/path on the SAME
/// physical Wasl node as the X1 operator API).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaslSessionRuntimeEntry {
    /// Marsad host-registry id this node belongs to ("local" for the local
    /// host).
    pub host_id: String,
    /// Runtime-control API base URL (`http://ip:port`, no trailing slash — the
    /// shape [`crate::WaslSessionClientConfig::base_url`] requires), reachable
    /// from the agent.
    pub base_url: String,
}
