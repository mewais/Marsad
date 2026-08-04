//! i14_opencode_live — headless end-to-end proof of the opencode launch loop
//! through Marsad's FULL transport: a real `marsad_core::Core` sends commands
//! over a Unix socket to a real `marsad-agent`, which runs a real `opencode acp`
//! (the ACP driver); the agent's broadcast events are pumped back into
//! `Core::on_agent_line`, and we assert the resulting `SessionView` reaches Done
//! with a non-empty assistant transcript.
//!
//! This is the opencode analogue of i10 (claude) / i12 (codex). It verifies the
//! wasit-opencode handshake (initialize + session/new intercept + session/prompt)
//! works end-to-end over Marsad's Core + agent-daemon + tmux + socket path.
//!
//! Single scenario:
//!   S1 — launch an opencode session with a short prompt → pump VmUpdates until
//!        Done → assert a SessionView exists with a non-empty assistant
//!        transcript. Surfaced tokens/cost are logged if present. Because model
//!        output is non-deterministic, we assert only that SOME assistant text
//!        appeared (like i10/i02 keep their live asserts robust) — not an exact
//!        string.
//!
//! # SKIP contract
//! `skip_if_no_deps!()` prints a loud SKIP and returns if `opencode`/`tmux` are
//! absent, or memory is too tight to host opencode (>2 GB RSS + large virtual
//! space, per the opencode backend notes). NEVER a silent pass.
//!
//! # TEARDOWN contract
//! A `TeardownGuard` kills `marsad-session-*` tmux sessions and removes the temp
//! dir on success or panic.
//!
//! # Threading
//! `Core` is not Send (it owns the transport + git probe), so it stays on the
//! main thread. A reader thread owns a second `LocalIpc`, subscribes, and
//! forwards every broadcast line over an mpsc channel; the main thread pumps
//! those lines into `Core` between commands.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use marsad_agent::{AgentConfig, ListenSpec};
use marsad_core::{Core, CoreConfig, LaunchMode, LaunchSpec, VmUpdate};
use marsad_events::Phase;
use marsad_transport::{LocalIpc, Transport};
use marsad_viewmodel::SessionView;

// ── test-local counter ──────────────────────────────────────────────────────

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn unique_id() -> String {
    let c = COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{}-{}", std::process::id(), c)
}

// ── memory + dependency gates (same thresholds as i10/i12) ───────────────────

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
    bin_ok("opencode", "--version") && bin_ok("tmux", "-V")
}

macro_rules! skip_if_no_deps {
    () => {
        if !deps_available() {
            eprintln!(
                "\n========================================================\n\
                 SKIP i14: opencode and/or tmux not available in PATH.\n\
                 Install opencode (~/.opencode/bin on PATH) and tmux.\n\
                 ========================================================"
            );
            return;
        }
        if !enough_memory() {
            eprintln!(
                "\n========================================================\n\
                 SKIP i14: insufficient memory for opencode (>2 GB RSS +\n\
                 large virtual space). Run on a host with >=3 GB available.\n\
                 ========================================================"
            );
            return;
        }
    };
}

// ── TeardownGuard ────────────────────────────────────────────────────────────

struct TeardownGuard {
    tmpdir: PathBuf,
    socket_path: PathBuf,
}

impl Drop for TeardownGuard {
    fn drop(&mut self) {
        // Kill any marsad-session-* tmux sessions spawned by this run.
        let _ = Command::new("sh")
            .arg("-c")
            .arg("tmux list-sessions -F '#{session_name}' 2>/dev/null | grep '^marsad-session-' | xargs -r -n1 tmux kill-session -t")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = std::fs::remove_file(&self.socket_path);
        let _ = std::fs::remove_dir_all(&self.tmpdir);
    }
}

// ── harness helpers ──────────────────────────────────────────────────────────

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

/// Spawn a reader: a 2nd connection that Subscribes and forwards every broadcast
/// line over the returned channel. The first forwarded line is the `Subscribed`
/// ack (used to confirm the subscription is live before launching).
fn start_reader(socket: &str) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    let socket = socket.to_string();
    std::thread::spawn(move || {
        let mut ipc = match poll_connect(&socket, Duration::from_secs(10)) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[i14] reader connect failed: {e}");
                return;
            }
        };
        let sub = serde_json::json!({ "type": "Subscribe", "session_id": "" }).to_string();
        if ipc.send_line(&sub).is_err() {
            return;
        }
        while let Ok(Some(line)) = ipc.recv_line() {
            if tx.send(line).is_err() {
                break; // main side dropped
            }
        }
    });
    rx
}

/// Pump broadcast lines into `Core` until `done(updates)` is true or `secs`
/// elapse. Lines Core can't ingest (acks like `Subscribed`/`SessionLaunched`)
/// produce a Protocol error and are skipped. Returns all VmUpdates collected.
fn pump_until<F>(core: &mut Core, rx: &Receiver<String>, secs: u64, mut done: F) -> Vec<VmUpdate>
where
    F: FnMut(&[VmUpdate]) -> bool,
{
    let mut all: Vec<VmUpdate> = Vec::new();
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
                if let Ok(updates) = core.on_agent_line(&line) {
                    all.extend(updates);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                eprintln!("[i14] pump: reader disconnected");
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => break,
        }
    }
    all
}

/// Most recent `SessionView` for `id` across the collected updates.
fn latest_sv(updates: &[VmUpdate], id: &str) -> Option<SessionView> {
    updates.iter().rev().find_map(|u| match u {
        VmUpdate::Session(sv) if sv.id == id => Some(sv.clone()),
        _ => None,
    })
}

