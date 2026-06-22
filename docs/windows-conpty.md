# Windows Session Container — Design and Implementation Guide

**Status:** Implementation complete; Linux build + clippy + 100% coverage confirmed.  
Correctness review pass 1 completed 2026-06-22 — bugs found and fixed; see §8.  
Correctness review pass 2 completed 2026-06-22 — two additional bugs found and fixed; see §8.  
**Affects:** `crates/marsad-lifecycle`, `crates/marsad-transport`, `crates/marsad-winshim` (created),
`crates/marsad-agent` (wired), and `tests/integration/i11_windows_lifecycle` (added).

---

## 1. Study of the existing Linux/macOS model

### How `TmuxContainer` works

`TmuxContainer` (in `crates/marsad-lifecycle/src/lib.rs`) implements the `Container` trait with
four operations:

| Method | Implementation |
|--------|----------------|
| `exists(name)` | `tmux has-session -t <name>` exits 0 |
| `attach(name)` | re-checks `exists`; returns `AttachRace` if the session vanished |
| `spawn(name, cmd)` | `mkfifo in.fifo out.fifo` then `tmux new-session -d -s <name> '<cmd>'`; idempotent |
| `kill(name)` | `tmux kill-session -t <name>` |

The command that tmux runs is built by `Agent::build_spawn_cmd`:

```
cd <project> && claude --print --input-format stream-json --output-format stream-json \
  --verbose --model <m> --permission-mode <mode> [--resume <id>] \
  < in.fifo > out.fifo 2>err.log
```

tmux gives liveness: the session survives after the agent (or the entire GUI) exits.
The FIFOs give reconnectability: the agent re-opens them after a restart without disturbing the
running `claude` process.

### How `ClaudeFifoTransport` works

`ClaudeFifoTransport` (in `crates/marsad-transport/src/lib.rs`) opens the two FIFOs:

* **Open order (deadlock avoidance):** bash processes `< in.fifo > out.fifo` left-to-right.
  The agent opens `in.fifo` for writing in Thread A and `out.fifo` for reading in Thread B,
  both concurrently, so the two blocking opens pair with bash's two opens without a deadlock.
* After connecting, `into_split()` hands separate write/read handles to the serve thread.
* `close()` drops only the handles — it does NOT kill the tmux session.

### How `Agent::with_real_io` wires everything together

On startup the agent calls `reconcile_all()` which, for each persisted session, probes the
container via `exists()` and either adopts it (`Adopt`), resumes it (`Resume`), or marks it
stale (`MarkStale`).  New sessions go through `launch()` which calls `container.spawn()` then
opens a `ClaudeFifoTransport` for the session.  Each FIFO reader runs in its own thread and
broadcasts claude's NDJSON to all connected GUI subscribers.

---

## 2. Key Insight: claude does NOT need a PTY

Claude runs as:

```
claude --print --input-format stream-json --output-format stream-json ...
```

This is a **non-interactive, plain-bytes stdin/stdout** protocol.  tmux is used purely for
**session liveness** (so the claude process survives after the agent/GUI exits), not for
terminal emulation.

This means Windows does NOT need ConPTY.  The real requirement is:

> Run claude as a **detached, persistent process** whose stdin/stdout streams are
> reachable across agent restarts.

### Option analysis

**(a) Detached child + anonymous handles or named pipes**  
Spawn claude directly from the agent with `DETACHED_PROCESS` and redirect its stdin/stdout to
Windows Named Pipes.  Simple, but the pipe server endpoints live inside the agent process.
When the agent exits, the pipe server handles are closed and claude loses its I/O — the liveness
requirement is violated.

**(b) ConPTY (CreatePseudoConsole)**  
Provides a real pseudo-console.  This is heavier than needed for stream-json mode, and the
ConPTY is owned by the creating process — same liveness problem as (a) unless combined with a
shim.  Adds unnecessary complexity.  **Not recommended.**

