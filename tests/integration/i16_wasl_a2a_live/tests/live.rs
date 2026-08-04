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
    let _ = (&node_a.a2a_bind, &node_a.agent_id, &node_a.host_id);
    drop(guard);
}

// ══════════════════════════════════════════════════════════════════════════════
// S2 — node→node A2A send with a send-gate answered through Marsad, against a
// DYNAMICALLY-registered peer (gaps 1+2 combined, end-to-end).
//
// Topology (mirrors x6's Tier-3 leg, but the peer is registered at RUNTIME via
// Marsad's client rather than baked into node A's `[[peers]]` config):
//   * node B ("beta"): `[a2a]` receiver, real ClaudeAdapter — replies to consults.
//   * node A ("alpha"): `[operator]` + `[gate] authority = external-first` +
//     `[runtime] control_bind` (so Marsad can register the peer). Driven as a
//     real rmcp MCP client by THIS test.
//
// The flow proves, in one live pass:
//   1. Marsad's `register_peer` wires B into A at runtime (gap 1, USED for a send).
//   2. A `consult` to that peer raises an EXTERNAL gate that Marsad's real
//      operator connector (`marsad-wasl::WaslOperatorClient`) observes over SSE.
//   3. Marsad's `post_decision(approved)` releases the gate (gap 2).
//   4. Node A dispatches to B, B's real claude replies, and the reply (carrying
//      this run's probe word) comes back through the consult tool result.
//
// Needs claude + tmux-free memory headroom (like x6) + a wasl-mcpd binary; loud
// SKIP otherwise. Never a silent pass.
// ══════════════════════════════════════════════════════════════════════════════

use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use marsad_wasl::{WaslClientConfig, WaslEvent, WaslOperatorClient};
use rmcp::model::{CallToolRequestParam, CallToolResult, ClientInfo};
use rmcp::transport::TokioChildProcess;
use rmcp::{ClientHandler, Peer, RoleClient, ServiceExt};

/// A caps-less MCP client: no elicitation capability, so node A's
/// `external-first` authority routes the gate to the OPERATOR (not back to us).
#[derive(Clone)]
struct BareClient;
impl ClientHandler for BareClient {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::default()
    }
}

fn tool_text(res: &CallToolResult) -> String {
    res.content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .unwrap_or_default()
}

fn tool_is_error(res: &CallToolResult) -> bool {
    res.is_error.unwrap_or(false)
}

async fn call_tool(peer: &Peer<RoleClient>, name: &str, args: serde_json::Value) -> CallToolResult {
    peer.call_tool(CallToolRequestParam {
        name: name.to_string().into(),
        arguments: args.as_object().cloned(),
    })
    .await
    .unwrap_or_else(|e| panic!("call_tool({name}) transport error: {e}"))
}

