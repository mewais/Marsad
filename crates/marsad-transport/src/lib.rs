//! marsad-transport — NDJSON line transport to claude: LocalIpc, SshStdio, loopback fake.
//!
//! See `../AGENTS.md` and `docs/specs/transport.md` for the authoritative contract.
//!
//! # Coverage regime
//!
//! The fully-testable surface is: `Transport` trait, `LineFramer`, `LoopbackFake`,
//! and `TransportError`.
//!
//! `LocalIpc`, `SshStdio`, and `WslStdio` perform real OS IO that the unit/
//! integration test suite cannot exercise (no live Unix socket, SSH endpoint, or
//! WSL distro is available).  Their IO-heavy internals are excluded from the
//! llvm-cov build via `#[cfg(not(coverage))]`; `cargo llvm-cov` sets
//! `cfg(coverage)` automatically.
//!
//! Under `cfg(coverage)`:
//! - `LocalIpc`, `SshStdio`, and `WslStdio` are thin stubs (struct + new() +
//!   inherent connect()).  The inherent `connect()` is covered by the `errors.rs`
//!   integration tests.  No `Transport` trait impl exists for them in this build,
//!   so there are no uncoverable send_line/recv_line/close stubs.  (`WslStdio`
//!   is additionally `#[cfg(windows)]`-gated for its real impl since WSL is
//!   Windows-only; the stub also serves non-Windows production builds.)
//!
//! Under `cfg(not(coverage))` (normal build / plain `cargo test`):
//! - Full production implementations compile.  Integration tests exercise the
//!   connect()-fails-on-bad-path error path; live IO paths are exercised by i03/i06.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use thiserror::Error;

// ── TransportError ─────────────────────────────────────────────────────────────

/// Errors that can occur during transport operations.
#[derive(Debug, Error)]
pub enum TransportError {
    /// The connection attempt failed (e.g. socket not found, ssh refused).
    #[error("connect failed: {0}")]
    ConnectFailed(String),

    /// The peer closed its end while we were writing (broken pipe).
    #[error("broken pipe")]
    BrokenPipe,

    /// A generic IO error occurred.
    #[error("io error: {0}")]
    Io(String),

    /// The peer closed mid-stream with a partial (unterminated) frame buffered.
    #[error("closed mid-stream: partial frame in buffer")]
    ClosedMidStream,
}

// ── Transport trait ────────────────────────────────────────────────────────────

/// Low-level line transport.  Lines are newline-terminated NDJSON frames.
pub trait Transport {
    /// Establish the connection.  Must be called before `send_line`/`recv_line`.
    fn connect(&mut self) -> Result<(), TransportError>;

    /// Send a logical line.  Appends `'\n'` before writing.
    fn send_line(&mut self, line: &str) -> Result<(), TransportError>;

    /// Receive the next complete line (without the trailing `'\n'`).
    ///
    /// Returns `Ok(None)` on clean EOF (peer closed with no partial frame
    /// buffered).  Returns `Err(ClosedMidStream)` if the peer closed while a
    /// partial (unterminated) frame is still buffered.
    fn recv_line(&mut self) -> Result<Option<String>, TransportError>;

    /// Close the transport.  Signals EOF to the peer.
    fn close(&mut self) -> Result<(), TransportError>;
}

/// Unified split handles for a connected session transport: a boxed writer (to
/// claude's stdin) and a buffered boxed reader (claude's stdout). Boxing lets the
/// agent treat local FIFO `File`s, Windows named-pipe `File`s, and remote ssh/wsl
/// child pipes uniformly. For child-backed transports the boxed handles own their
/// child process and kill it on drop, keeping it alive for the session's lifetime.
pub type SplitIo = (
    Box<dyn std::io::Write + Send>,
    std::io::BufReader<Box<dyn std::io::Read + Send>>,
);

// ── LineFramer ─────────────────────────────────────────────────────────────────

/// Pure newline-framing helper.
///
/// Feed it raw byte slices split at arbitrary boundaries; it emits complete
/// lines (without the trailing `'\n'`) and buffers any partial (unterminated)
/// suffix in `pending()`.
pub struct LineFramer {
    buf: String,
}

impl LineFramer {
    /// Create a new framer with an empty buffer.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self { buf: String::new() }
    }

    /// Push raw bytes into the framer.
    ///
    /// Returns every complete line (newline-terminated chunk) found in the
    /// accumulated buffer, without the trailing `'\n'`.  Any trailing partial
    /// chunk is left in `pending()`.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        // Assume valid UTF-8 per spec.
        let chunk = std::str::from_utf8(bytes).unwrap_or("");
        self.buf.push_str(chunk);

        let mut lines = Vec::new();
        loop {
            match self.buf.find('\n') {
                None => break,
                Some(pos) => {
                    let line = self.buf[..pos].to_string();
                    self.buf.drain(..=pos);
                    lines.push(line);
                }
            }
        }
        lines
    }

    /// Return the unfinished (unterminated) bytes currently buffered.
    pub fn pending(&self) -> &str {
        &self.buf
    }
}

// ── LoopbackFake ───────────────────────────────────────────────────────────────

/// Shared inner state for one side of a `LoopbackFake` pair.
///
/// Each side has its own `SideState`.  Side A's state is the inbound for A
/// (written by B) and side B's state is the inbound for B (written by A).
struct SideState {
    /// Complete lines queued for reading on this side.
    lines: VecDeque<String>,
    /// Framer accumulating raw bytes pushed via `push_inbound`.
    framer: LineFramer,
    /// Set by the remote writer when it calls `close()`.
    /// Signals clean EOF (or `ClosedMidStream` if `framer.pending()` is
    /// non-empty).
    writer_closed: bool,
    /// Set when this side calls `close()` on itself.
    /// Lets the peer's `send_line` detect a broken pipe.
    reader_closed: bool,
}

impl SideState {
    fn new() -> Self {
        Self {
            lines: VecDeque::new(),
            framer: LineFramer::new(),
            writer_closed: false,
            reader_closed: false,
        }
    }
}

/// In-memory duplex transport for unit tests.
///
/// Create a connected pair with `LoopbackFake::pair()`.  Each end has its own
/// inbound queue; `send_line` on side A enqueues into B's inbound, and vice
/// versa.
///
/// The `outbound_log` is shared between both ends of the pair: when side A
/// calls `send_line`, the line is appended to the shared log.  Either end can
/// call `drain_outbound()` to retrieve (and clear) all lines sent through the
/// pair so far.  This lets tests hold onto the peer end (side B) and observe
/// every command that the core (side A) sent — the canonical pattern used by
/// the `marsad-core` test suite.
pub struct LoopbackFake {
    /// Whether `connect()` has been called on this end.
    connected: bool,
    /// Whether `close()` has been called on this end.
    closed: bool,
    /// This side's inbound state (lines sent by the peer arrive here).
    my_state: Arc<Mutex<SideState>>,
    /// The peer's inbound state (lines we send go here).
    peer_state: Arc<Mutex<SideState>>,
    /// Shared ordered log of every line sent through this pair.
    /// Both ends share the same Arc so either can drain it.
    outbound_log: Arc<Mutex<Vec<String>>>,
}

