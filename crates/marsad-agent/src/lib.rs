//! marsad-agent — the one daemon binary, the same on every machine.
//!
//! * `serve [--hub]` composes, in one `wasit_control::ServiceMux`, the Wasit
//!   `RuntimeService` (sessions) and Marsad's host services ([`host`]: git, pty, fs,
//!   agent-config base layer, daemon control). With `--hub` (the GUI machine's daemon) it also
//!   serves the hub services ([`hub`]: hosts + links + provisioning, fleet, store, effective
//!   config and session launch) and routes host-service requests to remote daemons.
//! * `bridge` pipes stdio to the local daemon (starting it if needed) — the remote end of a
//!   hub link (`ssh <host> marsad-agent bridge`).
//! * `boot` is the host's end of a hub's bootstrap ([`boot`], protocol in [`bootwire`]): it
//!   reports, carries out the hub's decision, and then is the bridge.
//! * `shim` is the per-session process holder (`wasit_shim::run`).
//! * `stop` asks the local daemon to exit (sessions keep running in their shims).
//! * `mcp` is the Wasl MCP server an agent CLI launches (`wasl_mcp::run`); the hub injects it
//!   into every session it launches (locked, provenance `automatic`, pre-allowed).
//! * Every daemon embeds a Wasl node (`wasl.*`, journal under `<data>/wasl/`); the hub also
//!   owns the workstream documents and pushes their topologies to the nodes
//!   ([`hub::workstreams`]).
//! * `diagnostics export` writes the diagnostics export (the file the app's Settings saves) for
//!   when the app cannot start ([`export_cli`]).
//! * `--version` prints [`marsad_proto::daemon::VersionInfo`] as one JSON line.

#![forbid(unsafe_code)]

pub mod askmcp;
pub mod boot;
pub mod bootwire;
pub mod cli;
pub mod diag;
pub mod export_cli;
pub mod host;
pub mod hub;
pub mod hub_events;
pub mod migrate;
pub mod serve;
pub mod util;
pub mod version;
