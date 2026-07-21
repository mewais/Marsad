//! i12_codex_live — headless end-to-end proof of the codex launch loop:
//! a real `marsad_core::Core` sends commands over a Unix socket to a real
//! `marsad-agent`, which runs a real `codex exec`; the agent's broadcast
//! events are pumped back into `Core::on_agent_line`, and we assert the
//! resulting `SessionView`s reach Done with the expected assistant reply.
//!
//! Two scenarios:
//!   S1 — first turn: launch, reach Done, assert "OK" in transcript.
//!   S2 — follow-up turn (resume via thread_id): send a follow-up prompt,
//!         reach Done again, assert the answer proves continuity.
//!
//! # SKIP contract
//! `skip_if_no_deps!()` prints a loud SKIP and returns if `codex`, `tmux`,
//! or `OPENAI_API_KEY` are absent, or memory is too tight. NEVER silent pass.
//!
//! # TEARDOWN contract
//! A `TeardownGuard` kills `marsad-session-*` tmux sessions and removes the
//! temp dir on success or panic.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use marsad_agent::{AgentConfig, ListenSpec};
use marsad_core::{Core, CoreConfig, LaunchMode, LaunchSpec, Posture, VmUpdate};
use marsad_events::Phase;
use marsad_transport::{LocalIpc, Transport};
use marsad_viewmodel::SessionView;

// ── test-local counter ────────────────────────────────────────────────────────

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn unique_id() -> String {
    let c = COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{}-{}", std::process::id(), c)
}

// ── memory + dependency gates ──────────────────────────────────────────────────

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
    bin_ok("codex", "--version") && bin_ok("tmux", "-V")
}

fn openai_key_set() -> bool {
    std::env::var("OPENAI_API_KEY")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
}

macro_rules! skip_if_no_deps {
    () => {
        if !deps_available() {
            eprintln!(
                "\n========================================================\n\
                 SKIP i12: codex and/or tmux not available in PATH.\n\
                 Install codex (npm install -g @openai/codex) and tmux.\n\
                 ========================================================"
            );
            return;
        }
        if !openai_key_set() {
            eprintln!(
                "\n========================================================\n\
                 SKIP i12: OPENAI_API_KEY not set.\n\
                 The codex gateway requires a valid API key.\n\
                 ========================================================"
            );
            return;
        }
        if !enough_memory() {
            eprintln!(
                "\n========================================================\n\
                 SKIP i12: insufficient memory for codex (~700 MB RSS +\n\
                 large virtual space). Run on a host with >=3 GB available.\n\
                 ========================================================"
            );
            return;
        }
    };
}

// ── TeardownGuard ─────────────────────────────────────────────────────────────

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

// ── harness helpers ───────────────────────────────────────────────────────────

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
                eprintln!("[i12] reader connect failed: {e}");
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

/// Pump broadcast lines into `Core` until `done(updates)` is true or timeout.
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
                eprintln!("[i12] pump: reader disconnected");
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

/// True if any assistant transcript entry contains `needle` (case-insensitive).
fn assistant_says(sv: &SessionView, needle: &str) -> bool {
    let needle = needle.to_lowercase();
    sv.transcript.iter().any(|e| {
        matches!(e, marsad_events::TranscriptEntry::Assistant { text, .. }
            if text.to_lowercase().contains(&needle))
    })
}

/// The codex model to use: `MARSAD_CODEX_MODEL` env override, else `gpt-5-codex`.
fn codex_model() -> String {
    std::env::var("MARSAD_CODEX_MODEL").unwrap_or_else(|_| "gpt-5-codex".to_string())
}

