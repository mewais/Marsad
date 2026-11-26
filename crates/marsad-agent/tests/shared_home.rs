//! Several machines sharing one home directory (NFS-style), and adopting the flat layout of
//! earlier builds. The "machines" are localhost sshds whose logins share `HOME` but carry
//! their own machine key and runtime directory (`MARSAD_MACHINE_KEY`, `MARSAD_RUNTIME_DIR`).
#![cfg(unix)]

mod common;

use std::path::Path;
use std::time::Duration;

use common::*;
use marsad_client::{MarsadClient, Subscription};
use marsad_proto::agentconfig::{LaunchRequest, OverrideLayer};
use marsad_proto::fleet::FleetEvent;
use marsad_proto::hosts::HostDraft;
use marsad_proto::HostId;
use wasit_control::LinkState;
use wasit_fake_cli::scenario::{Scenario, Turn};

fn hub_client(dir: &Path) -> MarsadClient {
    let mut cfg = client_config(dir, wasit_control::Hello::new("test", "gui", "gui"));
    cfg.serve_args.extend(serve_args());
    cfg.supervise.backoff.initial = Duration::from_millis(50);
    MarsadClient::connect(cfg)
}

async fn until(
    f: &mut Subscription<FleetEvent>,
    what: &str,
    pred: impl Fn(&FleetEvent) -> bool,
) -> FleetEvent {
    let fut = async {
        loop {
            let (_, e) = next(f).await;
            if std::env::var_os("SHARED_HOME_DEBUG").is_some() {
                eprintln!("event: {e:?}");
            }
            if pred(&e) {
                return e;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(120), fut)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

fn idle_scenario() -> Scenario {
    Scenario::builder()
        .turn(Turn::builder().say("hello"))
        .build()
}

async fn launch(c: &MarsadClient, host: &HostId, cwd: &Path, env: Vec<(String, String)>) -> String {
    let mut req = LaunchRequest::new("claude");
    req.host = Some(host.clone());
    req.cwd = Some(cwd.to_string_lossy().into_owned());
    req.overrides = OverrideLayer {
        env,
        ..OverrideLayer::default()
    };
    c.launch(req).await.unwrap().id.to_string()
}

async fn session_ids(c: &MarsadClient, host: &HostId) -> Vec<String> {
    c.runtime(Some(host.clone()))
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.id.to_string())
        .collect()
}

// The localhost sshd runs on the Linux CI runners only.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_machines_sharing_a_home_keep_their_own_daemons_and_sessions() {
    let d = TempDir::new();
    let home = d.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let a = Sshd::start_machine(d.path(), "a", &home, "mach-a");
    let b = Sshd::start_machine(d.path(), "b", &home, "mach-b");
    let c = hub_client(&d.path().join("l"));
    let mut fleet = c.fleet(None);
    let (ha, hb) = (HostId::new("a"), HostId::new("b"));
    // Both added at once: they provision the shared helper cache concurrently.
    for (id, s) in [(&ha, &a), (&hb, &b)] {
        c.add_host(HostDraft {
            name: id.to_string(),
            kind: s.kind(&s.key),
            id: Some(id.clone()),
        })
        .await
        .unwrap();
    }
    // Each host counts once, from a `Link` event or from the `Reset` a fresh subscription starts
    // with (a host that connected before this wait is only in the `Reset`).
    let seen = std::sync::Mutex::new(std::collections::BTreeSet::new());
    until(&mut fleet, "a and b connected", |e| {
        for id in ["a", "b"] {
            if host_connected(e, id) {
                seen.lock().unwrap().insert(id);
            }
        }
        seen.lock().unwrap().len() == 2
    })
    .await;
    // Recorded from each hello (asynchronously).
    let hosts = tokio::time::timeout(T, async {
        loop {
            let hs = c.hosts().await.unwrap();
            if hs.iter().filter(|h| h.machine.is_some()).count() >= 3 {
                break hs;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("machine keys recorded");
    let key = |id: &HostId| hosts.iter().find(|h| &h.id == id).unwrap().machine.clone();
    assert_eq!(key(&ha).as_deref(), Some("mach-a"));
    assert_eq!(key(&hb).as_deref(), Some("mach-b"));
    // One helper, installed atomically (no temp files left behind).
    let bin = home.join(common_bin_rel());
    assert!(bin.is_file());
    let leftovers: Vec<_> = std::fs::read_dir(bin.parent().unwrap())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".new"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    // Two daemons, each with its endpoint in its own runtime directory.
    let ia = c.daemon_info(Some(&ha)).await.unwrap();
    let ib = c.daemon_info(Some(&hb)).await.unwrap();
    assert_ne!(ia.pid, ib.pid);
    assert!(ia.socket.starts_with(&*a.dir.join("run").to_string_lossy()));
    assert!(ib.socket.starts_with(&*b.dir.join("run").to_string_lossy()));
    assert_eq!(
        ia.data_dir,
        home.join(".marsad/machines/mach-a").display().to_string()
    );

    // A session on each; neither sees the other's.
    let work = home.join("w");
    std::fs::create_dir_all(&work).unwrap();
    let env = scenario_env(d.path(), &idle_scenario());
    let sa = launch(&c, &ha, &work, env.clone()).await;
    let sb = launch(&c, &hb, &work, env).await;
    let machines = home.join(".marsad/machines");
    assert!(machines
        .join("mach-a/sessions")
        .join(&sa)
        .join("session.json")
        .is_file());
    assert!(machines
        .join("mach-b/sessions")
        .join(&sb)
        .join("session.json")
        .is_file());
    assert!(!machines.join("mach-b/sessions").join(&sa).exists());
    // Locks and sockets are machine-local, not in the shared home.
    assert!(!machines
        .join("mach-a/sessions")
        .join(&sa)
        .join("shim.lock")
        .exists());
    let ra = std::fs::read_dir(a.dir.join("run"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(ra.join("sessions").join(&sa).join("shim.lock").is_file());
    assert!(ra.join("sessions").join(&sa).join("ctl.sock").exists());
    assert_eq!(session_ids(&c, &ha).await, vec![sa.clone()]);
    assert_eq!(session_ids(&c, &hb).await, vec![sb.clone()]);

    // Restart A's daemon: it re-adopts its own session only (the shim kept running).
    stop_daemon(ia.pid).await;
    until(&mut fleet, "a dropped", |e| {
        link_is(e, "a", |s| !s.is_connected())
    })
    .await;
    until(&mut fleet, "a back", |e| {
        link_is(e, "a", LinkState::is_connected)
    })
    .await;
    let ia2 = c.daemon_info(Some(&ha)).await.unwrap();
    assert_ne!(ia2.pid, ia.pid);
    assert_eq!(session_ids(&c, &ha).await, vec![sa.clone()]);
    let snap = c
        .runtime(Some(ha.clone()))
        .snapshot(&wasit_runtime::wire::SessionId::parse(&sa).unwrap())
        .await
        .unwrap();
    assert!(
        !matches!(snap.phase.name(), "lost" | "failed"),
        "{:?}",
        snap.phase
    );

    // The same machine added under a second name is recognised, not fought over.
    let a2 = HostId::new("a2");
    c.add_host(HostDraft {
        name: "a2".into(),
        kind: a.kind(&a.key),
        id: Some(a2.clone()),
    })
    .await
    .unwrap();
    until(&mut fleet, "a2 refused as a twin", |e| {
        link_is(e, "a2", |s| {
            // Parked with its reason: connecting again on a timer cannot change it.
            matches!(s, LinkState::Failed { reason, .. } if reason.contains("same machine as the host \"a\""))
        })
    })
    .await;
    assert_eq!(
        c.daemon_info(Some(&ha)).await.unwrap().pid,
        ia2.pid,
        "a's daemon kept"
    );
    c.remove_host(&a2).await.unwrap();

    for (h, s) in [(&ha, &sa), (&hb, &sb)] {
        c.runtime(Some(h.clone()))
            .delete(&wasit_runtime::wire::SessionId::parse(s).unwrap())
            .await
            .unwrap();
    }
}

/// A session started by an earlier build (flat layout: `<root>/sessions/<id>` with its lock
/// and socket inside) is adopted, still running, by the first start of the new layout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn flat_layout_with_a_live_session_is_adopted() {
    let d = TempDir::new();
    let root = d.path().join("m");
    let work = d.path().join("w");
    std::fs::create_dir_all(&work).unwrap();
    let id = legacy_session(&root, &work, d.path()).await;
    // A stale legacy daemon socket and lock (its daemon was killed) must not block anything.
    std::fs::write(root.join("marsad.sock"), "").unwrap();
    std::fs::write(root.join("marsad.sock.lock"), "").unwrap();

    let c = hub_client(&root);
    let layout = layout(&root);
    let rt = c.runtime(None);
    let ids = tokio::time::timeout(T, async {
        loop {
            if let Ok(l) = rt.list().await {
                break l;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        ids.iter().map(|s| s.id.to_string()).collect::<Vec<_>>(),
        vec![id.clone()]
    );
    assert!(layout
        .data
        .join("sessions")
        .join(&id)
        .join("journal.ndjson")
        .is_file());
    // The live shim's old path is a symlink to the claimed directory until it exits.
    let old = root.join("sessions").join(&id);
    assert!(std::fs::symlink_metadata(&old)
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(!root.join("marsad.sock").exists());
    // The adopted shim is still live: a prompt gets its answer.
    let sid = wasit_runtime::wire::SessionId::parse(&id).unwrap();
    let mut sub = rt.subscribe_session(&sid, None);
    if let Err(e) = rt.command(&sid, wasit_core::Command::prompt("go")).await {
        let snap = rt.snapshot(&sid).await;
        panic!(
            "prompt: {e:?}\nsnapshot: {snap:?}\nshim log: {}\ndaemon log: {}\ndaemon diagnostics: {}",
            std::fs::read_to_string(layout.data.join("sessions").join(&id).join("shim.log"))
                .unwrap_or_default(),
            std::fs::read_to_string(layout.log()).unwrap_or_default(),
            std::fs::read_to_string(layout.diag().join("daemon.ndjson")).unwrap_or_default()
        );
    }
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let Some(Ok((_, e))) = sub.next().await else {
                panic!("session stream ended");
            };
            if format!("{e:?}").contains("hello") {
                break;
            }
        }
    })
    .await
    .expect("the adopted session answers");
    rt.delete(&sid).await.unwrap();
}

/// A flat layout whose daemon runs on another machine (its lock is held, its socket does not
/// answer here) is left alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn foreign_flat_layout_is_left_alone() {
    let d = TempDir::new();
    let root = d.path().join("m");
    let work = d.path().join("w");
    std::fs::create_dir_all(&work).unwrap();
    let id = legacy_session(&root, &work, d.path()).await;
    // "Another machine" holds the legacy lock; nothing listens on the socket here.
    let lock = wasit_control::InstanceLock::try_acquire(root.join("marsad.sock.lock"))
        .await
        .unwrap();
    std::fs::write(root.join("marsad.sock"), "").unwrap();
    let c = hub_client(&root);
    let rt = c.runtime(None);
    let list = tokio::time::timeout(T, async {
        loop {
            if let Ok(l) = rt.list().await {
                break l;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    assert!(list.is_empty(), "{list:?}");
    assert!(root
        .join("sessions")
        .join(&id)
        .join("journal.ndjson")
        .is_file());
    assert!(root.join("marsad.sock").exists());
    drop(lock);
    pkill(&format!("{}/sessions", root.display()));
}

/// Start a session the way builds before the per-machine layout did (data and runtime files
/// together in `<root>/sessions/<id>`), and leave its shim running.
async fn legacy_session(root: &Path, work: &Path, scratch: &Path) -> String {
    // On its own runtime, shut down afterwards: the "old daemon" lets go of the shim.
    let (root, work, scratch) = (
        root.to_path_buf(),
        work.to_path_buf(),
        scratch.to_path_buf(),
    );
    tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let id = rt.block_on(legacy_session_inner(&root, &work, &scratch));
        rt.shutdown_timeout(Duration::from_secs(5));
        id
    })
    .await
    .unwrap()
}

async fn legacy_session_inner(root: &Path, work: &Path, scratch: &Path) -> String {
    let launcher = wasit_runtime::ShimLauncher::subcommand(agent(), "shim");
    let mut cfg = wasit_runtime::RuntimeConfig::new(root.to_path_buf(), launcher);
    cfg.derivation = "test-build".to_string();
    let o = cfg
        .overrides
        .entry(wasit_core::DriverId::new("claude"))
        .or_default();
    o.binary = Some(fake().display().to_string());
    o.env = scenario_env(scratch, &idle_scenario());
    let m = wasit_runtime::SessionManager::new(cfg).await.unwrap();
    let mut req = wasit_runtime::wire::CreateSession::new("claude");
    req.cwd = Some(work.to_string_lossy().into_owned());
    let id = m.create(req).await.unwrap().id;
    assert!(root
        .join("sessions")
        .join(id.as_str())
        .join("shim.lock")
        .is_file());
    id.to_string()
}
