//! i16_wasl_a2a_live — headless proof of Marsad's MULTINODE Wasl path that the
//! other live tests do not cover:
//!
//!   * **S1 (gap 1) — dynamic peer registration, LIVE, deterministic.**
//!     Marsad's real client (`marsad-wasl-session::RealWaslSessionProvider`)
//!     registers / lists / overwrites / unregisters a peer against a REAL
//!     `wasl-mcpd` node's runtime-control API (`/v1/peers`). i15 proved the
//!     node's control API is reachable; x6 proved the send-gate mechanism with a
//!     STATICALLY-configured peer. Neither drove Marsad's *dynamic* peer
//!     registry against a live node — that is the hole S1 closes. Needs only a
//!     `wasl-mcpd` binary (no claude, no LLM, no ssh).
//!
//!   * **S2 (gap 2) — node→node A2A send with a send-gate answered through
//!     Marsad, against a DYNAMICALLY-registered peer.** (Added after S1 lands.)
//!
//! # SKIP contract
//! `skip_if_no_mcpd!()` prints a loud SKIP and returns when no `wasl-mcpd`
//! binary can be located (see [`find_wasl_mcpd`]) — never a silent pass.
//!
//! Recommended invocation (with a wasl-mcpd built in the sibling Wasl checkout,
//! or pointed at explicitly):
//! ```
//! MARSAD_WASL_MCPD=/path/to/wasl-mcpd \
//!   cargo test -p marsad-it-i16_wasl_a2a_live --test live -- --nocapture
//! ```
//!
//! # TEARDOWN contract
//! A `TeardownGuard` kills every node it spawned (dropping the `Child` closes the
//! piped stdin → the stdio-MCP loop sees EOF and the whole `wasl-mcpd` exits) and
//! removes the temp dir — on success OR panic.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use marsad_agent::{render_wasl_node_toml, WaslNodeConfig};
use marsad_wasl_session::{
    RealWaslSessionProvider, WaslAgentCard, WaslSessionClientConfig, WaslSessionProvider,
};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn unique_id() -> String {
    let c = COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{}-{}", std::process::id(), c)
}

/// Locate a `wasl-mcpd` binary: explicit `MARSAD_WASL_MCPD`, else the usual
/// build outputs in the sibling Wasl checkout (`MARSAD_WASL_REPO`, else the
/// nested `/work/mewais/Marsad/Wasl`). Prefers a release/native build, then the
/// static musl build (the remote-deploy artifact), then debug.
fn find_wasl_mcpd() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MARSAD_WASL_MCPD") {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    let repo = std::env::var("MARSAD_WASL_REPO")
        .unwrap_or_else(|_| "/work/mewais/Marsad/Wasl".to_string());
    for cand in [
        format!("{repo}/target/release/wasl-mcpd"),
        format!("{repo}/target/x86_64-unknown-linux-musl/release/wasl-mcpd"),
        format!("{repo}/target/debug/wasl-mcpd"),
    ] {
        let pb = PathBuf::from(&cand);
        if pb.is_file() {
            return Some(pb);
        }
    }
    None
}

macro_rules! skip_if_no_mcpd {
    () => {
        match find_wasl_mcpd() {
            Some(p) => p,
            None => {
                eprintln!(
                    "\n========================================================\n\
                     SKIP i16: no `wasl-mcpd` binary found.\n\
                     Set MARSAD_WASL_MCPD=/path/to/wasl-mcpd, or build it in the\n\
                     sibling Wasl checkout (`cargo build --release -p wasl-mcp`).\n\
                     ========================================================"
                );
                return;
            }
        }
    };
}

/// A free loopback TCP port (bind :0, read the assigned port, drop the listener).
/// Racy in theory, fine in practice for a short-lived local test.
fn get_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    listener.local_addr().expect("local_addr").port()
}

