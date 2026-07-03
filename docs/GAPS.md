# Marsad — honest gap inventory (as of this handoff)

This records what is genuinely built + verified vs. what is scaffolding, unwired, or unverified. It exists
because prior status narration over-claimed "done"; treat THIS file as the source of truth for readiness.

Legend: ✅ real + verified here · ⚠️ built but UNVERIFIED here (needs the real runtime) · ❌ not built.

## Status update — 2026-07 cycle (supersedes the older sections below where they conflict)

Landed + verified on a machine with a REAL `claude` (2.1.198) + tmux since the original inventory:
- ✅ **Live integration tests RUN and PASS** on real tmux+claude: `i03_lifecycle_real`, `i07_core_facade`,
  `i09_gui_agent_live`, `i10_core_e2e_live` are green. This flushed a **production bug**: `claude` exits
  silently when stdin is a named FIFO inside a tmux pane — the spawn path now uses `cat fifo | claude`
  (`crates/marsad-agent` + `marsad-lifecycle`). (`i11_windows_lifecycle` still needs Windows.)
- ✅ **Playwright e2e wired + green** (`vite preview` webServer, CI builds before e2e). 3/3 headless.
- ✅ **Real recorded-real fixtures** captured; the reducer handles real stream shapes (thinking blocks,
  non-init system subtypes, post-compact string content, subagent tool named `Agent`).
- ✅ **Typed IPC**: `IpcMsg` is the single authoritative GUI↔agent wire schema (no hand-rolled JSON).
- ✅ **Dynamic capabilities** (no preset client lists): models/sessions/slash discovered live per driver;
  a CI grep-gate bans presets from returning. UI pickers, resume, slash menu are capability-driven.
- ✅ **Honest cost**: agent-reported cost is authoritative and no longer clobbered by estimates;
  estimates are backfill-only and UI-labeled `~`.
- ✅ **Activity surfaces**: subagents, questions, gate kinds, notices, compaction + rate-limit indicators.
- ✅ **Marsad↔Wasl connector BUILT** (was "not built"): `marsad-wasl` crate (operator-API client, 100/100,
  contract-tested vs vendored golden fixtures), wired into agent/core/gates (`VmUpdate::Wasl*`,
  `answer_wasl_gate`, `Gate::Handoff`), plus the launch-time Wasl toggle (`LaunchSpec.wasl` →
  `--mcp-config`). **Runtime-optional and proven inert when absent.** Remaining: the X5 Wasl UI
  (graph edges + real approval modal) and the X6 live e2e.

Still parked (unchanged): **`apps/desktop/src-tauri` has NEVER compiled here** (no webkit2gtk on this OS) —
every UI feature above is wired to real Tauri commands in code but the real app path is unrun; new
built-not-verified src-tauri surface this cycle: the U2 capabilities commands + `VmUpdate::Capabilities`
envelope arm + the `list_sessions` deletion. Mobile apps remain scaffolding.

## What is real and verified (in this sandbox)
- ✅ Rust library crates for the Workstreams upgrade: `marsad-workstream` (domain), `marsad-store`
  (persistence prefixes/CRUD/membership), `marsad-core` (B12 workstream layer: create/delete workstream,
  add/remove connection, create/delete profile, assign_session, set_active_profile, board(), graph()),
  `marsad-viewmodel` (board_view/graph_view). Unit-tested, 100% line+region where gated. These compute
  correctly given inputs.
- ✅ Desktop **frontend against the MOCK backend**: Board (L1), GraphScreen (L2), graph layout, `useVm`
  state + command helpers, `api.ts` wrappers, app-wiring (connected screens + router + nav). 301 Vitest
  tests pass; `tsc --noEmit` clean. The full board→graph→session flow works **in the test/mock harness**.

## Built but UNVERIFIED here (never compiled/run against the real runtime)
- ⚠️ **src-tauri workstream command layer** (`apps/desktop/src-tauri`): the 10 Tauri commands the UI invokes
  (`board`, `graph`, create/delete_workstream, add/remove_connection, create/delete_profile,
  assign_session, set_active_profile) + the `VmUpdate` Board/Workstreams envelope arms. **src-tauri was
  NEVER COMPILED** in the sandbox (no GTK/webkit). Core-facing signatures were validated only via a
  standalone harness crate — the Tauri macro layer, `invoke` arg wiring, and the real
  frontend→invoke→core→vm-update path have **never run**. There may be Tauri-level compile/wiring errors.
  → "UI 100% hooked to functionality" is TRUE for the mock path, UNVERIFIED for the real desktop app.
