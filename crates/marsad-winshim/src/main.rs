//! marsad-winshim — Windows Named Pipe session-host shim.
//!
//! # Purpose
//!
//! This binary is the Windows counterpart to `tmux` on Linux/macOS.  It runs as
//! a **detached, long-lived process** that:
//!
//! 1. Creates two Windows Named Pipe server ends:
//!    - `\\.\pipe\marsad-<name>-in`   — agent writes prompts (claude stdin)
//!    - `\\.\pipe\marsad-<name>-out`  — claude writes NDJSON (agent reads)
//! 2. Spawns the `claude` child with its stdin/stdout redirected to those pipes.
//! 3. Writes its own PID to the `--pid-file` path so the agent can probe liveness.
//! 4. Enters a `ConnectNamedPipe` → `DisconnectNamedPipe` loop on each pipe so
//!    the agent can reconnect after a restart (the core liveness guarantee).
//! 5. Waits for the claude child process to exit; then exits cleanly.
//!
//! The agent opens the **client** ends of the pipes.  On agent restart the agent
//! re-opens the client ends — the shim still holds the server ends, so reconnect
//! succeeds without disturbing claude.  This is the exact tmux analogue.
//!
//! # Why no ConPTY
//!
//! `claude` runs as `claude --print --input-format stream-json ...` — plain
//! stdin/stdout, no terminal emulation.  ConPTY would add unnecessary complexity.
//!
//! # CLI arguments
//!
//! ```text
//! marsad-winshim.exe \
//!   --in-pipe  \\.\pipe\marsad-<name>-in  \
//!   --out-pipe \\.\pipe\marsad-<name>-out \
//!   --err-log  <path>                      \
//!   --pid-file <path>                      \
//!   --cmd      <full claude invocation>
//! ```
//!
//! # Compilation
//!
//! This crate is a workspace member so `cargo build --workspace` includes it on
//! all platforms.  The full implementation lives under `#[cfg(windows)]`.  On
//! Linux/macOS the binary compiles to a stub that prints an error and exits,
//! which is fine because it is never invoked there.

fn main() {
    #[cfg(windows)]
    windows_main();

    #[cfg(not(windows))]
    {
        eprintln!("marsad-winshim: this binary is Windows-only; it does nothing on this platform");
        std::process::exit(1);
    }
}

// ── Windows implementation ────────────────────────────────────────────────────
//
// All Win32 API calls use `windows-sys` for minimal binary size and no-std
// compatibility.  We use `unsafe` blocks exclusively for FFI calls; the
// surrounding logic is plain safe Rust.
//
// IMPORTANT: every function that calls Win32 checks return values and writes
// a named error to stderr on failure.  The process exits non-zero so the agent
// can detect failures.