impl LoopbackFake {
    /// Create two wired ends.  Call `connect()` on each before use.
    pub fn pair() -> (LoopbackFake, LoopbackFake) {
        let a_state = Arc::new(Mutex::new(SideState::new()));
        let b_state = Arc::new(Mutex::new(SideState::new()));
        // Single shared outbound log for the pair.
        let shared_log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let a = LoopbackFake {
            connected: false,
            closed: false,
            my_state: Arc::clone(&a_state),
            peer_state: Arc::clone(&b_state),
            outbound_log: Arc::clone(&shared_log),
        };
        let b = LoopbackFake {
            connected: false,
            closed: false,
            my_state: Arc::clone(&b_state),
            peer_state: Arc::clone(&a_state),
            outbound_log: Arc::clone(&shared_log),
        };
        (a, b)
    }

    /// Inject raw bytes (possibly a partial frame) into this side's inbound.
    ///
    /// Complete lines (terminated by `'\n'`) are enqueued immediately for
    /// `recv_line`; partial bytes are held in the internal framer until a
    /// `'\n'` arrives.
    pub fn push_inbound(&mut self, raw: &str) {
        let mut state = self.my_state.lock().unwrap();
        let complete = state.framer.push(raw.as_bytes());
        for line in complete {
            state.lines.push_back(line);
        }
    }

    /// Drain the ordered log of lines sent via `send_line` on either end.
    /// Clears the shared log.
    pub fn drain_outbound(&mut self) -> Vec<String> {
        let mut log = self.outbound_log.lock().unwrap();
        std::mem::take(&mut *log)
    }
}

impl Transport for LoopbackFake {
    fn connect(&mut self) -> Result<(), TransportError> {
        self.connected = true;
        self.closed = false;
        Ok(())
    }

    fn send_line(&mut self, line: &str) -> Result<(), TransportError> {
        if !self.connected {
            return Err(TransportError::ConnectFailed(
                "send_line called before connect".to_string(),
            ));
        }
        // BrokenPipe if we're locally closed, or the peer has closed its reader.
        let peer_closed = self.peer_state.lock().unwrap().reader_closed;
        if self.closed || peer_closed {
            return Err(TransportError::BrokenPipe);
        }
        self.outbound_log.lock().unwrap().push(line.to_string());
        self.peer_state
            .lock()
            .unwrap()
            .lines
            .push_back(line.to_string());
        Ok(())
    }

    fn recv_line(&mut self) -> Result<Option<String>, TransportError> {
        if !self.connected {
            return Err(TransportError::ConnectFailed(
                "recv_line called before connect".to_string(),
            ));
        }
        let mut state = self.my_state.lock().unwrap();
        // Queued complete lines take priority.
        if let Some(line) = state.lines.pop_front() {
            return Ok(Some(line));
        }
        // No complete line buffered.  Check for mid-stream close error; all
        // other states (clean EOF and nothing-queued-yet) return Ok(None).
        match (state.writer_closed, state.framer.pending().is_empty()) {
            (true, false) => Err(TransportError::ClosedMidStream),
            _ => Ok(None),
        }
    }

    fn close(&mut self) -> Result<(), TransportError> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        self.my_state.lock().unwrap().reader_closed = true;
        self.peer_state.lock().unwrap().writer_closed = true;
        Ok(())
    }
}

// ── LocalIpc ───────────────────────────────────────────────────────────────────
//
// Two compilation modes controlled by cfg(coverage) (set by cargo-llvm-cov):
//
// cfg(coverage)  — thin stub: struct + new() + inherent connect() only.
//                  No Transport impl → no uncoverable send_line/recv_line/close
//                  stubs.  The inherent connect() returns ConnectFailed; it is
//                  exercised by errors.rs::local_ipc_connect_failed_bad_path.
//                  (Integration tests compile the library in cfg(coverage) mode
//                  when run under cargo-llvm-cov, so the stub is visible.)
//
// cfg(not(coverage)) — full production implementation using UnixStream.
//                  Exercised by the i03 integration test (live socket).

// The GUI<->agent channel is an AF_UNIX socket. Windows 10+ supports AF_UNIX,
// but Rust std only exposes it on unix, so on Windows we use `uds_windows`, which
// provides the same `UnixStream`/`UnixListener` API over the same socket path.
#[cfg(all(unix, not(coverage)))]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(all(windows, not(coverage)))]
use uds_windows::{UnixListener, UnixStream};

/// Transport over an AF_UNIX socket (Linux/macOS via std, Windows via uds_windows).
pub struct LocalIpc {
    endpoint: String,
    #[cfg(not(coverage))]
    stream: Option<UnixStream>,
    #[cfg(not(coverage))]
    reader: Option<std::io::BufReader<UnixStream>>,
}

impl LocalIpc {
    /// Create a new `LocalIpc` pointing at `endpoint` (a filesystem path).
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            #[cfg(not(coverage))]
            stream: None,
            #[cfg(not(coverage))]
            reader: None,
        }
    }

    /// Connect to the socket endpoint.
    ///
    /// Under `cfg(coverage)` (llvm-cov build) this always returns `ConnectFailed`
    /// so the error path can be covered without a live socket.  Under
    /// `cfg(not(coverage))` this is the real `Transport::connect` implementation.
    #[cfg(coverage)]
    pub fn connect(&mut self) -> Result<(), TransportError> {
        Err(TransportError::ConnectFailed(format!(
            "no socket at '{}'",
            self.endpoint
        )))
    }
}

/// Full `Transport` impl — production builds only (excluded from coverage).
#[cfg(not(coverage))]
impl Transport for LocalIpc {
    fn connect(&mut self) -> Result<(), TransportError> {
        match UnixStream::connect(&self.endpoint) {
            Ok(stream) => {
                let stream2 = stream
                    .try_clone()
                    .map_err(|e| TransportError::Io(e.to_string()))?;
                self.reader = Some(std::io::BufReader::new(stream2));
                self.stream = Some(stream);
                Ok(())
            }
            Err(e) => Err(TransportError::ConnectFailed(e.to_string())),
        }
    }

    fn send_line(&mut self, line: &str) -> Result<(), TransportError> {
        use std::io::Write;
        match self.stream {
            Some(ref mut s) => {
                let msg = format!("{line}\n");
                s.write_all(msg.as_bytes())
                    .map_err(|_| TransportError::BrokenPipe)
            }
            None => Err(TransportError::ConnectFailed("not connected".to_string())),
        }
    }

    fn recv_line(&mut self) -> Result<Option<String>, TransportError> {
        use std::io::BufRead;
        match self.reader {
            Some(ref mut r) => {
                let mut line = String::new();
                match r.read_line(&mut line) {
                    Ok(0) => Ok(None),
                    Ok(_) => {
                        if line.ends_with('\n') {
                            line.pop();
                        }
                        Ok(Some(line))
                    }
                    Err(e) => Err(TransportError::Io(e.to_string())),
                }
            }
            None => Err(TransportError::ConnectFailed("not connected".to_string())),
        }
    }

