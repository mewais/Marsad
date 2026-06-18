# B08 — `marsad-transport`

**Tier:** L1. **Mode:** continuous. **Coverage:** 100% line+region (via loopback fake; real
local-IPC and SSH paths exercised by `i03`/`i06`).

## Purpose

The byte/line transport that carries NDJSON between a client (GUI / core) and a `claude` process'
supervisor. Two production implementations — `LocalIpc` (unix domain socket on Linux/macOS, named
pipe on Windows) and `SshStdio` (over `ssh` stdio to a remote `marsad-agent`) — plus an in-memory
`LoopbackFake`. Handles NDJSON framing including partial-line buffering.

## Public API (authoritative)

```rust
pub trait Transport {
    fn connect(&mut self) -> Result<(), TransportError>;
    fn send_line(&mut self, line: &str) -> Result<(), TransportError>;     // appends '\n'
    fn recv_line(&mut self) -> Result<Option<String>, TransportError>;     // None = clean EOF
    fn close(&mut self) -> Result<(), TransportError>;
}

pub struct LocalIpc { /* socket/pipe path */ }
impl LocalIpc { pub fn new(endpoint: impl Into<String>) -> Self; }

pub struct SshStdio { /* target + remote agent cmd */ }
impl SshStdio { pub fn new(target: impl Into<String>, remote_cmd: impl Into<String>) -> Self; }

pub struct LoopbackFake { /* in-memory duplex */ }
impl LoopbackFake {
    pub fn pair() -> (LoopbackFake, LoopbackFake);     // two ends wired together
    pub fn push_inbound(&mut self, raw: &str);          // feed raw bytes (may be partial frames)
    pub fn drain_outbound(&mut self) -> Vec<String>;    // what was sent
}

/// Pure framing helper, separately testable.
pub struct LineFramer { /* buffer */ }
impl LineFramer {
    pub fn new() -> Self;
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String>;  // returns any complete lines; buffers the remainder
    pub fn pending(&self) -> &str;
}

pub enum TransportError { ConnectFailed(String), BrokenPipe, Io(String), ClosedMidStream }
```

## Behavior notes

- `LineFramer` is the heart of coverage: feed it bytes split at arbitrary boundaries (mid-line, mid
  multi-byte UTF-8 is out of scope — assume valid UTF-8 line content) and it must emit complete lines
  only, buffering partials in `pending`. Multiple lines in one `push` all emit.
- `recv_line` returns `Ok(None)` on clean EOF and `Err(ClosedMidStream)` if the peer drops with a
  partial frame buffered.
- `LoopbackFake::pair` lets tests drive both ends without IO; `push_inbound` can deliver partial
  frames to prove reassembly.
- Resilience (`i03`): a dropped `SshStdio` surfaces `BrokenPipe`; the lifecycle layer (B09) decides
  reconnect — transport itself does not auto-reconnect, it reports.

## Test buckets (`tests/`)

- `happy.rs` — loopback send/recv round-trip; multi-line frame; clean EOF → `None`.
- `errors.rs` — `ConnectFailed`, `BrokenPipe`, `ClosedMidStream` with a buffered partial.
- `branches.rs` — `LineFramer`: empty push, partial then completion, several lines at once, trailing
  partial in `pending`, blank lines.
- `fakes.rs` — `Transport` contract against `LoopbackFake` (connect/send/recv/close ordering).

## Edit scope

Code-agent: write `crates/marsad-transport/src/**` + `Cargo.toml`; read-only `tests/`, this spec.
