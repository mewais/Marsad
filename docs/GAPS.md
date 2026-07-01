# Marsad — honest gap inventory (as of this handoff)

This records what is genuinely built + verified vs. what is scaffolding, unwired, or unverified. It exists
because prior status narration over-claimed "done"; treat THIS file as the source of truth for readiness.

Legend: ✅ real + verified here · ⚠️ built but UNVERIFIED here (needs the real runtime) · ❌ not built.

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
- ⚠️ The Marsad "live/real" integration tests: `tests/integration/i03_lifecycle_real`,
  `i09_gui_agent_live`, `i10_core_e2e_live`, `i11_windows_lifecycle`. They spawn real tmux/claude sessions
  and cannot run headless — not executed here. (One sandbox-only spurious failure in `marsad-core`'s
  `discovery.rs` unreadable-file test was fixed to be root-robust; unrelated to the upgrade.)
- ⚠️ Playwright e2e (`apps/desktop/tests/e2e/board-flow.spec.ts`): written; `playwright.config.ts`'s
  webServer is a TODO, so it was not run against a live dev server.

## Not built
- ❌ **Codex / Gemini drivers (Phase 2).** Only claude is wired. There is a research-grounded plan in
  `docs/specs/phase2-drivers.md` (P2a–P2g: `LaunchSpec.driver`, per-driver protocol clients for
  `codex app-server` (JSON-RPC/stdio) and `gemini --acp` (ACP/stdio), events-normalizers tested against
  CAPTURED real-CLI transcripts, launcher wiring). NOT implemented. Do not offer a driver in the UI until
  its full chain is green against the real CLI.
- ❌ Any Marsad↔Wasl live integration (the Tier-3 connector on Marsad's side is Wasl's `wasl-marsad`
  crate's counterpart; the Marsad-side endpoint that receives Wasl gates + observation feed is not built).

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
