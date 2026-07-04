//! x6_wasl_live — headless proof of the FULL live Wasl<->Marsad loop, minus
//! desktop chrome: a real `marsad-agent` (`Agent::with_real_io`) runs a Wasl
//! connector against a REAL `wasl-mcpd` node (built from the sibling `Wasl`
//! checkout), a real `marsad_core::Core` sits on the GUI side of the agent
//! socket, and a real `claude` answers the consult that the gate guards.
//!
//! # Topology (mirrors `Wasl/scripts/live-verify.sh`'s Tier-3 leg)
//! Node **B** ("beta") binds `[a2a]` only and serves a REAL `ClaudeAdapter` —
//! the peer Marsad's own node consults. Node **A** ("alpha") binds
//! `[operator]`, sets `[gate] authority = "external-first"`, and peers at B.
//! Node A is spawned as a *child of* `wasl-mcp`'s `live_client` example
//! (stdio MCP transport) exactly as the Wasl live-verify script drives it —
//! `live_client bare-consult` is the "simplest reliable trigger" the task
//! calls for. Marsad's Wasl connector (`marsad-wasl`) points at node A's
//! `[operator]` bind; that is the hop under test here — `marsad-wasl` and
//! the `serve()` wiring in `crates/marsad-agent/src/serve.rs` have unit/
//! loopback coverage (own `MockServer`) but had never been driven against a
//! real `wasl-mcpd` end to end before this suite.
//!
//! # SKIP contract
//! `skip_if_no_deps!()` prints a loud SKIP and returns if `claude`/`tmux` are
//! absent, memory is too tight to host claude, or the sibling `Wasl` repo
//! checkout (`MARSAD_WASL_REPO` env override, else `/work/mewais/OpenSource/Wasl`)
//! is not present — never a silent pass. On a machine with all three, this
//! test RUNS for real.
//!
//! # TEARDOWN contract
//! A `TeardownGuard` kills every process it spawned (node B, `live_client`
//! — whose exit tears down its node-A child via stdin EOF, same mechanism
//! `live-verify.sh` relies on for node B), sweeps any stray `wasl-mcpd`
//! sharing this run's binary path, sweeps `marsad-`-prefixed tmux sessions,
//! and removes the temp dir — on success or panic.
//!
//! # Budget
//! Every consult probe is phrased as a mundane "reply with exactly the
//! single word <build-tag>" instruction (never a tier name like "elicit"/
//! "external" in the token itself) — real claude, live-verified against this
//! phrasing, refuses tier-flavoured tokens as suspected prompt-injection
//! canaries but answers a bare build-tag exactly (see
//! `Wasl/scripts/live-verify.sh`'s `RAND_WORD` comment and this crate's own
//! `bare_consult_reply_word` probe below).

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use marsad_agent::{AgentConfig, ListenSpec};
use marsad_core::{Core, CoreConfig, LaunchMode, LaunchSpec, Posture, VmUpdate};
use marsad_events::Phase;
use marsad_gates::Decision;
use marsad_transport::{LocalIpc, Transport};
use marsad_wasl::{WaslClientConfig, WaslOperatorClient};

// ── test-local counter ──────────────────────────────────────────────────────

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn unique_id() -> String {
    let c = COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{}-{}", std::process::id(), c)
}

/// A mundane, tier-name-free build-tag token for exact-word consult probes.
fn build_tag() -> String {
    format!("build-{}", unique_id())
}

// ── memory + dependency gates (same thresholds as i03/i09/i10) ──────────────

const MIN_AVAIL_KB: u64 = 3_000_000;
const MIN_AS_BYTES: u64 = 6_000_000_000;

fn proc_address_space_limit_bytes() -> u64 {
    let content = match std::fs::read_to_string("/proc/self/limits") {
        Ok(c) => c,
        Err(_) => return u64::MAX,
    };
    for line in content.lines() {
        if line.starts_with("Max address space") {
            let mut fields = line.split_whitespace().skip(3);
            if let Some(val) = fields.next() {
                if val == "unlimited" {
                    return u64::MAX;
                }
                if let Ok(bytes) = val.parse::<u64>() {
                    return bytes;
                }
            }
        }
    }
    u64::MAX
}