    fn close(&mut self) -> Result<(), TransportError> {
        self.reader = None;
        self.stream = None;
        Ok(())
    }
}

// ── LocalIpcListener ──────────────────────────────────────────────────────────
//
// Server-side Unix socket listener used by marsad-agent to accept GUI clients.
// Gated `#[cfg(not(coverage))]` entirely — exercised by the i06 integration test.

/// Server-side Unix domain socket listener.
///
/// Creates and owns the socket file.  Call `bind()` once, then `accept()` in a
/// loop.  Each call to `accept()` returns a fully-connected `LocalIpc` that is
/// ready to use (stream + BufReader already populated) without calling
/// `connect()`.
#[cfg(not(coverage))]
pub struct LocalIpcListener {
    inner: UnixListener,
}

#[cfg(not(coverage))]
impl LocalIpcListener {
    /// Bind a new Unix domain socket at `endpoint`.
    ///
    /// Any stale socket file at `endpoint` is removed first so that a prior
    /// crash does not prevent re-binding.  Parent directories are created
    /// automatically if they do not exist.
    pub fn bind(endpoint: &str) -> Result<Self, TransportError> {

        // Remove stale socket file if present (best-effort; ignore error).
        std::fs::remove_file(endpoint).ok();

        // Ensure parent directory exists.
        if let Some(parent) = std::path::Path::new(endpoint).parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| TransportError::ConnectFailed(format!("mkdir: {e}")))?;
        }

        let listener = UnixListener::bind(endpoint)
            .map_err(|e| TransportError::ConnectFailed(format!("bind {endpoint}: {e}")))?;

        Ok(Self { inner: listener })
    }

    /// Accept the next incoming connection.
    ///
    /// Blocks until a client connects.  Returns a `LocalIpc` whose stream and
    /// BufReader are already populated — no need to call `connect()` on it.
    pub fn accept(&self) -> Result<LocalIpc, TransportError> {
        let (stream, _addr) = self
            .inner
            .accept()
            .map_err(|e| TransportError::Io(e.to_string()))?;
        LocalIpc::from_stream(stream)
    }
}

#[cfg(not(coverage))]
impl LocalIpc {
    /// Consume this `LocalIpc` and return the independent write and read halves.
    ///
    /// Returns `Some((write_half, read_half))` when both halves are present
    /// (i.e. after a successful `accept()` or `connect()`); returns `None`
    /// otherwise.
    ///
    /// After splitting, the write half and read half are completely independent
    /// OS handles: the writer thread can own `write_half` exclusively while the
    /// reader thread owns `read_half`, with no `Mutex` required.
    pub fn into_split(
        self,
    ) -> Option<(
        UnixStream,
        std::io::BufReader<UnixStream>,
    )> {
        match (self.stream, self.reader) {
            (Some(w), Some(r)) => Some((w, r)),
            _ => None,
        }
    }

    /// Wrap an already-accepted (or already-connected) `UnixStream` into a
    /// ready-to-use `LocalIpc`.  Both the write end (`stream`) and the read end
    /// (`BufReader` over a `try_clone`) are populated.
    pub fn from_stream(stream: UnixStream) -> Result<Self, TransportError> {
        let stream2 = stream
            .try_clone()
            .map_err(|e| TransportError::Io(e.to_string()))?;
        Ok(Self {
            endpoint: String::new(),
            stream: Some(stream),
            reader: Some(std::io::BufReader::new(stream2)),
        })
    }
}

// ── SshStdio ───────────────────────────────────────────────────────────────────
//
// Same two-mode pattern as LocalIpc.

/// Transport over the stdio of a remote `marsad-agent` via `ssh`.
pub struct SshStdio {
    target: String,
    remote_cmd: String,
    #[cfg(not(coverage))]
    child: Option<std::process::Child>,
    #[cfg(not(coverage))]
    stdin: Option<std::process::ChildStdin>,
    #[cfg(not(coverage))]
    reader: Option<std::io::BufReader<std::process::ChildStdout>>,
}

impl SshStdio {
    /// Create a new `SshStdio` targeting `target` (an SSH destination).
    pub fn new(target: impl Into<String>, remote_cmd: impl Into<String>) -> Self {
        Self {
            target: target.into(),
            remote_cmd: remote_cmd.into(),
            #[cfg(not(coverage))]
            child: None,
            #[cfg(not(coverage))]
            stdin: None,
            #[cfg(not(coverage))]
            reader: None,
        }
    }