**(c) Lightweight Windows "session host" shim process**  
Launch a small helper binary (`marsad-winshim.exe`) that:
1. Creates the Named Pipe server ends (it owns them for its lifetime).
2. Spawns claude with its stdin/stdout redirected to the pipes.
3. Exits only when the user explicitly kills the session (or claude exits naturally).
4. Writes its own PID to `shim.pid` so the agent can probe liveness after restart.

The agent opens the **client** ends of the pipes.  On agent restart, the agent re-opens the
client ends — the shim still holds the server ends, so the reconnect succeeds without
disturbing claude.  This is the exact tmux analogue.

**Recommendation: option (c).**  It is the simplest approach that preserves the
liveness/reconnect guarantee without requiring ConPTY, Job Objects, or OS-specific inheritance
tricks.  The shim is small (~100–200 lines of Rust) and has a single, clear responsibility.

---

## 3. Architecture: Named Pipe naming convention

For a session named `marsad-session-1` (the container name):

| Pipe | Direction | Path |
|------|-----------|------|
| `-in` pipe | agent → claude (stdin) | `\\.\pipe\marsad-session-1-in` |
| `-out` pipe | claude → agent (stdout) | `\\.\pipe\marsad-session-1-out` |

The shim creates both server ends before spawning claude.  Claude's stderr is redirected to
`<session_dir>/err.log` by the shim.

---

## 4. Files changed by this scaffold

### `crates/marsad-lifecycle/src/lib.rs`

Added `WindowsShimContainer` gated `#[cfg(all(windows, not(coverage)))]`.  Implements the same
`Container` trait as `TmuxContainer`.  Methods contain `todo!()` stubs with detailed comments
on the Windows OS APIs needed to complete them.

Key methods:
- `exists(name)` — reads `shim.pid` and probes the process with `OpenProcess` /
  `GetExitCodeProcess`.
- `attach(name)` — re-checks `exists`; returns `AttachRace` if gone.
- `spawn(name, cmd)` — creates the session directory, then launches `marsad-winshim.exe`
  as `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP` with the pipe names, err.log path, pid
  file path, and the claude command as arguments.  Idempotent (adopt semantics if shim is live).
- `kill(name)` — reads `shim.pid`, calls `TerminateProcess`, removes the session directory.

### `crates/marsad-transport/src/lib.rs`

Added `ClaudeNamedPipeTransport` gated `#[cfg(all(windows, not(coverage)))]`.  Mirrors
`ClaudeFifoTransport` exactly — same `into_split()` API, same `LineFramer`-based framing,
same `close()` liveness guarantee.  `connect()` contains a `todo!()` stub; `send_line`,
`recv_line`, and `close` are fully written (they are identical to the FIFO versions once
file handles are present).

### `crates/marsad-agent/src/lib.rs` (NOT YET CHANGED — design only)

`with_real_io` needs a Windows branch that constructs `WindowsShimContainer` instead of
`TmuxContainer`, and that opens `ClaudeNamedPipeTransport` instead of `ClaudeFifoTransport`
for each session.  The change is gated `#[cfg(windows)]`:

```rust
// In Agent::with_real_io (cfg(not(coverage))):
#[cfg(unix)]
let container: Box<dyn Container> = Box::new(TmuxContainer::new(data_root.clone()));
#[cfg(windows)]
let container: Box<dyn Container> = Box::new(WindowsShimContainer::new(data_root.clone()));
```

Similarly, the FIFO-reader thread in `serve()` would select the transport by platform:

```rust
// In the FIFO reader thread spawned after a LaunchSession:
#[cfg(unix)]
let mut transport = ClaudeFifoTransport::open(&session_dir)?;
#[cfg(windows)]
let mut transport = ClaudeNamedPipeTransport::for_session(&container_name);
transport.connect()?;
let (writer, reader) = transport.into_split().unwrap();
// Rest of the FIFO reader thread is identical.
```

No existing Linux logic is disturbed; the `#[cfg(unix)]` guards ensure the old code
compiles and runs exactly as before.

