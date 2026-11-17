# Handoff — remaining Marsad work for a real (webkit2gtk-capable) machine

**Who this is for:** a follow-up **Opus** coding agent on a machine that CAN compile the Tauri desktop
app (Ubuntu 22.04+/macOS/Windows with webkit2gtk-4.1, or a container with it). Everything below was
built and mock/unit-tested on a machine that could NOT compile `apps/desktop/src-tauri` (EOL Ubuntu
21.04, no webkit2gtk). The Rust cores, the React UI against its mock backend, the Wasl side (all three
CLIs live-verified), and the live tmux+claude integration tests are all green and committed. What
remains is everything that needs the **real compiled desktop app** or is a deliberately-deferred
driver.

Repos: `/work/…/Wasl` and `/work/…/Marsad`, both on branch `claude/marsad-review-upgrade-1bahy2`.
Read each repo's `ARCHITECTURE.md`, `docs/GAPS.md` (Marsad), and the per-crate/`docs/specs/*.md`
files before touching code. `Marsad/docs/GAPS.md` has the authoritative built-vs-verified inventory.

## Non-negotiable consistency rules (hold these in EVERY task below)

1. **Asynchronous, never blocking.** The UI must never freeze on a synchronous backend action. No
   blocking IO while holding the Core/agent state lock; slow work spawns and reports via an event.
   There is a test contract for this (dynamic-capabilities §U5, the 2s-slow-probe test) — keep it.
2. **No preset client knowledge.** Slash commands, models, and resumable sessions are discovered LIVE
   from each CLI — never hardcoded lists in UI or bridge. A CI grep-gate enforces this
   (`apps/desktop/tests/DynCapKillList.test.tsx` + raw greps for `SLASH_CATALOG`, `bridge/models`,
   `claude-(opus|sonnet|haiku)-<digit>`, `.claude/projects` in src-tauri). Do not reintroduce any.
3. **Agent-reported cost is truth.** Never self-compute cost when the agent reports it; the
   litellm/hand table is backfill-only and must render labeled `~ estimated`.
4. **No TUI parsing.** Drive CLIs via structured JSON/NDJSON/stream-json only.
5. **Marsad starts and follows work; it never does the work.** It never authors content, never makes
   an agent decision, and never auto-answers a gate. Every gate answer (claude permission OR Wasl
   handoff) is a RELAYED human decision. The Wasl approval modal already enforces "no decision without
   a click" — keep that invariant.
6. **Wasl stays independent.** Do NOT add any Marsad-specific concept to the Wasl repo. Marsad talks to
   Wasl only through the generic operator API (`Wasl/docs/specs/operator-api.md`), whose v1 golden
   fixtures are vendored into `Marsad/fixtures/operator-v1/`. If you change the wire, re-vendor.
7. **Anti-cheat test discipline.** Test-first: write the failing test, gate it for cheats
   (`#[ignore]`, `todo!`, tautologies, `.skip`/`.only`, tests that stub the thing under test), THEN
   implement, THEN re-run tests + real coverage exit code + clippy + fmt + diff-scope yourself before
   committing. Live tests must FAIL LOUD, never pass-on-skip. Never trust a "done"/"100%" claim you
   didn't reproduce. Toolchains: `~/.cargo/bin` (Rust), Node 20 + pnpm (`cd apps/desktop`).

---

## Prompt 1 — Compile the real desktop app; fix Tauri wiring (highest priority; unblocks all else)

`apps/desktop/src-tauri` has NEVER compiled. It is the single biggest risk. Do this first, alone.

- `cd apps/desktop && pnpm install`, then `cargo build` in `src-tauri`, then `pnpm tauri dev`.
- Fix every real compile/wiring error. Likely culprits, all flagged built-not-verified in
  `docs/GAPS.md`: the `#[tauri::command]` macro layer, `rename_all="snake_case"` invoke-arg wiring, the
  U2 capabilities commands (`bridge::capabilities`, `bridge::refresh_capabilities`) and the
  `VmUpdateEnvelope::Capabilities` arm in `events.rs`, and the `list_sessions` deletion (make sure no
  dangling refs / unused imports remain in `generate_handler!`).