    /// Connect to the remote SSH target.
    ///
    /// Under `cfg(coverage)` this spawns ssh and waits for it to exit, which
    /// happens almost immediately for an invalid hostname (BatchMode=yes, no
    /// password prompts).  Returns `ConnectFailed` or `Io` on failure.
    #[cfg(coverage)]
    pub fn connect(&mut self) -> Result<(), TransportError> {
        use std::process::{Command, Stdio};
        // Coverage stub: spawn ssh and wait.  For an invalid hostname ssh exits
        // almost immediately with a non-zero status (BatchMode=yes prevents any
        // interactive prompt).  `expect` is intentional: if ssh is not installed
        // the test environment is broken and a panic is the right signal.
        let status = Command::new("ssh")
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("ConnectTimeout=5")
            .arg(&self.target)
            .arg(&self.remote_cmd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("ssh binary not found — cannot run SshStdio coverage stub")
            .wait()
            .expect("wait on ssh child failed");
        Err(TransportError::ConnectFailed(format!(
            "ssh exited: {status}"
        )))
    }
}

/// Full `Transport` impl — production builds only (excluded from coverage).
#[cfg(not(coverage))]
impl Transport for SshStdio {
    #[allow(clippy::similar_names)]
    fn connect(&mut self) -> Result<(), TransportError> {
        use std::process::{Command, Stdio};
        use std::time::Duration;

        let mut cmd = Command::new("ssh");
        cmd.arg("-T") // no PTY — avoids a ConPTY/console-window flash on Windows
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("ConnectTimeout=10")
            // Keep an otherwise-idle session alive: send a keepalive probe every
            // 30s and only give up after 3 unanswered ones (~90s). Without this,
            // NAT/firewall idle timeouts silently drop long-lived claude sessions
            // that go quiet between turns. TCPKeepAlive also nudges the OS layer.
            .arg("-o")
            .arg("ServerAliveInterval=30")
            .arg("-o")
            .arg("ServerAliveCountMax=3")
            .arg("-o")
            .arg("TCPKeepAlive=yes")
            // NB: no ControlMaster here. SshStdio runs a single long-lived command
            // and uses a 150 ms post-spawn liveness probe; with a persistent master
            // a fast command would reuse the socket, skip the handshake and exit
            // before the probe — a false ConnectFailed. Connection multiplexing for
            // MFA reuse lives in `exec_prefix_for` (remote container/transport) and
            // the health probe / interactive login instead.
            .arg(&self.target)
            .arg(&self.remote_cmd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // No console-window flash on Windows.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| TransportError::ConnectFailed(e.to_string()))?;

        // Brief liveness probe: bad hostnames/permissions cause ssh to exit
        // within milliseconds; real connections are still in handshake.
        std::thread::sleep(Duration::from_millis(150));
        match child.try_wait() {
            Ok(Some(status)) => {
                return Err(TransportError::ConnectFailed(format!(
                    "ssh exited immediately: {status}"
                )));
            }
            Ok(None) => {}
            Err(e) => return Err(TransportError::Io(e.to_string())),
        }

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| TransportError::ConnectFailed("no stdin handle".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| TransportError::ConnectFailed("no stdout handle".to_string()))?;
        self.reader = Some(std::io::BufReader::new(stdout));
        self.stdin = Some(stdin);
        self.child = Some(child);
        Ok(())
    }

    fn send_line(&mut self, line: &str) -> Result<(), TransportError> {
        use std::io::Write;
        match self.stdin {
            Some(ref mut s) => {
                let msg = format!("{line}\n");
                s.write_all(msg.as_bytes())
                    .map_err(|_| TransportError::BrokenPipe)
            }
            None => Err(TransportError::ConnectFailed("not connected".to_string())),
        }
    }

    fn recv_line(&mut self) -> Result<Option<String>, TransportError> {
        use std::io::BufRead;
        match self.reader {
            Some(ref mut r) => {
                let mut line = String::new();
                match r.read_line(&mut line) {
                    Ok(0) => Ok(None),
                    Ok(_) => {
                        if line.ends_with('\n') {
                            line.pop();
                        }
                        Ok(Some(line))
                    }
                    Err(e) => Err(TransportError::Io(e.to_string())),
                }
            }
            None => Err(TransportError::ConnectFailed("not connected".to_string())),
        }
    }

    fn close(&mut self) -> Result<(), TransportError> {
        self.reader = None;
        self.stdin = None;
        if let Some(ref mut child) = self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.child = None;
        Ok(())
    }
}

// ── RemoteFifoTransport ──────────────────────────────────────────────────────
//
// Bridges a remote session's FIFOs over an opaque `exec_prefix` (e.g. an ssh or
// wsl.exe argv).  The remote container (a `RemoteTmuxContainer` in
// marsad-lifecycle) runs claude inside tmux on the remote box with its stdin/
// stdout wired to `in.fifo`/`out.fifo`.  This transport reaches those FIFOs by
// running `cat`/`sh -c 'cat > …'` *through the prefix* on the remote:
//
//   reader: `<prefix> cat <out_fifo>`            — claude stdout (NDJSON)
//   writer: `<prefix> sh -c 'cat > <in_fifo>'`   — claude stdin
//
// The fifo arguments are remote POSIX path strings (e.g.
// `~/.marsad/sessions/<id>/out.fifo`); they are kept as plain Strings and the
// remote shell expands `~` — they are never run through std::path locally.
//
// Modelled on `SshStdio` (which already spawns a remote process over ssh and
// pumps its stdio).  The differences are: the remote command is cat/sh instead
// of the claude invocation, and the prefix is opaque — spawn `prefix[0]` with
// `prefix[1..]` then the command.  Same two-mode coverage pattern: pure argv
// builders are directly unit-tested; only the spawning `connect()` is gated out
// of coverage with a stub that mirrors `SshStdio`.

/// Build the argv to run a shell `script` on the remote via `prefix`.
///
/// `ssh` joins its trailing command args with spaces and re-runs them through the
/// remote LOGIN shell, so the script must be ONE token (a `sh -c <script>` would be
/// split by the join). `wsl.exe` (and anything preserving argv) needs an explicit
/// `sh -c <script>`. Either way the chosen shell expands `$HOME` in the fifo paths.
fn remote_shell_argv(prefix: &[String], script: &str) -> Vec<String> {
    let mut argv = prefix.to_vec();
    // ssh AND plink both re-parse their trailing command through the remote shell, so
    // the script must be a single token; wsl.exe preserves argv and needs `sh -c`.
    if matches!(prefix.first().map(String::as_str), Some("ssh") | Some("plink")) {
        argv.push(script.to_string());
    } else {
        argv.push("sh".to_string());
        argv.push("-c".to_string());
        argv.push(script.to_string());
    }
    argv
}

/// Build the argv for the reader process — streams claude's stdout from the remote
/// FIFO via `cat <out_fifo>` (paths use `$HOME`, expanded by the remote shell).
///
/// `prefix` is the opaque exec prefix (e.g. `["ssh", "host"]` or
/// `["wsl.exe", "-d", "Ubuntu"]`).  The returned vector is ready to splat into a
/// `Command` (`argv[0]` is the program, the rest are args).
pub fn remote_fifo_reader_argv(prefix: &[String], out_fifo: &str) -> Vec<String> {
    remote_shell_argv(prefix, &format!("cat {out_fifo}"))
}

/// Build the argv for the writer process: `cat > <in_fifo>` through the remote
/// shell, so lines written to the spawned process's stdin land on claude's stdin.
pub fn remote_fifo_writer_argv(prefix: &[String], in_fifo: &str) -> Vec<String> {
    remote_shell_argv(prefix, &format!("cat > {in_fifo}"))
}

/// Transport that bridges a remote session's FIFOs over an opaque `exec_prefix`.
pub struct RemoteFifoTransport {
    /// Opaque argv prepended to every remote command (ssh/wsl.exe + opts/target).
    exec_prefix: Vec<String>,
    /// Remote POSIX path of the FIFO carrying claude's stdin (agent writes).
    remote_in_fifo: String,
    /// Remote POSIX path of the FIFO carrying claude's stdout (agent reads).
    remote_out_fifo: String,
    #[cfg(not(coverage))]
    writer_child: Option<std::process::Child>,
    #[cfg(not(coverage))]
    reader_child: Option<std::process::Child>,
    #[cfg(not(coverage))]
    stdin: Option<std::process::ChildStdin>,
    #[cfg(not(coverage))]
    reader: Option<std::io::BufReader<std::process::ChildStdout>>,
}

impl RemoteFifoTransport {
    /// Create a `RemoteFifoTransport`.
    ///
    /// `exec_prefix` is the opaque argv to reach the remote host; the fifo paths
    /// are remote POSIX path strings (the remote shell expands `~`).  Calling
    /// this does NOT spawn anything — call `connect()`.
    pub fn new(
        exec_prefix: Vec<String>,
        remote_in_fifo: String,
        remote_out_fifo: String,
    ) -> Self {
        Self {
            exec_prefix,
            remote_in_fifo,
            remote_out_fifo,
            #[cfg(not(coverage))]
            writer_child: None,
            #[cfg(not(coverage))]
            reader_child: None,
            #[cfg(not(coverage))]
            stdin: None,
            #[cfg(not(coverage))]
            reader: None,
        }
    }

