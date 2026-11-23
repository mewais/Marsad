//! The store under concurrent writers: no lost update, every document on disk always parses,
//! the state survives reopening, and a second process cannot open the same store.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use marsad_proto::agentconfig::{ItemOverride, LayerScope, OverrideAction, OverrideLayer};
use marsad_proto::store::{RecentProject, SeenMark};
use marsad_proto::workstream::{Workstream, WorkstreamTemplate};
use marsad_proto::{ConfigItemKind, HostId, SessionId};
use marsad_store::{Doc, Store, StoreError};

fn tmp() -> std::path::PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let p = std::env::temp_dir().join(format!(
        "marsad-store-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&p);
    p
}

fn sid(n: usize) -> SessionId {
    // 12 chars of the session-id alphabet.
    let a = b"abcdefghijklmnopqrstuvwxyz234567";
    SessionId(
        (0..12)
            .map(|i| a[(n >> (i % 4 * 5)) % 32] as char)
            .collect(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_writers_lose_nothing_and_files_never_tear() {
    let dir = tmp();
    let store = Store::open(&dir).await.unwrap();

    // A reader thread parses every document continuously while the writers run.
    let stop = Arc::new(AtomicBool::new(false));
    let reads = Arc::new(AtomicU64::new(0));
    let reader = {
        let (dir, stop, reads) = (dir.clone(), stop.clone(), reads.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                for d in Doc::ALL {
                    if let Ok(text) = std::fs::read_to_string(dir.join(d.file_name())) {
                        let v: serde_json::Value = serde_json::from_str(&text)
                            .unwrap_or_else(|e| panic!("{} torn: {e}", d.file_name()));
                        assert_eq!(v["version"], 1);
                        reads.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        })
    };

    let mut tasks = Vec::new();
    for w in 0..16usize {
        let s = store.clone();
        tasks.push(tokio::spawn(async move {
            for i in 0..25usize {
                s.set_setting(format!("k{w}-{i}"), serde_json::json!(i))
                    .await
                    .unwrap();
                s.mark_seen(vec![SeenMark {
                    host: HostId::new("h"),
                    session: sid(w * 100 + i),
                    cursor: i as u64,
                    at_ms: 0,
                }])
                .await
                .unwrap();
                s.add_recent(RecentProject {
                    host: HostId::new(format!("h{w}")),
                    path: format!("/p/{i}"),
                    last_used_ms: 0,
                })
                .await
                .unwrap();
            }
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    stop.store(true, Ordering::Relaxed);
    reader.join().unwrap();
    assert!(reads.load(Ordering::Relaxed) > 0);

    assert_eq!(store.settings().await.unwrap().values.len(), 16 * 25);
    assert_eq!(store.seen().await.unwrap().len(), 16 * 25);
    assert_eq!(
        store.recent().await.unwrap().len(),
        marsad_store::MAX_RECENT
    );
    store.close().await.unwrap();
    let store = Store::open(&dir).await.unwrap();
    assert_eq!(store.settings().await.unwrap().values.len(), 16 * 25);
    assert_eq!(store.seen().await.unwrap().len(), 16 * 25);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn second_open_is_refused_and_semantics_hold() {
    let dir = tmp();
    let store = Store::open(&dir).await.unwrap();
    assert!(matches!(
        Store::open(&dir).await,
        Err(StoreError::Locked(_))
    ));

    // Seen markers never move backwards.
    let m = |c| SeenMark {
        host: HostId::new("h"),
        session: sid(1),
        cursor: c,
        at_ms: 5,
    };
    store.mark_seen(vec![m(10)]).await.unwrap();
    let after = store.mark_seen(vec![m(3)]).await.unwrap();
    assert_eq!(after[0].cursor, 10);

    // Layers: set, read back, an empty layer removes it.
    let scope = LayerScope::Host {
        host: HostId::new("h"),
    };
    let layer = OverrideLayer {
        items: vec![ItemOverride {
            kind: ConfigItemKind::Skill,
            name: "x".into(),
            driver: None,
            action: OverrideAction::Disable,
        }],
        ..OverrideLayer::default()
    };
    store.set_layer(scope.clone(), layer.clone()).await.unwrap();
    assert_eq!(store.layer(scope.clone()).await.unwrap(), layer);
    store
        .set_layer(scope.clone(), OverrideLayer::default())
        .await
        .unwrap();
    assert!(store.layers().await.unwrap().is_empty());

    // Workstreams and templates: stored as given, survive a reopen.
    let mut ws = Workstream::new("ws", "Review");
    ws.revision = 3;
    store.put_workstream(ws.clone()).await.unwrap();
    let mut bad = ws.clone();
    bad.schema = 99;
    assert!(matches!(
        store.put_workstream(bad).await,
        Err(StoreError::Invalid(_))
    ));
    assert!(matches!(
        store.delete_workstream("nope".into()).await,
        Err(StoreError::NotFound(_))
    ));
    let tpl = WorkstreamTemplate {
        id: "t".into(),
        schema: marsad_proto::workstream::SCHEMA,
        name: "T".into(),
        description: String::new(),
        nodes: vec![],
        edges: vec![],
        limits: Default::default(),
        spawn: Default::default(),
        layout: Default::default(),
        created_at_ms: 1,
    };
    store.put_template(tpl.clone()).await.unwrap();
    store.close().await.unwrap();
    let store = Store::open(&dir).await.unwrap();
    assert_eq!(store.workstream("ws".into()).await.unwrap(), ws);
    assert_eq!(store.templates().await.unwrap(), vec![tpl]);
    store.delete_template("t".into()).await.unwrap();
    assert!(store.templates().await.unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn foreign_versions_are_refused() {
    let dir = tmp();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("hosts.json"), r#"{"version": 99, "data": []}"#).unwrap();
    assert!(matches!(
        Store::open(&dir).await,
        Err(StoreError::Version { found: 99, .. })
    ));
    let _ = std::fs::remove_dir_all(&dir);
}