- **Known missing wiring you MUST add (I could not compile-check it):** the Wasl bridge is complete on
  the JS + Core side but NOT in `src-tauri`. Add: (a) a `#[tauri::command] answer_wasl_gate(host_id,
  gate_id, outcome, reason?)` in `bridge.rs` forwarding to `Core::answer_wasl_gate` (map `outcome`
  string → `marsad_gates::Decision`), registered in `generate_handler!`; (b) `VmUpdateEnvelope` arms
  for `VmUpdate::WaslGates` → `{"kind":"wasl_gates","gates":…}`, `VmUpdate::WaslGateDecided` →
  `{"kind":"wasl_gate_decided",…}`, `VmUpdate::WaslObservation` → `{"kind":"wasl_observation",…}` in
  `events.rs`. The JS side already expects exactly these `kind` strings (see
  `apps/desktop/src/bridge/useVm.ts` VM_UPDATE arms and `api.ts answerWaslGate`).
- Gate: `cargo build` clean, `pnpm tauri dev` launches, the mock and live backends both start. Verify
  the app opens, board→graph→session works, and the U-series UI (dynamic model/resume/slash pickers,
  activity indicators, cost `~` labels) renders against a REAL backend for the first time. Commit only
  after it genuinely runs.

## Prompt 2 — Playwright + live integration against the real dev server

- Wire `apps/desktop/playwright.config.ts` is already done (vite preview webServer); run `pnpm test:e2e`
  against the compiled app and against `tauri dev`. Confirm the golden path.
- Run the Marsad live integration suites that need the real app path (they pass headless on Rust today:
  `i03/i07/i09/i10`; `i11` needs Windows). Add a GUI-level live test if the real invoke path reveals
  gaps the mock path hid.

## Prompt 3 — X6 through the compiled desktop app (the headless loop is ALREADY GREEN)

**Update:** the headless X6 (`tests/integration/x6_wasl_live/`, real agent + Core + two real `wasl-mcpd`
nodes + real claude) already PASSES on the build machine (~42s, verified repeatedly): consult → Tier-3
gate surfaces in Marsad → `Core::answer_wasl_gate` Approve → real claude reply flows back → observed;
optionality proven. So the observe+approve LOGIC is done and live-verified. What remains for you is only
to re-run that same loop **through the compiled desktop app** — the real `WaslGateModal`, the activity
surface, and the `answer_wasl_gate` invoke path (the missing src-tauri command from Prompt 1). Original
scenario, now as a UI-level verification:

This is the one test that proves the whole optional-Wasl loop through the REAL app. Prereqs: a real
`claude`, and the `wasl-mcpd` binary built from the Wasl repo (`cargo build --release -p wasl-mcp`).
Reuse `Wasl/scripts/live-verify.sh` as the reference for standing up nodes.

Scenario: launch a **Wasl-enabled** workstream from Marsad (the `LaunchSpec.wasl` toggle → the agent
injects `--mcp-config` and adopt-or-spawns the host `wasl-mcpd` node — X4, built here, unrun live).
Session A consults session B. The Tier-3 gate surfaces in Marsad's **handoff-approval modal** (X5b) via
the operator SSE feed that `marsad-wasl` (X1, 100/100 vs vendored fixtures) consumes. A human clicks
Approve. The decision is relayed (`answer_wasl_gate` → operator POST). The reply flows back and the
consult appears on Marsad's **Wasl activity surface** (X5c). Assert each hop. Watch for: the operator
`gate_requested`/`gate_decided`/`audit` frame shapes matching the vendored fixtures live (fixtures are
contract-tested but never checked against the real running operator end-to-end); reconnect/resnapshot
behavior; and that removing the Wasl config leaves every Marsad test green (optionality — a test
already asserts inertness; keep it true).

Also do the real cross-host / real `scp -3` leg of **Wasl W6** here if you have two hosts (parked on
the build machine: single host, loopback stand-in only).

## Prompt 4 — Marsad Phase-2 drivers: codex (optional; research is done)