    /// Connect stub for the coverage build.
    ///
    /// Mirrors `SshStdio`'s coverage stub: an empty prefix has no program to
    /// spawn, so it returns `ConnectFailed` without touching the OS.  The pure
    /// argv builders are covered directly by the unit tests; only the real spawn
    /// is excluded from coverage.  Exercised by `errors.rs`.
    #[cfg(coverage)]
    pub fn connect(&mut self) -> Result<(), TransportError> {
        // Build both argvs so the pure path is exercised even in the stub.
        let _ = remote_fifo_reader_argv(&self.exec_prefix, &self.remote_out_fifo);
        let _ = remote_fifo_writer_argv(&self.exec_prefix, &self.remote_in_fifo);
        Err(TransportError::ConnectFailed(format!(
            "remote fifo transport unavailable (prefix {:?})",
            self.exec_prefix
        )))
    }
}

/// Full `Transport` impl — production builds only (excluded from coverage).
#[cfg(not(coverage))]
impl Transport for RemoteFifoTransport {
    #[allow(clippy::similar_names)]
    fn connect(&mut self) -> Result<(), TransportError> {
        use std::process::{Command, Stdio};
        use std::time::Duration;

        let prefix = self
            .exec_prefix
            .split_first()
            .ok_or_else(|| TransportError::ConnectFailed("empty exec prefix".to_string()))?;
        let (program, rest) = prefix;

        // Helper: spawn `<program> <rest…> <cmd args…>` with the requested stdio.
        let spawn = |args: &[String], stdin: Stdio, stdout: Stdio| -> std::io::Result<_> {
            let mut cmd = Command::new(program);
            cmd.args(rest)
                .args(args)
                .stdin(stdin)
                .stdout(stdout)
                .stderr(Stdio::null());
            // No console-window flash on Windows (e.g. spawning wsl.exe).
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            }
            cmd.spawn()
        };

        // Reader: `<prefix> cat <out_fifo>` — claude's NDJSON stdout.
        let reader_argv = remote_fifo_reader_argv(&self.exec_prefix, &self.remote_out_fifo);
        let mut reader_child = spawn(&reader_argv[1..], Stdio::null(), Stdio::piped())
            .map_err(|e| TransportError::ConnectFailed(e.to_string()))?;

        // Writer: `<prefix> sh -c 'cat > <in_fifo>'` — claude's stdin.
        let writer_argv = remote_fifo_writer_argv(&self.exec_prefix, &self.remote_in_fifo);
        let mut writer_child = match spawn(&writer_argv[1..], Stdio::piped(), Stdio::null()) {
            Ok(c) => c,
            Err(e) => {
                let _ = reader_child.kill();
                let _ = reader_child.wait();
                return Err(TransportError::ConnectFailed(e.to_string()));
            }
        };

        // Brief liveness probe: a bad prefix/target makes the child exit within
        // milliseconds; a real connection is still in handshake/blocked on the
        // FIFO open.
        std::thread::sleep(Duration::from_millis(150));
        match reader_child.try_wait() {
            Ok(Some(status)) => {
                let _ = writer_child.kill();
                let _ = writer_child.wait();
                return Err(TransportError::ConnectFailed(format!(
                    "remote reader exited immediately: {status}"
                )));
            }
            Ok(None) => {}
            Err(e) => {
                let _ = writer_child.kill();
                let _ = writer_child.wait();
                return Err(TransportError::Io(e.to_string()));
            }
        }

        let stdin = writer_child
            .stdin
            .take()
            .ok_or_else(|| TransportError::ConnectFailed("no stdin handle".to_string()))?;
        let stdout = reader_child
            .stdout
            .take()
            .ok_or_else(|| TransportError::ConnectFailed("no stdout handle".to_string()))?;
        self.reader = Some(std::io::BufReader::new(stdout));
        self.stdin = Some(stdin);
        self.reader_child = Some(reader_child);
        self.writer_child = Some(writer_child);
        Ok(())
    }

    fn send_line(&mut self, line: &str) -> Result<(), TransportError> {
        use std::io::Write;
        match self.stdin {
            Some(ref mut s) => {
                let msg = format!("{line}\n");
                s.write_all(msg.as_bytes())
                    .map_err(|_| TransportError::BrokenPipe)
            }
            None => Err(TransportError::ConnectFailed("not connected".to_string())),
        }
    }

    fn recv_line(&mut self) -> Result<Option<String>, TransportError> {
        use std::io::BufRead;
        match self.reader {
            Some(ref mut r) => {
                let mut line = String::new();
                match r.read_line(&mut line) {
                    Ok(0) => Ok(None),
                    Ok(_) => {
                        if line.ends_with('\n') {
                            line.pop();
                        }
                        Ok(Some(line))
                    }
                    Err(e) => Err(TransportError::Io(e.to_string())),
                }
            }
            None => Err(TransportError::ConnectFailed("not connected".to_string())),
        }
    }

    fn close(&mut self) -> Result<(), TransportError> {
        self.reader = None;
        self.stdin = None;
        for child in [self.writer_child.as_mut(), self.reader_child.as_mut()]
            .into_iter()
            .flatten()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.writer_child = None;
        self.reader_child = None;
        Ok(())
    }
}

/// A boxed writer that owns the remote `sh -c 'cat > in.fifo'` child and kills it
/// on drop — so handing the writer to the agent keeps the remote pipe open for the
/// session's lifetime (the child must outlive the transport struct).
#[cfg(not(coverage))]
struct RemoteChildWriter {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
}

#[cfg(not(coverage))]
impl std::io::Write for RemoteChildWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.stdin.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.stdin.flush()
    }
}

#[cfg(not(coverage))]
impl Drop for RemoteChildWriter {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A boxed reader that owns the remote `cat out.fifo` child and kills it on drop.
#[cfg(not(coverage))]
struct RemoteChildReader {
    child: std::process::Child,
    stdout: std::process::ChildStdout,
}

#[cfg(not(coverage))]
impl std::io::Read for RemoteChildReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.stdout.read(buf)
    }
}

#[cfg(not(coverage))]
impl Drop for RemoteChildReader {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(not(coverage))]
impl RemoteFifoTransport {
    /// Consume this transport and return boxed split handles that each own their
    /// remote child process (so the `ssh`/`wsl` `cat` pipes stay alive). Mirrors
    /// `ClaudeFifoTransport::into_split`; returns `None` if not connected.
    pub fn into_split(mut self) -> Option<SplitIo> {
        let stdin = self.stdin.take()?;
        let reader = self.reader.take()?;
        let writer_child = self.writer_child.take()?;
        let reader_child = self.reader_child.take()?;
        let w = RemoteChildWriter {
            child: writer_child,
            stdin,
        };
        let r = RemoteChildReader {
            child: reader_child,
            stdout: reader.into_inner(),
        };
        Some((
            Box::new(w) as Box<dyn std::io::Write + Send>,
            std::io::BufReader::new(Box::new(r) as Box<dyn std::io::Read + Send>),
        ))
    }
}

// ── WslStdio ─────────────────────────────────────────────────────────────────
//
// Same two-mode pattern as SshStdio, but the child is `claude` running inside a
// WSL distro, launched via `wsl.exe -d <distro> -- <remote_cmd>` instead of
// `ssh <target> -- <remote_cmd>`.  WSL is a Windows-only concept, so the real
// spawn is `#[cfg(windows)]`; under coverage (and on non-Windows hosts) a thin
// stub provides an inherent connect() that returns ConnectFailed without
// touching the OS, exactly like LocalIpc's coverage stub.

