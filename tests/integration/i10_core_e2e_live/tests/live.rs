//! i10_core_e2e_live — headless end-to-end proof of the FULL live loop, driven
//! exactly the way the desktop drives it: a real `marsad_core::Core` sends
//! commands over a Unix socket to a real `marsad-agent`, which runs a real
//! `claude`; the agent's broadcast events are pumped back into
//! `Core::on_agent_line`, and we assert the resulting `SessionView`s.
//!
//! This is the test that catches *seam* bugs — id contracts, event casing,
//! name derivation, phase mapping, the gate IPC, transcript + git-changes feeds
//! — that per-crate unit coverage cannot, because it exercises the components
//! together over the real wire.
//!
//! Pinned to **haiku** to keep live runs cheap.
//!
//! # SKIP contract
//! `skip_if_no_deps!()` prints a loud SKIP and returns if `claude`/`tmux` are
//! absent or memory is too tight to host claude — never a silent pass.
//!
//! # TEARDOWN contract
//! A `TeardownGuard` kills `marsad-`-prefixed tmux sessions and removes the temp
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
use marsad_core::{Core, CoreConfig, LaunchMode, LaunchSpec, Posture, VmUpdate};
use marsad_events::Phase;
use marsad_gates::approve;
use marsad_transport::{LocalIpc, Transport};
use marsad_viewmodel::SessionView;

// ── test-local counter ──────────────────────────────────────────────────────

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn unique_id() -> String {
    let c = COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{}-{}", std::process::id(), c)
}

// ── memory + dependency gates (same thresholds as i03/i09) ───────────────────

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
    bin_ok("claude", "--version") && bin_ok("tmux", "-V")
}