fn codex_spec(project: &str, first_prompt: &str) -> LaunchSpec {
    LaunchSpec {
        project: project.to_string(),
        host_id: "local".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: codex_model(),
        posture: Posture::FullAutonomy,
        first_prompt: Some(first_prompt.to_string()),
        mode: LaunchMode::Fresh,
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "codex".to_string(),
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// The live test
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn codex_launch_stream_resume() {
    skip_if_no_deps!();

    let id = unique_id();
    let tmpdir = std::env::temp_dir().join(format!("marsad-it-i12-{id}"));
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

    // ── Agent daemon thread ───────────────────────────────────────────────────
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
                    eprintln!("[i12] agent serve error: {e}");
                }
            }
            Err(e) => eprintln!("[i12] Agent::with_real_io failed: {e}"),
        }
    });

    // ── Core (the GUI's send side) ────────────────────────────────────────────
    let conn = poll_connect(&socket, Duration::from_secs(10)).expect("Core connect");
    let cfg = CoreConfig {
        agent_endpoint: socket.clone(),
        data_root: data_root.clone(),
    };
    let mut core = Core::new(cfg, Box::new(conn)).expect("Core::new");

    // ── Reader (broadcast subscription) ──────────────────────────────────────
    let rx = start_reader(&socket);
    let _ack = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("Subscribed ack before launch");

    // ─────────────────────────────────────────────────────────────────────────
    // S1 — launch: first turn, assert Done + "OK" in transcript.
    // ─────────────────────────────────────────────────────────────────────────
    eprintln!("\n[i12/S1] launch codex session with first prompt");
    let id_a = core
        .launch_session(codex_spec(
            &project,
            "Reply with exactly the single word OK and nothing else.",
        ))
        .expect("launch codex session A");

    let updates = pump_until(&mut core, &rx, 120, |u| {
        latest_sv(u, &id_a)
            .map(|sv| sv.state == Phase::Done)
            .unwrap_or(false)
    });

    let sv = latest_sv(&updates, &id_a).expect("S1: a SessionView for A");
    assert_eq!(sv.state, Phase::Done, "S1: session must reach Done; state={:?}", sv.state);
    assert!(
        assistant_says(&sv, "ok"),
        "S1: transcript must contain 'OK'; got {:?}",
        sv.transcript
    );
    eprintln!(
        "[i12/S1] PASS — Done, tokens={}, transcript entries={}",
        sv.tokens,
        sv.transcript.len()
    );

    // ─────────────────────────────────────────────────────────────────────────
    // S2 — follow-up turn: proves codex resume via thread_id worked.
    // ─────────────────────────────────────────────────────────────────────────
    eprintln!("\n[i12/S2] follow-up prompt — proves codex resume continuity");
    core.send_prompt(
        &id_a,
        "What single word did I ask you to reply with in your previous response? Reply with just that word.",
    )
    .expect("send_prompt A follow-up");

    let updates = pump_until(&mut core, &rx, 120, |u| {
        // We want a NEW Done after the follow-up — check the updates list for
        // a Done that came after the first one (i.e., after at least one
        // Working/Starting state following the first Done).
        let mut found_second_done = false;
        let mut seen_first_done = false;
        let mut seen_working_after_done = false;
        for update in u.iter() {
            if let VmUpdate::Session(sv) = update {
                if sv.id == id_a {
                    if sv.state == Phase::Done && !seen_first_done {
                        seen_first_done = true;
                    } else if seen_first_done
                        && (sv.state == Phase::Working || sv.state == Phase::Starting)
                    {
                        seen_working_after_done = true;
                    } else if seen_working_after_done && sv.state == Phase::Done {
                        found_second_done = true;
                    }
                }
            }
        }
        found_second_done
    });

    let sv = latest_sv(&updates, &id_a).expect("S2: a SessionView for A");
    assert_eq!(
        sv.state,
        Phase::Done,
        "S2: session must reach Done after follow-up; state={:?}",
        sv.state
    );
    assert!(
        assistant_says(&sv, "ok"),
        "S2: follow-up reply must still contain 'OK' (proves resume worked); got {:?}",
        sv.transcript
    );
    eprintln!(
        "[i12/S2] PASS — resume continuity confirmed, transcript entries={}",
        sv.transcript.len()
    );

    eprintln!("\n[i12] ALL SCENARIOS PASSED — codex launched, streamed, and resumed.");
}