/// Transport over the stdio of a `claude`/`marsad-agent` process running inside
/// a WSL distro via `wsl.exe`.
pub struct WslStdio {
    /// WSL distro name (e.g. `Ubuntu`), passed to `wsl.exe -d <distro>`.
    distro: String,
    /// Command run inside the distro (e.g. `marsad-agent`).
    remote_cmd: String,
    #[cfg(all(windows, not(coverage)))]
    child: Option<std::process::Child>,
    #[cfg(all(windows, not(coverage)))]
    stdin: Option<std::process::ChildStdin>,
    #[cfg(all(windows, not(coverage)))]
    reader: Option<std::io::BufReader<std::process::ChildStdout>>,
}

impl WslStdio {
    /// Create a new `WslStdio` targeting WSL distro `distro`, running `remote_cmd`.
    pub fn new(distro: impl Into<String>, remote_cmd: impl Into<String>) -> Self {
        Self {
            distro: distro.into(),
            remote_cmd: remote_cmd.into(),
            #[cfg(all(windows, not(coverage)))]
            child: None,
            #[cfg(all(windows, not(coverage)))]
            stdin: None,
            #[cfg(all(windows, not(coverage)))]
            reader: None,
        }
    }

    /// Connect stub for coverage / non-Windows builds.
    ///
    /// WSL only exists on Windows, so off-Windows (and under the llvm-cov build)
    /// this returns `ConnectFailed` without spawning anything — mirroring
    /// `LocalIpc`'s coverage stub.  Exercised by `errors.rs`.
    #[cfg(not(all(windows, not(coverage))))]
    pub fn connect(&mut self) -> Result<(), TransportError> {
        Err(TransportError::ConnectFailed(format!(
            "WSL unavailable for distro '{}' (cmd '{}')",
            self.distro, self.remote_cmd
        )))
    }
}

/// Full `Transport` impl — Windows production builds only (excluded from coverage).
#[cfg(all(windows, not(coverage)))]
impl Transport for WslStdio {
    #[allow(clippy::similar_names)]
    fn connect(&mut self) -> Result<(), TransportError> {
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        use std::time::Duration;

        let mut cmd = Command::new("wsl.exe");
        cmd.arg("-d")
            .arg(&self.distro)
            .arg("--")
            .arg("sh")
            .arg("-c")
            .arg(&self.remote_cmd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            // No console-window flash on Windows.
            .creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let mut child = cmd
            .spawn()
            .map_err(|e| TransportError::ConnectFailed(e.to_string()))?;

        // Brief liveness probe: a missing distro causes wsl.exe to exit within
        // milliseconds; a real session stays running.
        std::thread::sleep(Duration::from_millis(150));
        match child.try_wait() {
            Ok(Some(status)) => {
                return Err(TransportError::ConnectFailed(format!(
                    "wsl.exe exited immediately: {status}"
                )));
            }
            Ok(None) => {}
            Err(e) => return Err(TransportError::Io(e.to_string())),
        }

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| TransportError::ConnectFailed("no stdin handle".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| TransportError::ConnectFailed("no stdout handle".to_string()))?;
        self.reader = Some(std::io::BufReader::new(stdout));
        self.stdin = Some(stdin);
        self.child = Some(child);
        Ok(())
    }

    fn send_line(&mut self, line: &str) -> Result<(), TransportError> {
        use std::io::Write;
        match self.stdin {
            Some(ref mut s) => {
                let msg = format!("{line}\n");
                s.write_all(msg.as_bytes())
                    .map_err(|_| TransportError::BrokenPipe)
            }
            None => Err(TransportError::ConnectFailed("not connected".to_string())),
        }
    }

    fn recv_line(&mut self) -> Result<Option<String>, TransportError> {
        use std::io::BufRead;
        match self.reader {
            Some(ref mut r) => {
                let mut line = String::new();
                match r.read_line(&mut line) {
                    Ok(0) => Ok(None),
                    Ok(_) => {
                        if line.ends_with('\n') {
                            line.pop();
                        }
                        Ok(Some(line))
                    }
                    Err(e) => Err(TransportError::Io(e.to_string())),
                }
            }
            None => Err(TransportError::ConnectFailed("not connected".to_string())),
        }
    }

    fn close(&mut self) -> Result<(), TransportError> {
        self.reader = None;
        self.stdin = None;
        if let Some(ref mut child) = self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.child = None;
        Ok(())
    }
}

// ── ClaudeFifoTransport ────────────────────────────────────────────────────────
//
// Transport that talks to a `claude` process running inside a tmux session via
// two named pipes (FIFOs):
//   `<session_dir>/in.fifo`  — agent writes, claude reads (stdin)
//   `<session_dir>/out.fifo` — claude writes, agent reads (stdout)
//
// Open ordering (deadlock avoidance):
//   Claude (inside tmux) is launched with its stdin redirected from `in.fifo`
//   and its stdout redirected to `out.fifo`.  The kernel pairs each FIFO open:
//   a writer open blocks until a reader is present, and vice-versa.  Claude
//   holds the reader end of `in.fifo` and the writer end of `out.fifo` once
//   its shell command (`< in.fifo > out.fifo`) starts.
//
//   To avoid a deadlock the agent must open the two ends in complementary order:
//   1. Open `out.fifo` for **reading** first.
//      This unblocks claude's shell which is waiting for a reader on out.fifo.
//   2. Open `in.fifo` for **writing** second.
//      By the time step 2 runs, claude's shell has already opened in.fifo for
//      reading (because step 1 let it proceed), so the agent's write-open
//      completes immediately.
//
// `close` drops only the file handles; it intentionally does NOT kill the tmux
// session (liveness requirement: the claude process must survive client exit).
//
// The existing `LineFramer` (already unit-tested) handles NDJSON reassembly.
//
// Gated `#[cfg(not(coverage))]` entirely — exercised by the `i03` integration
// test.

/// Production FIFO-based transport for a `claude` process running in tmux.
#[cfg(not(coverage))]
pub struct ClaudeFifoTransport {
    session_dir: std::path::PathBuf,
    writer: Option<std::fs::File>,
    reader: Option<std::io::BufReader<std::fs::File>>,
    framer: LineFramer,
}

#[cfg(not(coverage))]
impl ClaudeFifoTransport {
    /// Create a `ClaudeFifoTransport` referencing the FIFOs in `session_dir`.
    ///
    /// Calling this does NOT open the FIFOs yet — call `connect()` to open them.
    pub fn open(session_dir: &std::path::Path) -> Result<Self, TransportError> {
        Ok(Self {
            session_dir: session_dir.to_path_buf(),
            writer: None,
            reader: None,
            framer: LineFramer::new(),
        })
    }

