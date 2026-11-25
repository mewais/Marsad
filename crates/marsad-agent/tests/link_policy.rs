//! What a remote daemon may call on the hub over its link: the Marsad allow-list
//! (`marsad_proto::remote_link_allows`) installed through the hub's `LinkGate`.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use marsad_proto::wasl::methods as wasl;
use serde_json::{json, Value};
use wasit_control::{
    codes, memory_pair, CallContext, CallError, ClientOptions, ConnConfig, ControlClient,
    ControlServer, Hello, HostId, LinkGate, RequestOpts, RpcError, Service,
};

const D: std::time::Duration = std::time::Duration::from_secs(10);

/// Stands in for the hub's routing service: records every call that gets through the gate.
#[derive(Default)]
struct Reached(Mutex<Vec<String>>);

#[async_trait]
impl Service for Reached {
    async fn call(&self, _ctx: CallContext, method: &str, _p: Value) -> Result<Value, RpcError> {
        self.0.lock().unwrap().push(method.to_string());
        Ok(json!("reached"))
    }
}

/// A hub (`hub`) whose link slot is gated the way the Marsad hub gates it, and a remote daemon
/// (`remote`) calling it over that link.
async fn remote_daemon_calls_hub() -> (ControlClient, Arc<Reached>) {
    let reached = Arc::new(Reached::default());
    let gate = LinkGate::new(
        HostId::new("hub"),
        reached.clone(),
        Arc::new(marsad_proto::remote_link_allows),
    );
    let server = ControlServer::new(
        Arc::new(gate),
        Hello::new("test-build", "hub", "hub"),
        ConnConfig::default(),
    );
    let (a, b) = memory_pair(64 * 1024);
    server.serve_connection(b);
    let client = ControlClient::connect(
        a,
        ClientOptions::new(Hello::new("test-build", "remote", "daemon")),
    )
    .await
    .expect("connect");
    (client, reached)
}

fn to(host: Option<&str>) -> RequestOpts {
    let mut o = RequestOpts::new(D);
    o.host = host.map(HostId::new);
    o
}

fn refusal(e: CallError) -> RpcError {
    match e {
        CallError::Rpc(r) => r,
        other => panic!("expected a typed refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn remote_daemon_is_refused_host_exec_start() {
    let (client, reached) = remote_daemon_calls_hub().await;
    let e = refusal(
        client
            .call("host.exec.start", json!({}), to(None))
            .await
            .unwrap_err(),
    );
    assert_eq!(e.code, codes::LINK_REFUSED);
    assert!(reached.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn remote_daemon_is_refused_forwarding_to_another_host() {
    let (client, reached) = remote_daemon_calls_hub().await;
    for method in ["host.exec.start", "pty.start", "fs.read", "sessions.launch"] {
        let e = refusal(
            client
                .call(method, json!({}), to(Some("other-host")))
                .await
                .unwrap_err(),
        );
        assert_eq!(e.code, codes::LINK_REFUSED, "{method}");
    }
    assert!(reached.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn remote_daemon_is_refused_the_hub_only_surfaces() {
    let (client, reached) = remote_daemon_calls_hub().await;
    // Host, store, hub-internal workstream and daemon surfaces: none is a route a node needs.
    for method in [
        "daemon.shutdown",
        "git.status",
        "store.recent.list",
        "hosts.list",
        "workstreams.spawn.plan",
        "workstreams.create",
        "diagnostics.export",
        "wasl.topology.set",
        "wasl.topology.get",
        "wasl.task.cancel",
    ] {
        let e = refusal(client.call(method, json!({}), to(None)).await.unwrap_err());
        assert_eq!(e.code, codes::LINK_REFUSED, "{method}");
    }
    assert!(reached.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn remote_node_spawn_authority_and_deliveries_still_work() {
    let (client, reached) = remote_daemon_calls_hub().await;
    // The remote node has no spawn authority: it asks the hub's.
    for method in [
        wasl::SUPERVISOR_SPAWN_PLAN,
        wasl::SUPERVISOR_SPAWN_LAUNCH,
        wasl::SUPERVISOR_DISMISS,
        wasl::SUPERVISOR_SPAWNABLE,
    ] {
        let v: String = client
            .request_with(method, json!({}), to(None))
            .await
            .unwrap();
        assert_eq!(v, "reached", "{method}");
    }
    // Node-to-node deliveries go through the hub to the peer's host.
    for method in [
        wasl::NODE_DELIVER,
        wasl::NODE_UPDATE,
        wasl::NODE_CANCEL,
        wasl::NODE_PEER_STATUS,
    ] {
        let v: String = client
            .request_with(method, json!({}), to(Some("peer-host")))
            .await
            .unwrap();
        assert_eq!(v, "reached", "{method}");
    }
    assert_eq!(reached.0.lock().unwrap().len(), 8);
}

#[tokio::test]
async fn remote_daemon_cannot_subscribe_on_the_hub() {
    let (client, _reached) = remote_daemon_calls_hub().await;
    let e = refusal(client.subscribe("fleet", None).await.unwrap_err());
    assert_eq!(e.code, codes::LINK_REFUSED);
}