Marsad still only LAUNCHES claude sessions. The *Wasl* side already drives its CLIs (agent-to-
agent); this is the different axis of Marsad supervising a codex session. Research is committed and
verified: `Marsad/docs/specs/codex-research.md` (codex `exec`/`app-server`, JSON-RPC schema extracted
live). U1 already discovers codex models +
sessions. Build the launch/supervise driver chain test-first against captured real-CLI transcripts;
map approvals onto the existing gate pipeline. **Do not offer a driver in the UI until its full chain
is green against the real CLI** (the CLI contracts are treacherous; verify live, cheaply, smallest models).

## Prompt 5 — Mobile (optional, lowest priority)

`apps/ios` and `apps/android` are scaffolding ("UI not yet implemented" placeholders). They sit on the
shared Rust core via UniFFI — which is currently vaporware (no `.udl`, no `uniffi` dep; `Core`'s public
API uses types UniFFI can't export as-is, e.g. `Box<dyn Transport>`). Building mobile means first
adding a real UniFFI wrapper layer over `marsad-core`. Large; only if the owner wants phones this cycle.

---

## Cheat-sheet: what is DONE and verified (don't rebuild it)

- **Wasl**: standalone MCP→A2A messaging, three-tier human gating, config-driven peers, generic
  operator API (v1 frozen), **CLI adapters — claude/codex/opencode — each live-verified**, W5
  two-node dress rehearsal green. Legacy transport deleted. Docs truthful.
- **Marsad Rust + mock UI**: real-fixture-verified reducer, typed IPC (single wire schema), dynamic
  capabilities (no presets, CI-gated), honest cost (no clobber, labeled estimates), full activity
  surfaces (subagents/questions/gates/plan/compaction/rate-limit), live tmux+claude integration green
  (incl. the FIFO-under-tmux production-bug fix), the `marsad-wasl` connector + agent/core/gates wiring
  + launch toggle, and the Wasl approval modal + activity surface (mock-tested). ~380 Vitest, Rust
  workspace green, coverage gates met.
- **Only genuinely unrun here:** the compiled desktop app (Prompt 1), its live e2e (Prompts 2–3), the
  codex Marsad driver (Prompt 4), and mobile (Prompt 5).

---

## Wasl known-issues / hardening backlog (from the final review, non-blocking)

These were found by an adversarial final review. The merge-blocking ones (Tier-2 commit could lose an
approved decision on audit-write failure; operator `gate_decided`-before-`gate_requested` race;
guessable sequential Tier-2 tokens) were fixed test-first this cycle. The following are real but
non-blocking — address on the real machine or before a public release:

- **No process-level timeout in any adapter's `ProcessCliInvoker::invoke`** (claude/codex/opencode).
  A wedged CLI (e.g. one hitting an OAuth prompt on an unauthenticated box) blocks `deliver()`
  forever. Add a `tokio::time::timeout`/deadline around `wait_with_output`.
- **Unbounded state growth, no TTL/pruning:** `wasl-gate` `GateLedger.resolved`, `wasl-mcp`
  `pending_calls`/`decisions`, `wasl-operator` `resolved`. A client that prepares-and-never-commits
  leaks memory on a long-lived node. Add expiry/caps.
- **Blind Continuation→First retry** in the codex adapter can double-deliver content if a
  turn fails *after* the model consumed the input. The specs assume failure is always pre-token-spend;
  undefended. Consider idempotency/dedup or narrowing the retry trigger.
- **Low:** default `gate.authority = "elicitation"` silently no-ops an attached `[operator]` for
  non-elicitation clients (document the trap); `OperatorHub::open_gate` without a matching `wait`
  leaks the gate past its deadline (latent API gap, not hit today); `scripts/live-verify.sh` uses
  python `eval()` on loopback JSON (not exploitable; add a justifying comment).

The generic operator API has **no auth in v1** by design (documented) — it binds loopback-only by
default and cannot be silently widened (no env override, no `0.0.0.0`). Adding auth is a v1.x item if
the operator surface is ever exposed beyond loopback.