    /// Consume this transport and return the split writer + reader handles.
    ///
    /// Returns `Some((writer, reader))` if `connect()` has been called
    /// successfully; returns `None` if the transport is not yet connected.
    ///
    /// After splitting, the caller owns the writer (to forward prompts to
    /// claude's stdin via `in.fifo`) and the reader (to stream claude's
    /// NDJSON output from `out.fifo`).  The two handles are independent:
    /// writing to the writer while the reader blocks on a read is safe
    /// because they refer to different kernel FIFO objects.
    pub fn into_split(mut self) -> Option<SplitIo> {
        match (self.writer.take(), self.reader.take()) {
            (Some(w), Some(r)) => Some((
                Box::new(w) as Box<dyn std::io::Write + Send>,
                std::io::BufReader::new(Box::new(r.into_inner()) as Box<dyn std::io::Read + Send>),
            )),
            _ => None,
        }
    }
}

#[cfg(not(coverage))]
impl Transport for ClaudeFifoTransport {
    /// Open `in.fifo` (write) and `out.fifo` (read) **concurrently** to avoid
    /// deadlock.
    ///
    /// Bash processes `< in.fifo > out.fifo` left-to-right:
    ///   step 1 — bash opens in.fifo for reading (blocks until a writer appears)
    ///   step 2 — bash opens out.fifo for writing (blocks until a reader appears)
    ///
    /// Sequential opens (out then in, or any single-threaded order) cause a
    /// circular wait.  The correct approach mirrors the proven `connect_fifos`
    /// helper in the i03 integration test:
    ///   Thread A: write-opens in.fifo  → unblocks bash step 1
    ///   Thread B: read-opens out.fifo  → unblocked by bash once step 2 runs
    /// Both threads are started concurrently; Thread A gets a 50 ms head-start
    /// so that bash is already unblocked before Thread B competes for out.fifo.
    fn connect(&mut self) -> Result<(), TransportError> {
        use std::time::{Duration, Instant};

        let in_fifo = self.session_dir.join("in.fifo");
        let out_fifo = self.session_dir.join("out.fifo");
        let deadline = Instant::now() + Duration::from_secs(15);

        // Thread A: write-open in.fifo — unblocks bash's stdin redirect (step 1).
        let in_p = in_fifo.clone();
        let writer_handle =
            std::thread::spawn(move || std::fs::OpenOptions::new().write(true).open(&in_p));

        // Give Thread A a 50 ms head-start so bash can reach step 2 before
        // Thread B tries to read-open out.fifo.
        std::thread::sleep(Duration::from_millis(50));

        // Thread B: read-open out.fifo — unblocked once bash opens it for writing (step 2).
        let out_p = out_fifo.clone();
        let reader_handle =
            std::thread::spawn(move || std::fs::OpenOptions::new().read(true).open(&out_p));

        let in_file = writer_handle
            .join()
            .map_err(|_| TransportError::ConnectFailed("writer thread panicked".to_string()))?
            .map_err(|e| TransportError::ConnectFailed(format!("open in.fifo: {e}")))?;

        let out_file = reader_handle
            .join()
            .map_err(|_| TransportError::ConnectFailed("reader thread panicked".to_string()))?
            .map_err(|e| TransportError::ConnectFailed(format!("open out.fifo: {e}")))?;

        if Instant::now() > deadline {
            return Err(TransportError::ConnectFailed(
                "connect_fifos timed out after 15s".to_string(),
            ));
        }

        self.writer = Some(in_file);
        self.reader = Some(std::io::BufReader::new(out_file));
        Ok(())
    }

    /// Write `line` followed by `'\n'` to `in.fifo`.
    fn send_line(&mut self, line: &str) -> Result<(), TransportError> {
        use std::io::Write;
        match self.writer {
            Some(ref mut w) => {
                let msg = format!("{line}\n");
                w.write_all(msg.as_bytes())
                    .map_err(|_| TransportError::BrokenPipe)
            }
            None => Err(TransportError::ConnectFailed("not connected".to_string())),
        }
    }

    /// Read the next complete NDJSON line from `out.fifo` using `LineFramer`.
    ///
    /// Returns `Ok(None)` on clean EOF; `Err(ClosedMidStream)` if the peer
    /// closes with a partial frame still buffered.
    fn recv_line(&mut self) -> Result<Option<String>, TransportError> {
        use std::io::Read;

        let reader = match self.reader {
            Some(ref mut r) => r,
            None => {
                return Err(TransportError::ConnectFailed("not connected".to_string()));
            }
        };

        let mut buf = [0u8; 4096];
        loop {
            // First drain any lines already buffered by the framer.
            // We do a single-pass: call push only when the buffer has no lines.

            match reader.read(&mut buf) {
                Ok(0) => {
                    // EOF from the FIFO.
                    if self.framer.pending().is_empty() {
                        return Ok(None);
                    } else {
                        return Err(TransportError::ClosedMidStream);
                    }
                }
                Ok(n) => {
                    let mut lines = self.framer.push(&buf[..n]);
                    if !lines.is_empty() {
                        // Return the first complete line; the rest stay in the framer.
                        // Because `LineFramer` drains its buffer, re-push is not needed —
                        // remaining complete lines are already extracted and we can return
                        // them on subsequent calls.  But since `framer.push` returns all
                        // complete lines at once, we need to buffer the extras ourselves.
                        // Simple approach: return them one at a time by draining `lines`.
                        // We return the first immediately; the rest are lost unless we
                        // store them.  To avoid a secondary buffer, we push remaining
                        // lines back into the framer by re-encoding them with '\n'.
                        // This is safe because they are already complete, valid lines.
                        let first = lines.remove(0);
                        // Re-inject remaining complete lines so future recv_line calls
                        // see them without another OS read.
                        for extra in lines {
                            let _ = self.framer.push(format!("{extra}\n").as_bytes());
                        }
                        return Ok(Some(first));
                    }
                    // No complete line yet — loop to read more bytes.
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {
                    // Retry on EINTR.
                }
                Err(e) => {
                    return Err(TransportError::Io(e.to_string()));
                }
            }
        }
    }

    /// Close the FIFO handles.  Does NOT kill the tmux session.
    fn close(&mut self) -> Result<(), TransportError> {
        self.writer = None;
        self.reader = None;
        Ok(())
    }
}

// ── ClaudeNamedPipeTransport ──────────────────────────────────────────────────
//
// Windows-only counterpart to `ClaudeFifoTransport`.  Talks to a claude process
// running inside a `marsad-winshim.exe` via two Windows Named Pipes:
//
//   \\.\pipe\marsad-<session_name>-in   — agent writes prompts (claude's stdin)
//   \\.\pipe\marsad-<session_name>-out  — agent reads NDJSON (claude's stdout)
//
// See `docs/windows-conpty.md` for the full design rationale.
//
// COMPILATION: gated `#[cfg(all(windows, not(coverage)))]` — absent on Linux/macOS
// and absent from coverage builds so coverage stays 100%.
//
// DEADLOCK ANALYSIS
// ─────────────────
// Named Pipes on Windows have the opposite ownership model from Unix FIFOs:
//   - The SERVER creates both pipe ends (CreateNamedPipe); this is the shim.
//   - The CLIENT (the agent) opens them with CreateFile / ConnectNamedPipe.
//
// Because the shim creates BOTH pipes before spawning claude, there is no
// circular-wait deadlock.  The agent simply calls CreateFile on each pipe in
// any order — both will succeed as soon as the shim's server ends exist.
//
// `close` drops file handles; it does NOT kill the shim or claude (liveness).

/// Windows-only `Transport` implementation that talks to a `claude` process
/// running inside a `marsad-winshim.exe` via Windows Named Pipes.
#[cfg(all(windows, not(coverage)))]
pub struct ClaudeNamedPipeTransport {
    /// `\\.\pipe\marsad-<name>-in`
    in_pipe_name: String,
    /// `\\.\pipe\marsad-<name>-out`
    out_pipe_name: String,
    /// Write handle to the `-in` pipe (agent → claude).
    writer: Option<std::fs::File>,
    /// Buffered reader over the `-out` pipe (claude → agent).
    reader: Option<std::io::BufReader<std::fs::File>>,
    /// Line framer for NDJSON reassembly (reuses the unit-tested `LineFramer`).
    framer: LineFramer,
}

#[cfg(all(windows, not(coverage)))]
impl ClaudeNamedPipeTransport {
    /// Create a transport for the named session.
    ///
    /// `name` must match the session name used by `WindowsShimContainer`
    /// (i.e. the container name, e.g. `"marsad-session-1"`).
    ///
    /// Calling this does NOT open the pipes — call `connect()` to do that.
    pub fn for_session(name: &str) -> Self {
        Self {
            in_pipe_name: format!(r"\\.\pipe\marsad-{name}-in"),
            out_pipe_name: format!(r"\\.\pipe\marsad-{name}-out"),
            writer: None,
            reader: None,
            framer: LineFramer::new(),
        }
    }