fn claude_ok() -> bool {
    Command::new("claude")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Memory gate identical to x6/i10: claude needs ~700 MB RSS + a large virtual
/// address space (self-gated so the sandbox's 2 GB RLIMIT_AS run skips instead
/// of OOM-killing claude mid-turn).
fn enough_memory() -> bool {
    const MIN_AVAIL_KB: u64 = 3_000_000;
    const MIN_AS_BYTES: u64 = 6_000_000_000;
    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    for line in meminfo.lines() {
        if let Some(rest) = line.strip_prefix("MemAvailable:") {
            let kb: u64 = rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(u64::MAX);
            if kb < MIN_AVAIL_KB {
                return false;
            }
            break;
        }
    }
    let limits = std::fs::read_to_string("/proc/self/limits").unwrap_or_default();
    for line in limits.lines() {
        if line.starts_with("Max address space") {
            if let Some(val) = line.split_whitespace().nth(3) {
                if val == "unlimited" {
                    return true;
                }
                return val.parse::<u64>().map(|b| b >= MIN_AS_BYTES).unwrap_or(true);
            }
        }
    }
    true
}

/// Spawn node B ("beta"): an `[a2a]` receiver with a real ClaudeAdapter. Plain
/// `Command`, piped stdin held open by the returned `Child` (no EOF → stays up).
/// Neutral cwd (never a project checkout) per x6's prompt-injection finding.
fn spawn_node_b(mcpd: &Path, tmpdir: &Path, a2a_port: u16, id: &str) -> Child {
    let cfg = format!(
        "[identity]\n\
         agent_id = \"beta-{id}\"\n\
         host_id = \"hostB-{id}\"\n\
         \n\
         [a2a]\n\
         bind = \"127.0.0.1:{a2a_port}\"\n\
         \n\
         [audit]\n\
         path = {audit:?}\n\
         \n\
         [adapter]\n\
         kind = \"claude\"\n",
        audit = tmpdir.join(format!("audit-B-{id}.jsonl")).to_string_lossy(),
    );
    let cfg_path = tmpdir.join(format!("configB-{id}.toml"));
    std::fs::write(&cfg_path, cfg).expect("write configB");
    let home = tmpdir.join(format!("home-B-{id}"));
    std::fs::create_dir_all(&home).expect("node B home");
    let log = std::fs::File::create(tmpdir.join(format!("nodeB-{id}.log"))).expect("node B log");
    let log_err = log.try_clone().expect("clone node B log");
    Command::new(mcpd)
        .current_dir(&home)
        .env("WASL_CONFIG", &cfg_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err))
        .spawn()
        .expect("spawn node B (wasl-mcpd)")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dynamic_peer_consult_gate_answered_through_marsad() {
    let mcpd = skip_if_no_mcpd!();
    if !claude_ok() || !enough_memory() {
        eprintln!(
            "\n========================================================\n\
             SKIP i16/S2: needs `claude` on PATH + >=3 GB memory / >=6 GB address\n\
             space (node B runs a real claude turn to answer the consult).\n\
             ========================================================"
        );
        return;
    }

    let id = unique_id();
    let tmpdir = std::env::temp_dir().join(format!("marsad-it-i16-s2-{id}"));
    std::fs::create_dir_all(&tmpdir).expect("tmpdir");
    let mut guard = TeardownGuard {
        tmpdir: tmpdir.clone(),
        children: Vec::new(),
    };

    // ── node B: a2a receiver (real ClaudeAdapter) ────────────────────────────
    let b_a2a = get_free_port();
    eprintln!("\n[i16/S2] starting node B (a2a=:{b_a2a}) — real ClaudeAdapter receive side");
    let node_b = spawn_node_b(&mcpd, &tmpdir, b_a2a, &id);
    guard.track(node_b);
    assert!(
        wait_tcp(&format!("127.0.0.1:{b_a2a}"), StdDuration::from_secs(20)),
        "[S2] node B never bound its a2a port :{b_a2a}"
    );
    eprintln!("[i16/S2] node B a2a listening on :{b_a2a}");

    // ── node A config: control + a2a + operator + external-first gate ────────
    let a_control = get_free_port();
    let a_a2a = get_free_port();
    let a_operator = get_free_port();
    let control_url = format!("http://127.0.0.1:{a_control}");
    let operator_url = format!("http://127.0.0.1:{a_operator}");
    let mut toml_a = render_wasl_node_toml(&WaslNodeConfig {
        agent_id: &format!("alpha-{id}"),
        host_id: &format!("hostA-{id}"),
        a2a_bind: &format!("127.0.0.1:{a_a2a}"),
        operator_bind: &format!("127.0.0.1:{a_operator}"),
        control_bind: &control_url,
        default_protocol: "claude",
        claude_bin: None,
        opencode_bin: None,
    });
    // render_wasl_node_toml emits no [gate] block → default authority is
    // "elicitation" (gate goes back to the MCP client). Force external-first so
    // the gate surfaces on the OPERATOR API, where Marsad's connector decides it.
    toml_a.push_str("\n[gate]\nauthority = \"external-first\"\n");
    let cfg_a = tmpdir.join(format!("configA-{id}.toml"));
    std::fs::write(&cfg_a, &toml_a).expect("write configA");
    let home_a = tmpdir.join(format!("home-A-{id}"));
    std::fs::create_dir_all(&home_a).expect("node A home");

    // ── spawn node A as a real rmcp MCP client (stdio child transport) ───────
    eprintln!("[i16/S2] spawning node A as an rmcp child (control=:{a_control} operator=:{a_operator})");
    let mut cmd = tokio::process::Command::new(&mcpd);
    cmd.current_dir(&home_a).env("WASL_CONFIG", &cfg_a);
    let transport = TokioChildProcess::new(cmd).expect("spawn node A via rmcp");
    let client = BareClient
        .serve(transport)
        .await
        .expect("[S2] rmcp client must initialize against node A");
    assert!(
        wait_tcp(&format!("127.0.0.1:{a_control}"), StdDuration::from_secs(20)),
        "[S2] node A control API never bound :{a_control}"
    );
    assert!(
        wait_tcp(&format!("127.0.0.1:{a_operator}"), StdDuration::from_secs(20)),
        "[S2] node A operator API never bound :{a_operator}"
    );

    // ── gap 1 in-flow: dynamically register B as A's peer ────────────────────
    let peer_id = format!("beta-{id}");
    let provider = RealWaslSessionProvider::new(WaslSessionClientConfig {
        base_url: control_url.clone(),
    });
    provider
        .register_peer(WaslAgentCard {
            id: peer_id.clone(),
            host: format!("hostB-{id}"),
            endpoint: Some(format!("http://127.0.0.1:{b_a2a}/")),
        })
        .expect("[S2] register_peer must succeed against live node A");
    let peers = provider.list_peers().expect("[S2] list_peers");
    assert!(
        peers.iter().any(|p| p.id == peer_id),
        "[S2] dynamically-registered peer must be present; got {peers:?}"
    );
    eprintln!("[i16/S2] registered peer {peer_id} on node A at runtime (endpoint -> node B a2a)");

    // ── Marsad's REAL operator connector: SSE thread + decision channel ──────
    let (tx, rx) = mpsc::channel::<WaslEvent>();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_run = stop.clone();
    let op_url_run = operator_url.clone();
    let op_thread = std::thread::spawn(move || {
        let mut op = WaslOperatorClient::new(WaslClientConfig {
            base_url: op_url_run,
            initial_backoff_ms: 150,
        });
        op.run(tx, stop_run);
    });

    // ── trigger the consult (blocks until the operator releases the gate) ────
    let word = format!("build-{id}");
    let question = format!("Reply with exactly the single word {word} and nothing else.");
    eprintln!("[i16/S2] calling consult(peers=[{peer_id}]) — gate will surface to the operator");
    let consult = tokio::spawn(async move {
        call_tool(
            &client,
            "consult",
            serde_json::json!({ "peers": [peer_id], "question": question }),
        )
        .await
    });

    // ── observe the gate on Marsad's connector, then approve it ──────────────
    let op_url_dec = operator_url.clone();
    let gate_id = tokio::task::spawn_blocking(move || {
        let deadline = Instant::now() + StdDuration::from_secs(60);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                panic!("[S2] no GateRequested from Marsad's operator connector within 60s");
            }
            match rx.recv_timeout(remaining) {
                Ok(WaslEvent::GateRequested(gr)) if gr.decidable => {
                    eprintln!(
                        "[i16/S2] Marsad connector saw gate {} kind={} preview={:?}",
                        gr.gate_id, gr.request.kind, gr.request.preview
                    );
                    // Approve via Marsad's real one-shot decision POST.
                    let dec = WaslOperatorClient::new(WaslClientConfig {
                        base_url: op_url_dec.clone(),
                        initial_backoff_ms: 150,
                    });
                    let ack = dec
                        .post_decision(&gr.gate_id, "approved", None)
                        .expect("[S2] post_decision must reach the node");
                    eprintln!("[i16/S2] post_decision(approved) -> {ack:?}");
                    // Drain until we also see the matching GateDecided(approved).
                    let gate_id = gr.gate_id.clone();
                    let dd = Instant::now() + StdDuration::from_secs(30);
                    loop {
                        let rem = dd.saturating_duration_since(Instant::now());
                        if rem.is_zero() {
                            break;
                        }
                        match rx.recv_timeout(rem) {
                            Ok(WaslEvent::GateDecided(gd))
                                if gd.gate_id == gate_id && gd.outcome == "approved" =>
                            {
                                eprintln!("[i16/S2] Marsad connector saw gate_decided(approved)");
                                break;
                            }
                            Ok(_) => continue,
                            Err(_) => break,
                        }
                    }
                    return gate_id;
                }
                Ok(_) => continue,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    panic!("[S2] timed out waiting for GateRequested")
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("[S2] operator event channel disconnected before a gate")
                }
            }
        }
    })
    .await
    .expect("[S2] gate-observer task panicked");

    // ── the consult must now complete with node B's real reply ───────────────
    let res = tokio::time::timeout(StdDuration::from_secs(120), consult)
        .await
        .expect("[S2] consult did not complete within 120s after approval")
        .expect("[S2] consult task panicked");
    let text = tool_text(&res);
    assert!(
        !tool_is_error(&res),
        "[S2] consult returned an error after approval: {text:?}"
    );
    assert!(
        text.contains(&word),
        "[S2] consult result must carry node B's real reply (the probe word {word}); got {text:?}"
    );
    eprintln!("[i16/S2] consult delivered to the dynamic peer; reply carried {word:?}");

    stop.store(true, Ordering::SeqCst);
    let _ = op_thread.join();
    eprintln!(
        "\n[i16/S2] PASS (gate {gate_id}) — Marsad registered a peer at runtime, a consult to it \
         raised a gate Marsad's operator connector observed + approved, and node B's real reply \
         came back. Full dynamic-peer multinode loop verified."
    );
    drop(guard);
}