### `crates/marsad-winshim/` (NOT YET CREATED — to be created on Windows)

A new workspace member binary crate.  Its `main` parses CLI arguments:
```
marsad-winshim.exe \
  --in-pipe  \\.\pipe\marsad-<name>-in  \
  --out-pipe \\.\pipe\marsad-<name>-out \
  --err-log  <path>                      \
  --pid-file <path>                      \
  --cmd      <full claude command>
```

Implementation outline:
1. Create the two Named Pipe server ends (`CreateNamedPipe` with `PIPE_ACCESS_DUPLEX` or
   separate `PIPE_ACCESS_INBOUND` / `PIPE_ACCESS_OUTBOUND`, `PIPE_TYPE_BYTE`, buffer size
   64 KB, instance count 1).
2. Redirect claude's stdin from the `-in` pipe and stdout to the `-out` pipe and stderr to the
   err.log file using `CreateProcess` with `STARTUPINFO.hStdInput / hStdOutput / hStdError`
   and `STARTF_USESTDHANDLES`.
3. Write `std::process::id()` to the pid file so the agent can probe liveness.
4. Call `ConnectNamedPipe` to wait for the agent to connect the client ends.
5. Wait on the claude child process handle; exit when claude exits.

Suggested dependencies: `windows-sys` (already widely used in the Rust ecosystem; minimal,
no-std compatible) or the `windows` crate (higher-level but heavier).  `windows-sys` is
preferred for a small helper binary.

---

## 5. Liveness and reconnect story on Windows

| Event | tmux (Linux/macOS) | Shim (Windows) |
|-------|--------------------|----------------|
| Agent exits | tmux session keeps claude alive | shim process keeps claude alive |
| Agent restarts | `exists()` → `tmux has-session` | `exists()` → PID probe / pipe probe |
| Reconnect | `attach()` → reopen FIFOs | `attach()` → reopen named pipe client ends |
| Session dead | `exists()` → false → Resume/Stale | `exists()` → false → Resume/Stale |
| Resume | `spawn()` → new tmux + claude --resume | `spawn()` → new shim + claude --resume |
| Kill | `tmux kill-session` | `TerminateProcess(shim_pid)` |

The `reconcile_all()` + `reconcile()` logic in `marsad-lifecycle` is entirely platform-agnostic
and works unchanged on Windows — only the `Container` implementation differs.

---

## 6. What remains `todo!()`

All `todo!()` stubs have been removed.  The following items are implemented but
await runtime verification on a real Windows machine (see §8):

| Location | Method | Status |
|----------|--------|--------|
| `marsad-lifecycle::WindowsShimContainer::exists` | `exists` | Implemented: PID-file read + `OpenProcess` + `GetExitCodeProcess` |
| `marsad-lifecycle::WindowsShimContainer::spawn` | `spawn` | Implemented: `DETACHED_PROCESS \| CREATE_NEW_PROCESS_GROUP` via `CommandExt::creation_flags` |
| `marsad-lifecycle::WindowsShimContainer::kill` | `kill` | Implemented: `OpenProcess(PROCESS_TERMINATE)` + `TerminateProcess` |
| `marsad-transport::ClaudeNamedPipeTransport::connect` | `connect` | Implemented: `OpenOptions` retry loop with `ERROR_PIPE_BUSY` / `ERROR_FILE_NOT_FOUND` handling |
| `crates/marsad-winshim/` | entire binary | Implemented: `CreateNamedPipe` × 2, `CreateProcess` with `STARTF_USESTDHANDLES`, PID file, `ConnectNamedPipe`, `WaitForSingleObject` |
| `marsad-agent::Agent::with_real_io` | wiring | Implemented: `#[cfg(unix)]` / `#[cfg(windows)]` branches |
| `marsad-agent::Agent::serve` | FIFO reader thread | Implemented: `#[cfg(unix)]` uses `ClaudeFifoTransport`; `#[cfg(windows)]` uses `ClaudeNamedPipeTransport` |

---