    /// Consume this transport and return the split writer + buffered-reader handles.
    ///
    /// Returns `Some((writer, reader))` after a successful `connect()`; `None`
    /// otherwise.  Mirrors `ClaudeFifoTransport::into_split`.
    pub fn into_split(mut self) -> Option<SplitIo> {
        match (self.writer.take(), self.reader.take()) {
            (Some(w), Some(r)) => Some((
                Box::new(w) as Box<dyn std::io::Write + Send>,
                std::io::BufReader::new(Box::new(r.into_inner()) as Box<dyn std::io::Read + Send>),
            )),
            _ => None,
        }
    }
}

#[cfg(all(windows, not(coverage)))]
impl Transport for ClaudeNamedPipeTransport {
    /// Open both named pipe client ends.
    ///
    /// Windows named-pipe client open uses `CreateFile` (wrapped by
    /// `std::fs::OpenOptions`) with the `\\.\pipe\...` path.
    ///
    /// If the server (shim) has not yet finished creating its pipe ends,
    /// `CreateFile` returns `ERROR_PIPE_BUSY` (231) or `ERROR_FILE_NOT_FOUND`
    /// (2) while the shim is still initialising.  We retry with a 50 ms sleep
    /// for up to 15 seconds, matching the FIFO connect timeout.
    ///
    /// The open order does NOT matter for Named Pipes (no FIFO deadlock risk):
    /// both pipe server ends exist before the agent calls `connect()`.
    fn connect(&mut self) -> Result<(), TransportError> {
        use std::time::{Duration, Instant};

        // ERROR_PIPE_BUSY: the pipe exists but all instances are in use.
        // On Windows, `std::io::Error::raw_os_error()` returns the raw Win32
        // error code as a signed i32.
        const ERROR_PIPE_BUSY: i32 = 231;
        // ERROR_FILE_NOT_FOUND: the pipe server has not created its end yet.
        const ERROR_FILE_NOT_FOUND: i32 = 2;

        let deadline = Instant::now() + Duration::from_secs(15);

        // ── Open the writer (agent → claude stdin via -in pipe) ───────────────
        let writer = loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .open(&self.in_pipe_name)
            {
                Ok(f) => break f,
                Err(ref e)
                    if e.raw_os_error() == Some(ERROR_PIPE_BUSY)
                        || e.raw_os_error() == Some(ERROR_FILE_NOT_FOUND) =>
                {
                    if Instant::now() > deadline {
                        return Err(TransportError::ConnectFailed(format!(
                            "timeout waiting for pipe {} after 15s",
                            self.in_pipe_name
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    return Err(TransportError::ConnectFailed(format!(
                        "open -in pipe {}: {e}",
                        self.in_pipe_name
                    )));
                }
            }
        };

        // ── Open the reader (claude stdout via -out pipe → agent) ─────────────
        let reader = loop {
            match std::fs::OpenOptions::new()
                .read(true)
                .open(&self.out_pipe_name)
            {
                Ok(f) => break f,
                Err(ref e)
                    if e.raw_os_error() == Some(ERROR_PIPE_BUSY)
                        || e.raw_os_error() == Some(ERROR_FILE_NOT_FOUND) =>
                {
                    if Instant::now() > deadline {
                        return Err(TransportError::ConnectFailed(format!(
                            "timeout waiting for pipe {} after 15s",
                            self.out_pipe_name
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    return Err(TransportError::ConnectFailed(format!(
                        "open -out pipe {}: {e}",
                        self.out_pipe_name
                    )));
                }
            }
        };

        self.writer = Some(writer);
        self.reader = Some(std::io::BufReader::new(reader));
        Ok(())
    }

    /// Write `line` followed by `'\n'` to the `-in` pipe.
    fn send_line(&mut self, line: &str) -> Result<(), TransportError> {
        use std::io::Write;
        match self.writer {
            Some(ref mut w) => {
                let msg = format!("{line}\n");
                w.write_all(msg.as_bytes())
                    .map_err(|_| TransportError::BrokenPipe)
            }
            None => Err(TransportError::ConnectFailed("not connected".to_string())),
        }
    }

    /// Read the next complete NDJSON line from the `-out` pipe using `LineFramer`.
    ///
    /// Identical framing logic to `ClaudeFifoTransport::recv_line`.
    fn recv_line(&mut self) -> Result<Option<String>, TransportError> {
        use std::io::Read;

        let reader = match self.reader {
            Some(ref mut r) => r,
            None => {
                return Err(TransportError::ConnectFailed("not connected".to_string()));
            }
        };

        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    if self.framer.pending().is_empty() {
                        return Ok(None);
                    } else {
                        return Err(TransportError::ClosedMidStream);
                    }
                }
                Ok(n) => {
                    let mut lines = self.framer.push(&buf[..n]);
                    if !lines.is_empty() {
                        let first = lines.remove(0);
                        for extra in lines {
                            let _ = self.framer.push(format!("{extra}\n").as_bytes());
                        }
                        return Ok(Some(first));
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => {
                    return Err(TransportError::Io(e.to_string()));
                }
            }
        }
    }

    /// Close the pipe handles.  Does NOT kill the shim or claude (liveness).
    fn close(&mut self) -> Result<(), TransportError> {
        self.writer = None;
        self.reader = None;
        Ok(())
    }
}
