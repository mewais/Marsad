# Marsad — Test Plan & Coverage Regime

## Definition of "100% coverage"

- Rust crates: **`cargo llvm-cov --fail-under-lines 100 --fail-under-regions 100`** per crate, all
  assertions green. Tests live in the crate's `tests/` directory (separate compilation unit) and
  exercise the **public API only** (black-box). Crates are designed API-first so every private branch
  is reachable from public calls; an unreachable branch must be deleted, not left uncovered.
- UI packages (D02/M01/M02): not line-100%, but **100% of the interaction matrix** (see below).

## Tests-first invariant

After the test stage, `cargo test -p <crate>` fails for every block (the impl is absent), yet the
suite already describes 100% of the documented public surface from `docs/specs/<block>.md`. Code-
agents then create `src/` until green.

## Per-block test buckets (each block's `tests/`)

- `happy.rs` — nominal paths for every public fn/variant.
- `errors.rs` — every enumerated error/edge.
- `branches.rs` — table-driven cases closing remaining regions to 100%.
- `fixtures.rs` (B01/B02/B06) — recorded-real data tests.
- `fakes.rs` (L1 crates) — exercise the trait via the in-crate fake (Backend/SshRunner/Transport/
  Container/Notifier).

## Real-behavior coverage matrix (authoritative)

"Recorded-real" = captured once from a live `claude` run into `fixtures/`, deterministic, runs
anywhere. "Live" = requires the real `claude` binary (+ tmux / SSH) at test time; CI-gated behind a
`claude`-present job and part of the integration coverage gate.

| Functional surface | Real signal that proves it | Tier / location | Kind |
|---|---|---|---|
| Parse every stream-json msg type | `system/init`, `assistant`, `user`, `result`, control `can_use_tool` parse losslessly | B01 `fixtures.rs` | Recorded-real |
| Read JSON logs / derive state | folding a real stream → correct `working/needs/error/done`, tokens, cost, context%, subagents | B02 `fixtures.rs` | Recorded-real |
| Hooks fire & are honored | real hook payloads (Notification, Stop, SubagentStop, Pre/PostToolUse, SessionStart/End, PreCompact) reduce correctly | B02+B10 `fixtures.rs` / `i05` | Recorded-real |
| Launch a new session | spawn real `claude` in tmux → observe `system/init` (session_id, model, cwd, tools) | B13 / `i03` | Live |
| Resume an existing session | kill container, `claude --resume <session_id>` → continuity | B13 / `i03` | Live |
| Attach/adopt a running session | pre-existing detached `marsad-<id>` tmux → re-adopt, no new process, no duplication | B09+B13 / `i03` | Live |
| Stale vs Detached classification | container dead→Stale+resume; alive-unattached→Detached+adopt | B09 / `i04` | Live + fake |
| Permission gate round-trip | real permission request → approve → run continues; reject+reason → run adjusts | B03+B13 / `i02` | Live |
| Plan gate round-trip | real plan-mode request → approve/reject routes back, session proceeds | B03+B13 / `i02` | Live |
| Edit gate round-trip | real edit-approval → approve applies / reject blocks, with before/after diff | B03+B04+B13 / `i02` | Live |
| Gate routing & idempotency | decision lands on exactly one session; re-delivery is a no-op | B03 / `i02` | Live + recorded |
| Send a prompt | real prompt over stream-json `user` input → assistant responds | B12+B13 / `i07` | Live |
| Interrupt (⎋) | interrupt control msg halts an in-flight real turn | B12+B13 / `i07` | Live |
| Slash commands | `/clear /compact /cost /model /config /init /review /agents /resume /terminal /help` dispatch correctly | B11 parse + B12/B13 effect / `i07` | Recorded + live |
| Changes / diff fidelity | real changed files → side-by-side (desktop) & unified (mobile) rows match git diff | B04+B06 / `i01` | Recorded-real |
| Stage & commit | stage subset → real `git commit` in the session repo (local & over SSH) | B13 / `i03` | Live |
| Usage/cost/context accuracy | values from real `result`+assistant usage (dedup by msg id) match `claude` totals | B02 / `i01` | Recorded-real |
| Compaction warning ≥75% | context% crossing 75 raises the warning flag | B02+B11 | Recorded-real |
| Notifications content/priority/DND/grouping | real events → correct title/body/priority; DND mutes; bursts group | B10 / `i05` | Recorded-real + fake sink |
| Host registry & SSH config | real `~/.ssh/config` parsed; localhost protected; add/remove | B07 / `i06` | Real file + fake runner |
| Remote session over SSH | launch+drive real `claude` on a remote (or localhost-as-SSH) host via stdio | B08+B13 / `i06` | Live (localhost smoke) |
| Transport resilience | SSH drop / broken pipe / partial NDJSON frame → reconnect without losing the session | B08+B09 / `i03` | Live + fake |
| GUI-independent liveness | kill the client; `claude` keeps running; relaunch re-adopts | B13 / `i03` | Live |
| Persistence & restore | continuous snapshot → simulated crash → restore reattaches & replays | B06+B09 / `i04` | Recorded + fake |
| Workspace export/import | `.marsad.json` round-trips with full fidelity; re-scopes dashboard | B06+B11 / `i08` | Pure |
| Terminal escape hatch | raw command typed → routed to live session over tmux; output streams back | B13 / `i03` | Live |
| Concurrency | many sessions across hosts advance independently; no cross-routing of gates/prompts | B12+B13 / `i07` | Live (N≥3) |

## Fixture capture

`scripts/capture_fixtures.sh` drives a single real `claude` run through: plain Q&A, a permission
gate, a plan gate, an edit gate, a subagent spawn, and a compaction — recording stream-json into
`fixtures/streams/*.ndjson` and hook payloads into `fixtures/hooks/*.json`. This one capture feeds
every "Recorded-real" row deterministically. Live rows additionally re-run against the real binary in
the `claude`-gated CI job.

## UI interaction matrix (D02/M01/M02)

Filters re-scope the dashboard; gate approve/reject routes to exactly one session; slash popover runs
the top match on Enter; diff renders (side-by-side desktop / unified mobile); staging + commit;
theme + glass toggle; workspace switch re-scopes counts/lists/cost; new-session launch; terminal
keypad sticky CTRL/ALT modifiers (mobile); notification/push tap routes to the right session.

- D02: Vitest + React Testing Library (components/interactions) + Playwright (golden-path e2e against
  a mocked core bridge).
- M01: XCTest / ViewInspector (SwiftUI).
- M02: Compose UI tests / Robolectric.
