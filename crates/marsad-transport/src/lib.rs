//! marsad-transport — NDJSON line transport to claude: LocalIpc, SshStdio, loopback fake.
//!
//! See `../AGENTS.md` and `docs/specs/transport.md` for the authoritative contract.
//!
//! # Coverage regime
//!
//! The fully-testable surface is: `Transport` trait, `LineFramer`, `LoopbackFake`,
//! and `TransportError`.
//!
//! `LocalIpc` and `SshStdio` perform real OS IO that the unit/integration test
//! suite cannot exercise (no live Unix socket or SSH endpoint is available).
//! Their IO-heavy internals are excluded from the llvm-cov build via
//! `#[cfg(not(coverage))]`; `cargo llvm-cov` sets `cfg(coverage)` automatically.
//!
//! Under `cfg(coverage)`:
//! - `LocalIpc` and `SshStdio` are thin stubs (struct + new() + inherent connect()).
//!   The inherent `connect()` is covered by the `errors.rs` integration tests.
//!   No `Transport` trait impl exists for them in this build, so there are no
//!   uncoverable send_line/recv_line/close stubs.
//!
//! Under `cfg(not(coverage))` (normal build / plain `cargo test`):
//! - Full production implementations compile.  Integration tests exercise the
//!   connect()-fails-on-bad-path error path; live IO paths are exercised by i03/i06.

use thiserror::Error;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

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
        self.peer_state.lock().unwrap().lines.push_back(line.to_string());
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

/// Transport over a Unix domain socket (Linux/macOS) or named pipe (Windows).
pub struct LocalIpc {
    endpoint: String,
    #[cfg(not(coverage))]
    stream: Option<std::os::unix::net::UnixStream>,
    #[cfg(not(coverage))]
    reader: Option<std::io::BufReader<std::os::unix::net::UnixStream>>,
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
        use std::os::unix::net::UnixStream;
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
            .arg("-o").arg("BatchMode=yes")
            .arg("-o").arg("ConnectTimeout=5")
            .arg(&self.target)
            .arg(&self.remote_cmd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("ssh binary not found — cannot run SshStdio coverage stub")
            .wait()
            .expect("wait on ssh child failed");
        Err(TransportError::ConnectFailed(format!("ssh exited: {status}")))
    }
}

/// Full `Transport` impl — production builds only (excluded from coverage).
#[cfg(not(coverage))]
impl Transport for SshStdio {
    fn connect(&mut self) -> Result<(), TransportError> {
        use std::process::{Command, Stdio};
        use std::time::Duration;

        let mut child = Command::new("ssh")
            .arg("-o").arg("BatchMode=yes")
            .arg("-o").arg("ConnectTimeout=10")
            .arg(&self.target)
            .arg(&self.remote_cmd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
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
