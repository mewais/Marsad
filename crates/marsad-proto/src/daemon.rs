//! `daemon.*` and the `--version` line.

use serde::{Deserialize, Serialize};

/// What `marsad-agent --version` prints (one JSON line); also the result of `daemon.info`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct VersionInfo {
    /// Exact build identifier (compared to decide provisioning).
    pub build: String,
    /// Rust target triple the binary was built for.
    pub target: String,
}

/// Result of `daemon.info`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DaemonInfo {
    pub version: VersionInfo,
    pub host_id: String,
    pub hub: bool,
    pub data_dir: String,
    pub pid: u32,
    /// The daemon's own executable (also the `shim` and `mcp` entry).
    pub exe: String,
    /// The path of the endpoint it serves (on Windows the path its named pipe derives from).
    pub socket: String,
}

/// The readiness line `marsad-agent serve` prints on stdout once it accepts connections.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Readiness {
    pub ready: bool,
    pub pid: u32,
    pub endpoint: String,
    pub build: String,
}
