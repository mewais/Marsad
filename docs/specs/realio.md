# Real-IO layer — TmuxContainer, FIFO process transport, Agent wiring

This spec covers the production real-IO that was deferred during the unit-coverage phase. It is
exercised by the **live** integration scenarios (i02/i03/i06/i07), NOT by crate unit tests. All
real-IO code is gated `#[cfg(not(coverage))]` so the 13 crates keep 100% unit coverage; its
correctness is proven by the `claude`-gated integration suite.

## Locked decision: tmux + named-pipe (FIFO) stdio

claude must run **inside tmux** (survives SSH drop / sleep / GUI exit) yet be driven via
**stream-json over stdio**. You cannot pipe a parent's stdio into a process inside a detached tmux
session, so we bridge with named pipes:

Per session `<id>`, in the agent runtime dir `~/.local/share/marsad/sessions/<id>/` (or
`$MARSAD_DATA_ROOT/sessions/<id>/`):
- `in.fifo`  — agent writes stream-json user input here; claude reads its stdin from it.
- `out.fifo` — claude writes stream-json output here; agent reads it.
- `meta.json` — `{ session_id, claude_session_id, model, project, created_at }` (for resume/adopt).

Launch (spawn):
```
mkfifo in.fifo out.fifo
tmux new-session -d -s marsad-<id> \
  'cd <project> && claude --print --input-format stream-json \
     --output-format stream-json --verbose --model <m> --permission-mode <mode> \
     [--resume <claude_session_id>] < in.fifo > out.fifo 2>err.log'
```
The agent then opens `out.fifo` for reading (its inbound stream) and `in.fifo` for writing.

## B09 `marsad-lifecycle` — `TmuxContainer`

Re-introduce, gated `#[cfg(not(coverage))]`, implementing the existing `Container` trait:
```rust
#[cfg(not(coverage))]
pub struct TmuxContainer { runtime_dir: std::path::PathBuf }
#[cfg(not(coverage))]
impl TmuxContainer { pub fn new(runtime_dir: impl Into<PathBuf>) -> Self; }
#[cfg(not(coverage))]
impl Container for TmuxContainer {
    fn exists(&self, name: &str) -> bool;     // `tmux has-session -t <name>` exit 0
    fn attach(&mut self, name: &str) -> Result<(), LifecycleError>;  // verify session live; reopen FIFOs
    fn spawn(&mut self, name: &str, cmd: &str) -> Result<(), LifecycleError>; // mkfifo + tmux new-session -d
    fn kill(&mut self, name: &str) -> Result<(), LifecycleError>;    // `tmux kill-session -t <name>`
}
```
- `exists` shells out to `tmux has-session`; absence → false (never errors).
- `spawn` is idempotent-safe: if the session already exists it does NOT create a second (no
  duplication) — returns Ok (adopt semantics) or a distinct already-exists signal the agent treats as
  adopt.
- All shape mirrors `FakeContainer` so `reconcile`/`reconcile_all` work unchanged.
- Unit coverage: this whole block is `#[cfg(not(coverage))]` → absent from the measured surface, so
  lifecycle stays 100%. The FakeContainer remains the unit-tested path.

## B08 `marsad-transport` — `ClaudeFifoTransport` (or extend existing)

A `Transport` impl that talks to a claude process via its FIFOs (local) — distinct from `SshStdio`
(remote). Gated `#[cfg(not(coverage))]`.
```rust
#[cfg(not(coverage))]
pub struct ClaudeFifoTransport { in_fifo: PathBuf, out_fifo: PathBuf, /* open handles */ }
#[cfg(not(coverage))]
impl ClaudeFifoTransport { pub fn open(session_dir: &Path) -> Result<Self, TransportError>; }
#[cfg(not(coverage))]
impl Transport for ClaudeFifoTransport {
    fn connect(&mut self) -> Result<(), TransportError>;  // open in.fifo (write), out.fifo (read)
    fn send_line(&mut self, line: &str) -> Result<(), TransportError>;
    fn recv_line(&mut self) -> Result<Option<String>, TransportError>; // LineFramer over out.fifo bytes
    fn close(&mut self) -> Result<(), TransportError>;     // close handles; does NOT kill tmux
}
```
- Reuse the already-tested `LineFramer` for NDJSON reassembly (it is unit-covered).
- `close` must NOT tear down the tmux session — liveness requires the process to outlive the client.

## B13 `marsad-agent` — real wiring

Add a `#[cfg(not(coverage))]` real constructor used by `serve()`/`main` (the fake path stays for
units):
```rust
#[cfg(not(coverage))]
impl Agent {
    pub fn with_real_io(cfg: AgentConfig) -> Result<Agent, AgentError>;
    // builds TmuxContainer(runtime_dir) + a real Notifier (DbusNotifier on Linux) + FsBackend(data_root)
}
```
- `launch` already calls `claude_argv` + `container.spawn(name, cmd)`. With `TmuxContainer`, `cmd` is
  the FIFO-wrapped tmux invocation above — so `launch`/`reconcile_all` logic is unchanged; only the
  injected container differs.
- After spawn, the agent opens a `ClaudeFifoTransport` for that session to read its stream-json and
  fold via `marsad_events::reduce` (the existing handle path), and to fire hooks→notify.
- On startup, `reconcile_all` adopts live tmux sessions from the runtime dir (meta.json per session)
  with NO new claude.

## Coverage & verification

- Unit coverage of all 13 crates MUST stay 100% — every item above is `#[cfg(not(coverage))]`, so it
  is excluded from the measured surface. Do NOT cfg-gate any pure logic; ONLY the real-IO bodies.
- Real correctness is proven by the live integration scenarios:
  - **i03:** `TmuxContainer` + agent — spawn real claude in tmux → observe `system/init` on out.fifo;
    kill + `--resume <claude_session_id>` → continuity; pre-existing tmux session → adopt, no
    duplication (assert a single claude PID / single tmux session); kill the *client* → tmux+claude
    persist → re-adopt; terminal-over-tmux + git stage/commit smoke.
  - **i07:** N≥3 concurrent real sessions advance independently; no cross-routing of gates/prompts.
  - **i02/i06:** real gate round-trip over a live session; real ssh smoke (localhost-as-SSH).
- Live tests MUST detect `claude`/`tmux`/`ssh` and **SKIP loudly** if absent (never silently pass).
- Clean up: each live test kills its `marsad-it-*` tmux sessions and removes its temp runtime dir in
  teardown (even on failure) so reruns are deterministic and no orphan claude processes leak.
