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
    /// Why the daemon link is not up: the last error while reconnecting, the reason it gave
    /// up (`offline`), or the version clash. Absent while connecting or connected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daemon_error: Option<String>,
    pub hosts: Vec<HostLink>,
}

impl Default for ConnectionState {
    fn default() -> Self {
        ConnectionState {
            daemon: "connecting",
            daemon_error: None,
            hosts: Vec::new(),
        }
    }
}

impl ConnectionState {
    /// The state for the daemon link `daemon` and the fleet's `hosts`.
    pub fn new(daemon: &LinkState, hosts: &[FleetHost]) -> Self {
        ConnectionState {
            daemon: link_word(daemon),
            daemon_error: link_detail(daemon).1,
            hosts: hosts.iter().map(host_link).collect(),
        }
    }
}

/// What a link state adds to its word: when the next attempt is due (relative, in
/// milliseconds) and why it is not up.
pub(crate) fn link_detail(state: &LinkState) -> (Option<u64>, Option<String>) {
    match state {
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
        LinkState::Failed { reason, .. } => (None, Some(reason.clone())),
        LinkState::Incompatible { ours, theirs } => (
            None,
            Some(format!("incompatible: ours {ours}, theirs {theirs}")),
        ),
        _ => (None, None),
    }
}

pub fn host_link(h: &FleetHost) -> HostLink {
    let (link, retry_in_ms, error) = match &h.link {
        FleetLink::Local => ("connected", None, None),
        FleetLink::Remote { state } => {
            let (retry, err) = link_detail(state);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_daemon_link_carries_its_reason() {
        let up = ConnectionState::new(&LinkState::Connecting, &[]);
        assert_eq!((up.daemon, up.daemon_error), ("connecting", None));
        let failed = ConnectionState::new(
            &LinkState::Failed {
                reason: "cannot start marsad-agent".into(),
                problem: None,
            },
            &[],
        );
        assert_eq!(failed.daemon, "offline");
        assert_eq!(
            failed.daemon_error.as_deref(),
            Some("cannot start marsad-agent")
        );
        let json = serde_json::to_value(ConnectionState::new(&LinkState::Connecting, &[])).unwrap();
        assert!(json.get("daemon_error").is_none());
    }
}
