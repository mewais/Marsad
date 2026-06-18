# B13 — `marsad-agent`

**Tier:** L2 (integration-bound). **Mode:** daemon (binary + lib). **Coverage:** unit-cover the pure
composition with fakes; the **real** behavior is proven by the `claude`-gated integration tests
(`i03`, `i07`, parts of `i02`/`i06`). Unit coverage target 100% of the lib's non-IO logic.

## Purpose

The detached supervisor. It owns every `claude` process so **sessions survive GUI exit**. It composes
`marsad-transport` + `marsad-lifecycle` + `marsad-events` + `marsad-store` + `marsad-hosts` +
`marsad-notify`, serves the IPC protocol to one or more cores, runs each `claude` under a tmux/ConPTY
container, and exposes the hook endpoint that Claude Code calls. The GUI is a client; this is the
process that must keep running.

## Public API (authoritative)

```rust
pub struct Agent { /* registry of per-session supervisors */ }

pub struct AgentConfig { pub listen: ListenSpec, pub data_root: String }
pub enum ListenSpec { UnixSocket(String), NamedPipe(String), Stdio /* for SshStdio remote mode */ }

impl Agent {
    pub fn new(cfg: AgentConfig) -> Result<Agent, AgentError>;
    pub fn serve(self) -> Result<(), AgentError>;                 // blocking event loop (the daemon)

    // Composition seams (unit-testable without real claude):
    pub fn launch(&mut self, spec: marsad_proto::LaunchSpec) -> Result<String, AgentError>;
    pub fn handle_ipc(&mut self, line: &str) -> Result<Vec<String>, AgentError>;   // inbound IPC -> outbound IPC lines
    pub fn handle_hook(&mut self, payload: &str) -> Result<(), AgentError>;        // Claude Code hook callback
    pub fn reconcile_all(&mut self) -> Vec<marsad_lifecycle::SessionRuntime>;       // on startup, re-adopt/resume/stale
}

/// The exact claude invocation the supervisor runs inside a container.
pub fn claude_argv(spec: &marsad_proto::LaunchSpec) -> Vec<String>;
// => ["claude","--print","--input-format","stream-json","--output-format","stream-json","--verbose", ...]

pub enum AgentError { Listen(String), Spawn(String), Transport(String), Protocol(String), Lifecycle(String) }
```

The binary (`src/main.rs`) parses config and calls `Agent::serve`.

## Behavior notes

- **Never parse rendered terminal output.** Drive `claude` only via stream-json + hooks + the control
  protocol. `claude_argv` centralizes the invocation (unit-tested for exact flags).
- Each session runs in container `marsad-<id>` (tmux on Linux/macOS, ConPTY on Windows) via the
  `marsad-lifecycle::Container` abstraction. The agent holds the process; cores attach/detach freely.
- `handle_ipc` decodes commands (launch/resume/attach/stop/prompt/interrupt/answer_gate/subscribe)
  and returns IPC events to push to subscribed cores. `handle_hook` ingests Notification/Stop/…
  payloads, folds them via `marsad-events`, and triggers `marsad-notify`.
- On startup `reconcile_all` runs `marsad-lifecycle::reconcile` per persisted session: adopt live,
  resume recoverable, mark stale. **No duplication** on adopt.
- Client disconnect must NOT kill sessions: dropping a core connection only removes a subscriber.

## Test buckets

Block-level (`tests/`, no real claude):
- `happy.rs` — `claude_argv` exact flags; `handle_ipc` command→event mapping with a fake transport;
  `handle_hook` → state/notify via fakes; `reconcile_all` over `FakeContainer`.
- `errors.rs` — `Listen`/`Spawn`/`Protocol` failures; malformed IPC/hook payloads.
- `branches.rs` — each IPC command; each hook type; adopt/resume/stale branches; subscriber add/drop
  without killing the session.

Integration (real claude, in `tests/integration/`):
- `i03` — launch→`system/init`; kill+`--resume` continuity; adopt running container with no
  duplication; client-disconnect liveness; terminal command over tmux; stage+commit smoke.
- `i07` — concurrency (N≥3 independent sessions, no cross-routing); prompt/interrupt round-trip.

## Edit scope

Code-agent: write `crates/marsad-agent/src/**` + `Cargo.toml`; read-only `tests/`, this spec.
