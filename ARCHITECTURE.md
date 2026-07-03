# Marsad — Architecture

This is the authoritative, condensed architecture. Each block has a full long-form spec under
[`docs/specs/`](./docs/specs/) and a working contract in its own `AGENTS.md`. The design source of
truth for all UI and view-model behavior is the handoff in `_extracted/design_handoff_marsad/`
(`Marsad.dc.html`, `Marsad Mobile.dc.html`); the `renderVals()` logic in those files is the
reference implementation to port — do not invent new behavior.

## Locked decisions

- **Desktop = Tauri** (Rust core + React/TypeScript UI). **Phones = fully native** (iOS SwiftUI,
  Android Compose) — not a cross-platform mobile framework.
- **Maximize the shared Rust core.** Protocol, state, gates, diff, persistence, hosts, transport,
  lifecycle, notifications, and view-models all live in Rust and are bound to phones via **UniFFI**.
  Native code is UI + thin platform glue only.
- **Never parse rendered terminal output.** Drive `claude` as a persistent process
  (`claude --print --input-format stream-json --output-format stream-json --verbose`, NDJSON over
  stdio) plus hooks and the permission control protocol. State/usage/gates come only from these
  structured surfaces.
- **Session liveness is independent of the GUI.** A detached supervisor (`marsad-agent`) owns every
  `claude` process; the GUI is a *client* of it. Liveness container: **tmux** `marsad-<id>` on
  Linux/macOS, **ConPTY** on Windows. On relaunch per session: re-adopt live container → else
  `claude --resume <session_id>` in a fresh container → else mark **Stale** and offer
  resume/restart. Session states: `starting · working · needs · error · done · stale · detached`.
- **Local-first, no cloud.** Workspaces snapshot continuously; export/import as `.marsad.json`.

## Block map (blocks = crates/packages)

Tiers: **L0** pure data/logic; **L1** logic behind mockable traits; **L2** integration-bound (real
`claude`/tmux/SSH); **UI** component/interaction tested.

### Rust core crates — `crates/<name>/{src,tests}`

| # | Crate | Tier | Purpose |
|---|---|---|---|
| B01 | `marsad-proto` | L0 | Wire types + serde for Claude stream-json and the internal GUI⇄agent IPC protocol. |
| B02 | `marsad-events` | L0 | Pure reducer: fold parsed msgs + hooks → `SessionState`, usage/cost/context%, subagents, gate. |
| B03 | `marsad-gates` | L0 | Gate domain: permission/plan/edit, idempotent decision IDs, single-session routing. |
| B04 | `marsad-diff` | L0 | Diff model: side-by-side + unified rows + cell styling. |
| B05 | `marsad-format` | L0 | Formatters + meta (`fmt_tokens`, `fmt_elapsed`, `state_meta`, `ctx_color`, `gate_meta`). |
| B06 | `marsad-store` | L1 | Persistence: sessions/transcripts/workspaces, snapshot/restore, `.marsad.json`. `Backend` trait. |
| B07 | `marsad-hosts` | L1 | Host registry; `~/.ssh/config` parse; health. `SshRunner` trait. |
| B08 | `marsad-transport` | L1 | Line transport to `claude`: `LocalIpc`, `SshStdio`, loopback fake. NDJSON framing. |
| B09 | `marsad-lifecycle` | L1 | Reconnect-or-resume algorithm + `Container` trait (tmux/ConPTY/fake). |
| B10 | `marsad-notify` | L1 | Notification content/priority/prefs/DND/grouping. `Notifier` trait. |
| B11 | `marsad-viewmodel` | L0 | Dashboard aggregation/grouping/filter/search/slash — port of `renderVals()`. |
| B12 | `marsad-core` | L1 | Facade tying B01–B11 into the desktop/phone API; UniFFI surface; client of the agent. |
| B13 | `marsad-agent` | L2 | Detached supervisor daemon: owns `claude` processes, serves IPC, fires hooks. |
| X1 | `marsad-wasl` | L1 | Wasl operator-API v1 connector: HTTP+SSE client, reconnect/gap/resnapshot, `WaslConnector` trait + fake. No dep on any other marsad crate. |

