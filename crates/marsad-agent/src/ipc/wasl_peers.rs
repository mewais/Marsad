//! IPC handlers for dynamic Wasl peer registration (RegisterWaslPeer /
//! UnregisterWaslPeer). Best-effort: a call to the provider that fails is
//! logged to stderr but does NOT fail the IPC call — the connection add/remove
//! on the Core side is already committed, peer propagation is additive.

use marsad_wasl_session::WaslAgentCard;

use super::super::{Agent, AgentError};

impl Agent {
    /// Handle `RegisterWaslPeer`: register the peer described by `peer_id`,
    /// `peer_host`, and (optional) `peer_endpoint` on the node identified by
    /// `host_id`. Best-effort: logs errors and returns `Ok([])` regardless.
    pub(crate) fn handle_register_wasl_peer(
        &mut self,
        host_id: String,
        peer_id: String,
        peer_host: String,
        peer_endpoint: Option<String>,
    ) -> Result<Vec<String>, AgentError> {
        let card = WaslAgentCard {
            id: peer_id.clone(),
            host: peer_host.clone(),
            endpoint: peer_endpoint.clone(),
        };

        // Obtain the provider if one is registered; best-effort (log on absence).
        let result: Result<(), String> = self
            .wasl_session_provider
            .0
            .as_ref()
            .ok_or_else(|| {
                format!(
                    "no wasl session provider for host {host_id} — cannot register peer {peer_id}"
                )
            })
            .and_then(|provider| {
                provider
                    .register_peer(card)
                    .map_err(|e| format!("register_peer failed for host {host_id}: {e}"))
            });

        match result {
            Ok(()) => {
                eprintln!(
                    "marsad-agent: peer registered host={host_id} peer_id={peer_id} \
                     peer_host={peer_host} endpoint={peer_endpoint:?}"
                );
            }
            Err(msg) => {
                eprintln!("marsad-agent: RegisterWaslPeer best-effort error: {msg}");
            }
        }

        // Fire-and-forget — no ack to core (core already committed the connection).
        Ok(vec![])
    }

    /// Handle `UnregisterWaslPeer`: deregister the peer with `peer_id` from the
    /// node identified by `host_id`. Best-effort: logs errors and returns
    /// `Ok([])` regardless.
    pub(crate) fn handle_unregister_wasl_peer(
        &mut self,
        host_id: String,
        peer_id: String,
    ) -> Result<Vec<String>, AgentError> {
        let result: Result<bool, String> = self
            .wasl_session_provider
            .0
            .as_ref()
            .ok_or_else(|| {
                format!(
                    "no wasl session provider for host {host_id} — cannot unregister peer {peer_id}"
                )
            })
            .and_then(|provider| {
                provider
                    .unregister_peer(&peer_id)
                    .map_err(|e| format!("unregister_peer failed for host {host_id}: {e}"))
            });

        match result {
            Ok(existed) => {
                eprintln!(
                    "marsad-agent: peer unregistered host={host_id} peer_id={peer_id} existed={existed}"
                );
            }
            Err(msg) => {
                eprintln!("marsad-agent: UnregisterWaslPeer best-effort error: {msg}");
            }
        }

        // Fire-and-forget — no ack to core.
        Ok(vec![])
    }
}
