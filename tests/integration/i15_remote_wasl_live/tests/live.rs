//! i15_remote_wasl_live — headless end-to-end proof of the REMOTE Wasl node path.
//!
//! This is the LIVE proof for Marsad's remote-host Wasl runtime-node feature:
//! `Agent::ensure_wasl_runtime_node(host_id, Some(<ssh_target>), …)` must
//!   1. render the node's `WASL_CONFIG` TOML (remote binds),
//!   2. durably spawn `wasl-mcpd` on the REMOTE host over ssh, holding stdin
//!      open so the stdio-MCP control task survives EOF (the crux recipe),
//!   3. open per-host `ssh -L` tunnels to its control/operator/a2a loopback binds,
//!   4. register the provider factory at the LOCAL control-tunnel endpoint, and
//!   5. be REACHABLE: a `RealWaslSessionProvider` dialed at the returned local
//!      base_url answers `GET /v1/targets` THROUGH the tunnel — programmatic
//!      proof (not a manual curl) that Marsad reached a real node on the host.
//!
//! Scenarios:
//!   S1 (the crux) — ensure_wasl_runtime_node(remote) → the node's control API is
//!      reachable over the tunnel: `provider.list()` returns Ok (an empty target
//!      list on a fresh node is a valid, reachable answer).
//!   S2 (stretch) — a second ensure call for the same host is idempotent (factory
//!      already registered → cheap no-op, reuses the live tunnels).
//!
//! # SKIP contract
//! `skip_if_no_deps!()` prints a loud SKIP and returns when:
//!   - `MARSAD_WASL_HOST` is not set (no remote SSH target configured)
//!   - `ssh` is not available locally
//!   - `wasl-mcpd` is not on PATH on the remote host (probed via ssh)
//!
//! Recommended invocation (against agent33, with wasl-mcpd deployed to its
//! ~/.local/bin per the deploy recipe):
//! ```
//! MARSAD_WASL_HOST=agent33 \
//!   cargo test -p marsad-it-i15_remote_wasl_live --test live -- --nocapture
//! ```
//!
//! # TEARDOWN contract
//! A `TeardownGuard` kills the remote node (`remote_wasl_node_kill_script`) and
//! removes its deployed config/log/pid, tears down the agent (which reaps the
//! ssh -L tunnel children on Drop), and removes the temp dir — on success OR panic.
//! The deployed `wasl-mcpd` binary is left in place (it simulates host setup and
//! is cleaned up by the operator harness, not the test).

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use marsad_agent::{
    remote_wasl_node_kill_script, run_remote_script, wasl_control_bind, wasl_local_tunnel_port,
    Agent, AgentConfig, ListenSpec, WaslBindKind,
};
use marsad_wasl_session::{RealWaslSessionProvider, WaslSessionClientConfig, WaslSessionProvider};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn unique_id() -> String {
    let c = COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{}-{}", std::process::id(), c)
}

fn wasl_host() -> Option<String> {
    std::env::var("MARSAD_WASL_HOST")
        .ok()
        .filter(|v| !v.trim().is_empty())
}