### App packages

| # | Package | Tier | Stack | Purpose |
|---|---|---|---|---|
| D01 | `apps/desktop/src-tauri` | L1 | Rust/Tauri | Tauri commands + event bridge over `marsad-core`. |
| D02 | `apps/desktop/src` | UI | React/TS | Desktop UI faithful to `Marsad.dc.html`. |
| M01 | `apps/ios` | UI | SwiftUI | iOS UI over the UniFFI core; SSH-only. |
| M02 | `apps/android` | UI | Compose | Android UI over the UniFFI core; SSH-only. |

**Dependency DAG (acyclic):** `proto ← events,gates,transport`; `format,diff,gates ← viewmodel`;
`proto,events,transport,lifecycle,store,hosts,notify ← agent`; `everything ← core`;
`core ← D01/M01/M02`; `wasl ← agent,core` (X1 `marsad-wasl` sits beside `marsad-transport`: no dep
on any other marsad crate; consumed by both `agent` — the runtime-optional connector, §agent.md/
`docs/specs/wasl-connector.md` §4 — and `core` — inbound arms + `VmUpdate`s, §5.2 — with **no**
`agent → core` edge introduced). Shared session-launch types (`LaunchSpec`, `Posture`) live in
`marsad-proto` (protocol data) and are re-exported by `marsad-core`; `marsad-agent` consumes them
from `marsad-proto` directly — so there is **no `agent → core` edge** (core is the client of agent,
never the reverse).

## Test-first isolation regime

The codebase decomposes into blocks so that implementation can be delegated to **parallel, isolated
agents** that cannot cheat:

- Each block has a **code directory** (`src/`) and a **physically separate test directory**
  (`tests/`). Cargo compiles `tests/` as a separate crate against the block's **public API only** —
  this black-box boundary is the anti-cheat mechanism.
- **Tests are written before code.** Test-agents write the suite from the block's spec; afterward
  every block's tests fail (the impl is absent) yet already describe 100% of the public surface.
- **Code-agents** then implement `src/` until the suite is 100% green. They have **read-only** access
  to their own `tests/` and spec, and no access to other blocks. They cannot pass by editing tests,
  and cannot half-ass because the tests they must satisfy are fixed.
- **Coverage gate:** `cargo llvm-cov --fail-under-lines 100 --fail-under-regions 100` per crate.
  Unreachable branches must be deleted, not left uncovered.

**Real-behavior coverage:** every surface whose correctness depends on real Claude Code / tmux / SSH
behavior is proven against the real thing — either **recorded-real** (captured once into `fixtures/`,
deterministic, runs anywhere) or **live** (real `claude` binary at test time, CI-gated). The full
matrix lives in `docs/specs/TESTPLAN.md`. This guarantees the app is verified block-by-block and is
never "finished, then debugged."

## Integration scenarios — `tests/integration/i0*`

`i01` stream→view-model · `i02` gate round-trip · `i03` real claude+tmux lifecycle (launch/resume/
adopt/no-dup/liveness) · `i04` persistence & restore · `i05` notifications · `i06` hosts/SSH ·
`i07` core facade & concurrency · `i08` workspace export/import.

## Directory access contract (for dispatching agents)

- **test-agent(block):** write `crates/<b>/tests/`; read `docs/specs/<b>.md`; nothing else writable.
- **code-agent(block):** write `crates/<b>/src/` + `crates/<b>/Cargo.toml`; read-only
  `crates/<b>/tests/` and `docs/specs/<b>.md`; no access to other blocks.
- **integration-agent(scenario):** write one `tests/integration/i0x/`; read-only everything else.

Every working directory carries an `AGENTS.md` stating its purpose, the relevant design slice, the
exact public API, the enumerated test cases + coverage target, the I/O contract, and the
edit-scope restriction.