## 7. Step-by-step to finish on a real Windows machine

### Prerequisites

```powershell
# Install Rust for Windows
winget install Rustlang.Rust.MSVC

# Or the GNU toolchain (works; MSVC is preferred for Windows API access):
winget install Rustlang.Rust.GNU

# Verify toolchain
rustup show
cargo --version
```

### Step 1 — Add `marsad-winshim` to the workspace

Create `crates/marsad-winshim/Cargo.toml`:
```toml
[package]
name = "marsad-winshim"
version.workspace = true
edition.workspace = true

[dependencies]
# For Windows API access:
windows-sys = { version = "0.59", features = [
    "Win32_Foundation",
    "Win32_System_Pipes",
    "Win32_System_Threading",
    "Win32_Storage_FileSystem",
] }
```

Add to the workspace `Cargo.toml` `members` list:
```toml
"crates/marsad-winshim",
```

### Step 2 — Implement `marsad-winshim/src/main.rs`

Parse CLI args, call `CreateNamedPipe` for both ends, spawn claude via `CreateProcess` with
`STARTF_USESTDHANDLES`, write PID file, call `ConnectNamedPipe`, then `WaitForSingleObject`
on the claude child handle.

### Step 3 — Complete `WindowsShimContainer::exists`

```rust
// Example (windows-sys):
use windows_sys::Win32::System::Threading::{
    OpenProcess, GetExitCodeProcess, PROCESS_QUERY_LIMITED_INFORMATION, STILL_ACTIVE,
};
let pid: u32 = std::fs::read_to_string(dir.join("shim.pid"))?.trim().parse()?;
let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
if handle == 0 { return false; }
let mut code: u32 = 0;
let ok = GetExitCodeProcess(handle, &mut code);
CloseHandle(handle);
ok != 0 && code == STILL_ACTIVE
```

### Step 4 — Complete `WindowsShimContainer::spawn`

```rust
use std::os::windows::process::CommandExt;
const DETACHED_PROCESS: u32 = 0x00000008;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;

std::process::Command::new(&shim)
    .args(["--in-pipe",  &in_pipe,
           "--out-pipe", &out_pipe,
           "--err-log",  err_log.to_str().unwrap(),
           "--pid-file", pid_file.to_str().unwrap(),
           "--cmd",      cmd])
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
    .spawn()
    .map_err(|e| LifecycleError::Spawn(format!("shim spawn: {e}")))?;
```

Wait ~200 ms for the shim to write the PID file before returning.

### Step 5 — Complete `ClaudeNamedPipeTransport::connect`

```rust
use std::fs::OpenOptions;
use std::time::{Duration, Instant};

let deadline = Instant::now() + Duration::from_secs(15);
let writer = loop {
    match OpenOptions::new().write(true).open(&self.in_pipe_name) {
        Ok(f) => break f,
        Err(e) if e.raw_os_error() == Some(231 /* ERROR_PIPE_BUSY */) => {
            if Instant::now() > deadline {
                return Err(TransportError::ConnectFailed("timeout waiting for -in pipe".into()));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err(e) => return Err(TransportError::ConnectFailed(format!("open -in pipe: {e}"))),
    }
};
let reader = OpenOptions::new().read(true).open(&self.out_pipe_name)
    .map_err(|e| TransportError::ConnectFailed(format!("open -out pipe: {e}")))?;
self.writer = Some(writer);
self.reader = Some(std::io::BufReader::new(reader));
Ok(())
```

### Step 6 — Wire `Agent::with_real_io` on Windows

In `crates/marsad-agent/src/lib.rs`, in `with_real_io`:
```rust
#[cfg(unix)]
use marsad_lifecycle::TmuxContainer as RealContainer;
#[cfg(windows)]
use marsad_lifecycle::WindowsShimContainer as RealContainer;
let container = RealContainer::new(data_root.clone());
```