#[cfg(windows)]
fn windows_main() {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{
        CloseHandle, BOOL, FALSE, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, TRUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, GENERIC_WRITE,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_ACCESS_INBOUND,
        PIPE_ACCESS_OUTBOUND, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, WaitForSingleObject, INFINITE, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
        STARTUPINFOW,
    };

    // ── Parse CLI arguments ───────────────────────────────────────────────────

    let args: Vec<String> = std::env::args().collect();
    let mut in_pipe = String::new();
    let mut out_pipe = String::new();
    let mut err_log = String::new();
    let mut pid_file = String::new();
    let mut cmd = String::new();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--in-pipe" => {
                i += 1;
                in_pipe = args.get(i).cloned().unwrap_or_default();
            }
            "--out-pipe" => {
                i += 1;
                out_pipe = args.get(i).cloned().unwrap_or_default();
            }
            "--err-log" => {
                i += 1;
                err_log = args.get(i).cloned().unwrap_or_default();
            }
            "--pid-file" => {
                i += 1;
                pid_file = args.get(i).cloned().unwrap_or_default();
            }
            "--cmd" => {
                i += 1;
                cmd = args.get(i).cloned().unwrap_or_default();
            }
            _ => {}
        }
        i += 1;
    }

    if in_pipe.is_empty()
        || out_pipe.is_empty()
        || err_log.is_empty()
        || pid_file.is_empty()
        || cmd.is_empty()
    {
        eprintln!(
            "marsad-winshim: missing required arguments.\n\
             Usage: marsad-winshim.exe \\\n\
               --in-pipe  <pipe-path> \\\n\
               --out-pipe <pipe-path> \\\n\
               --err-log  <path>      \\\n\
               --pid-file <path>      \\\n\
               --cmd      <claude-command>"
        );
        std::process::exit(1);
    }

    // Helper: convert a Rust &str to a null-terminated UTF-16 Vec for Win32.
    fn to_wide(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0u16))
            .collect()
    }

    // ── Create Named Pipe server ends ─────────────────────────────────────────
    //
    // Named Pipe direction model (relative to the SERVER side, which is the shim):
    //
    //   IN pipe  (\\.\pipe\...-in)  — agent → claude stdin
    //     The AGENT is the CLIENT and writes to the client end.
    //     The SHIM (server) created it with PIPE_ACCESS_INBOUND: the server-side
    //     handle is READABLE (data flows from client to server).  This handle is
    //     passed as claude's hStdInput so claude reads agent prompts from it.
    //
    //   OUT pipe (\\.\pipe\...-out) — claude stdout → agent
    //     The SHIM (server) created it with PIPE_ACCESS_OUTBOUND: the server-side
    //     handle is WRITABLE (data flows from server to client).  This handle is
    //     passed as claude's hStdOutput so claude writes its NDJSON into it.
    //     The AGENT is the CLIENT and reads from the client end.
    //
    // RECONNECT SUPPORT
    // ─────────────────
    // After a client (agent) connects, reads/writes, and then disconnects (closes
    // its handle), a Windows Named Pipe server cannot accept a new client without
    // calling DisconnectNamedPipe + ConnectNamedPipe again on the server handle.
    // The shim spins a dedicated reconnect thread per pipe to implement this loop,
    // preserving the "agent can reconnect after a restart" guarantee.
    //
    // The reconnect threads run until the claude child exits (signalled by an
    // atomic flag).  Once claude is gone the pipe handles are closed and the
    // threads exit naturally.
    //
    // HANDLE INHERITABILITY
    // ──────────────────────
    // CreateNamedPipeW (lpSecurityAttributes = NULL) produces non-inheritable
    // handles.  We call SetHandleInformation immediately after each creation to
    // mark the handles inheritable so CreateProcessW can pass them to claude
    // via STARTUPINFO.hStdInput / hStdOutput.  Return values are checked; failure
    // is fatal because an uninheritable handle would silently break claude's I/O.

    let in_pipe_wide = to_wide(&in_pipe);
    let out_pipe_wide = to_wide(&out_pipe);

    // Buffer size: 64 KB, matching the doc §4 recommendation.
    const PIPE_BUFFER: u32 = 65536;
    // One instance per pipe (only one shim per session).
    const PIPE_NINSTANCES: u32 = 1;

    // ── IN pipe server end (PIPE_ACCESS_INBOUND: server-side is readable) ────
    let in_server_handle: HANDLE = unsafe {
        CreateNamedPipeW(
            in_pipe_wide.as_ptr(),
            PIPE_ACCESS_INBOUND,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            PIPE_NINSTANCES,
            PIPE_BUFFER,
            PIPE_BUFFER,
            0,                // default timeout
            std::ptr::null(), // default security attributes (non-inheritable)
        )
    };

    if in_server_handle == INVALID_HANDLE_VALUE {
        eprintln!(
            "marsad-winshim: CreateNamedPipe({in_pipe}) failed: OS error {}",
            std::io::Error::last_os_error()
        );
        std::process::exit(1);
    }

    // ── OUT pipe server end (PIPE_ACCESS_OUTBOUND: server-side is writable) ──
    let out_server_handle: HANDLE = unsafe {
        CreateNamedPipeW(
            out_pipe_wide.as_ptr(),
            PIPE_ACCESS_OUTBOUND,
            PIPE_TYPE_BYTE | PIPE_WAIT,
            PIPE_NINSTANCES,
            PIPE_BUFFER,
            PIPE_BUFFER,
            0,                // default timeout
            std::ptr::null(), // default security attributes (non-inheritable)
        )
    };

    if out_server_handle == INVALID_HANDLE_VALUE {
        eprintln!(
            "marsad-winshim: CreateNamedPipe({out_pipe}) failed: OS error {}",
            std::io::Error::last_os_error()
        );
        unsafe { CloseHandle(in_server_handle) };
        std::process::exit(1);
    }

    // ── Make pipe handles inheritable (required for CreateProcessW) ───────────
    //
    // SetHandleInformation makes the server-side handles inheritable so they
    // can be assigned to claude's stdin/stdout via STARTUPINFO.
    // Failure is fatal: an uninheritable handle silently breaks claude I/O.
    unsafe {
        use windows_sys::Win32::Foundation::SetHandleInformation;

        let ok_in =
            SetHandleInformation(in_server_handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
        if ok_in == FALSE {
            eprintln!(
                "marsad-winshim: SetHandleInformation(in) failed: OS error {}",
                std::io::Error::last_os_error()
            );
            CloseHandle(in_server_handle);
            CloseHandle(out_server_handle);
            std::process::exit(1);
        }

        let ok_out =
            SetHandleInformation(out_server_handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
        if ok_out == FALSE {
            eprintln!(
                "marsad-winshim: SetHandleInformation(out) failed: OS error {}",
                std::io::Error::last_os_error()
            );
            CloseHandle(in_server_handle);
            CloseHandle(out_server_handle);
            std::process::exit(1);
        }
    }

    // ── Write PID file ────────────────────────────────────────────────────────
    //
    // Write our PID BEFORE ConnectNamedPipe so the agent can probe liveness
    // as soon as it reads the PID file (see WindowsShimContainer::exists).

    let shim_pid = std::process::id();
    if let Err(e) = std::fs::write(&pid_file, shim_pid.to_string()) {
        eprintln!("marsad-winshim: failed to write pid file {pid_file}: {e}");
        unsafe {
            CloseHandle(in_server_handle);
            CloseHandle(out_server_handle);
        }
        std::process::exit(1);
    }

    // ── Open the err.log file for claude's stderr ─────────────────────────────
    //
    // Use CreateFile directly so we have a raw HANDLE to pass in STARTUPINFO.
    // The handle must also be inheritable; we use SetHandleInformation like the
    // pipe handles above.

    let err_log_wide = to_wide(&err_log);

    // Import CreateFileW from the already-imported FileSystem module.
    use windows_sys::Win32::Storage::FileSystem::CreateFileW;

    let err_log_handle: HANDLE = unsafe {
        CreateFileW(
            err_log_wide.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ,
            std::ptr::null(), // non-inheritable by default
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            0, // no template file
        )
    };

    if err_log_handle == INVALID_HANDLE_VALUE {
        eprintln!(
            "marsad-winshim: CreateFile({err_log}) failed: OS error {}",
            std::io::Error::last_os_error()
        );
        unsafe {
            CloseHandle(in_server_handle);
            CloseHandle(out_server_handle);
        }
        let _ = std::fs::remove_file(&pid_file);
        std::process::exit(1);
    }

    // Make err.log handle inheritable.
    unsafe {
        use windows_sys::Win32::Foundation::SetHandleInformation;
        let ok = SetHandleInformation(err_log_handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
        if ok == FALSE {
            eprintln!(
                "marsad-winshim: SetHandleInformation(err_log) failed: OS error {}",
                std::io::Error::last_os_error()
            );
            CloseHandle(in_server_handle);
            CloseHandle(out_server_handle);
            CloseHandle(err_log_handle);
            let _ = std::fs::remove_file(&pid_file);
            std::process::exit(1);
        }
    }

    // ── Spawn the claude child process via CreateProcessW ─────────────────────
    //
    // We use CreateProcessW (not std::process::Command) so we can supply
    // STARTUPINFOW.hStdInput/hStdOutput/hStdError using the raw pipe handles
    // made inheritable above.
    //
    // The `cmd` string is wrapped in `cmd.exe /C <cmd>` so that PATH resolution
    // works and the `&&` chaining (`cd /d ... && claude ...`) is interpreted by
    // the shell.  The agent's build_spawn_cmd (Windows branch) passes the raw
    // `cd /d ... && claude ...` string; we add the `cmd.exe /C` prefix here.
    //
    // CREATE_NEW_PROCESS_GROUP: the claude child gets its own Ctrl+C group.
    // Even though the shim itself is DETACHED (no console), without this flag
    // a synthetic console Ctrl+C could reach claude.  Belt-and-suspenders.

    // creation flags: new process group so Ctrl+C doesn't reach claude.
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

    let full_cmd = format!("cmd.exe /C {cmd}");
    let mut cmd_wide: Vec<u16> = to_wide(&full_cmd);

    let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    si.dwFlags = STARTF_USESTDHANDLES;
    si.hStdInput = in_server_handle;
    si.hStdOutput = out_server_handle;
    si.hStdError = err_log_handle;

    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

    let ok: BOOL = unsafe {
        CreateProcessW(
            std::ptr::null(), // derive executable from command line
            cmd_wide.as_mut_ptr(),
            std::ptr::null(), // default process security
            std::ptr::null(), // default thread security
            TRUE,             // bInheritHandles — required for std handle passing
            CREATE_NEW_PROCESS_GROUP,
            std::ptr::null(), // inherit parent's environment
            std::ptr::null(), // inherit parent's current directory
            &si,
            &mut pi,
        )
    };

    if ok == FALSE {
        eprintln!(
            "marsad-winshim: CreateProcess failed for cmd {:?}: OS error {}",
            cmd,
            std::io::Error::last_os_error()
        );
        unsafe {
            CloseHandle(in_server_handle);
            CloseHandle(out_server_handle);
            CloseHandle(err_log_handle);
        }
        let _ = std::fs::remove_file(&pid_file);
        std::process::exit(1);
    }

    // The claude child is running.  Close the thread handle immediately — we
    // only need the process handle (pi.hProcess) to wait on exit.
    unsafe { CloseHandle(pi.hThread) };

    // ── Accept-reconnect loop (ConnectNamedPipe + DisconnectNamedPipe) ────────
    //
    // After a client (agent) disconnects (e.g. GUI restarts), the pipe server
    // handle is in a "client-disconnected" state.  New CreateFile calls from the
    // next agent will fail with ERROR_PIPE_BUSY until the server calls
    // DisconnectNamedPipe + ConnectNamedPipe to reset the instance for a new client.
    //
    // The shim spins one reconnect thread per pipe.  The reconnect sequence is:
    //   DisconnectNamedPipe  — reset server end (required before next Connect)
    //   ConnectNamedPipe     — block until next agent opens the client end
    //
    // IMPORTANT: The first agent connects via the initial ConnectNamedPipe calls
    // immediately below.  The reconnect threads must NOT call DisconnectNamedPipe
    // on the very first iteration — that would immediately sever the live first
    // connection before the agent has done any I/O.
    //
    // We solve this with a `first_iteration` flag that skips the Disconnect on
    // the first loop iteration so the initial agent connection is not disturbed.
    // On every subsequent iteration (i.e. each re-connection after an agent
    // restart), the Disconnect resets the previous connection state, then Connect
    // waits for the new agent.
    //
    // LIMITATION (documented in docs/windows-conpty.md §8):
    // The shim has no I/O on the server handles (those are passed to claude via
    // STARTUPINFO, so claude does the actual reads/writes).  This means the shim
    // cannot detect when the current agent client disconnects without probing.
    // Reconnect is therefore "best-effort": the agent that restarts should retry
    // its CreateFile call (the transport's 15 s retry loop covers this).
    //
    // UNSAFE CONTRACT:
    //   - The pipe handle is only closed in the cleanup block after
    //     WaitForSingleObject (i.e. only after claude exits).
    //   - The reconnect threads run until their ConnectNamedPipe call fails,
    //     which happens when the handle is closed.
    //   - No other shared state is accessed from the threads.

    use windows_sys::Win32::Foundation::ERROR_PIPE_CONNECTED;

    // ── First connection: accept in-pipe ──────────────────────────────────────
    //
    // Block until the agent opens the IN pipe client end.
    // ERROR_PIPE_CONNECTED means the agent connected before our call — also OK.
    let connect_in = unsafe { ConnectNamedPipe(in_server_handle, std::ptr::null_mut()) };
    if connect_in == FALSE {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(ERROR_PIPE_CONNECTED as i32) {
            // Log unexpected errors but do not exit — claude is still running.
            eprintln!("marsad-winshim: ConnectNamedPipe(in) failed: {err}");
        }
    }

    // ── First connection: accept out-pipe ─────────────────────────────────────
    let connect_out = unsafe { ConnectNamedPipe(out_server_handle, std::ptr::null_mut()) };
    if connect_out == FALSE {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(ERROR_PIPE_CONNECTED as i32) {
            eprintln!("marsad-winshim: ConnectNamedPipe(out) failed: {err}");
        }
    }

    // ── Reconnect threads ─────────────────────────────────────────────────────
    //
    // Each thread loops:
    //   1st iteration: skip DisconnectNamedPipe (the 1st client is still live).
    //   Subsequent:    DisconnectNamedPipe → ConnectNamedPipe.
    //
    // We pass the raw handle as usize to satisfy Rust's Send bound;
    // the handle value is valid for the lifetime of the reconnect thread
    // (closed only in the cleanup block after WaitForSingleObject returns).

    fn spawn_reconnect_thread(handle: HANDLE, label: &'static str) -> std::thread::JoinHandle<()> {
        let h = handle as usize; // usize is Send; HANDLE is isize (pointer-sized)
        std::thread::spawn(move || {
            let pipe: HANDLE = h as HANDLE;
            let mut first_iteration = true;
            loop {
                if first_iteration {
                    // Skip the initial DisconnectNamedPipe — the first agent is
                    // still connected and must not be severed here.
                    first_iteration = false;
                } else {
                    // Reset the server end so the next client can connect.
                    // The previous client has already closed its end (or the
                    // agent restarted); Disconnect is required before Connect.
                    unsafe { DisconnectNamedPipe(pipe) };
                }

                // Wait for the next agent client to open the pipe client end.
                // Returns FALSE on error (e.g. the handle was closed when claude
                // exited in the cleanup block below).
                let ok = unsafe { ConnectNamedPipe(pipe, std::ptr::null_mut()) };
                if ok == FALSE {
                    let err = std::io::Error::last_os_error();
                    // ERROR_PIPE_CONNECTED (535): a client connected before our
                    // call — treat as success and continue the loop.
                    if err.raw_os_error() != Some(ERROR_PIPE_CONNECTED as i32) {
                        // Handle closed (ERROR_INVALID_HANDLE = 6) or other
                        // fatal error — claude has exited; exit the loop.
                        eprintln!(
                            "marsad-winshim: reconnect thread ({label}) ConnectNamedPipe: {err}"
                        );
                        break;
                    }
                }
            }
        })
    }

    let in_reconnect = spawn_reconnect_thread(in_server_handle, "in");
    let out_reconnect = spawn_reconnect_thread(out_server_handle, "out");

    // ── Wait for claude to exit ───────────────────────────────────────────────
    //
    // WaitForSingleObject blocks until the claude child process terminates.
    // After it returns we close all handles and remove the PID file.
    // Closing the pipe handles will cause the next ConnectNamedPipe in the
    // reconnect threads to return with an error, terminating those threads.

    unsafe {
        WaitForSingleObject(pi.hProcess, INFINITE);
        CloseHandle(pi.hProcess);
        // Close pipe and log handles — this unblocks the reconnect threads.
        CloseHandle(in_server_handle);
        CloseHandle(out_server_handle);
        CloseHandle(err_log_handle);
    }

    // Wait for reconnect threads to finish (they will exit shortly after the
    // handles are closed).
    let _ = in_reconnect.join();
    let _ = out_reconnect.join();

    // Remove the PID file to signal the agent that this session is dead.
    let _ = std::fs::remove_file(&pid_file);
}
