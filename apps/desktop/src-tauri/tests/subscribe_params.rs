//! A topic's parameters reach the service that serves it. The in-process demo service's topic
//! `count {limit?}` ends after `limit` events, so a stream that ends proves that the daemon got
//! the parameter, and one that goes on proves that none was sent.

use std::sync::Arc;

use marsad_client::{ClientConfig, MarsadClient};
use marsad_desktop_lib::bridge;
use serde_json::{json, Value};
use wasit_control::demo::DemoService;
use wasit_control::{ConnConfig, ControlServer, Hello};

async fn client(name: &str) -> MarsadClient {
    let dir = std::env::temp_dir().join(format!("mrsd-subparams-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("c.sock");
    let server = ControlServer::new(
        Arc::new(DemoService::new("local")),
        Hello::new("test", "local", "daemon"),
        ConnConfig::default(),
    );
    let listener = wasit_control::bind_local_exclusive(&sock).await.unwrap();
    tokio::spawn(async move { server.serve(listener).await });
    let mut cfg = ClientConfig::new(&dir, Hello::new("test", "marsad-desktop", "gui"));
    cfg.socket = Some(sock);
    cfg.start_daemon = false;
    MarsadClient::connect(cfg)
}

/// What a subscription delivers until it ends, or `cap` events.
async fn drain(mut sub: marsad_client::Subscription<Value>, cap: usize) -> (Vec<Value>, bool) {
    let mut got = Vec::new();
    while got.len() < cap {
        match sub.next().await {
            Some(Ok((_, v))) => got.push(v),
            Some(Err(e)) => panic!("subscription failed: {e}"),
            None => return (got, true),
        }
    }
    (got, false)
}

#[tokio::test]
async fn the_client_carries_a_topics_params_to_the_service() {
    let c = client("client").await;
    let (got, ended) = drain(
        c.subscribe_raw_with(None, "count", None, json!({"limit": 3})),
        50,
    )
    .await;
    assert_eq!(got, [json!(1), json!(2), json!(3)]);
    assert!(ended, "the service ended the topic at its limit");
    // The param-less subscribe is the same path with no parameters.
    let (got, ended) = drain(c.subscribe_raw(None, "count", None), 50).await;
    assert_eq!(got.len(), 50);
    assert!(!ended);
}

#[tokio::test]
async fn the_shells_subscribe_passes_params_and_a_cursor_to_the_service() {
    let c = client("shell").await;
    let t = || "count".to_string();
    // The webview's `params` (an object) arrive as given; `from` resumes after a cursor.
    let (got, ended) = drain(
        bridge::open_topic(&c, None, t(), None, Some(json!({"limit": 2}))),
        50,
    )
    .await;
    assert_eq!(got, [json!(1), json!(2)]);
    assert!(ended);
    let (got, ended) = drain(
        bridge::open_topic(&c, None, t(), Some(json!("5")), Some(json!({"limit": 7}))),
        50,
    )
    .await;
    assert_eq!(got, [json!(6), json!(7)]);
    assert!(ended);
    // Without `params` (or with null) the topic gets none.
    for params in [None, Some(Value::Null)] {
        let (got, ended) = drain(bridge::open_topic(&c, None, t(), None, params), 20).await;
        assert_eq!(got.len(), 20);
        assert!(!ended);
    }
}
