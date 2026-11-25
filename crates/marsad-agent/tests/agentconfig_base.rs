//! `agentconfig.base` answers at once while discovery's probes are still running, reporting
//! that with `refreshing`; it does not hold the call open for them.

#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;
use std::time::{Duration, Instant};

use common::*;
use tokio::io::{AsyncBufReadExt, BufReader};

/// The claude binary is a script that runs for longer than any call should take, so every probe
/// of it is still running when the call is made.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn base_config_answers_at_once_while_probes_run() {
    let d = TempDir::new();
    let slow = d.path().join("slow-claude");
    std::fs::write(&slow, "#!/bin/sh\nexec sleep 25\n").unwrap();
    std::fs::set_permissions(&slow, std::fs::Permissions::from_mode(0o755)).unwrap();

    // Every home the daemon's drivers read is this test's own and empty: the owner's real
    // `~/.claude.json`, `~/.codex`, `~/.kilo` and `~/.marsad` must never be read by a test.
    let home = d.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    let data = d.path().join("d");
    // Appended after `serve_args`, whose claude binary it replaces.
    let mut daemon = tokio::process::Command::new(agent())
        .arg("serve")
        .arg("--hub")
        .args(data_args(&data))
        .args(serve_args())
        .arg("--driver-bin")
        .arg(format!("claude={}", slow.display()))
        .envs(isolated_home_env(&home).iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut line = String::new();
    tokio::time::timeout(
        T,
        BufReader::new(daemon.stdout.take().unwrap()).read_line(&mut line),
    )
    .await
    .expect("the daemon said it is ready")
    .unwrap();
    assert!(line.contains("ready"), "{line}");

    let c = client_of(&data);
    let began = Instant::now();
    let base = c
        .agentconfig_base(None, "claude", None, true)
        .await
        .unwrap();
    let took = began.elapsed();
    // Not the config itself in a message: it can carry credentials of the host.
    assert!(
        base.refreshing,
        "the probes are still running (refreshing: {}, {} items)",
        base.refreshing,
        base.items.len()
    );
    assert!(
        took < Duration::from_secs(10),
        "the call was held open for {took:?} while the probes ran"
    );
    end_own_daemon(daemon).await;
}
