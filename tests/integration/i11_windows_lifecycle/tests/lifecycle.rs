// i11_windows_lifecycle — Windows Named Pipe + shim integration test.
//
// Verifies that `WindowsShimContainer` + `ClaudeNamedPipeTransport` provide
// the same session-liveness and reconnect guarantees on Windows that
// `TmuxContainer` + `ClaudeFifoTransport` provide on Linux/macOS.
//
// # SKIP contract
//
// Every test calls `skip_if_no_deps!()` at the top.  The macro skips LOUDLY
// (prints a conspicuous banner, then `return`s) when ANY of the following are
// true:
//   - We are not running on Windows (compiling with `cfg(not(windows))`).
//   - `marsad-winshim.exe` is not found next to the test binary or on PATH.
//   - `claude` is not found on PATH.
//
// A SKIP is NOT a pass: the CI script for the Windows job must assert that
// this test ran and did not skip when `claude` and `marsad-winshim.exe` are
// available.
//
// # What is tested (Windows only)
//
// 1. `spawn()` launches `marsad-winshim.exe`, which creates the Named Pipes
//    and starts `claude`.  A `shim.pid` file appears within 5 s.
// 2. `exists()` probes the shim PID and returns `true` while it is alive.
// 3. `ClaudeNamedPipeTransport::connect()` opens the client ends successfully
//    within 15 s.
// 4. A `system` (system/init) NDJSON line is received from claude via the OUT
//    pipe within 90 s.
// 5. `kill()` terminates the shim (and transitively claude).  `exists()` then
//    returns `false`.
// 6. After `kill()`, re-connecting to the same pipe names fails (pipe is gone).
// 7. `spawn()` on an already-alive session (idempotent path) returns `Ok`
//    without starting a second shim.
//
// # Teardown
//
// A `TeardownGuard` removes the temp directory and attempts to kill any
// surviving shim processes by reading `shim.pid`.  Runs on both success and
// panic so no orphans leak.

// ── Platform gate — the whole test body is gated on Windows ──────────────────
//
// On Linux/macOS this file still compiles (it is part of `cargo build
// --workspace`) but the single test function skips immediately.  This keeps the
// test discoverable by `cargo test` without requiring a Windows runner.

fn deps_available() -> bool {
    // Only meaningful on Windows; on other platforms always return false so the
    // skip_if_no_deps! macro fires immediately.
    #[cfg(not(windows))]
    return false;

    // On Windows: check for marsad-winshim.exe and claude.
    #[cfg(windows)]
    {
        use std::process::{Command, Stdio};

        // 1. marsad-winshim.exe — look next to the test binary first, then PATH.
        let shim_ok = {
            let candidate = std::env::current_exe()
                .ok()
                .map(|exe| exe.with_file_name("marsad-winshim.exe"));
            match candidate {
                Some(p) if p.exists() => true,
                _ => Command::new("marsad-winshim.exe")
                    .arg("--help")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .map(|_| true)
                    .unwrap_or(false),
            }
        };

        // 2. claude — must be on PATH.
        let claude_ok = Command::new("claude")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        shim_ok && claude_ok
    }
}

macro_rules! skip_if_no_deps {
    () => {
        if cfg!(not(windows)) {
            eprintln!(
                "\n========================================================\n\
                 SKIP i11: this test only runs on Windows.\n\
                 Current platform: {}\n\
                 ========================================================",
                std::env::consts::OS
            );
            return;
        }
        if !deps_available() {
            eprintln!(
                "\n========================================================\n\
                 SKIP i11: marsad-winshim.exe and/or claude not available.\n\
                 To run this test on Windows:\n\
                   1. cargo build --workspace (builds marsad-winshim.exe)\n\
                   2. Ensure claude.exe is on PATH.\n\
                   3. Re-run: cargo test -p marsad-it-i11_windows_lifecycle\n\
                 ========================================================"
            );
            return;
        }
    };
}

// ── TeardownGuard ─────────────────────────────────────────────────────────────
//
// Used only on Windows (inside windows_all_scenarios).  Gated here to avoid
// dead-code warnings on Linux.

#[cfg(windows)]
struct TeardownGuard {
    tmpdir: std::path::PathBuf,
    /// Path to `shim.pid` for the test session (used for best-effort kill).
    pid_file: Option<std::path::PathBuf>,
}