fn enough_memory() -> bool {
    let content = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    for line in content.lines() {
        if line.starts_with("MemAvailable:") {
            let kb: u64 = line
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(u64::MAX);
            if kb < MIN_AVAIL_KB {
                return false;
            }
            break;
        }
    }
    proc_address_space_limit_bytes() >= MIN_AS_BYTES
}

/// The sibling `Wasl` checkout root: `MARSAD_WASL_REPO` env override, else
/// the path this worker was told the repo lives at.
fn wasl_repo_root() -> PathBuf {
    std::env::var("MARSAD_WASL_REPO")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/work/mewais/OpenSource/Wasl"))
}

fn deps_available() -> bool {
    let bin_ok = |bin: &str, arg: &str| {
        Command::new(bin)
            .arg(arg)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    bin_ok("claude", "--version")
        && bin_ok("tmux", "-V")
        && wasl_repo_root().join("Cargo.toml").is_file()
}

macro_rules! skip_if_no_deps {
    () => {
        if !deps_available() {
            eprintln!(
                "\n========================================================\n\
                 SKIP x6: claude and/or tmux not on PATH, or the sibling\n\
                 Wasl checkout (MARSAD_WASL_REPO, else /work/mewais/OpenSource/Wasl)\n\
                 is not present. This live test requires all three.\n\
                 ========================================================"
            );
            return;
        }
        if !enough_memory() {
            eprintln!(
                "\n========================================================\n\
                 SKIP x6: insufficient memory; claude needs ~700 MB RSS +\n\
                 large virtual space. Run on a host with >=3 GB available.\n\
                 ========================================================"
            );
            return;
        }
    };
}

// ── build the Wasl binaries this test drives ─────────────────────────────────

/// `cargo build --release -p wasl-mcp` (+ the `live_client` example driver).
/// A build failure is a hard test failure (deps_available already proved the
/// checkout exists), not a SKIP — mirrors `live-verify.sh` step "a".
fn build_wasl_binaries(repo: &Path) {
    for args in [
        vec!["build", "--release", "-p", "wasl-mcp"],
        vec![
            "build",
            "--release",
            "-p",
            "wasl-mcp",
            "--example",
            "live_client",
        ],
    ] {
        let status = Command::new("cargo")
            .args(&args)
            .current_dir(repo)
            .status()
            .unwrap_or_else(|e| panic!("[x6] spawn `cargo {}` failed: {e}", args.join(" ")));
        assert!(
            status.success(),
            "[x6] `cargo {}` failed in {}",
            args.join(" "),
            repo.display()
        );
    }
}

// ── TeardownGuard ────────────────────────────────────────────────────────────

struct TeardownGuard {
    tmpdir: PathBuf,
    socket_paths: Vec<PathBuf>,
    children: Vec<Child>,
    mcpd_bin: String,
}

impl TeardownGuard {
    fn track(&mut self, child: Child) {
        self.children.push(child);
    }
}

impl Drop for TeardownGuard {
    fn drop(&mut self) {
        // Kill every process we directly spawned (node B, `live_client` —
        // killing live_client closes the pipe its node-A child reads stdin
        // from, which per wasl-mcpd's design tears node A down too, the same
        // mechanism live-verify.sh documents for why node B needs a FIFO).
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
        // Belt-and-braces: any wasl-mcpd sharing this run's exact binary path
        // that somehow survived the kills above (e.g. a grandchild we never
        // held a handle to).
        let _ = Command::new("pkill")
            .args(["-f", &self.mcpd_bin])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        // Sweep any marsad-session-* tmux sessions spawned by the
        // optionality leg's real claude session.
        let _ = Command::new("sh")
            .arg("-c")
            .arg("tmux list-sessions -F '#{session_name}' 2>/dev/null | grep '^marsad-session-' | xargs -r -n1 tmux kill-session -t")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        for p in &self.socket_paths {
            let _ = std::fs::remove_file(p);
        }
        let _ = std::fs::remove_dir_all(&self.tmpdir);
    }
}

// ── harness helpers (same shape as i10) ──────────────────────────────────────

fn poll_connect(socket: &str, timeout: Duration) -> Result<LocalIpc, String> {
    let deadline = Instant::now() + timeout;
    loop {
        let mut client = LocalIpc::new(socket);
        if client.connect().is_ok() {
            return Ok(client);
        }
        if Instant::now() >= deadline {
            return Err(format!("socket {socket} not ready in time"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn start_reader(socket: &str) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    let socket = socket.to_string();
    std::thread::spawn(move || {
        let mut ipc = match poll_connect(&socket, Duration::from_secs(10)) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[x6] reader connect failed: {e}");
                return;
            }
        };
        let sub = serde_json::json!({ "type": "Subscribe", "session_id": "" }).to_string();
        if ipc.send_line(&sub).is_err() {
            return;
        }
        while let Ok(Some(line)) = ipc.recv_line() {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

/// Pump broadcast lines into `Core` until `done(updates)` is true or `secs`
/// elapse, returning both the `VmUpdate`s and the RAW lines seen during this
/// call (the optionality leg needs the raw wire text to prove the absence of
/// any `"type":"Wasl…"` tag, not just the absence of a parsed `VmUpdate`).
fn pump_capture<F>(
    core: &mut Core,
    rx: &Receiver<String>,
    secs: u64,
    mut done: F,
) -> (Vec<VmUpdate>, Vec<String>)
where
    F: FnMut(&[VmUpdate]) -> bool,
{
    let mut all: Vec<VmUpdate> = Vec::new();
    let mut raw: Vec<String> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if done(&all) {
            break;
        }
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        match rx.recv_timeout(deadline - now) {
            Ok(line) => {
                raw.push(line.clone());
                if let Ok(updates) = core.on_agent_line(&line) {
                    all.extend(updates);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                eprintln!("[x6] pump: reader disconnected");
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => break,
        }
    }
    (all, raw)
}

/// The live model: `MARSAD_LIVE_MODEL` env override, else `haiku` (the cheapest
/// live model, the original default). An override is needed on gateways where a
/// given model's deployment rejects fields the CLI sends — e.g. AMD's gateway
/// 400s `haiku` on `output_config.effort` but accepts `sonnet`.
fn live_model() -> String {
    std::env::var("MARSAD_LIVE_MODEL").unwrap_or_else(|_| "haiku".to_string())
}

fn spec(project: &str, first_prompt: &str) -> LaunchSpec {
    LaunchSpec {
        project: project.to_string(),
        host_id: "local".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: live_model(),
        posture: Posture::FullAutonomy,
        first_prompt: Some(first_prompt.to_string()),
        mode: LaunchMode::Fresh,
        name: None,
        disallowed_tools: vec![],
        wasl: false,
    }
}

fn git_init(dir: &str) {
    for args in [
        vec!["-C", dir, "init", "-q"],
        vec!["-C", dir, "config", "user.email", "x6@test"],
        vec!["-C", dir, "config", "user.name", "x6"],
    ] {
        let _ = Command::new("git")
            .args(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

// ── Wasl-node plumbing ────────────────────────────────────────────────────────

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

fn wait_child(child: &mut Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Some(status);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Node B ("beta"): `[a2a]` receive server only, real `ClaudeAdapter`. Spawned
/// with a FIFO held read-write as stdin so EOF never reaches its stdio MCP
/// loop — per `live-verify.sh`'s documented finding, EOF on stdin tears the
/// WHOLE `wasl-mcpd` process down, including the A2A receive server node B
/// exists to run. Also run from a NEUTRAL cwd (never this repo checkout, nor
/// Wasl's) — same script's finding: a real claude spawned with cwd inside a
/// project whose own history is "build an agent-to-agent consult/gate
/// system" pattern-matches a prompt-injection canary and refuses build-tag
/// echo probes it answers normally from a neutral directory.
fn spawn_node_b(mcpd_bin: &str, config_path: &Path, tmpdir: &Path, log_path: &Path) -> Child {
    let fifo_path = tmpdir.join("b.stdin.fifo");
    let status = Command::new("mkfifo")
        .arg(&fifo_path)
        .status()
        .expect("spawn mkfifo");
    assert!(status.success(), "mkfifo {} failed", fifo_path.display());
    let keepalive = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&fifo_path)
        .expect("open fifo O_RDWR");
    let child_stdin = keepalive
        .try_clone()
        .expect("clone fifo fd for child stdin");
    // Leak the keepalive handle deliberately: it must outlive this fn (its
    // sole purpose is to keep a writer open on the FIFO for the lifetime of
    // node B, which this fn does not own past its own return). The fd is
    // reclaimed when the test process exits; `TeardownGuard` kills node B
    // itself well before then.
    std::mem::forget(keepalive);

    let node_b_home = tmpdir.join("node-b-home");
    std::fs::create_dir_all(&node_b_home).expect("node-b-home dir");

    let log = std::fs::File::create(log_path).expect("node B log file");
    let log_err = log.try_clone().expect("clone node B log handle");

    Command::new(mcpd_bin)
        .current_dir(&node_b_home)
        .env("WASL_CONFIG", config_path)
        .stdin(Stdio::from(child_stdin))
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err))
        .spawn()
        .expect("spawn node B (wasl-mcpd)")
}

// ══════════════════════════════════════════════════════════════════════════════
// The full loop.
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn full_loop() {
    skip_if_no_deps!();

    let repo = wasl_repo_root();
    eprintln!(
        "[x6] building wasl-mcp + live_client from {}",
        repo.display()
    );
    build_wasl_binaries(&repo);
    let mcpd_bin = repo.join("target/release/wasl-mcpd");
    let live_client_bin = repo.join("target/release/examples/live_client");
    assert!(mcpd_bin.is_file(), "wasl-mcpd binary missing after build");
    assert!(
        live_client_bin.is_file(),
        "live_client example binary missing after build"
    );
    let mcpd_bin_str = mcpd_bin.to_string_lossy().to_string();
    let live_client_str = live_client_bin.to_string_lossy().to_string();

    let id = unique_id();
    let tmpdir = std::env::temp_dir().join(format!("marsad-it-x6-{id}"));
    std::fs::create_dir_all(&tmpdir).expect("tmpdir");

    let mut guard = TeardownGuard {
        tmpdir: tmpdir.clone(),
        socket_paths: Vec::new(),
        children: Vec::new(),
        mcpd_bin: mcpd_bin_str.clone(),
    };

    // ── Two-node Wasl topology (mirrors live-verify.sh's Tier-3 leg) ──────────
    let port_b = get_free_port();
    let port_op = get_free_port();

    let config_b = tmpdir.join("configB.toml");
    std::fs::write(
        &config_b,
        format!(
            "[identity]\n\
             agent_id = \"beta\"\n\
             host_id  = \"hostB\"\n\
             \n\
             [a2a]\n\
             bind = \"127.0.0.1:{port_b}\"\n\
             \n\
             [audit]\n\
             path = \"{audit}\"\n\
             \n\
             [adapter]\n\
             kind = \"claude\"\n",
            port_b = port_b,
            audit = tmpdir.join("audit-B.jsonl").to_string_lossy(),
        ),
    )
    .expect("write configB.toml");

    let config_a = tmpdir.join("configA.toml");
    std::fs::write(
        &config_a,
        format!(
            "[identity]\n\
             agent_id = \"alpha\"\n\
             host_id  = \"hostA\"\n\
             \n\
             [gate]\n\
             authority = \"external-first\"\n\
             \n\
             [operator]\n\
             bind = \"127.0.0.1:{port_op}\"\n\
             \n\
             [audit]\n\
             path = \"{audit}\"\n\
             \n\
             [[peers]]\n\
             agent_id = \"beta\"\n\
             host_id  = \"hostB\"\n\
             url      = \"http://127.0.0.1:{port_b}/\"\n",
            port_op = port_op,
            port_b = port_b,
            audit = tmpdir.join("audit-A.jsonl").to_string_lossy(),
        ),
    )
    .expect("write configA.toml");

    eprintln!("\n[x6/hop1] starting node B (a2a=:{port_b}) — real ClaudeAdapter receive side");
    let node_b_log = tmpdir.join("nodeB.log");
    let node_b_child = spawn_node_b(&mcpd_bin_str, &config_b, &tmpdir, &node_b_log);
    guard.track(node_b_child);
    assert!(
        wait_tcp(&format!("127.0.0.1:{port_b}"), Duration::from_secs(20)),
        "[hop1] node B never bound its [a2a] port :{port_b}; log: {:?}",
        std::fs::read_to_string(&node_b_log).unwrap_or_default()
    );
    eprintln!("[x6/hop1] PASS — node B's A2A receive server is listening on :{port_b}");

    // ── marsad-agent, real IO, with a Wasl connector pointed at node A ────────
    let socket_path = tmpdir.join("agent.sock");
    let socket = socket_path.to_string_lossy().to_string();
    guard.socket_paths.push(socket_path.clone());
    let project = tmpdir.join("project");
    std::fs::create_dir_all(&project).expect("project dir");
    let project = project.to_string_lossy().to_string();
    git_init(&project);
    let data_root = tmpdir.join("data").to_string_lossy().to_string();
    std::fs::create_dir_all(&data_root).expect("data_root dir");

    // The agent's Wasl config: one operator entry, host_id "local", pointed
    // at node A's [operator] bind (§2.7's wasl.json shape). Written BEFORE
    // the agent thread starts — `serve()` reads it exactly once, at startup.
    std::fs::write(
        Path::new(&data_root).join("wasl.json"),
        serde_json::json!({
            "operators": [
                { "host_id": "local", "url": format!("http://127.0.0.1:{port_op}") }
            ]
        })
        .to_string(),
    )
    .expect("write wasl.json");

    let socket_t = socket.clone();
    let data_root_t = data_root.clone();
    std::thread::spawn(move || {
        let cfg = AgentConfig {
            listen: ListenSpec::UnixSocket(socket_t),
            data_root: data_root_t,
        };
        match marsad_agent::Agent::with_real_io(cfg) {
            Ok(mut agent) => {
                // Snappier reconnect than the 1s production default, purely
                // to keep this test's wall-clock bounded — the documented
                // `set_wasl_connector_factory` test seam, real client.
                agent.set_wasl_connector_factory(Box::new(|entry| {
                    Box::new(WaslOperatorClient::new(WaslClientConfig {
                        base_url: entry.url.clone(),
                        initial_backoff_ms: 150,
                    }))
                }));
                if let Err(e) = agent.serve() {
                    eprintln!("[x6] agent serve error: {e}");
                }
            }
            Err(e) => eprintln!("[x6] Agent::with_real_io failed: {e}"),
        }
    });

    let conn = poll_connect(&socket, Duration::from_secs(10)).expect("Core connect");
    let cfg = CoreConfig {
        agent_endpoint: socket.clone(),
        data_root: data_root.clone(),
    };
    let mut core = Core::new(cfg, Box::new(conn)).expect("Core::new");

    let rx = start_reader(&socket);
    let _ack = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("Subscribed ack before driving a consult");

    // ─────────────────────────────────────────────────────────────────────────
    // hop 2 — a real claude session (proves the agent/core rails still carry
    // ordinary traffic alongside the Wasl connector; sequenced before the
    // consult so its tmux session doesn't race the Wasl-gate assertions).
    // ─────────────────────────────────────────────────────────────────────────
    eprintln!("\n[x6/hop2] real marsad-agent + Core session, sanity leg");
    let id_a = core
        .launch_session(spec(
            &project,
            "Reply with exactly the single word READY and nothing else.",
        ))
        .expect("launch session");
    let (updates, _raw) = pump_capture(&mut core, &rx, 120, |u| {
        u.iter().any(|x| match x {
            VmUpdate::Session(sv) if sv.id == id_a => sv.state == Phase::Done,
            _ => false,
        })
    });
    let done = updates.iter().any(|x| match x {
        VmUpdate::Session(sv) if sv.id == id_a => sv.state == Phase::Done,
        _ => false,
    });
    assert!(done, "[hop2] session {id_a} never reached Done");
    eprintln!("[x6/hop2] PASS — real marsad-agent <-> Core <-> claude session completed");

    // ─────────────────────────────────────────────────────────────────────────
    // hop 3 — drive a consult INTO node A (external-first gate) via
    // `live_client bare-consult`; the Tier-3 gate must surface in Marsad as
    // a `VmUpdate::WaslGates` pending entry.
    // ─────────────────────────────────────────────────────────────────────────
    eprintln!("\n[x6/hop3] driving a Tier-3 consult into node A via live_client");
    let word = build_tag();
    let question = format!("Reply with exactly the single word {word} and nothing else.");
    let live_stdout_path = tmpdir.join("live_client.stdout");
    let live_stderr_path = tmpdir.join("live_client.stderr");
    let live_stdout = std::fs::File::create(&live_stdout_path).expect("live_client stdout file");
    let live_stderr = std::fs::File::create(&live_stderr_path).expect("live_client stderr file");
    let mut live_child = Command::new(&live_client_str)
        .args([
            "bare-consult",
            &mcpd_bin_str,
            &config_a.to_string_lossy(),
            "beta",
            &question,
        ])
        .stdout(Stdio::from(live_stdout))
        .stderr(Stdio::from(live_stderr))
        .spawn()
        .expect("spawn live_client bare-consult");

    let (updates, _raw) = pump_capture(&mut core, &rx, 60, |u| {
        u.iter().any(|x| matches!(x, VmUpdate::WaslGates(gates) if gates.iter().any(|g| g.host_id == "local")))
    });
    let gate = updates
        .iter()
        .rev()
        .find_map(|x| match x {
            VmUpdate::WaslGates(gates) => gates.iter().find(|g| g.host_id == "local").cloned(),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!(
                "[hop3] no VmUpdate::WaslGates surfaced within 60s; live_client stderr: {}",
                std::fs::read_to_string(&live_stderr_path).unwrap_or_default()
            )
        });
    assert!(
        gate.preview.contains(&word),
        "[hop3] pending gate preview must carry this run's probe word; gate={gate:?}"
    );
    eprintln!(
        "[x6/hop3] PASS — Tier-3 gate surfaced in Marsad: host={} gate_id={} kind={} preview={:?}",
        gate.host_id, gate.gate_id, gate.kind, gate.preview
    );

    // ─────────────────────────────────────────────────────────────────────────
    // hop 4 — relay the human decision through Core::answer_wasl_gate; assert
    // the wasl side delivers (consult completes; live_client output carries
    // the reply) AND Marsad observes wasl_gate_decided + at least one audit
    // observation.
    // ─────────────────────────────────────────────────────────────────────────
    eprintln!("\n[x6/hop4] Core::answer_wasl_gate(Approve) -> wasl side + Marsad observation");
    core.answer_wasl_gate("local", &gate.gate_id, Decision::Approve)
        .expect("answer_wasl_gate must succeed for a pending gate");

    let gate_id = gate.gate_id.clone();
    let (updates, _raw) = pump_capture(&mut core, &rx, 45, |u| {
        let decided = u.iter().any(|x| {
            matches!(
                x,
                VmUpdate::WaslGateDecided { host_id, gate_id: gid, outcome, .. }
                    if host_id == "local" && gid == &gate_id && outcome == "approved"
            )
        });
        let observed = u
            .iter()
            .any(|x| matches!(x, VmUpdate::WaslObservation { host_id, .. } if host_id == "local"));
        decided && observed
    });
    let decided = updates.iter().any(|x| {
        matches!(
            x,
            VmUpdate::WaslGateDecided { host_id, gate_id: gid, outcome, .. }
                if host_id == "local" && gid == &gate_id && outcome == "approved"
        )
    });
    let observed_count = updates
        .iter()
        .filter(|x| matches!(x, VmUpdate::WaslObservation { host_id, .. } if host_id == "local"))
        .count();
    assert!(
        decided,
        "[hop4] no VmUpdate::WaslGateDecided{{outcome:\"approved\"}} for gate {gate_id} within 45s"
    );
    assert!(
        observed_count >= 1,
        "[hop4] expected at least one VmUpdate::WaslObservation, got {observed_count}"
    );
    eprintln!(
        "[x6/hop4] PASS — Marsad observed wasl_gate_decided(approved) + {observed_count} audit observation(s)"
    );

    // The consult itself must now complete on the wasl side: live_client
    // exits 0 with a WASL_LIVE::OK line carrying the real claude reply.
    let status = wait_child(&mut live_child, Duration::from_secs(90)).unwrap_or_else(|| {
        panic!(
            "[hop4] live_client did not exit within 90s after approval; stderr: {}",
            std::fs::read_to_string(&live_stderr_path).unwrap_or_default()
        )
    });
    let stdout = std::fs::read_to_string(&live_stdout_path).unwrap_or_default();
    assert!(
        status.success(),
        "[hop4] live_client exited non-zero ({status:?}); stdout={stdout:?} stderr={}",
        std::fs::read_to_string(&live_stderr_path).unwrap_or_default()
    );
    assert!(
        stdout.contains("WASL_LIVE::OK"),
        "[hop4] live_client stdout must carry the OK marker; got {stdout:?}"
    );
    assert!(
        stdout.contains(&word),
        "[hop4] live_client output must carry the real claude reply (the probe word); got {stdout:?}"
    );
    eprintln!("[x6/hop4] PASS — wasl side delivered: {}", stdout.trim());

    // live_client has exited (it closed node A's stdin on its way out, which
    // per wasl-mcpd's design tears node A down too — see TeardownGuard's doc
    // comment); nothing further to track for node A.

    eprintln!("\n[x6] hops 1-4 PASSED — full Wasl<->Marsad live loop verified.");

    // ─────────────────────────────────────────────────────────────────────────
    // hop 5 — optionality: a SECOND agent, with NO wasl.json at all, must
    // never emit a single Wasl IpcMsg line, and Core must never produce a
    // single Wasl VmUpdate — across a real launch+Done session.
    // ─────────────────────────────────────────────────────────────────────────
    eprintln!("\n[x6/hop5] optionality leg — agent with no wasl.json");
    let socket_path2 = tmpdir.join("agent2.sock");
    let socket2 = socket_path2.to_string_lossy().to_string();
    guard.socket_paths.push(socket_path2.clone());
    let project2 = tmpdir.join("project2");
    std::fs::create_dir_all(&project2).expect("project2 dir");
    let project2 = project2.to_string_lossy().to_string();
    git_init(&project2);
    let data_root2 = tmpdir.join("data2").to_string_lossy().to_string();
    std::fs::create_dir_all(&data_root2).expect("data_root2 dir");
    // Deliberately no <data_root2>/wasl.json — the entire X2 wiring block in
    // serve.rs is `if let Some(wasl_cfg) = load_wasl_config(..)`; absent file
    // means zero threads, zero connections, empty registry (§6).

    let socket2_t = socket2.clone();
    let data_root2_t = data_root2.clone();
    std::thread::spawn(move || {
        let cfg = AgentConfig {
            listen: ListenSpec::UnixSocket(socket2_t),
            data_root: data_root2_t,
        };
        match marsad_agent::Agent::with_real_io(cfg) {
            Ok(agent) => {
                if let Err(e) = agent.serve() {
                    eprintln!("[x6] agent2 serve error: {e}");
                }
            }
            Err(e) => eprintln!("[x6] Agent2::with_real_io failed: {e}"),
        }
    });

    let conn2 = poll_connect(&socket2, Duration::from_secs(10)).expect("Core2 connect");
    let cfg2 = CoreConfig {
        agent_endpoint: socket2.clone(),
        data_root: data_root2.clone(),
    };
    let mut core2 = Core::new(cfg2, Box::new(conn2)).expect("Core2::new");
    let rx2 = start_reader(&socket2);
    let _ack2 = rx2
        .recv_timeout(Duration::from_secs(10))
        .expect("Subscribed ack (agent2)");

    let id_b = core2
        .launch_session(spec(
            &project2,
            "Reply with exactly the single word READY and nothing else.",
        ))
        .expect("launch session (agent2)");
    let (updates2, raw2) = pump_capture(&mut core2, &rx2, 120, |u| {
        u.iter().any(|x| match x {
            VmUpdate::Session(sv) if sv.id == id_b => sv.state == Phase::Done,
            _ => false,
        })
    });
    let done2 = updates2.iter().any(|x| match x {
        VmUpdate::Session(sv) if sv.id == id_b => sv.state == Phase::Done,
        _ => false,
    });
    assert!(
        done2,
        "[hop5] no-wasl-config session {id_b} never reached Done"
    );

    let wasl_lines: Vec<&String> = raw2
        .iter()
        .filter(|l| l.contains("\"type\":\"Wasl"))
        .collect();
    assert!(
        wasl_lines.is_empty(),
        "[hop5] agent WITHOUT wasl.json must broadcast zero Wasl IpcMsg lines; got {wasl_lines:?}"
    );
    let wasl_updates: Vec<&VmUpdate> = updates2
        .iter()
        .filter(|x| {
            matches!(
                x,
                VmUpdate::WaslGates(_)
                    | VmUpdate::WaslGateDecided { .. }
                    | VmUpdate::WaslObservation { .. }
            )
        })
        .collect();
    assert!(
        wasl_updates.is_empty(),
        "[hop5] Core fed only non-Wasl traffic must never produce a Wasl VmUpdate; got {wasl_updates:?}"
    );
    eprintln!(
        "[x6/hop5] PASS — {} broadcast lines observed, zero Wasl-tagged, zero Wasl VmUpdates",
        raw2.len()
    );

    eprintln!("\n[x6] ALL HOPS PASSED — live Wasl<->Marsad end-to-end verified.");
}