fn wait_tcp(addr: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if std::net::TcpStream::connect(addr).is_ok() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

struct TeardownGuard {
    tmpdir: PathBuf,
    children: Vec<Child>,
}

impl TeardownGuard {
    fn track(&mut self, child: Child) {
        self.children.push(child);
    }
}

impl Drop for TeardownGuard {
    fn drop(&mut self) {
        for child in &mut self.children {
            // Dropping/killing the child closes the piped stdin write end → the
            // node's stdio-MCP loop sees EOF and the whole wasl-mcpd exits.
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.tmpdir);
    }
}

/// One real `wasl-mcpd` node: control + a2a + operator binds, spawned with a
/// PIPED stdin held open by the returned `Child` (never taken/dropped) so the
/// stdio-MCP loop never sees EOF and the process stays up. Returns the child +
/// the `http://…` control base_url the runtime-control API (incl. `/v1/peers`)
/// answers on.
struct Node {
    child: Child,
    control_url: String,
    a2a_bind: String,
    #[allow(dead_code)]
    operator_bind: String,
    agent_id: String,
    host_id: String,
}

fn spawn_node(mcpd: &Path, tmpdir: &Path, label: &str) -> Node {
    let id = unique_id();
    let agent_id = format!("{label}-{id}");
    let host_id = format!("host-{label}-{id}");
    let control_port = get_free_port();
    let a2a_port = get_free_port();
    let operator_port = get_free_port();
    let control_url = format!("http://127.0.0.1:{control_port}");
    let a2a_bind = format!("127.0.0.1:{a2a_port}");
    let operator_bind = format!("127.0.0.1:{operator_port}");

    // Reuse Marsad's OWN known-good node-config renderer (i15 proved a node
    // spawned from it brings up the runtime-control API).
    let toml = render_wasl_node_toml(&WaslNodeConfig {
        agent_id: &agent_id,
        host_id: &host_id,
        a2a_bind: &a2a_bind,
        operator_bind: &operator_bind,
        control_bind: &control_url,
        default_protocol: "claude",
        claude_bin: None,
        opencode_bin: None,
    });
    let cfg_path = tmpdir.join(format!("config-{label}-{id}.toml"));
    std::fs::write(&cfg_path, &toml).expect("write node config");
    let log_path = tmpdir.join(format!("node-{label}-{id}.log"));
    let log = std::fs::File::create(&log_path).expect("node log file");
    let log_err = log.try_clone().expect("clone node log handle");

    // Neutral cwd (never a project checkout) — matches x6's finding that a real
    // agent spawned inside an agent-building project cwd trips prompt-injection
    // canaries; harmless here (no LLM) but keeps the node's cwd clean.
    let node_home = tmpdir.join(format!("home-{label}-{id}"));
    std::fs::create_dir_all(&node_home).expect("node home dir");

    let child = Command::new(mcpd)
        .current_dir(&node_home)
        .env("WASL_CONFIG", &cfg_path)
        .stdin(Stdio::piped()) // held open by Child → no EOF → node stays up
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err))
        .spawn()
        .unwrap_or_else(|e| panic!("spawn wasl-mcpd ({label}): {e}"));

    assert!(
        wait_tcp(&format!("127.0.0.1:{control_port}"), Duration::from_secs(20)),
        "node {label}: control API never bound :{control_port}; log: {:?}",
        std::fs::read_to_string(&log_path).unwrap_or_default()
    );

    Node {
        child,
        control_url,
        a2a_bind,
        operator_bind,
        agent_id,
        host_id,
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// S1 — dynamic peer registration against a real node (gap 1).
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn dynamic_peer_registration_against_live_node() {
    let mcpd = skip_if_no_mcpd!();

    let id = unique_id();
    let tmpdir = std::env::temp_dir().join(format!("marsad-it-i16-s1-{id}"));
    std::fs::create_dir_all(&tmpdir).expect("tmpdir");
    let mut guard = TeardownGuard {
        tmpdir: tmpdir.clone(),
        children: Vec::new(),
    };

    eprintln!("\n[i16/S1] spawning node A (real wasl-mcpd) with a runtime-control API");
    let node_a = spawn_node(&mcpd, &tmpdir, "alpha");
    eprintln!("[i16/S1] node A up; control API = {}", node_a.control_url);

    // Marsad's REAL client, dialed at the live node's control endpoint.
    let provider = RealWaslSessionProvider::new(WaslSessionClientConfig {
        base_url: node_a.control_url.clone(),
    });

    // 1. A fresh node has no peers.
    let peers = provider
        .list_peers()
        .expect("list_peers must reach the live node's /v1/peers");
    assert!(
        peers.is_empty(),
        "[S1] a fresh node must have zero peers; got {peers:?}"
    );
    eprintln!("[i16/S1] initial list_peers() -> 0 peers (reached live /v1/peers)");

    // 2. Register a peer (peer B, addressed at a loopback A2A endpoint).
    let peer_b_id = format!("beta-{id}");
    let endpoint1 = "http://127.0.0.1:59991/".to_string();
    provider
        .register_peer(WaslAgentCard {
            id: peer_b_id.clone(),
            host: "host-beta".to_string(),
            endpoint: Some(endpoint1.clone()),
        })
        .expect("[S1] register_peer must succeed against the live node");
    eprintln!("[i16/S1] register_peer({peer_b_id}) -> Ok");

    // 3. It is now readable, with the exact card we registered.
    let peers = provider.list_peers().expect("list_peers after register");
    assert_eq!(peers.len(), 1, "[S1] exactly one peer after register");
    assert_eq!(peers[0].id, peer_b_id, "[S1] peer id round-trips");
    assert_eq!(peers[0].host, "host-beta", "[S1] peer host round-trips");
    assert_eq!(
        peers[0].endpoint.as_deref(),
        Some(endpoint1.as_str()),
        "[S1] peer endpoint round-trips"
    );
    eprintln!("[i16/S1] list_peers() -> the registered peer (card round-trips live)");

    // 4. Idempotent overwrite: same id, new endpoint → replaces (not duplicates).
    let endpoint2 = "http://127.0.0.1:59992/".to_string();
    provider
        .register_peer(WaslAgentCard {
            id: peer_b_id.clone(),
            host: "host-beta".to_string(),
            endpoint: Some(endpoint2.clone()),
        })
        .expect("[S1] idempotent overwrite register_peer");
    let peers = provider.list_peers().expect("list_peers after overwrite");
    assert_eq!(
        peers.len(),
        1,
        "[S1] overwrite must NOT create a duplicate; got {peers:?}"
    );
    assert_eq!(
        peers[0].endpoint.as_deref(),
        Some(endpoint2.as_str()),
        "[S1] overwrite must update the endpoint to the new value"
    );
    eprintln!("[i16/S1] register_peer(same id) -> idempotent overwrite (1 peer, new endpoint)");

    // 5. Unregister: first call removed it (Ok(true)); second is a no-op (Ok(false)).
    let removed = provider
        .unregister_peer(&peer_b_id)
        .expect("[S1] unregister_peer must reach the node");
    assert!(removed, "[S1] first unregister must report the peer existed");
    let removed_again = provider
        .unregister_peer(&peer_b_id)
        .expect("[S1] second unregister must still be a clean Ok");
    assert!(
        !removed_again,
        "[S1] second unregister of an absent peer must be Ok(false), not an error"
    );
    let peers = provider.list_peers().expect("list_peers after unregister");
    assert!(
        peers.is_empty(),
        "[S1] node must have zero peers after unregister; got {peers:?}"
    );
    eprintln!("[i16/S1] unregister_peer -> Ok(true) then Ok(false); registry empty again");

    eprintln!(
        "\n[i16/S1] PASS — Marsad's dynamic peer registry (register/list/overwrite/unregister) \
         verified against a REAL wasl-mcpd node."
    );

    guard.track(node_a.child);
    // Silence unused-field warnings for fields S2 will use.
    let _ = (&node_a.a2a_bind, &node_a.agent_id, &node_a.host_id);
    drop(guard);
}
