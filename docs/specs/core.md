# B12 — `marsad-core`

**Tier:** L1. **Mode:** facade + FFI. **Coverage:** 100% line+region (via a fake agent endpoint
over `marsad-transport::LoopbackFake` + recorded IPC; live behavior in `i07`).

## Purpose

The orchestration facade that ties B01–B11 into the single API the desktop (Tauri) and the phones
(UniFFI) call. It is the **client** of `marsad-agent` (never spawns `claude` itself). It owns the
in-memory app model, applies agent events through `marsad-events`/`marsad-store`, exposes view-models
via `marsad-viewmodel`, and sends commands (launch/resume/attach/stop/prompt/gate) over
`marsad-transport`. Exposes a UniFFI interface for the phones.

## Public API (authoritative; this is also the UniFFI surface)

```rust
pub struct Core { /* ... */ }

pub struct CoreConfig { pub agent_endpoint: String, pub data_root: String }

// LaunchSpec/Posture are protocol data defined in marsad-proto (shared with marsad-agent). Core
// re-exports them so callers can keep naming `marsad_core::{LaunchSpec, Posture}`.
pub use marsad_proto::{LaunchSpec, Posture};

pub enum VmUpdate { Overview(/*...*/), Session(/*...*/), Toast { icon: String, text: String }, HostHealth(/*...*/) }

impl Core {
    pub fn new(cfg: CoreConfig, transport: Box<dyn marsad_transport::Transport>) -> Result<Core, CoreError>;
    pub fn launch_session(&mut self, spec: LaunchSpec) -> Result<String, CoreError>;   // returns session id
    pub fn resume_session(&mut self, id: &str) -> Result<(), CoreError>;
    pub fn attach_session(&mut self, id: &str) -> Result<(), CoreError>;
    pub fn stop_session(&mut self, id: &str) -> Result<(), CoreError>;
    pub fn send_prompt(&mut self, id: &str, text: &str) -> Result<(), CoreError>;
    pub fn interrupt(&mut self, id: &str) -> Result<(), CoreError>;
    pub fn answer_gate(&mut self, decision: marsad_gates::GateDecision) -> Result<(), CoreError>;
    pub fn run_slash(&mut self, id: &str, cmd: marsad_viewmodel::SlashCmd) -> Result<(), CoreError>;
    pub fn set_workspace(&mut self, id: &str) -> Result<(), CoreError>;
    pub fn snapshot(&self) -> marsad_viewmodel::AppSnapshot;     // for the UI to derive view-models
    pub fn on_agent_line(&mut self, line: &str) -> Result<Vec<VmUpdate>, CoreError>;  // feed inbound IPC; returns updates
    // host/notif/settings ops mirror the registry/prefs crates
}

pub enum CoreError { Transport(String), Protocol(String), UnknownSession(String), Agent(String) }
```

UniFFI: the crate exports a `.udl` (or proc-macro scaffolding) generating Swift + Kotlin bindings for
`Core`, the spec/enum types, and a `VmUpdate` callback/stream. Keep FFI types simple (no borrowed
refs across the boundary).

## Behavior notes

- `on_agent_line` parses an `IpcMsg` (B01), routes `AgentEvent`s into the right session's
  `SessionState` via `marsad-events`, persists via `marsad-store`, and emits `VmUpdate`s. This is the
  unit-testable seam: drive the whole facade by feeding recorded IPC lines and asserting updates.
- Commands serialize to `IpcMsg` and go out via `Transport::send_line`. With `LoopbackFake`, tests
  assert exact outbound lines (e.g. `answer_gate` routes to exactly one session — idempotent via
  `marsad-gates::DecisionLedger`).
- Gate decisions go through `marsad-gates` so routing + idempotency are enforced centrally.
- `CoreError` wraps lower-layer errors; never panics across the FFI boundary.

## Test buckets (`tests/`)

- `happy.rs` — launch/resume/attach/stop/send_prompt/answer_gate produce the expected outbound IPC;
  `on_agent_line` for a recorded init→working→needs→done sequence yields correct `VmUpdate`s.
- `errors.rs` — `UnknownSession`, transport failure, malformed inbound line → `Protocol`.
- `branches.rs` — every command; every `VmUpdate` variant; workspace switch re-scopes; slash dispatch;
  idempotent re-`answer_gate`.
- `fakes.rs` — drive `Core` over `LoopbackFake` end-to-end with a scripted agent.

## Edit scope

Code-agent: write `crates/marsad-core/src/**` + `Cargo.toml` (+ `.udl`); read-only `tests/`, this spec.
