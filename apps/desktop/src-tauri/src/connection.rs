//! What the webview knows about connections (`connection_state` / the `connection` event):
//! the link to the local daemon and every host's link, derived from the `fleet` topic.

use std::time::SystemTime;

use marsad_proto::fleet::{FleetHost, FleetLink};
use marsad_proto::hosts::HostKind;
use marsad_proto::LinkState;
use serde::Serialize;

use crate::bridge::link_word;

/// One host's link (matches `HostLink` in `apps/desktop/src/bridge/types.ts`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HostLink {
    pub id: String,
    pub name: String,
    /// `local`, `ssh` or `wsl`.
    #[serde(rename = "where")]
    pub where_: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    /// `connected`, `connecting`, `reconnecting` or `offline`.
    pub link: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_in_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Payload of `connection_state` and the `connection` event.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConnectionState {
    /// The link to the local daemon.
    pub daemon: &'static str,
    pub hosts: Vec<HostLink>,
}

impl Default for ConnectionState {
    fn default() -> Self {
        ConnectionState {
            daemon: "connecting",
            hosts: Vec::new(),
        }
    }
}

pub fn host_link(h: &FleetHost) -> HostLink {
    let (link, retry_in_ms, error) = match &h.link {
        FleetLink::Local => ("connected", None, None),
        FleetLink::Remote { state } => {
            let (retry, err) = match state {
                LinkState::Reconnecting {
                    next_at,
                    last_error,
                    ..
                } => (
                    Some(
                        next_at
                            .duration_since(SystemTime::now())
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0),
                    ),
                    Some(last_error.clone()),
                ),
                LinkState::AuthNeeded { detail, .. } => (None, Some(detail.clone())),
                LinkState::Failed { reason } => (None, Some(reason.clone())),
                LinkState::Incompatible { ours, theirs } => (
                    None,
                    Some(format!("incompatible: ours {ours}, theirs {theirs}")),
                ),
                _ => (None, None),
            };
            (link_word(state), retry, err)
        }
    };
    HostLink {
        id: h.host.id.to_string(),
        name: h.host.name.clone(),
        where_: match h.host.kind {
            HostKind::Local => "local",
            HostKind::Ssh { .. } => "ssh",
            HostKind::Wsl { .. } => "wsl",
        },
        os: h.host.platform.as_ref().map(|p| p.os.clone()),
        link,
        retry_in_ms,
        error,
    }
}
