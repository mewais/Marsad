# B09 — `marsad-lifecycle`

**Tier:** L1. **Mode:** continuous. **Coverage:** 100% line+region (via `Container` fake; real
tmux exercised by `i03`).

## Purpose

The reconnect-or-resume algorithm and the container abstraction that makes **sessions outlive the
GUI**. Decides, per session on relaunch, whether to adopt a live container, `--resume` a recoverable
one, or mark it Stale. Owns the `Stale`/`Detached` classification.

## Public API (authoritative)

```rust
pub trait Container {
    fn exists(&self, name: &str) -> bool;            // e.g. tmux has-session -t marsad-<id>
    fn attach(&mut self, name: &str) -> Result<(), LifecycleError>;
    fn spawn(&mut self, name: &str, cmd: &str) -> Result<(), LifecycleError>;  // new detached container
    fn kill(&mut self, name: &str) -> Result<(), LifecycleError>;
}

pub struct TmuxContainer;     // Linux/macOS
pub struct ConPtyContainer;   // Windows
pub struct FakeContainer { /* scripted exists/attach/spawn results */ }
impl FakeContainer { pub fn new() -> Self; pub fn set_exists(&mut self, name: &str, v: bool); pub fn fail_attach(&mut self, name: &str); }

pub struct Persisted { pub session_id: Option<String>, pub container_name: String, pub project: String, pub model: String }
pub struct Probe { pub container_alive: bool }

pub enum Action {
    Adopt,                       // container alive -> reattach, no new process
    Resume(String),              // container dead, session_id known -> claude --resume <id> in fresh container
    MarkStale,                   // neither -> Stale, offer resume/restart
}
pub fn plan_reconnect(known: &Persisted, probe: &Probe) -> Action;

pub enum RuntimePhase { Detached, Stale }   // results mapped into events::Phase by the caller
pub struct SessionRuntime { pub action: Action, pub phase: Option<RuntimePhase> }
pub fn reconcile(known: &Persisted, container: &mut dyn Container) -> Result<SessionRuntime, LifecycleError>;

pub enum LifecycleError { AttachRace(String), ResumeFailed(String), Spawn(String), Container(String) }
```

## Behavior notes (port the session-liveness rules)

- `plan_reconnect`:
  - container alive → `Adopt` (re-adopt, **no new process, no duplication**).
  - container dead **and** `session_id` present → `Resume(session_id)`.
  - container dead **and** no `session_id` → `MarkStale`.
- `reconcile` probes via `Container.exists`, then performs the action: `Adopt` calls `attach`
  (an `attach` failure after `exists` returned true is the `AttachRace` case → re-probe → may fall to
  Resume/Stale); `Resume` `spawn`s a fresh container running `claude --resume`; on spawn/resume
  failure → `ResumeFailed`, surface Stale.
- `Detached` = container alive but the client isn't currently attached (adoptable); `Stale` = gone but
  recoverable. These are the two new states beyond the design's original five.
- A re-adopt must never start a second `claude` for the same session — the `i03` test asserts no
  duplication by checking process/container identity.

## Test buckets (`tests/`)

- `happy.rs` — each `Action` from `plan_reconnect`; `reconcile` adopt path; resume path.
- `errors.rs` — `AttachRace` (exists true, attach fails), `ResumeFailed`, spawn failure.
- `branches.rs` — alive/dead × session_id present/absent matrix; `Detached` vs `Stale` mapping.
- `fakes.rs` — `Container` contract via `FakeContainer` (exists/attach/spawn/kill).

## Edit scope

Code-agent: write `crates/marsad-lifecycle/src/**` + `Cargo.toml`; read-only `tests/`, this spec.
