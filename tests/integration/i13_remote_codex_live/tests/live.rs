//! i13_remote_codex_live — headless end-to-end proof of the REMOTE codex launch loop.
//!
//! A real `marsad_core::Core` sends a LaunchSession (driver="codex", host_target set to
//! `MARSAD_REMOTE_HOST`) over a Unix socket to a real `marsad-agent`. The agent:
//!   1. Builds the eval-loop run.sh via `build_remote_codex_eval_loop`.
//!   2. Calls `remote_durable_launch_script` to create `in.fifo`, keepalive, and
//!      `start.sh` on the remote host.
//!   3. Spawns the remote reader child (`tail -n0 --pid=<pid> -f out.log`).
//!   4. On first_prompt: writes the `codex_turn_cmd` eval-loop command to in.fifo
//!      via the writer child.
//!   5. Streams JSONL from out.log, translates via `codex_event_to_stream_json`, and
//!      broadcasts `AgentEvent`s → `Core` → `SessionView`.
//!
//! The broadcast events are pumped into `Core::on_agent_line` and we assert the
//! resulting `SessionView` reaches `Done` with a meaningful assistant reply.
//!
//! Two scenarios:
//!   S1 — first turn: launch remote codex session, reach Done, assert "OK" in transcript.
//!   S2 — follow-up turn: write a second prompt via SendPrompt, reach Done again, assert
//!         the answer proves resume continuity (codex exec resume <thread_id>).
//!
//! # SKIP contract
//! `skip_if_no_deps!()` prints a loud SKIP and returns when:
//!   - `codex` is not on PATH on the remote host (probed via ssh)
//!   - `MARSAD_REMOTE_HOST` is not set (no remote target configured)
//!   - `OPENAI_API_KEY` is not set
//!   - tmux or ssh are not available locally
//!
//! Recommended invocation:
//! ```
//! MARSAD_REMOTE_HOST=wsl:Ubuntu OPENAI_API_KEY=sk-... \
//!   cargo test -p marsad-it-i13_remote_codex_live --test live -- --nocapture
//! ```
//!
//! Or for a real SSH host:
//! ```
//! MARSAD_REMOTE_HOST=user@build-box OPENAI_API_KEY=sk-... \
//!   cargo test -p marsad-it-i13_remote_codex_live --test live -- --nocapture
//! ```
//!
//! # TEARDOWN contract
//! A `TeardownGuard` kills the remote session dir (`remote_kill_script`) and local
//! tmux sessions, and removes the temp dir, on success or panic.
//!
//! # Relationship to i12
//! i12 tests LOCAL codex (no host_target). i13 tests REMOTE codex (host_target set).
//! The reader path diverges at `exec_prefix_for(host_target)`:
//!   - i12: local FIFO transport (ClaudeFifoTransport)
//!   - i13: remote durable model (writer child = tee -a in.fifo; reader child = tail -f out.log)

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

const MIN_AVAIL_KB: u64 = 2_000_000; // 2 GB (remote codex uses less local memory than local)
const MIN_AS_BYTES: u64 = 4_000_000_000;

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