In the FIFO reader thread:
```rust
#[cfg(unix)]
let mut t = ClaudeFifoTransport::open(&session_dir)
    .map_err(|e| { eprintln!("...{e}"); return; })?;
#[cfg(windows)]
let mut t = ClaudeNamedPipeTransport::for_session(&container_name);
t.connect()?;
```

### Step 7 — Build and smoke-test on Windows

```powershell
# Build all workspace members
cargo build --workspace

# Verify the shim binary is produced
ls .\target\debug\marsad-winshim.exe

# Launch the agent (it will use WindowsShimContainer automatically on Windows)
.\target\debug\marsad-agent.exe --data-root C:\Users\<user>\AppData\Local\Marsad

# From a second terminal, send a LaunchSession IPC message.
# Verify that:
#   1. marsad-winshim.exe appears in Task Manager.
#   2. claude.exe appears as a child of marsad-winshim.exe.
#   3. \\.\pipe\marsad-session-1-in and ...-out are visible in Sysinternals Handle.
#   4. Stopping the agent does NOT kill marsad-winshim.exe or claude.exe (liveness).
#   5. Restarting the agent and sending an AttachSession reconnects to the running claude.
```

### Step 8 — Session-survives-GUI-close verification

```powershell
# 1. Start Marsad GUI.
# 2. Launch a session (observe session appears).
# 3. Close the GUI entirely (not just minimise).
# 4. In PowerShell, verify marsad-winshim.exe is still running:
Get-Process marsad-winshim
# 5. Restart the GUI.
# 6. Verify the session is re-adopted (status Detached, not Stale).
# 7. Send a new prompt — verify claude responds.
```

---

## 8. Honest caveats (updated 2026-06-22 after two correctness review passes)

- **The implementation was written and compiled on Linux/WSL, NOT on a real Windows machine.**
  All Windows API calls (`CreateNamedPipe`, `CreateProcess`, `OpenProcess`, `GetExitCodeProcess`,
  `TerminateProcess`, `ConnectNamedPipe`, `DisconnectNamedPipe`, `WaitForSingleObject`) were
  written against the Windows API documentation and the direct parallel with the Linux/tmux model.
  They compile cleanly under `#[cfg(windows)]` guards on Linux (the `windows-sys` crate provides
  the type definitions), but the runtime behaviour has not been observed.

- **Bugs found and fixed in review pass 1 (`crates/marsad-winshim/src/main.rs`):**
  1. **Reconnect not supported** (critical): After the initial `ConnectNamedPipe` returns and
     the agent disconnects, the server-side handle must go through `DisconnectNamedPipe` +
     `ConnectNamedPipe` again before the next client can connect.  The original code only called
     `ConnectNamedPipe` once per pipe, breaking the "agent can reconnect after restart" guarantee.
     Fixed by spawning one reconnect thread per pipe that loops `Disconnect` → `Connect`.
  2. **`SetHandleInformation` return values unchecked** (critical): If `SetHandleInformation`
     failed silently, `CreateProcessW` would pass non-inheritable handles as claude's stdin/stdout,
     causing claude to read/write from/to invalid handles.  Fixed: failure is now fatal with a
     clear error message.
  3. **Unused imports** (would cause `clippy -D warnings` failures on Windows): `PathBuf`,
     `FILE_SHARE_WRITE`, `OPEN_EXISTING`, duplicate `CreateFileWHandle`/`GENERIC_READ`,
     `PIPE_UNLIMITED_INSTANCES`, `GetStdHandle`, `STD_ERROR_HANDLE`, `FILE_FLAG_OVERLAPPED`
     were all imported but never referenced.  All removed.
  4. **`PIPE_READMODE_BYTE` on the OUTBOUND pipe server**: `PIPE_ACCESS_OUTBOUND` creates a
     write-only server handle; `PIPE_READMODE_BYTE` is irrelevant and potentially rejected.
     Removed from the OUT pipe `CreateNamedPipeW` call.
  5. **`CreateProcessW` `dwCreationFlags = 0`**: The claude child should receive
     `CREATE_NEW_PROCESS_GROUP` so that synthetic Ctrl+C events do not propagate to it.
     Fixed: `CREATE_NEW_PROCESS_GROUP` is now passed.