fn local_bin_ok(bin: &str, arg: &str) -> bool {
    Command::new(bin)
        .arg(arg)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Probe that `wasl-mcpd` is reachable on the remote host's login PATH. The node
/// binary has no `--version` guarantee, so we probe `command -v` (login shell).
fn remote_wasl_mcpd_available(host_target: &str) -> bool {
    let script = "command -v wasl-mcpd >/dev/null 2>&1 && echo WASL_MCPD_OK";
    match run_remote_script(host_target, script) {
        Some(out) => out.contains("WASL_MCPD_OK"),
        None => false,
    }
}

macro_rules! skip_if_no_deps {
    ($host:expr) => {
        if !local_bin_ok("ssh", "-V") {
            eprintln!(
                "\n========================================================\n\
                 SKIP i15: ssh not available locally.\n\
                 ========================================================"
            );
            return;
        }
        if !remote_wasl_mcpd_available($host) {
            eprintln!(
                "\n========================================================\n\
                 SKIP i15: `wasl-mcpd` not on PATH on remote host {:?}.\n\
                 Deploy a static (musl) wasl-mcpd to the remote ~/.local/bin\n\
                 (must be on the login PATH, like the agent CLIs).\n\
                 ========================================================",
                $host
            );
            return;
        }
    };
}

struct TeardownGuard {
    tmpdir: PathBuf,
    host_id: String,
    host_target: String,
}

impl Drop for TeardownGuard {
    fn drop(&mut self) {
        // Kill the remote durable node + remove its config/log/pid.
        if !self.host_id.is_empty() && !self.host_target.is_empty() {
            let script = remote_wasl_node_kill_script(&self.host_id);
            let _ = run_remote_script(&self.host_target, &script);
        }
        let _ = std::fs::remove_dir_all(&self.tmpdir);
        // The agent (dropped before this guard) reaps its ssh -L tunnel children.
    }
}

/// Build a real-IO Agent with an isolated data_root and NO listener socket
/// (stdio listen spec — we drive `ensure_wasl_runtime_node` directly, never
/// `serve()`). Uses `with_fakes` for the store/notify slots but the tunnel +
/// remote spawn paths are the REAL ones (they don't depend on the container).
fn make_agent(data_root: &str) -> Agent {
    Agent::with_real_io(AgentConfig {
        listen: ListenSpec::Stdio,
        data_root: data_root.to_string(),
    })
    .expect("Agent::with_real_io")
}

#[test]
fn remote_wasl_node_spawn_tunnel_and_reach() {
    let host_target = match wasl_host() {
        Some(h) => h,
        None => {
            eprintln!(
                "\n========================================================\n\
                 SKIP i15: MARSAD_WASL_HOST not set.\n\
                 Set it to an ssh alias / user@host with a deployed wasl-mcpd\n\
                 to run the remote Wasl node live test (e.g. agent33).\n\
                 ========================================================"
            );
            return;
        }
    };

    skip_if_no_deps!(&host_target);

    let id = unique_id();
    // A unique host_id per run so ports + remote config path don't collide with
    // a concurrent run or a stale node.
    let host_id = format!("i15-{id}");
    let tmpdir = std::env::temp_dir().join(format!("marsad-it-i15-{id}"));
    std::fs::create_dir_all(&tmpdir).expect("tmpdir");
    let data_root = tmpdir.join("data").to_string_lossy().to_string();
    std::fs::create_dir_all(&data_root).expect("data_root");

    let _guard = TeardownGuard {
        tmpdir: tmpdir.clone(),
        host_id: host_id.clone(),
        host_target: host_target.clone(),
    };

    let mut agent = make_agent(&data_root);

    eprintln!(
        "\n[i15/S1] ensure_wasl_runtime_node(host_id={host_id:?}, host_target=Some({host_target:?}))"
    );
    let base_url = agent
        .ensure_wasl_runtime_node(&host_id, Some(&host_target), &[], None)
        .expect("ensure_wasl_runtime_node(remote) must spawn the node + open the tunnel");

    // The returned base_url must be the LOCAL control tunnel endpoint, NOT the
    // remote/own control bind — that is the whole point of the tunnel.
    let expected_lport = wasl_local_tunnel_port(&host_id, WaslBindKind::Control);
    assert_eq!(
        base_url,
        format!("http://127.0.0.1:{expected_lport}"),
        "base_url must be the LOCAL control-tunnel endpoint"
    );
    assert_ne!(
        base_url,
        wasl_control_bind(&host_id),
        "base_url must NOT be the node's own remote control bind — it must be the tunnel endpoint"
    );
    eprintln!("[i15/S1] node up; local control tunnel endpoint = {base_url}");

    // ── THE CRUX: reach the node's control API THROUGH the tunnel ───────────
    // A fresh RealWaslSessionProvider dialed at the LOCAL tunnel endpoint must
    // reach the REMOTE node's `GET /v1/targets` and get a valid answer. An empty
    // target list on a fresh node IS a valid, reachable response — the proof is
    // that `list()` returns Ok (a connection failure would be Err).
    let provider = RealWaslSessionProvider::new(WaslSessionClientConfig {
        base_url: base_url.clone(),
    });
    // Give the tunnel + node a moment to fully settle (the spawn path already
    // sleeps ~0.9s; a short extra poll makes the test robust to a slow remote).
    let mut last_err = None;
    let mut reached = false;
    for attempt in 0..20 {
        match provider.list() {
            Ok(targets) => {
                eprintln!(
                    "[i15/S1] REACHED node control API over tunnel (attempt {attempt}): \
                     GET /v1/targets -> {} target(s)",
                    targets.len()
                );
                reached = true;
                break;
            }
            Err(e) => {
                last_err = Some(format!("{e:?}"));
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
    }
    assert!(
        reached,
        "S1 CRUX FAILED: could not reach the remote Wasl node's control API over the \
         ssh -L tunnel at {base_url}; last error: {last_err:?}"
    );
    eprintln!("[i15/S1] PASS — Marsad's remote path spawned + tunneled + REACHED a real node.");

    // ── S2 (stretch): a second ensure for the same host is idempotent ───────
    eprintln!("\n[i15/S2] second ensure_wasl_runtime_node — must be a cheap idempotent no-op");
    let base_url2 = agent
        .ensure_wasl_runtime_node(&host_id, Some(&host_target), &[], None)
        .expect("second ensure must succeed (factory already registered)");
    assert_eq!(
        base_url2, base_url,
        "second ensure must return the same local control tunnel endpoint"
    );
    // The node is still reachable (tunnels reused, not re-opened).
    assert!(
        provider.list().is_ok(),
        "S2: node must still be reachable over the reused tunnel"
    );
    eprintln!("[i15/S2] PASS — idempotent, tunnels reused, node still reachable.");

    eprintln!("\n[i15] ALL SCENARIOS PASSED — remote Wasl node spawned, tunneled, and reached.");

    // Explicitly drop the agent BEFORE the guard so its Drop reaps the ssh -L
    // tunnel children while the guard then kills the remote node.
    drop(agent);
}