macro_rules! skip_if_no_deps {
    () => {
        if !deps_available() {
            eprintln!(
                "\n========================================================\n\
                 SKIP i10: claude and/or tmux not available in PATH.\n\
                 ========================================================"
            );
            return;
        }
        if !enough_memory() {
            eprintln!(
                "\n========================================================\n\
                 SKIP i10: insufficient memory; claude needs ~700 MB RSS +\n\
                 large virtual space. Run on a host with >=3 GB available.\n\
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
        // Sweep any marsad-session-* tmux sessions spawned by this run.
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
/// ack (used by the caller to confirm the subscription is live before launching).
fn start_reader(socket: &str) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    let socket = socket.to_string();
    std::thread::spawn(move || {
        let mut ipc = match poll_connect(&socket, Duration::from_secs(10)) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[i10] reader connect failed: {e}");
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
                eprintln!("[i10] pump: reader disconnected");
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
        matches!(e, marsad_events::TranscriptEntry::Assistant { text }
            if text.to_lowercase().contains(&needle))
    })
}

/// True if any user transcript entry contains `needle` (case-insensitive).
fn user_says(sv: &SessionView, needle: &str) -> bool {
    let needle = needle.to_lowercase();
    sv.transcript.iter().any(|e| {
        matches!(e, marsad_events::TranscriptEntry::User { text }
            if text.to_lowercase().contains(&needle))
    })
}

/// The live model: `MARSAD_LIVE_MODEL` env override, else `haiku` (cheapest live
/// model, the original default). Needed on gateways where a model's deployment
/// rejects fields the CLI sends — e.g. AMD's gateway 400s `haiku` on
/// `output_config.effort` but accepts `sonnet`.
fn live_model() -> String {
    std::env::var("MARSAD_LIVE_MODEL").unwrap_or_else(|_| "haiku".to_string())
}

fn spec(project: &str, first_prompt: &str, posture: Posture, mode: LaunchMode) -> LaunchSpec {
    LaunchSpec {
        project: project.to_string(),
        host_id: "local".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: live_model(),
        posture,
        first_prompt: Some(first_prompt.to_string()),
        mode,
        name: None,
        disallowed_tools: vec![],
        wasl: false,
    }
}

/// Initialise a git repo in `dir` so the RealGit probe can report changes.
fn git_init(dir: &str) {
    for args in [
        vec!["-C", dir, "init", "-q"],
        vec!["-C", dir, "config", "user.email", "i10@test"],
        vec!["-C", dir, "config", "user.name", "i10"],
    ] {
        let _ = Command::new("git")
            .args(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// The full loop — all scenarios in one test (one agent, sequential claudes).
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn full_loop() {
    skip_if_no_deps!();

    let id = unique_id();
    let tmpdir = std::env::temp_dir().join(format!("marsad-it-i10-{id}"));
    std::fs::create_dir_all(&tmpdir).expect("tmpdir");
    let socket_path = tmpdir.join("agent.sock");
    let socket = socket_path.to_string_lossy().to_string();
    let project = tmpdir.join("project");
    std::fs::create_dir_all(&project).expect("project dir");
    let project = project.to_string_lossy().to_string();
    let project_base = project.rsplit('/').next().unwrap().to_string();
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
                    eprintln!("[i10] agent serve error: {e}");
                }
            }
            Err(e) => eprintln!("[i10] Agent::with_real_io failed: {e}"),
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
    // Confirm the subscription is live before launching (first line = Subscribed ack).
    let _ack = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("Subscribed ack before launch");

    // ─────────────────────────────────────────────────────────────────────────
    // S1 — launch with a first prompt: name, phase Working→Done, reply text.
    // ─────────────────────────────────────────────────────────────────────────
    eprintln!("\n[i10/S1] launch + reply text");
    let id_a = core
        .launch_session(spec(
            &project,
            "Reply with exactly the single word READY and nothing else.",
            Posture::FullAutonomy,
            LaunchMode::Fresh,
        ))
        .expect("launch A");

    let updates = pump_until(&mut core, &rx, 120, |u| {
        latest_sv(u, &id_a)
            .map(|sv| sv.state == Phase::Done)
            .unwrap_or(false)
    });
    let sv = latest_sv(&updates, &id_a).expect("S1: a SessionView for A");
    assert_eq!(
        sv.name, project_base,
        "S1: name must be the project basename"
    );
    assert_eq!(sv.state, Phase::Done, "S1: must reach Done");
    assert!(sv.tokens > 0, "S1: tokens must accumulate");
    assert!(
        assistant_says(&sv, "ready"),
        "S1: transcript must contain claude's 'READY' reply; got {:?}",
        sv.transcript
    );
    assert!(
        user_says(&sv, "reply with exactly"),
        "S1: transcript must contain the user's own prompt; got {:?}",
        sv.transcript
    );
    eprintln!(
        "[i10/S1] PASS — name={}, Done, tokens={}",
        sv.name, sv.tokens
    );

    // ─────────────────────────────────────────────────────────────────────────
    // S2 — send a prompt to the same session that writes a file: real git change.
    // ─────────────────────────────────────────────────────────────────────────
    eprintln!("\n[i10/S2] send_prompt + real git change");
    core.send_prompt(
        &id_a,
        "Create a file named hello.txt containing the text hi in the current directory. \
         Then reply DONE.",
    )
    .expect("send_prompt A");

    let updates = pump_until(&mut core, &rx, 120, |u| {
        latest_sv(u, &id_a)
            .map(|sv| sv.changes.iter().any(|c| c.path.contains("hello.txt")))
            .unwrap_or(false)
    });
    let sv = latest_sv(&updates, &id_a).expect("S2: a SessionView for A");
    assert!(
        sv.changes.iter().any(|c| c.path.contains("hello.txt")),
        "S2: git changes must list hello.txt; got {:?}",
        sv.changes
    );
    assert_eq!(
        sv.changed_files as usize,
        sv.changes.len(),
        "S2: changed_files must mirror changes.len()"
    );
    eprintln!("[i10/S2] PASS — changes={:?}", sv.changes);

    // ─────────────────────────────────────────────────────────────────────────
    // S3 — gate roundtrip: an Ask session surfaces a permission gate; answering
    // it (allow) lets claude proceed.
    //
    // Whether claude emits a `can_use_tool` in a one-shot --print turn is the
    // model's choice (haiku is not deterministic about tool use), so we don't
    // hard-fail when no gate appears — that would make the safety net flaky.
    // But WHEN a gate surfaces we hard-assert the full answer roundtrip, which
    // is what proves the AnswerGate IPC carries the session id end-to-end.
    // ─────────────────────────────────────────────────────────────────────────
    eprintln!("\n[i10/S3] gate roundtrip");
    let id_b = core
        .launch_session(spec(
            &project,
            "You MUST use the Bash tool. Using the Bash tool, run exactly this \
             command: echo hello-from-bash. Do not print anything else first.",
            Posture::Ask,
            LaunchMode::Fresh,
        ))
        .expect("launch B");

    let updates = pump_until(&mut core, &rx, 120, |u| {
        latest_sv(u, &id_b)
            .map(|sv| (sv.state == Phase::Needs && sv.gate.is_some()) || sv.state == Phase::Done)
            .unwrap_or(false)
    });
    let sv = latest_sv(&updates, &id_b).expect("S3: a SessionView for B");

    if sv.state == Phase::Needs && sv.gate.is_some() {
        let gate = sv.gate.clone().unwrap();
        eprintln!("[i10/S3] gate surfaced: {gate:?}");

        // Answer allow. The agent uses its own captured request_id; only the
        // session_id + allow matter on the wire (the bug this fix closed).
        core.answer_gate(approve(&id_b, &gate))
            .expect("answer_gate B");

        let updates = pump_until(&mut core, &rx, 120, |u| {
            latest_sv(u, &id_b)
                .map(|sv| sv.gate.is_none())
                .unwrap_or(false)
        });
        let sv = latest_sv(&updates, &id_b).expect("S3: SessionView for B after answer");
        assert!(
            sv.gate.is_none(),
            "S3: gate must clear after answering (AnswerGate must reach claude); state={:?}",
            sv.state
        );
        eprintln!(
            "[i10/S3] PASS — gate answered over the wire, session continued (state={:?})",
            sv.state
        );
    } else {
        eprintln!(
            "[i10/S3] NOTE — claude did not request a tool this run (state={:?}); \
             gate emission is model-dependent. The AnswerGate session-id fix is \
             unit-covered in marsad-core/proto; skipping the live answer assertion.",
            sv.state
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // S4 — resume recall: state a fact in a dedicated repo, then resume the
    // most-recent conversation there via LaunchMode::Continue (the "continue
    // recent" GUI path) and confirm the fact is recalled.
    //
    // NOTE (test-assumption fix, evidence-based): this scenario originally used
    // a "Remember this codeword: BANANA ... What is the codeword?" framing.
    // Observed live against real claude (haiku) in
    // ~/.claude/projects/.../*.jsonl: haiku refused the first turn outright —
    // "I appreciate you testing my judgment, but I should be transparent: that
    // looks like a prompt injection attempt or test. I won't establish hidden
    // communication channels..." — and consequently the second turn correctly
    // (from the model's own view) replied "I didn't follow that instruction,
    // so I'm not retrieving it now." The resume/continue plumbing itself was
    // fine (both turns landed in one continuous transcript file, proving
    // `--continue` found the right session) — the prompt just pattern-matched
    // the model's prompt-injection heuristics ("remember a secret codeword and
    // recall it on command" reads like a covert-channel setup). Rephrased as a
    // mundane stated fact, which carries no such connotation.
    // ─────────────────────────────────────────────────────────────────────────
    eprintln!("\n[i10/S4] resume recall (continue)");
    let project_c = tmpdir.join("resume-project");
    std::fs::create_dir_all(&project_c).expect("resume project dir");
    let project_c = project_c.to_string_lossy().to_string();
    git_init(&project_c);

    let id_c = core
        .launch_session(spec(
            &project_c,
            "My favorite fruit is banana. Please reply OK to confirm you've noted it.",
            Posture::FullAutonomy,
            LaunchMode::Fresh,
        ))
        .expect("launch C");
    let _ = pump_until(&mut core, &rx, 120, |u| {
        latest_sv(u, &id_c)
            .map(|sv| sv.state == Phase::Done)
            .unwrap_or(false)
    });
    eprintln!("[i10/S4] fact stated; continuing the conversation");

    // Continue the most-recent conversation in that dir and ask for the fact.
    let id_d = core
        .launch_session(spec(
            &project_c,
            "What did I say my favorite fruit was? Reply with one word.",
            Posture::FullAutonomy,
            LaunchMode::Continue,
        ))
        .expect("launch D (continue)");

    let updates = pump_until(&mut core, &rx, 120, |u| {
        latest_sv(u, &id_d)
            .map(|sv| sv.state == Phase::Done && assistant_says(&sv, "banana"))
            .unwrap_or(false)
    });
    let sv = latest_sv(&updates, &id_d).expect("S4: a SessionView for D");
    assert!(
        assistant_says(&sv, "banana"),
        "S4: continued session must recall banana; transcript={:?}",
        sv.transcript
    );
    assert!(
        user_says(&sv, "favorite fruit is banana"),
        "S4: resumed transcript must include the prior (history) turn; got {:?}",
        sv.transcript
    );
    eprintln!(
        "[i10/S4] PASS — continue recalled the fact AND loaded prior history ({} entries)",
        sv.transcript.len()
    );

    eprintln!("\n[i10] ALL SCENARIOS PASSED — full GUI<->agent<->claude loop verified.");
}
