//! marsad-agent — the one daemon binary, the same on every machine.
//!
//! * `serve [--hub]` composes, in one `wasit_control::ServiceMux`, the Wasit
//!   `RuntimeService` (sessions) and Marsad's host services ([`host`]: git, pty, fs,
//!   agent-config base layer, daemon control). With `--hub` (the GUI machine's daemon) it also
//!   serves the hub services ([`hub`]: hosts + links + provisioning, fleet, store, effective
//!   config and session launch) and routes host-service requests to remote daemons.
//! * `bridge` pipes stdio to the local daemon (starting it if needed) — the remote end of a
//!   hub link (`ssh <host> marsad-agent bridge`).
//! * `shim` is the per-session process holder (`wasit_shim::run`).
//! * `stop` asks the local daemon to exit (sessions keep running in their shims).
//! * `mcp` is reserved for the orchestration MCP server (not yet available).
//! * `--version` prints [`marsad_proto::daemon::VersionInfo`] as one JSON line.

#![forbid(unsafe_code)]

pub mod cli;
pub mod host;
pub mod hub;
pub mod serve;
pub mod util;
pub mod version;