- ⚠️ **src-tauri capabilities command layer** (U2, `docs/specs/dynamic-capabilities.md` §3.6, added on top of
  the already-landed + Rust-tested U1 backend in `marsad-proto`/`marsad-agent`/`marsad-core`): new Tauri
  commands `bridge::capabilities` (lock-clone-return `Core::capabilities()`, mirrors `board()`) and
  `bridge::refresh_capabilities(host_id?, driver?, project?)` (forwards to `Core::refresh_capabilities`,
  fire-and-forget) registered in `lib.rs`'s `generate_handler!`; the `VmUpdateEnvelope::Capabilities`
  arm in `events.rs` (`VmUpdate::Capabilities(CapabilitiesView) → {"kind":"capabilities","capabilities":…}`).
  Hand-written against `Core`'s already-compiling, already-tested signatures
  (`crates/marsad-core/src/lib.rs:1718,1729`) but, like the rest of src-tauri, **never compiled here** —
  the `#[tauri::command]` macro expansion, `rename_all = "snake_case"` arg wiring (`host_id`/`driver`/
  `project` as `Option<String>` invoke args), and the real emit→`vm-update`→`useVm` path are unverified.
  Kill-list companion change in the same file: `bridge::list_sessions` (Tauri command) and its private
  `parse_remote_session_list` helper are DELETED — `apps/desktop/tests/DynCapKillList.test.tsx` greps
  `bridge.rs` source text for their absence (a compile-independent check) and passes, but whether the
  desktop app still builds with `list_sessions` gone from `generate_handler!` (no dangling references,
  no now-unused imports) is unverified beyond `grep`/manual read. The mock-mode session-seed literals in
  `mock.rs` were renamed off the `claude-(opus|sonnet|haiku)-<digit>` pattern (kill-list grep gate #2) —
  cosmetic only, no behavior change, also unverified by compilation.
- ⚠️ The Marsad "live/real" integration tests: `tests/integration/i03_lifecycle_real`,
  `i09_gui_agent_live`, `i10_core_e2e_live`, `i11_windows_lifecycle`. They spawn real tmux/claude sessions
  and cannot run headless — not executed here. (One sandbox-only spurious failure in `marsad-core`'s
  `discovery.rs` unreadable-file test was fixed to be root-robust; unrelated to the upgrade.)
- ⚠️ Playwright e2e (`apps/desktop/tests/e2e/board-flow.spec.ts`): written; `playwright.config.ts`'s
  webServer is a TODO, so it was not run against a live dev server.

## Not built (as of this cycle's end)
- ❌ **Codex / Antigravity Marsad DRIVERS (Phase 2).** Marsad still only LAUNCHES claude sessions. (Note:
  the *Wasl* side has live-verified claude/codex/antigravity adapters — that is agent-to-agent messaging,
  a different axis from Marsad supervising a codex/agy session.) Research is done: `docs/specs/codex-research.md`
  (codex `exec`/`app-server`, verified live) and `docs/specs/antigravity-research.md` (`agy`; the old
  `gemini --acp` plan in `phase2-drivers.md` is VOID — Google killed the legacy gemini CLI 2026-06-18,
  replaced by Antigravity `agy`). U1 already discovers codex/agy models + sessions; the launch/supervise
  driver chain is the remaining work. Do not offer a driver in the UI until its full chain is green vs the
  real CLI.
- ✅/⚠️ **Marsad↔Wasl**: the connector, agent/core/gates wiring, launch toggle, AND the X5 Wasl UI are built.
  The **X5 approval modal** (`WaslGateModal`, replacing the interim `onOpenGate` placeholder), the bridge/
  state layer, and the Overview **activity surface** are done and mock-tested (`X5aWaslBridge`/`X5bWaslModal`/
  `X5cWaslActivity` suites green). Graph typed-edges specifically were scoped out (observations render on
  the activity surface instead). NOT yet done: the **X6 live end-to-end** (launch a Wasl-enabled workstream
  → A consults B → gate surfaces in Marsad → human approves → observed) — needs the real runtime.
  ✅ Two correctness gaps found in the final review are now FIXED (regression-tested; see
  `crates/marsad-agent/tests/wasl_launch_wiring.rs`, `crates/marsad-agent/tests/wasl_lock_safety.rs`,
  `crates/marsad-wasl/tests/deadline.rs`): (a) `Agent::launch_with_id` now calls
  `adopt_or_spawn_wasl_mcpd` for every LOCAL `wasl:true` launch (previously the helper existed +
  was unit-tested but had zero production call sites, so the host node never actually started); a
  `wasl:true` REMOTE launch — for which the injected `--mcp-config` placeholder can't be resolved
  yet (`materialize_wasl_mcp_config` is local-only) — now fails fast with a clear
  `AgentError::Protocol` instead of silently handing claude a broken, unresolved
  `--mcp-config wasl-mcp.json` argument; wasl+remote support is a follow-up, not shipped here. (b)
  `handle_answer_wasl_gate` no longer holds the `wasl_connectors` registry lock across the blocking
  `decide()` POST (it clones an `Arc<dyn WaslConnector>` handle and drops the lock first), AND
  `marsad-wasl`'s `post_decision`/`read_head` read loop now has an overall retry-cap deadline (not
  just `connect_timeout`), so a wedged/dead operator returns `WaslError::Io` within a bounded window
  instead of hanging the read loop (and therefore the agent's single command-processing thread)
  forever.

## First things to do on a machine with the real runtime
1. Actually build `apps/desktop/src-tauri` (`cargo build`, then `cargo tauri dev`); fix any real
   compile/wiring errors in the workstream commands / events envelope. This is the highest-value unblock:
   it's the first time the real app path is exercised.
2. Run the live integration tests (i03/i09/i10/i11) and the Playwright e2e against a running dev server.
3. Then Phase-2 drivers per `docs/specs/phase2-drivers.md`, test-first against captured real transcripts.

Apply the same methodology used elsewhere: spec → isolated test-agent (tests only, black-box) → gate the
tests for cheats → code-agent (src only) → INDEPENDENTLY re-run tests/coverage/clippy/tsc + check the diff
scope before committing. If something can't be verified in your environment, say UNTESTED there — do not
mark it done.