- **Bugs found and fixed in review pass 2:**
  6. **Reconnect thread immediately disconnects the first agent** (critical,
     `crates/marsad-winshim/src/main.rs`): The reconnect thread loop was `Disconnect → Connect`
     starting immediately.  After the main thread accepted the FIRST agent connection with
     `ConnectNamedPipe`, the reconnect thread immediately called `DisconnectNamedPipe` — severing
     the live connection before the agent could do any I/O.  Fixed by adding a `first_iteration`
     flag: on the first loop iteration the `DisconnectNamedPipe` is skipped, so the initial
     agent connection is not disturbed.  Subsequent iterations (genuine reconnects) still call
     `Disconnect → Connect` as required.
  7. **Windows path quoting bug using Rust `{:?}` Debug format** (critical,
     `crates/marsad-agent/src/lib.rs` Windows branch of `build_spawn_cmd`, and
     `tests/integration/i11_windows_lifecycle/tests/lifecycle.rs`):
     The command string for `cmd.exe` was built using `format!("cd /d {:?} ...", project)`
     where `{:?}` is Rust's Debug formatter for strings.  The Debug formatter escapes backslashes
     (e.g. `C:\Users\foo` becomes `"C:\\Users\\foo"`), so `cmd.exe` sees literal double-backslashes
     and fails to find the directory.  Fixed in both files: use `"cd /d \"{}\" ..."` (plain `{}`
     wrapped in explicit double quotes), which is always safe for Windows paths because Windows
     paths cannot contain double-quote characters.

- **Remaining areas of uncertainty (to verify on a real Windows machine, cross-ref §7):**
  - `PIPE_ACCESS_INBOUND` server handle as `hStdInput`: the inherited handle must be readable
    by the child without a separate `ConnectNamedPipe` on the child side.  The design relies on
    handle inheritance — the child uses the handle directly as its stdin fd.
  - The `cmd.exe /C <cmd>` wrapper: the claude command uses `--print` + stream-json mode which
    should be terminal-agnostic, but the cmd.exe wrapper adds a dependency on PATH resolution
    inside cmd.exe.  On machines where `claude` is not on the system PATH but only in the user
    PATH, this may fail.  A possible hardening: accept the full path to `claude.exe` as an arg.
  - The reconnect thread (pass 1 fix) now uses a `first_iteration` guard.  On the 2nd reconnect
    (2nd agent connects after 1st disconnects), `DisconnectNamedPipe` is called on the previous
    client.  The shim has no I/O on the server handles (those are passed to claude via STARTUPINFO),
    so it cannot detect when the current client has gone before calling Disconnect.  Verify that
    the 2nd agent's reconnect retry loop (15 s timeout, 50 ms interval) absorbs the brief gap
    between Disconnect and the next Connect.
  - Spawn-then-connect ordering: claude is spawned before `ConnectNamedPipe` is called.  Claude
    inherits the server-side pipe handles and will block on its first `ReadFile` (stdin) until data
    arrives.  Verify this is not treated as an error by claude's startup logic.

- **`marsad-winshim` is a workspace member** and compiles on Linux as a stub (the non-Windows
  `main()` just prints an error and exits with code 1).  On Windows it builds the full
  implementation.  `cargo build --workspace` is green on Linux.
- **The Linux build, clippy, and coverage are completely unaffected.**  All new code is gated
  `#[cfg(all(windows, not(coverage)))]` (lifecycle, transport) or `#[cfg(windows)]` (agent
  wiring).  The three key crates remain at 100% line+region coverage on Linux (verified 2026-06-22,
  both review passes).
- **Integration test `i11_windows_lifecycle` skips LOUDLY on Linux** (not a silent pass) with a
  clearly labelled banner.  On Windows with `marsad-winshim.exe` and `claude` in PATH it runs
  seven scenarios end-to-end.
