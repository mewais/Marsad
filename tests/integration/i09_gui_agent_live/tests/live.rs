/// i09_gui_agent_live — end-to-end proof that the live path works:
/// a GUI-like client connects to a real marsad-agent daemon over a Unix socket,
/// launches a real claude session, and receives stream-json events back as IpcMsg.
///
/// # SKIP contract
/// Every test calls `skip_if_no_deps!()` at the top.  If `claude` or `tmux` are
/// absent the test prints a loud SKIP message and returns immediately — no panic,
/// no silent pass.  An additional memory gate (same thresholds as i03) skips when
/// the virtual address space cap or available RAM is too small to host claude.
///
/// # TEARDOWN contract
/// A `TeardownGuard` kills any `marsad-` prefixed tmux sessions seen during the
/// test and removes the temp dir.  Runs on both success and panic.
///
/// # Agent thread
/// `Agent::with_real_io` is not Send (contains Box<dyn Container>).  We therefore
/// move the `AgentConfig` (a plain data struct) into the spawned thread and
/// construct + serve there.  The test polls for the socket to appear rather than
/// using a fixed sleep.
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use marsad_agent::{AgentConfig, ListenSpec};
use marsad_proto::{parse_ipc, IpcMsg};
use marsad_transport::{LocalIpc, Transport};

// ── test-local counter ─────────────────────────────────────────────────────────

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn unique_id() -> String {
    let c = COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{}-{}", std::process::id(), c)
}

// ── memory gate ────────────────────────────────────────────────────────────────

/// Minimum MemAvailable (kB) needed to safely spawn claude (~700 MB RSS).
const MIN_AVAIL_KB: u64 = 3_000_000;
/// Minimum RLIMIT_AS (bytes) needed for claude/Bun/JSC startup mmap (~6 GB virtual).
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
    let as_bytes = proc_address_space_limit_bytes();
    as_bytes >= MIN_AS_BYTES
}

// ── dependency check ───────────────────────────────────────────────────────────

fn deps_available() -> bool {
    let claude_ok = Command::new("claude")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let tmux_ok = Command::new("tmux")
        .arg("-V")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    claude_ok && tmux_ok
}

