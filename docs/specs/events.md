# B02 — `marsad-events`

**Tier:** L0. **Mode:** continuous (pure reducer). **Coverage:** 100% line+region.

## Purpose

The session state machine. Folds a stream of parsed `StreamMsg` (B01) and hook events into a derived
`SessionState`: phase, usage/cost/context%, last action, subagents, and any pending gate. Pure and
total — `reduce(state, event) -> state` never errors and never does IO. This is how Marsad detects
state **without parsing rendered terminal output**.

## Inputs / Outputs

- In: current `SessionState`, next `Event` (a `StreamMsg` or a `HookEvent`).
- Out: next `SessionState`.

## Public API (authoritative)

```rust
pub enum Phase { Starting, Working, Needs, Error, Done, Stale, Detached }

pub struct SessionState {
    pub session_id: Option<String>,
    pub phase: Phase,
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub tokens: u64,                 // total (input+output) across the session
    pub cost_usd: f64,
    pub context_pct: f32,           // consumed / model context window, 0..=100
    pub last_action: Option<String>,
    pub subagents: Vec<Subagent>,
    pub gate: Option<marsad_gates::Gate>,
    pub changed_files: u32,
}

pub struct Subagent { pub name: String, pub model: Option<String>, pub status: SubagentStatus }
pub enum SubagentStatus { Running, Done }

pub enum HookEvent { Notification { message: String }, Stop, SubagentStop { name: String }, PreToolUse { tool: String }, PostToolUse { tool: String }, SessionStart, SessionEnd, PreCompact }

pub enum Event { Stream(marsad_proto::StreamMsg), Hook(HookEvent) }

impl SessionState {
    pub fn initial() -> SessionState;        // Phase::Starting, zeros
    pub fn context_window_for(model: &str) -> u64;  // model -> context window size (table)
}

pub fn reduce(state: SessionState, event: Event) -> SessionState;
```

Derive `Debug, Clone, PartialEq` (+ serde) on all public types.

## Behavior notes (port the README "State detection" rules + .dc.html semantics)

- **Phase transitions:** `SystemInit` → `Working` (records session_id, model, cwd); assistant/tool
  activity → `Working`; a pending `ControlRequest` (can_use_tool) **or** a `Notification` hook →
  `Needs`; `Result{is_error|ErrorDuringExecution|ErrorMaxTurns}` → `Error`; `Result{Success}` or
  `Stop` → `Done`. `Stale`/`Detached` are set by the lifecycle layer, not by stream folding, but
  `reduce` must preserve them until a stream event supersedes.
- **Usage/cost/context:** accumulate from `Result.usage`/`total_cost_usd` and assistant `usage`;
  **dedup assistant usage by message id** (folding the same assistant message twice must not double
  count). `context_pct = consumed_tokens / context_window_for(model) * 100`, clamped to 100.
- **last_action:** latest tool call (`ToolUse.name` + summarized input) or assistant activity line.
- **subagents:** `SubagentStop`/subagent stream blocks update the matching `Subagent`.
- **gate:** set when a permission/plan/edit request is pending; cleared when answered (a subsequent
  user/assistant turn).
- **Totality:** an out-of-order or impossible event clamps/ignores rather than panicking; values
  never go negative; `context_pct` never exceeds 100.
- **Stale/Detached supersede rule:** when the current phase is `Stale` or `Detached` (set by the
  lifecycle layer, not by stream folding), `reduce` PRESERVES it for hook events and
  `StreamMsg::Unknown`, and only a `SystemInit` supersedes it (a fresh `system/init` means the
  session is genuinely live again → `Working`). All other stream messages (`Assistant`/`User`/
  `Result`/control) while `Stale`/`Detached` are treated as totality no-ops that preserve the phase
  (they cannot arrive from a dead/detached container without an init first). This keeps the two
  lifecycle states authoritative until a real re-init is observed.

## Test buckets (`tests/`)

- `happy.rs` — full nominal lifecycle: initial → init → working → needs → answered → done; usage
  accumulation; subagent run→done.
- `errors.rs` — error result subtypes → `Error`; max-turns; out-of-order events clamp.
- `branches.rs` — every `Phase` reached; dedup-by-message-id; context window per known model + fallback;
  context clamp at 100; each `HookEvent`; `Stale`/`Detached` preservation.
- `fixtures.rs` — fold each `fixtures/streams/*.ndjson` and assert final `SessionState` matches the
  expected snapshot (tokens, cost, context%, phase, subagents).

## Edit scope

Code-agent: write `crates/marsad-events/src/**` + `Cargo.toml`; read-only `tests/`, this spec.