/// Check that `bin` exists locally.
fn local_bin_ok(bin: &str, arg: &str) -> bool {
    Command::new(bin)
        .arg(arg)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Probe that `codex` is on PATH on the remote host by running
/// `codex --version` via the host's connection.  Uses `run_remote_script`
/// internally (ssh/wsl, stdin-piped bash -l -s).
fn remote_codex_available(host_target: &str) -> bool {
    // Build a minimal one-line script that exits 0 iff codex is on PATH.
    let script = "command -v codex >/dev/null 2>&1 && codex --version >/dev/null 2>&1 && echo CODEX_OK";
    match marsad_agent::run_remote_script(host_target, script) {
        Some(out) => out.contains("CODEX_OK"),
        None => false,
    }
}

fn openai_key_set() -> bool {
    std::env::var("OPENAI_API_KEY")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
}

fn remote_host() -> Option<String> {
    std::env::var("MARSAD_REMOTE_HOST")
        .ok()
        .filter(|v| !v.trim().is_empty())
}

macro_rules! skip_if_no_deps {
    ($host:expr) => {
        if !local_bin_ok("tmux", "-V") {
            eprintln!(
                "\n========================================================\n\
                 SKIP i13: tmux not available locally.\n\
                 ========================================================"
            );
            return;
        }
        if !openai_key_set() {
            eprintln!(
                "\n========================================================\n\
                 SKIP i13: OPENAI_API_KEY not set.\n\
                 The codex gateway requires a valid API key.\n\
                 ========================================================"
            );
            return;
        }
        if !enough_memory() {
            eprintln!(
                "\n========================================================\n\
                 SKIP i13: insufficient local memory.\n\
                 ========================================================"
            );
            return;
        }
        if !remote_codex_available($host) {
            eprintln!(
                "\n========================================================\n\
                 SKIP i13: `codex` not on PATH on remote host {:?}.\n\
                 Install codex on the remote: npm install -g @openai/codex\n\
                 ========================================================"
                , $host
            );
            return;
        }
    };
}

// ── TeardownGuard ─────────────────────────────────────────────────────────────

struct TeardownGuard {
    tmpdir: PathBuf,
    socket_path: PathBuf,
    session_id: String,
    host_target: String,
}

impl Drop for TeardownGuard {
    fn drop(&mut self) {
        // Kill the remote durable session (SIGTERM ka + claude-pid, rm -rf dir).
        if !self.session_id.is_empty() && !self.host_target.is_empty() {
            let script = marsad_agent::remote_kill_script(&self.session_id);
            let _ = marsad_agent::run_remote_script(&self.host_target, &script);
        }
        // Kill any local marsad tmux sessions.
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

/// Spawn a subscriber reader forwarding every broadcast line over the channel.
fn start_reader(socket: &str) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    let socket = socket.to_string();
    std::thread::spawn(move || {
        let mut ipc = match poll_connect(&socket, Duration::from_secs(10)) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[i13] reader connect failed: {e}");
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
                eprintln!("[i13] pump: reader disconnected");
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

fn remote_codex_spec(project: &str, host_target: &str, first_prompt: &str) -> LaunchSpec {
    LaunchSpec {
        project: project.to_string(),
        host_id: "remote".to_string(),
        host_target: Some(host_target.to_string()),
        claude_bin: None,
        setup: None,
        // The OPENAI_API_KEY must be set in the remote env; pass it explicitly
        // so the eval-loop inherits it regardless of remote login shell config.
        env: vec![(
            "OPENAI_API_KEY".to_string(),
            std::env::var("OPENAI_API_KEY").unwrap_or_default(),
        )],
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
fn remote_codex_launch_stream_resume() {
    // Require MARSAD_REMOTE_HOST to be set — this test is explicitly for the
    // remote codex path.  Without a configured remote host, skip loudly.
    let host_target = match remote_host() {
        Some(h) => h,
        None => {
            eprintln!(
                "\n========================================================\n\
                 SKIP i13: MARSAD_REMOTE_HOST not set.\n\
                 Set it to a wsl:<distro>, user@host, or plink:<target>\n\
                 to run the remote codex live test.\n\
                 ========================================================"
            );
            return;
        }
    };

    skip_if_no_deps!(&host_target);

    let id = unique_id();
    let tmpdir = std::env::temp_dir().join(format!("marsad-it-i13-{id}"));
    std::fs::create_dir_all(&tmpdir).expect("tmpdir");
    let socket_path = tmpdir.join("agent.sock");
    let socket = socket_path.to_string_lossy().to_string();
    let data_root = tmpdir.join("data").to_string_lossy().to_string();

    // The project dir is on the REMOTE host (a well-known dir that codex can cd into).
    // We use ~ so it resolves to the remote user's home.
    let project = "~".to_string();

    // Placeholder session id for teardown (will be filled in after launch).
    let teardown_sid = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let teardown_sid_t = std::sync::Arc::clone(&teardown_sid);

    let _guard = TeardownGuard {
        tmpdir: tmpdir.clone(),
        socket_path: socket_path.clone(),
        session_id: String::new(),  // filled in after launch via teardown_sid
        host_target: host_target.clone(),
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
                    eprintln!("[i13] agent serve error: {e}");
                }
            }
            Err(e) => eprintln!("[i13] Agent::with_real_io failed: {e}"),
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
    // S1 — launch: remote codex first turn, assert Done + "OK" in transcript.
    //
    // Under the hood the agent:
    //   1. Calls build_remote_codex_eval_loop("~", None, [("OPENAI_API_KEY", ...)])
    //      to build the run.sh body.
    //   2. Calls remote_durable_launch_script(sid, run_cmd) to create in.fifo +
    //      keepalive + start.sh + eval-loop on the remote via run_remote_script.
    //   3. Spawns local writer child (tee -a <abs_dir>/in.fifo) and reader child
    //      (tail -n0 --pid=<eval_loop_pid> -f <abs_dir>/out.log).
    //   4. Writes codex_turn_cmd(spec, None, first_prompt) to in.fifo.
    //   5. The eval-loop runs `codex exec --json ... "$decoded_prompt"` → JSONL → out.log.
    //   6. Reader child streams out.log lines → codex_event_to_stream_json → AgentEvent.
    // ─────────────────────────────────────────────────────────────────────────
    eprintln!("\n[i13/S1] launch remote codex session on {:?}", host_target);
    let id_a = core
        .launch_session(remote_codex_spec(
            &project,
            &host_target,
            "Reply with exactly the single word OK and nothing else.",
        ))
        .expect("launch remote codex session A");

    // Store the session id for teardown (the remote kill script needs it).
    *teardown_sid_t.lock().unwrap() = id_a.clone();

    // Remote codex takes longer than local — allow up to 3 minutes for the
    // eval-loop to spin up, the first codex exec to run, and the JSONL to stream.
    let updates = pump_until(&mut core, &rx, 180, |u| {
        latest_sv(u, &id_a)
            .map(|sv| sv.state == Phase::Done)
            .unwrap_or(false)
    });

    let sv = latest_sv(&updates, &id_a).expect("S1: a SessionView for A");
    assert_eq!(
        sv.state,
        Phase::Done,
        "S1: remote codex session must reach Done; state={:?}", sv.state
    );
    assert!(
        assistant_says(&sv, "ok"),
        "S1: transcript must contain 'OK'; got {:?}", sv.transcript
    );
    eprintln!(
        "[i13/S1] PASS — Done, tokens={}, transcript entries={}",
        sv.tokens,
        sv.transcript.len()
    );

    // ─────────────────────────────────────────────────────────────────────────
    // S2 — follow-up turn: the writer child writes a second codex_turn_cmd
    //      (exec resume <thread_id>) to in.fifo; the eval-loop runs it;
    //      the reader child streams the new JSONL events from out.log.
    // ─────────────────────────────────────────────────────────────────────────
    eprintln!("\n[i13/S2] follow-up prompt — proves remote codex resume via thread_id");
    core.send_prompt(
        &id_a,
        "What single word did I ask you to reply with in your previous response? Reply with just that word.",
    )
    .expect("send_prompt A follow-up");

    let updates = pump_until(&mut core, &rx, 180, |u| {
        // Wait for a second Done state after at least one Working/Starting
        // following the first Done (proves the follow-up turn ran).
        let mut seen_first_done = false;
        let mut seen_working_after_done = false;
        let mut found_second_done = false;
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
        "S2: remote codex session must reach Done after follow-up; state={:?}", sv.state
    );
    assert!(
        assistant_says(&sv, "ok"),
        "S2: follow-up reply must still contain 'OK' (proves resume worked); got {:?}",
        sv.transcript
    );
    eprintln!(
        "[i13/S2] PASS — remote resume continuity confirmed, transcript entries={}",
        sv.transcript.len()
    );

    eprintln!(
        "\n[i13] ALL SCENARIOS PASSED — remote codex launched via eval-loop, streamed, and resumed."
    );
}