/// Any non-empty assistant transcript text across the view (proves the opencode
/// ACP handshake + agent_message_chunk coalescing produced a real reply).
fn assistant_text(sv: &SessionView) -> Option<String> {
    sv.transcript.iter().find_map(|e| match e {
        marsad_events::TranscriptEntry::Assistant { text, .. } if !text.trim().is_empty() => {
            Some(text.clone())
        }
        _ => None,
    })
}

/// The live model: `MARSAD_LIVE_MODEL` env override, else empty string, meaning
/// "use opencode's configured default" (opencode owns model selection in ACP
/// mode; an empty `session_mode`/model is applied as the backend default).
fn live_model() -> String {
    std::env::var("MARSAD_LIVE_MODEL").unwrap_or_default()
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
        session_mode: String::new(),
        first_prompt: Some(first_prompt.to_string()),
        mode: LaunchMode::Fresh,
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "opencode".to_string(),
    }
}

/// Initialise a git repo in `dir` so the RealGit probe can report changes.
fn git_init(dir: &str) {
    for args in [
        vec!["-C", dir, "init", "-q"],
        vec!["-C", dir, "config", "user.email", "i14@test"],
        vec!["-C", dir, "config", "user.name", "i14"],
    ] {
        let _ = Command::new("git")
            .args(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// S1 — the full opencode loop through Marsad's real transport.
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn opencode_full_loop() {
    skip_if_no_deps!();

    let id = unique_id();
    let tmpdir = std::env::temp_dir().join(format!("marsad-it-i14-{id}"));
    std::fs::create_dir_all(&tmpdir).expect("tmpdir");
    let socket_path = tmpdir.join("agent.sock");
    let socket = socket_path.to_string_lossy().to_string();
    let project = tmpdir.join("project");
    std::fs::create_dir_all(&project).expect("project dir");
    let project = project.to_string_lossy().to_string();
    let data_root = tmpdir.join("data").to_string_lossy().to_string();

    let _guard = TeardownGuard {
        tmpdir: tmpdir.clone(),
        socket_path: socket_path.clone(),
    };

    git_init(&project);

    // ── Agent daemon thread (AgentConfig is the only Send payload) ─────────────
    let socket_t = socket.clone();
    let data_root_t = data_root.clone();
    std::thread::spawn(move || {
        let cfg = AgentConfig {
            listen: ListenSpec::UnixSocket(socket_t),
            data_root: data_root_t,
        };
        match marsad_agent::Agent::with_real_io(cfg) {
            Ok(agent) => {
                if let Err(e) = agent.serve() {
                    eprintln!("[i14] agent serve error: {e}");
                }
            }
            Err(e) => eprintln!("[i14] Agent::with_real_io failed: {e}"),
        }
    });

    // ── Core (the GUI's send side) over a real socket ─────────────────────────
    let conn = poll_connect(&socket, Duration::from_secs(10)).expect("Core connect");
    let cfg = CoreConfig {
        agent_endpoint: socket.clone(),
        data_root: data_root.clone(),
    };
    let mut core = Core::new(cfg, Box::new(conn)).expect("Core::new");

    // ── Reader (broadcast subscription) ───────────────────────────────────────
    let rx = start_reader(&socket);
    let _ack = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("Subscribed ack before launch");

    // ── S1 — launch opencode with a first prompt; reach Done + assistant text ──
    eprintln!("\n[i14/S1] launch opencode + assistant reply text");
    let id_a = core
        .launch_session(spec(&project, "Say hi in exactly 3 words."))
        .expect("launch opencode session");

    // opencode's ACP handshake (initialize + session/new intercept) + first turn
    // can take a while on a cold gateway; give it a generous window like i10/i12.
    let updates = pump_until(&mut core, &rx, 180, |u| {
        latest_sv(u, &id_a)
            .map(|sv| sv.state == Phase::Done && assistant_text(&sv).is_some())
            .unwrap_or(false)
    });

    let sv = latest_sv(&updates, &id_a).expect("S1: a SessionView for the opencode session");

    // Robust asserts: the session must have reached a terminal Done state and
    // produced SOME assistant text. Exact wording is the model's choice.
    assert_eq!(
        sv.state,
        Phase::Done,
        "S1: opencode session must reach Done; state={:?}, transcript={:?}",
        sv.state,
        sv.transcript
    );
    let reply = assistant_text(&sv).unwrap_or_else(|| {
        panic!(
            "S1: transcript must contain non-empty assistant text; got {:?}",
            sv.transcript
        )
    });
    assert!(
        !reply.trim().is_empty(),
        "S1: assistant reply must be non-empty"
    );

    // Tokens / cost are surfaced if the backend reports them (opencode usage over
    // ACP is not guaranteed on every gateway) — log, don't hard-fail.
    eprintln!(
        "[i14/S1] PASS — Done, transcript entries={}, tokens={}, cost={:.6}, assistant reply={:?}",
        sv.transcript.len(),
        sv.tokens,
        sv.cost_usd,
        reply.trim()
    );
    if sv.tokens == 0 && sv.cost_usd == 0.0 {
        eprintln!("[i14/S1] NOTE — no tokens/cost surfaced by opencode this run (backend-dependent).");
    }

    eprintln!("\n[i14] SCENARIO PASSED — full Core<->agent<->opencode loop verified.");
}