#[cfg(windows)]
impl TeardownGuard {
    fn new(tmpdir: std::path::PathBuf) -> Self {
        Self {
            tmpdir,
            pid_file: None,
        }
    }

    fn set_pid_file(&mut self, p: std::path::PathBuf) {
        self.pid_file = Some(p);
    }
}

#[cfg(windows)]
impl Drop for TeardownGuard {
    fn drop(&mut self) {
        // Best-effort: kill the shim if the pid file exists.
        if let Some(ref pid_path) = self.pid_file {
            if let Ok(s) = std::fs::read_to_string(pid_path) {
                if let Ok(pid) = s.trim().parse::<u32>() {
                    let _ = std::process::Command::new("taskkill")
                        .args(["/F", "/PID", &pid.to_string()])
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status();
                }
            }
        }
        let _ = std::fs::remove_dir_all(&self.tmpdir);
    }
}

// ── Main test ─────────────────────────────────────────────────────────────────

#[test]
fn all_scenarios() {
    skip_if_no_deps!();

    // All real assertions below are gated #[cfg(windows)] so the compiler does
    // not complain about undefined types (WindowsShimContainer, etc.) on Linux.
    #[cfg(windows)]
    windows_all_scenarios();
}

#[cfg(windows)]
fn windows_all_scenarios() {
    use marsad_lifecycle::{Container, WindowsShimContainer};
    use marsad_transport::{ClaudeNamedPipeTransport, Transport};
    use std::time::{Duration, Instant};

    let id = format!("{}", std::process::id());
    let tmpdir = std::env::temp_dir().join(format!("marsad-it-i11-{id}"));
    std::fs::create_dir_all(&tmpdir).expect("create tmpdir");

    let mut guard = TeardownGuard::new(tmpdir.clone());

    // ── Session name ──────────────────────────────────────────────────────────
    // Must be a valid Named Pipe path component (no backslashes, no spaces).
    let session_name = format!("marsad-i11-{id}");

    // ── Construct the container ───────────────────────────────────────────────
    let mut container = WindowsShimContainer::new(tmpdir.clone());

    // ── Register pid file for teardown ────────────────────────────────────────
    let pid_file = tmpdir.join("sessions").join(&session_name).join("shim.pid");
    guard.set_pid_file(pid_file.clone());

    // ─────────────────────────────────────────────────────────────────────────
    // Scenario 1: spawn() creates the shim and PID file appears within 5s.
    // ─────────────────────────────────────────────────────────────────────────
    eprintln!("\n[i11/s1] === Scenario 1: spawn() ===");

    // Build a minimal claude command (same pattern as the agent uses).
    let project_dir = tmpdir.join("project");
    std::fs::create_dir_all(&project_dir).expect("create project dir");
    let project_str = project_dir.to_string_lossy().to_string();

    // Build a claude command that immediately outputs a system event and exits.
    //
    // QUOTING: Windows paths must NOT use {:?} (Rust Debug format) because that
    // escapes backslashes (e.g. "C:\foo" becomes "C:\\foo"), which cmd.exe
    // interprets as literal double-backslashes and fails to find the directory.
    // Windows paths never contain double quotes, so wrapping in "..." is safe.
    let claude_cmd = format!(
        "cd /d \"{}\" && claude --print --input-format stream-json --output-format stream-json \
         --verbose --model claude-opus-4-5 --permission-mode bypassPermissions \"{}\"",
        project_str, project_str
    );

    container
        .spawn(&session_name, &claude_cmd)
        .expect("[s1] spawn() must succeed");

    // Poll for the PID file to appear (shim writes it before ConnectNamedPipe).
    let pid_deadline = Instant::now() + Duration::from_secs(5);
    while !pid_file.exists() && Instant::now() < pid_deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        pid_file.exists(),
        "[s1] shim.pid did not appear within 5s at {:?}",
        pid_file
    );
    eprintln!("[i11/s1] PASS — shim.pid appeared.");

    // ─────────────────────────────────────────────────────────────────────────
    // Scenario 2: exists() returns true while shim is alive.
    // ─────────────────────────────────────────────────────────────────────────
    eprintln!("\n[i11/s2] === Scenario 2: exists() ===");
    assert!(
        container.exists(&session_name),
        "[s2] exists() must return true while shim is alive"
    );
    eprintln!("[i11/s2] PASS — exists() = true.");

    // ─────────────────────────────────────────────────────────────────────────
    // Scenario 3: ClaudeNamedPipeTransport::connect() opens client ends.
    // ─────────────────────────────────────────────────────────────────────────
    eprintln!("\n[i11/s3] === Scenario 3: transport connect() ===");
    let mut transport = ClaudeNamedPipeTransport::for_session(&session_name);
    transport.connect().expect("[s3] transport.connect() must succeed");
    eprintln!("[i11/s3] PASS — Named Pipe client ends opened.");

    // ─────────────────────────────────────────────────────────────────────────
    // Scenario 4: receive a system/init event from claude.
    // ─────────────────────────────────────────────────────────────────────────
    eprintln!("\n[i11/s4] === Scenario 4: recv system/init ===");

    // Send the control-protocol initialize handshake first.
    {
        let init = "{\"type\":\"control_request\",\"request\":{\"type\":\"initialize\",\"request_id\":\"req_1_init\"}}";
        transport
            .send_line(init)
            .expect("[s4] send initialize handshake must succeed");
    }

    let recv_deadline = Instant::now() + Duration::from_secs(90);
    let mut got_system = false;
    while Instant::now() < recv_deadline {
        match transport.recv_line() {
            Ok(Some(line)) => {
                eprintln!("[i11/s4] recv_line: {:.120}", line);
                if line.contains("\"system\"") || line.contains("\"type\":\"system\"") {
                    got_system = true;
                    break;
                }
            }
            Ok(None) => {
                eprintln!("[i11/s4] recv_line: EOF (claude exited early)");
                break;
            }
            Err(e) => {
                eprintln!("[i11/s4] recv_line error: {e}");
                break;
            }
        }
    }

    assert!(
        got_system,
        "[s4] Expected a system/init NDJSON event from claude within 90 s"
    );
    eprintln!("[i11/s4] PASS — system/init received via Named Pipe.");

    // Close the transport (drops handles; does NOT kill shim).
    transport.close().expect("[s4] close must succeed");

    // ─────────────────────────────────────────────────────────────────────────
    // Scenario 5: spawn() idempotency — calling again while shim is alive returns Ok.
    // ─────────────────────────────────────────────────────────────────────────
    eprintln!("\n[i11/s5] === Scenario 5: spawn() idempotency ===");
    container
        .spawn(&session_name, &claude_cmd)
        .expect("[s5] spawn() on live session must return Ok (adopt semantics)");
    // exists() must still return true (no second shim was started).
    assert!(
        container.exists(&session_name),
        "[s5] exists() must still return true after idempotent spawn"
    );
    eprintln!("[i11/s5] PASS — idempotent spawn returned Ok, shim still alive.");

    // ─────────────────────────────────────────────────────────────────────────
    // Scenario 6: kill() terminates the shim; exists() returns false.
    // ─────────────────────────────────────────────────────────────────────────
    eprintln!("\n[i11/s6] === Scenario 6: kill() ===");
    container
        .kill(&session_name)
        .expect("[s6] kill() must succeed");

    // Give the OS a moment to propagate the kill.
    std::thread::sleep(Duration::from_millis(500));

    assert!(
        !container.exists(&session_name),
        "[s6] exists() must return false after kill()"
    );
    eprintln!("[i11/s6] PASS — kill() succeeded, exists() = false.");

    // ─────────────────────────────────────────────────────────────────────────
    // Scenario 7: connect() fails when shim is gone (pipe does not exist).
    // ─────────────────────────────────────────────────────────────────────────
    eprintln!("\n[i11/s7] === Scenario 7: connect() fails when shim is gone ===");
    let mut dead_transport = ClaudeNamedPipeTransport::for_session(&session_name);
    // Since the pipe is gone, the first open attempt should fail immediately
    // (ERROR_FILE_NOT_FOUND rather than ERROR_PIPE_BUSY).
    let conn_result = dead_transport.connect();
    assert!(
        conn_result.is_err(),
        "[s7] connect() must fail when shim is dead, got Ok"
    );
    eprintln!(
        "[i11/s7] PASS — connect() correctly failed after kill: {:?}",
        conn_result.unwrap_err()
    );

    eprintln!("\n[i11] ALL SCENARIOS DONE — Windows shim + Named Pipe lifecycle verified.");
}
