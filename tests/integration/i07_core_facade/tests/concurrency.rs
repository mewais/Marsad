//! i07 — Core facade & concurrency — LIVE integration tests.
//!
//! Scenario: N=3 real `claude` sessions, each in its own tmux session + FIFO
//! pair, advance independently.  No cross-routing of prompts or responses.
//!
//! SKIP LOUDLY (no panic, no silent pass) when `claude` or `tmux` is absent.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use wasit_claude::LineParser;
use wasit_core::AgentEvent;

// ── unique ID generation ──────────────────────────────────────────────────────

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn unique_id() -> String {
    let pid = std::process::id();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{pid}-{seq}")
}

// ── binary detection ──────────────────────────────────────────────────────────

fn find_binary(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var("PATH").unwrap_or_default();
    for dir in path_var.split(':') {
        let candidate = PathBuf::from(dir).join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

// ── mkfifo helper ─────────────────────────────────────────────────────────────

fn mkfifo(path: &Path) -> Result<(), String> {
    let st = std::process::Command::new("mkfifo")
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("mkfifo: {e}"))?;
    if st.success() {
        Ok(())
    } else {
        Err(format!("mkfifo {}: {st}", path.display()))
    }
}

// ── per-session helper ────────────────────────────────────────────────────────

/// One live session: temp dir, tmux name, FIFO paths.
struct LiveSession {
    id: String,
    tmux_name: String,
    dir: PathBuf,
    proj_dir: PathBuf,
}

impl LiveSession {
    fn new(unique: &str) -> Self {
        let id = format!("i07-{unique}");
        let tmux_name = format!("marsad-it-i07-{unique}");
        let dir = std::env::temp_dir().join(format!("marsad-it-i07-{unique}"));
        let proj_dir = dir.join("proj");
        LiveSession {
            id,
            tmux_name,
            dir,
            proj_dir,
        }
    }

    fn in_fifo(&self) -> PathBuf {
        self.dir.join("in.fifo")
    }
    fn out_fifo(&self) -> PathBuf {
        self.dir.join("out.fifo")
    }

    /// Create temp dirs and FIFOs on disk.
    fn prepare(&self) -> Result<(), String> {
        std::fs::create_dir_all(&self.proj_dir).map_err(|e| format!("mkdir proj: {e}"))?;
        std::fs::write(self.proj_dir.join("README.md"), b"marsad i07 test\n")
            .map_err(|e| format!("write README: {e}"))?;
        mkfifo(&self.in_fifo())?;
        mkfifo(&self.out_fifo())?;
        Ok(())
    }

    /// Launch `claude` inside a detached tmux session wired to our FIFOs.
    ///
    /// `--bare` suppresses CLAUDE.md discovery, MCP servers, hooks and
    /// background prefetches — the session starts faster and produces cleaner
    /// stream-json output with no cross-session state.
    fn spawn_in_tmux(&self, claude_bin: &Path) -> Result<(), String> {
        let in_fifo = self.in_fifo();
        let out_fifo = self.out_fifo();
        let err_log = self.dir.join("err.log");

        // NOTE: stdin must be an anonymous pipe (`cat fifo |`), not a direct
        // named-FIFO redirect — claude exits silently with `< fifo` stdin when
        // a controlling terminal exists (any tmux pane). See fa327cb / realio.md.
        let cmd = format!(
            "cd '{proj}' && cat '{in_f}' | '{claude}' \
             --print --input-format stream-json --output-format stream-json \
             --verbose --model claude-haiku-4-5 --permission-mode default \
             > '{out_f}' 2>'{err}'",
            proj = self.proj_dir.display(),
            claude = claude_bin.display(),
            in_f = in_fifo.display(),
            out_f = out_fifo.display(),
            err = err_log.display(),
        );

        let st = std::process::Command::new("tmux")
            .args(["new-session", "-d", "-s", &self.tmux_name, &cmd])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| format!("tmux new-session: {e}"))?;

        if st.success() {
            Ok(())
        } else {
            Err(format!(
                "tmux new-session -d -s {} failed: {st}",
                self.tmux_name
            ))
        }
    }

    /// Send `prompt_text` to this session and collect all `AgentEvent`s until
    /// a `TurnCompleted` event arrives or `timeout` elapses.
    ///
    /// Open ordering (deadlock-free, per realio.md):
    ///   1. A background thread opens `out.fifo` for reading — this unblocks
    ///      claude's `> out.fifo` shell redirect inside tmux.
    ///   2. The main thread opens `in.fifo` for writing — by the time step 2
    ///      runs, claude has already opened `in.fifo` for reading (because
    ///      step 1 let it proceed past its stdout open), so we don't block.
    fn run_prompt(&self, prompt_text: &str, timeout: Duration) -> Result<Vec<AgentEvent>, String> {
        let out_fifo = self.out_fifo();
        let in_fifo = self.in_fifo();

        // Background reader thread: opens out.fifo (blocks until claude's tmux
        // session reaches the `> out.fifo` part of its shell command), then
        // streams lines to us via a channel.
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || {
            let file = match std::fs::File::open(&out_fifo) {
                Ok(f) => f,
                Err(e) => {
                    let _ = tx.send(format!("__ERR__: open out.fifo: {e}"));
                    return;
                }
            };
            for line in BufReader::new(file).lines() {
                match line {
                    Ok(l) => {
                        // Send the raw line; the main thread parses it.
                        // Keep sending even after result so the parser sees it.
                        let done = l.contains("\"type\":\"result\"");
                        if tx.send(l).is_err() {
                            break;
                        }
                        if done {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Small pause: give tmux a moment to exec the shell command before we
        // try to open in.fifo (which would block if claude hasn't started yet).
        std::thread::sleep(Duration::from_millis(600));

        // Encode the user message via wasit_claude encoder.
        let prompt_json = wasit_claude::encode_user_message(prompt_text);

        // Open in.fifo for writing (blocks until claude opens it for reading —
        // which happens right after its stdout redirect unblocks in step 1).
        let mut writer = std::fs::OpenOptions::new()
            .write(true)
            .open(&in_fifo)
            .map_err(|e| format!("[{}] open in.fifo: {e}", self.id))?;
        writeln!(writer, "{prompt_json}")
            .map_err(|e| format!("[{}] write in.fifo: {e}", self.id))?;
        drop(writer); // EOF → claude processes the input and exits cleanly

        // Drain the reader thread's channel with a deadline.
        let deadline = Instant::now() + timeout;
        let mut events: Vec<AgentEvent> = Vec::new();
        let mut parser = LineParser::new();

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match rx.recv_timeout(remaining.min(Duration::from_millis(500))) {
                Ok(line) => {
                    if line.starts_with("__ERR__:") {
                        return Err(format!("[{}] {line}", self.id));
                    }
                    for event in parser.on_line(&line) {
                        let done = matches!(event, AgentEvent::TurnCompleted { .. });
                        events.push(event);
                        if done {
                            // Drain the rest of the loop via break.
                            break;
                        }
                    }
                    // Also break the outer loop if we got TurnCompleted.
                    if events.iter().any(|e| matches!(e, AgentEvent::TurnCompleted { .. })) {
                        break;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // Check if we already have a TurnCompleted (channel closed normally).
                    if events.iter().any(|e| matches!(e, AgentEvent::TurnCompleted { .. })) {
                        break;
                    }
                }
            }
        }

        if events.is_empty() {
            // Dump err.log for diagnostics.
            let err_log = self.dir.join("err.log");
            let err_content = std::fs::read_to_string(&err_log).unwrap_or_default();
            return Err(format!(
                "[{}] no events within {:?}; err.log: {:?}",
                self.id,
                timeout,
                err_content.lines().take(5).collect::<Vec<_>>().join(" | ")
            ));
        }

        Ok(events)
    }

    /// Kill the tmux session and remove the temp directory.
    fn teardown(&self) {
        let _ = std::process::Command::new("tmux")
            .args(["kill-session", "-t", &self.tmux_name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// ── RAII guard — teardown on success AND panic ────────────────────────────────

struct TeardownGuard(Vec<LiveSession>);

impl Drop for TeardownGuard {
    fn drop(&mut self) {
        for s in &self.0 {
            s.teardown();
        }
    }
}

// ── AgentEvent assertion helpers ──────────────────────────────────────────────
//
// Wave 1 replaced the old StreamMsg/ContentBlock/ResultSubtype with AgentEvent.
// These helpers mirror the original assertions using the new event shapes.

fn has_system_init(events: &[AgentEvent]) -> bool {
    events
        .iter()
        .any(|e| matches!(e, AgentEvent::SessionInit { .. }))
}

/// True if any AssistantDelta with non-empty text was received.
fn has_assistant_text(events: &[AgentEvent]) -> bool {
    events.iter().any(|e| {
        if let AgentEvent::AssistantDelta { text, .. } = e {
            !text.is_empty()
        } else {
            false
        }
    })
}

/// True if a successful TurnCompleted (not interrupted) was received.
fn has_result_success(events: &[AgentEvent]) -> bool {
    events.iter().any(|e| {
        if let AgentEvent::TurnCompleted { was_interrupted, .. } = e {
            !was_interrupted
        } else {
            false
        }
    })
}

fn session_id_of(events: &[AgentEvent]) -> Option<String> {
    events.iter().find_map(|e| {
        if let AgentEvent::SessionInit {
            native_session_id, ..
        } = e
        {
            native_session_id.clone()
        } else {
            None
        }
    })
}

/// Return the concatenated text of all AssistantDelta events.
fn first_assistant_text(events: &[AgentEvent]) -> String {
    events
        .iter()
        .find_map(|e| {
            if let AgentEvent::AssistantDelta { text, .. } = e {
                if !text.is_empty() {
                    return Some(text.clone());
                }
            }
            None
        })
        .unwrap_or_default()
}

// ─────────────────────────────────────────────────────────────────────────────
// Scenario 1 + 2: concurrent independent advance & no cross-routing
// ─────────────────────────────────────────────────────────────────────────────

/// Launch N=3 real sessions concurrently, send each a distinct prompt, and
/// assert:
///   1. Each session emits `system/init` → `assistant` (text) → `result/success`
///      independently on its own out.fifo (never another session's out.fifo).
///   2. Each response contains the keyword from its own prompt, not another
///      session's keyword (no cross-routing).
///   3. All three claude `session_id` values (from `system/init`) are distinct.
#[test]
fn test_concurrent_independent_advance_and_no_cross_routing() {
    // ── skip if binaries absent ───────────────────────────────────────────────
    let claude_bin = match find_binary("claude") {
        Some(p) => p,
        None => {
            eprintln!("SKIP i07/concurrency: `claude` binary not found in PATH");
            return;
        }
    };
    if find_binary("tmux").is_none() {
        eprintln!("SKIP i07/concurrency: `tmux` binary not found in PATH");
        return;
    }

    // ── create sessions ───────────────────────────────────────────────────────
    let uid = unique_id();
    let sessions = TeardownGuard(vec![
        LiveSession::new(&format!("{uid}-a")),
        LiveSession::new(&format!("{uid}-b")),
        LiveSession::new(&format!("{uid}-c")),
    ]);

    // Distinct prompts → distinct expected keywords in responses.
    let prompts = [
        "reply with the single word ALPHA",
        "reply with the single word BETA",
        "reply with the single word GAMMA",
    ];
    let expected = ["ALPHA", "BETA", "GAMMA"];

    for s in &sessions.0 {
        s.prepare()
            .unwrap_or_else(|e| panic!("prepare {}: {e}", s.id));
    }

    // ── spawn all three sessions in tmux ──────────────────────────────────────
    for s in &sessions.0 {
        s.spawn_in_tmux(&claude_bin)
            .unwrap_or_else(|e| panic!("spawn {}: {e}", s.id));
    }

    // ── run prompts concurrently ──────────────────────────────────────────────
    let timeout = Duration::from_secs(90);
    let results: Vec<Result<Vec<AgentEvent>, String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = sessions
            .0
            .iter()
            .zip(prompts.iter())
            .map(|(s, p)| scope.spawn(move || s.run_prompt(p, timeout)))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    // ── per-session assertions ────────────────────────────────────────────────
    let mut session_ids: Vec<String> = Vec::new();

    for (i, res) in results.iter().enumerate() {
        let msgs = res
            .as_ref()
            .unwrap_or_else(|e| panic!("session {} failed: {e}", sessions.0[i].id));

        assert!(
            has_system_init(msgs),
            "session {} missing system/init (got {} msgs)",
            sessions.0[i].id,
            msgs.len()
        );
        assert!(
            has_assistant_text(msgs),
            "session {} missing assistant text (got {} msgs)",
            sessions.0[i].id,
            msgs.len()
        );
        assert!(
            has_result_success(msgs),
            "session {} missing result/success (got {} msgs)",
            sessions.0[i].id,
            msgs.len()
        );

        // No cross-routing: the assistant text must contain this session's keyword.
        let text = first_assistant_text(msgs);
        assert!(
            text.to_uppercase().contains(expected[i]),
            "session {} cross-routing detected — expected keyword {:?}, got {:?}",
            sessions.0[i].id,
            expected[i],
            &text[..text.len().min(120)]
        );

        if let Some(sid) = session_id_of(msgs) {
            session_ids.push(sid);
        }
    }

    // All three claude session_ids must be distinct.
    assert_eq!(
        session_ids.len(),
        3,
        "expected 3 session_ids, got {}: {:?}",
        session_ids.len(),
        session_ids
    );
    let unique_set: HashSet<_> = session_ids.iter().collect();
    assert_eq!(
        unique_set.len(),
        3,
        "session_ids not all distinct: {:?}",
        session_ids
    );

    eprintln!(
        "PASS i07/concurrency: 3 concurrent sessions ran independently; \
         session_ids={:?}",
        session_ids
    );
    // TeardownGuard drops here → kills all tmux sessions + removes temp dirs.
}

// ─────────────────────────────────────────────────────────────────────────────
// Scenario 3 (optional): interrupt one session; others survive
// ─────────────────────────────────────────────────────────────────────────────

/// Interrupt session A (by killing its tmux session) while B and C are running
/// their prompts.  Assert B and C still complete successfully.
///
/// This proves that per-session isolation holds: tearing down one session's
/// FIFO/tmux pair has no effect on the others.
#[test]
fn test_interrupt_one_session_others_unaffected() {
    let claude_bin = match find_binary("claude") {
        Some(p) => p,
        None => {
            eprintln!("SKIP i07/interrupt: `claude` binary not found in PATH");
            return;
        }
    };
    if find_binary("tmux").is_none() {
        eprintln!("SKIP i07/interrupt: `tmux` binary not found in PATH");
        return;
    }

    let uid = unique_id();
    let sessions = TeardownGuard(vec![
        LiveSession::new(&format!("{uid}-int-a")), // will be interrupted
        LiveSession::new(&format!("{uid}-int-b")), // must survive
        LiveSession::new(&format!("{uid}-int-c")), // must survive
    ]);

    for s in &sessions.0 {
        s.prepare()
            .unwrap_or_else(|e| panic!("prepare {}: {e}", s.id));
    }

    for s in &sessions.0 {
        s.spawn_in_tmux(&claude_bin)
            .unwrap_or_else(|e| panic!("spawn {}: {e}", s.id));
    }

    let timeout = Duration::from_secs(90);
    let tmux_a = sessions.0[0].tmux_name.clone();

    let (res_b, res_c) = std::thread::scope(|scope| {
        // B and C run their full prompts.
        let hb =
            scope.spawn(|| sessions.0[1].run_prompt("reply with the single word BETA", timeout));
        let hc =
            scope.spawn(|| sessions.0[2].run_prompt("reply with the single word GAMMA", timeout));

        // A is interrupted shortly after launch: kill its tmux session.
        // This closes in.fifo (EOF) and out.fifo, simulating an interrupt.
        std::thread::sleep(Duration::from_millis(800));
        let kill_st = std::process::Command::new("tmux")
            .args(["kill-session", "-t", &tmux_a])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        eprintln!(
            "i07/interrupt: killed tmux session A ({}): {:?}",
            tmux_a, kill_st
        );

        (hb.join().unwrap(), hc.join().unwrap())
    });

    for (label, res) in [("B", &res_b), ("C", &res_c)] {
        let msgs = res
            .as_ref()
            .unwrap_or_else(|e| panic!("session {label} failed after A interrupted: {e}"));
        assert!(
            has_system_init(msgs),
            "session {label} missing system/init after interrupt"
        );
        assert!(
            has_assistant_text(msgs),
            "session {label} missing assistant text after interrupt"
        );
        assert!(
            has_result_success(msgs),
            "session {label} missing result/success after interrupt"
        );
    }

    eprintln!("PASS i07/interrupt: session A interrupted; B and C completed independently");
    // TeardownGuard drops → all three sessions torn down.
}