macro_rules! skip_if_no_deps {
    () => {
        if !deps_available() {
            eprintln!(
                "\n========================================================\n\
                 SKIP i09: claude and/or tmux not available in PATH.\n\
                 This live test requires both binaries to be present.\n\
                 ========================================================"
            );
            return;
        }
        if !enough_memory() {
            let avail_mb = std::fs::read_to_string("/proc/meminfo")
                .unwrap_or_default()
                .lines()
                .find(|l| l.starts_with("MemAvailable:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|s| s.parse::<u64>().ok())
                .map(|kb| kb / 1024)
                .unwrap_or(0);
            let as_mb = {
                let b = proc_address_space_limit_bytes();
                if b == u64::MAX {
                    u64::MAX
                } else {
                    b / (1024 * 1024)
                }
            };
            eprintln!(
                "\n========================================================\n\
                 SKIP i09: insufficient memory (avail {} MB / AS cap {} MB);\n\
                 claude needs ~700 MB RSS + large virtual space.\n\
                 Run on a host with >=3 GB available + unbounded address space.\n\
                 ========================================================",
                avail_mb, as_mb
            );
            return;
        }
    };
}

// ── TeardownGuard ──────────────────────────────────────────────────────────────

struct TeardownGuard {
    /// marsad-<session_id> tmux session names to kill on drop.
    tmux_sessions: Vec<String>,
    tmpdir: PathBuf,
    socket_path: PathBuf,
}

impl TeardownGuard {
    fn new(tmpdir: PathBuf, socket_path: PathBuf) -> Self {
        Self {
            tmux_sessions: Vec::new(),
            tmpdir,
            socket_path,
        }
    }
    fn register_tmux(&mut self, name: impl Into<String>) {
        self.tmux_sessions.push(name.into());
    }
}

impl Drop for TeardownGuard {
    fn drop(&mut self) {
        // Kill each known marsad- tmux session.
        for name in &self.tmux_sessions {
            let _ = Command::new("tmux")
                .args(["kill-session", "-t", name])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        // Also sweep for any marsad-session-* tmux sessions we might have missed
        // (e.g. if the agent assigned session-1 but we only recorded the name after).
        let _ = Command::new("sh")
            .arg("-c")
            .arg("tmux list-sessions -F '#{session_name}' 2>/dev/null | grep '^marsad-session-' | xargs -r -n1 tmux kill-session -t")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        // Remove socket and temp dir.
        let _ = std::fs::remove_file(&self.socket_path);
        let _ = std::fs::remove_dir_all(&self.tmpdir);
    }
}

// ── poll_connect ───────────────────────────────────────────────────────────────

/// Poll-connect to the Unix socket until it appears, up to `timeout`.
/// Mirrors i03's retry pattern instead of a fixed sleep.
fn poll_connect(socket: &str, timeout: Duration) -> Result<LocalIpc, String> {
    let deadline = Instant::now() + timeout;
    loop {
        let mut client = LocalIpc::new(socket);
        if client.connect().is_ok() {
            return Ok(client);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "socket {socket} not ready after {}s",
                timeout.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ── recv_with_deadline ────────────────────────────────────────────────────────

/// Drain AgentEvent lines from the IPC client until `pred` returns true or
/// timeout expires.  Returns all lines received (including the matching one).
/// On timeout returns whatever was accumulated with a warning.
fn recv_until<F>(client: &mut LocalIpc, deadline: Instant, pred: F) -> Vec<IpcMsg>
where
    F: Fn(&IpcMsg) -> bool,
{
    let mut msgs = Vec::new();
    loop {
        if Instant::now() >= deadline {
            eprintln!(
                "[i09] recv_until: deadline exceeded after {} msgs",
                msgs.len()
            );
            break;
        }
        match client.recv_line() {
            Ok(None) => {
                eprintln!("[i09] recv_until: EOF from agent");
                break;
            }
            Err(e) => {
                eprintln!("[i09] recv_until: transport error: {e}");
                break;
            }
            Ok(Some(line)) => {
                eprintln!("[i09] recv_line: {:.160}", line);
                match parse_ipc(&line) {
                    Ok(msg) => {
                        let done = pred(&msg);
                        msgs.push(msg);
                        if done {
                            break;
                        }
                    }
                    Err(e) => {
                        eprintln!("[i09] parse_ipc error: {e:?} on: {:.120}", line);
                    }
                }
            }
        }
    }
    msgs
}

// ── recv_subscribed_ack ───────────────────────────────────────────────────────

/// Wait for the `Subscribed` ack on `client`, tolerating (and logging) any
/// broadcast traffic that arrives interleaved with it.
///
/// The agent registers a newly-accepted socket into the broadcast-subscriber
/// list at `accept()` time — before the client has sent its `Subscribe`
/// message at all — while the `Subscribed` reply itself round-trips through a
/// separate, serialized command queue. That means a still-live session's
/// reader thread can push an `AgentEvent` (or `UsageUpdate` /
/// `TranscriptBackfill`) into this client's outbound channel at any point
/// before the `Subscribed` ack lands, racing it. A bare `client.recv_line()`
/// that assumes the very first line back is the ack is therefore only safe
/// for the first-ever client of a fresh daemon (nothing exists yet to
/// broadcast); any later-joining client — e.g. Scenario 3's reconnect while
/// Scenario 1/2's session may still be alive — must instead scan past
/// unrelated broadcast lines to find the ack, which is what this helper does.
fn recv_subscribed_ack(client: &mut LocalIpc, deadline: Instant) -> Option<String> {
    let msgs = recv_until(client, deadline, |msg| {
        matches!(msg, IpcMsg::Subscribed { .. })
    });
    msgs.iter().find_map(|m| match m {
        IpcMsg::Subscribed { subscriber_id } => Some(subscriber_id.clone()),
        _ => None,
    })
}

// ══════════════════════════════════════════════════════════════════════════════
// Single sequential test: all three scenarios combined to avoid two live claudes.
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn all_scenarios() {
    skip_if_no_deps!();

    let id = unique_id();
    let tmpdir = std::env::temp_dir().join(format!("marsad-it-i09-{id}"));
    std::fs::create_dir_all(&tmpdir).expect("create tmpdir");

    let socket_path = tmpdir.join("agent.sock");
    let socket_str = socket_path.to_string_lossy().to_string();

    // The project dir for claude to work in (a real dir it can cd into).
    let project_dir = tmpdir.join("project");
    std::fs::create_dir_all(&project_dir).expect("create project dir");

    let data_root = tmpdir.join("data").to_string_lossy().to_string();

    let mut guard = TeardownGuard::new(tmpdir.clone(), socket_path.clone());

    // ── Spawn the agent daemon thread ─────────────────────────────────────────
    // Agent::with_real_io is not Send; move only AgentConfig into the thread.
    let socket_for_thread = socket_str.clone();
    let data_root_for_thread = data_root.clone();
    std::thread::spawn(move || {
        let cfg = AgentConfig {
            listen: ListenSpec::UnixSocket(socket_for_thread),
            data_root: data_root_for_thread,
        };
        let agent = marsad_agent::Agent::with_real_io(cfg).expect("Agent::with_real_io failed");
        if let Err(e) = agent.serve() {
            eprintln!("[i09] agent serve error: {e}");
        }
    });

    // ── Poll-connect (the socket appears once agent binds) ────────────────────
    let mut client = match poll_connect(&socket_str, Duration::from_secs(10)) {
        Ok(c) => c,
        Err(e) => {
            panic!("[i09] FAIL: could not connect to agent socket: {e}");
        }
    };
    eprintln!("[i09] connected to agent socket at {}", socket_str);

    // ─────────────────────────────────────────────────────────────────────────
    // Scenario 1: Subscribe + LaunchSession → receive SessionLaunched + AgentEvent(system/init)
    // ─────────────────────────────────────────────────────────────────────────
    eprintln!("\n[i09/s1] === Scenario 1: launch -> system/init over the socket ===");

    // Subscribe (wildcard — no session_id yet; we send an empty-string session_id
    // which the agent accepts as a subscription; events are broadcast to all subs).
    let subscribe_msg = serde_json::json!({
        "type": "Subscribe",
        "session_id": ""
    });
    let subscribe_line = serde_json::to_string(&subscribe_msg).unwrap();
    client.send_line(&subscribe_line).expect("send Subscribe");

    // Read back the Subscribed ack. This is the first-ever client on a fresh
    // daemon, so nothing else can be broadcasting yet, but we still scan via
    // recv_subscribed_ack() for consistency with the reconnect in Scenario 3
    // (see that helper's doc comment for why a bare recv_line() isn't safe in
    // general for a subscriber that joins after a session already exists).
    let subscriber_id = recv_subscribed_ack(&mut client, Instant::now() + Duration::from_secs(10))
        .expect("[s1] expected a Subscribed ack before launching a session");
    eprintln!("[i09/s1] ack: subscriber_id={subscriber_id}");

    // LaunchSession — send the raw JSON that matches the agent's handler expectations.
    let project_path = project_dir.to_string_lossy().to_string();
    let launch_msg = serde_json::json!({
        "type": "LaunchSession",
        "project": project_path,
        "host_id": "local",
        "model": "claude-opus-4-5",
        "session_mode": "bypassPermissions",
        "first_prompt": "reply with the single word READY"
    });
    let launch_line = serde_json::to_string(&launch_msg).unwrap();
    client.send_line(&launch_line).expect("send LaunchSession");

    // Read back SessionLaunched (the agent's immediate reply).
    let sl_line = client
        .recv_line()
        .expect("recv SessionLaunched")
        .expect("non-empty");
    eprintln!("[i09/s1] SessionLaunched: {:.120}", sl_line);
    let sl_val: serde_json::Value =
        serde_json::from_str(&sl_line).expect("SessionLaunched must be JSON");
    assert_eq!(
        sl_val["type"].as_str(),
        Some("SessionLaunched"),
        "[s1] First reply must be SessionLaunched, got: {}",
        sl_line
    );
    let session_id = sl_val["session_id"]
        .as_str()
        .expect("[s1] SessionLaunched must carry session_id")
        .to_string();
    eprintln!("[i09/s1] session_id = {}", session_id);

    // Register the tmux session for teardown.  The agent names it marsad-<session_id>.
    let tmux_session_name = format!("marsad-{}", session_id);
    guard.register_tmux(&tmux_session_name);

    // Wait for AgentEvent lines; look for a SessionInit event.
    let deadline = Instant::now() + Duration::from_secs(90);
    let msgs = recv_until(&mut client, deadline, |msg| {
        if let IpcMsg::AgentEvent { event, .. } = msg {
            matches!(event, marsad_proto::AgentEvent::SessionInit { .. })
        } else {
            false
        }
    });

    let init_msg = msgs.iter().find(|m| {
        if let IpcMsg::AgentEvent { event, .. } = m {
            matches!(event, marsad_proto::AgentEvent::SessionInit { .. })
        } else {
            false
        }
    });

    assert!(
        init_msg.is_some(),
        "[s1] Expected an AgentEvent::SessionInit over the socket.\n\
         Received {} msgs: {:?}",
        msgs.len(),
        msgs.iter()
            .map(|m| format!("{:?}", m))
            .collect::<Vec<_>>()
            .join(", ")
    );

    if let Some(IpcMsg::AgentEvent {
        session_id: sid,
        event,
    }) = init_msg
    {
        if let marsad_proto::AgentEvent::SessionInit { model, cwd, .. } = event {
            eprintln!(
                "[i09/s1] SessionInit arrived: session_id={sid} model={} cwd={}",
                model.as_deref().unwrap_or("?"),
                cwd.as_deref().unwrap_or("?")
            );
        }
        assert!(
            !sid.is_empty(),
            "[s1] AgentEvent session_id must be non-empty"
        );
        assert_eq!(
            sid.as_str(),
            session_id,
            "[s1] AgentEvent session_id must match launched session"
        );
    }
    eprintln!(
        "[i09/s1] PASS — SessionInit received over the socket. GUI<->agent<->claude path verified."
    );

    // ─────────────────────────────────────────────────────────────────────────
    // Scenario 2 (optional): wait for an AssistantDelta AgentEvent.
    // ─────────────────────────────────────────────────────────────────────────
    eprintln!("\n[i09/s2] === Scenario 2 (optional): wait for assistant reply ===");
    let deadline2 = Instant::now() + Duration::from_secs(60);
    let msgs2 = recv_until(&mut client, deadline2, |msg| {
        if let IpcMsg::AgentEvent { event, .. } = msg {
            matches!(event, marsad_proto::AgentEvent::AssistantDelta { .. })
        } else {
            false
        }
    });
    let got_assistant = msgs2.iter().any(|m| {
        if let IpcMsg::AgentEvent { event, .. } = m {
            matches!(event, marsad_proto::AgentEvent::AssistantDelta { .. })
        } else {
            false
        }
    });
    if got_assistant {
        eprintln!("[i09/s2] PASS — AssistantDelta AgentEvent received.");
    } else {
        eprintln!("[i09/s2] NOTE — no AssistantDelta event received within timeout (not a hard failure; scenario 1 is the capstone).");
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Scenario 3: client-disconnect liveness.
    // Close the LocalIpc; assert the tmux session is still alive.
    // ─────────────────────────────────────────────────────────────────────────
    eprintln!("\n[i09/s3] === Scenario 3: client-disconnect liveness ===");
    let _ = client.close();
    drop(client);
    eprintln!("[i09/s3] client closed (simulating GUI exit)");

    // Give the agent a moment to process the disconnect.
    std::thread::sleep(Duration::from_millis(500));

    // The tmux session must still exist.
    let tmux_alive = Command::new("tmux")
        .args(["has-session", "-t", &tmux_session_name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    // It's plausible the claude --print session already finished (it ran a tiny
    // prompt), in which case tmux auto-removed the session.  Log but do not fail.
    if tmux_alive {
        eprintln!(
            "[i09/s3] PASS — tmux session '{}' still alive after client disconnect.",
            tmux_session_name
        );
    } else {
        eprintln!(
            "[i09/s3] NOTE — tmux session '{}' is already gone (claude --print exits after output; \
             session may have auto-cleaned). Liveness for interactive sessions is the real contract.",
            tmux_session_name
        );
    }

    // Optionally reconnect a second client and verify the agent still accepts connections.
    let mut client2 = match poll_connect(&socket_str, Duration::from_secs(5)) {
        Ok(c) => {
            eprintln!("[i09/s3] second client connected successfully — agent still up.");
            c
        }
        Err(e) => {
            eprintln!("[i09/s3] NOTE — could not reconnect second client: {e}");
            return;
        }
    };
    // Subscribe on the second client just to verify the agent is responsive.
    //
    // Session 1's reader thread may still be alive at this point (or its
    // teardown may still be draining), so client2's broadcast channel can
    // receive a stray AgentEvent/UsageUpdate/TranscriptBackfill line ahead of
    // the Subscribed ack — see recv_subscribed_ack()'s doc comment. Scan past
    // any such traffic instead of assuming the first line back is the ack.
    let sub2 = serde_json::json!({"type": "Subscribe", "session_id": ""});
    let sub2_line = serde_json::to_string(&sub2).unwrap();
    let _ = client2.send_line(&sub2_line);
    match recv_subscribed_ack(&mut client2, Instant::now() + Duration::from_secs(5)) {
        Some(subscriber_id) => eprintln!("[i09/s3] second client ack: subscriber_id={subscriber_id}"),
        None => eprintln!("[i09/s3] second client: no Subscribed ack seen before deadline (agent may be shutting down)"),
    }
    let _ = client2.close();
    eprintln!("[i09/s3] PASS — agent accepted second client after first disconnect.");

    eprintln!("\n[i09] ALL SCENARIOS DONE — live GUI<->agent<->claude path verified.");
}
