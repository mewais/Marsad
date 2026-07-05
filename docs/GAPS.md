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
- ✅/⚠️ **WAVE 3b — interactive PTY escape-hatch terminal** (Session Level 3 "Terminal" tab): a SEPARATE
  channel from the claude driver — a real interactive shell (local `portable-pty`, or the session's
  existing SSH/WSL host with real pty allocation requested), not `--print`/stream-json. `marsad-proto`
  (5 new additive `IpcMsg` variants: `PtyStart`/`PtyInput`/`PtyResize`/`PtyOutput`/`PtyExit`) and
  `marsad-agent` (the `pty` module + IPC wiring + `StopSession` teardown) are REAL + VERIFIED here —
  see below. `apps/desktop/src-tauri` (3 new commands + a `"pty"` event channel) and the frontend
  (`Terminal.tsx` rebuilt on `xterm.js`) are BUILT but UNVERIFIED (see the dedicated entry below).
- ✅ **Marsad↔Wasl FULLY BUILT and LIVE-VERIFIED** (was "not built"): `marsad-wasl` operator-API client
  (100/100, contract-tested vs vendored golden fixtures), wired into agent/core/gates, the launch-time
  toggle (`LaunchSpec.wasl` → `--mcp-config`, now actually calling `adopt_or_spawn_wasl_mcpd`; wasl+remote
  fails fast as unsupported), the **X5 Wasl UI** (approval modal + bridge + Overview activity surface,
  mock-tested), AND the **X6 live end-to-end** (`marsad-it-x6_wasl_live`): real agent + Core + two real
  wasl-mcpd nodes + real claude — consult → gate surfaces in Marsad → human approves → real reply flows
  back → observed, **passing live on this machine (~42s)**. Optionality proven (no wasl.json → zero Wasl
  activity). The only Wasl piece NOT done: full live verification *through the compiled desktop app*
  (needs webkit2gtk — see HANDOFF-OPUS.md Prompt 3; the headless loop above already proves the logic).

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
- ✅ **WAVE 3b — pty escape-hatch, `marsad-proto` + `marsad-agent`** (fully REAL + VERIFIED, no mocks/fakes
  standing in for the OS): `crates/marsad-proto/src/lib.rs` gained 5 additive `IpcMsg` variants
  (`PtyStart{session_id,cols,rows}`, `PtyInput{session_id,data}` — plain UTF-8 keystrokes,
  `PtyResize{session_id,cols,rows}`, `PtyOutput{session_id,data}` — base64 raw bytes (arbitrary/partial-UTF-8
  safe), `PtyExit{session_id,code}`); `parse_ipc`'s closed `KNOWN` list grows 35→40. Golden-JSON contract
  tests in the new `crates/marsad-proto/tests/pty_ipc.rs` (19 tests) — `cargo test -p marsad-proto` green,
  100% line+region coverage, all pre-existing frozen suites (`ipc_typed.rs`/`capabilities_ipc.rs`/
  `wasl_ipc.rs`) untouched and still green.
  `crates/marsad-agent/src/pty.rs` (new module) spawns a REAL OS pty via the `portable-pty` crate (added
  dependency) — local: the session's `$SHELL` (or `/bin/sh`/`cmd.exe` fallback); remote: the SAME
  `remote::exec_prefix_for` argv the agent already uses for that host, with the one-shot script
  transport's no-pty flags (`-T`) swapped for real-pty-allocation ones (`-tt` for ssh, `-t` for plink) —
  wired into `handle_ipc` (`PtyStart`/`PtyInput`/`PtyResize`), `handle_stop_session` (pty teardown on
  session stop), and `serve.rs` (the broadcast-pump thread, mirroring `spawn_session_reader`'s pattern).
  UNIT-TESTED test-first in this sandbox (no external dependency beyond a pty device + a shell — unlike
  `TmuxContainer`, which needs a live `tmux`+`claude` and is only exercised by the gated `i03` live test):
  `crates/marsad-agent/tests/pty.rs` (11 tests, pure argv/shell-selection logic, always compiled — real
  coverage, not gated) and `crates/marsad-agent/tests/pty_live.rs` (8 tests — spawn a real local shell,
  write a command, assert the echoed output arrives, resize, exit with both a clean and a non-zero
  status). `crates/marsad-agent/tests/pty_ipc_dispatch.rs` (6 tests) exercises the `handle_ipc`
  dispatch/coverage-mode no-op paths through `Agent::with_fakes`. `cargo test -p marsad-agent` green (243
  tests total incl. the above); `cargo llvm-cov -p marsad-agent --fail-under-lines 100
  --fail-under-regions 100` passes — the real pty spawn/resize/`PtyError` surface is
  `#[cfg(not(coverage))]`-gated (same disposition as `TmuxContainer`/`ClaudeFifoTransport`) so it doesn't
  count toward the coverage number, but IS exercised for real by `pty_live.rs` under plain `cargo test`
  (that file is `#![cfg(not(coverage))]` at the top so it compiles to zero tests, not a build error,
  under `cargo llvm-cov`). `cargo clippy --workspace -- -D warnings` and `cargo fmt --check` both clean.
  Known, deliberate simplifications (not bugs): (a) the REMOTE pty path (ssh/wsl with real pty allocation)
  is structurally in place but has no live remote host to verify against here — genuinely
  BUILT-NOT-VERIFIED, unlike the local path; (b) a remote pty starts in the host's login/default
  directory, not the session's `project` dir (the one-shot transport's `cd` step doesn't compose cleanly
  with the pty-allocation argv — see `pty::spawn_pty_session`'s doc comment); (c) re-opening the Terminal
  tab for a session whose pty is already live is a no-op re-attach with NO scrollback replay (unlike
  claude's `TranscriptBackfill`) — the frontend's xterm starts empty; (d) there is deliberately no
  `PtyStop` wire message / Tauri command — the agent tears the pty down on `StopSession` only, mirroring
  tmux detach/reattach (leaving the Terminal tab does not kill the shell).

## Built but UNVERIFIED here (never compiled/run against the real runtime)
- ⚠️ **src-tauri workstream command layer** (`apps/desktop/src-tauri`): the 10 Tauri commands the UI invokes
  (`board`, `graph`, create/delete_workstream, add/remove_connection, create/delete_profile,
  assign_session, set_active_profile) + the `VmUpdate` Board/Workstreams envelope arms. **src-tauri was
  NEVER COMPILED** in the sandbox (no GTK/webkit). Core-facing signatures were validated only via a
  standalone harness crate — the Tauri macro layer, `invoke` arg wiring, and the real
  frontend→invoke→core→vm-update path have **never run**. There may be Tauri-level compile/wiring errors.
  → "UI 100% hooked to functionality" is TRUE for the mock path, UNVERIFIED for the real desktop app.
- ⚠️ **src-tauri `cost_breakdown` command** (Workstreams redesign Wave 3a — Usage tab's "Cost breakdown" By
  host / By node, `docs/design/DESIGN_HANDOFF.md` §"Usage tab"): new `#[tauri::command] cost_breakdown(ws_id)`
  in `apps/desktop/src-tauri/src/bridge.rs`, registered in `lib.rs`'s `generate_handler!`. Hand-written to
  mirror `board`/`graph` exactly (lock-clone-return, `AppState::Mock(_) => Ok(None)`) against the already
  Rust-tested `Core::cost_breakdown(&self, ws_id: &str) -> Option<marsad_viewmodel::WorkstreamCostBreakdown>`
  (`crates/marsad-core/src/lib.rs:1723`, landed+tested in commit `92d5029`) — but, like the rest of
  src-tauri, **never compiled here** (no webkit2gtk). The `#[tauri::command(rename_all = "snake_case")]`
  macro expansion and the real `invoke("cost_breakdown", { ws_id })` → command → `Core::cost_breakdown` wire
  path are unverified. The TS side (`bridge/api.ts:fetchCostBreakdown`, `bridge/types.ts:CostRow`/
  `WorkstreamCostBreakdown`, `screens/tabs/Usage.tsx`'s Cost breakdown section) IS verified — built against
  a browser-mode mock and covered by Vitest + a served-dist Playwright screenshot (see the Usage tab test
  suite and `tests/e2e`).
- ⚠️ **WAVE 3b — pty escape-hatch terminal, `src-tauri` + frontend** (built against the REAL, VERIFIED
  `marsad-proto`/`marsad-agent` pty surface above, but — like the rest of src-tauri — never
  compiled/run here, no webkit2gtk): three new Tauri commands in `apps/desktop/src-tauri/src/bridge.rs`
  (`pty_start(session_id,cols,rows)`, `pty_input(session_id,data)`, `pty_resize(session_id,cols,rows)`,
  registered in `lib.rs`'s `generate_handler!`) that each open a FRESH, one-shot `LocalIpc` connection to
  the agent socket (`crate::resolve_socket_path()`, now `pub(crate)`) and send one encoded `IpcMsg` line —
  deliberately NOT routed through `Core` (no pty-aware methods; `marsad-core` was NOT touched by this
  wave — raw terminal bytes have no session/business meaning for it to reduce) and NOT reusing the
  reader thread's long-lived subscriber connection (a second writer on that same socket handle would
  race its blocking `recv_line` loop). A persistent, reused connection would cut the per-keystroke
  connect overhead; deferred as a follow-up to keep this unverified surface simple. `events.rs` gained two
  new `VmUpdateEnvelope` arms (`PtyOutput`/`PtyExit`, constructed directly — NOT via `From<VmUpdate>`,
  since there is no `VmUpdate::Pty*`) and a NEW `emit_pty_update` helper that emits on a SEPARATE `"pty"`
  Tauri event (not `"vm-update"`) — `run_reader_thread` (`lib.rs`) intercepts raw `PtyOutput`/`PtyExit`
  agent lines BEFORE the existing type allow-list would silently `continue`-skip them, and forwards them
  straight to the webview via `emit_pty_update` instead of `Core::on_agent_line` (which has no arm for
  either). Two new Rust unit tests in `events.rs` pin the `pty_output`/`pty_exit` envelope wire shape
  (written, unverified — never compiled). The `#[tauri::command]` macro expansion, the real
  `invoke("pty_start"/"pty_input"/"pty_resize", …)` → command → one-shot-connection → agent path, and the
  real `"pty"` event → webview path have **never run**.
  Frontend: `Terminal.tsx` fully rebuilt on **`xterm.js`** (`@xterm/xterm` + `@xterm/addon-fit`, added
  deps) — a real dark mono terminal (`term.write` fed base64-decoded `PtyOutput` bytes as a `Uint8Array`,
  `term.onData`/`term.onResize` wired to `ptyInput`/`ptyResize`, `ResizeObserver` + window-resize
  re-fitting). `bridge/api.ts` gained `ptyStart`/`ptyInput`/`ptyResize`/`listenPty` wrappers (no-op in
  browser/mock mode, same convention as every other command) and `bridge/types.ts` a `PtyEvent` union for
  the `"pty"` channel (kept separate from `VmUpdateEvent` for the same layering reason as the Rust side).
  THIS part IS verified: `SessionDetail.test.tsx`'s `TerminalTab` suite was rewritten for the real
  component (asserts a genuine `.xterm` DOM node mounts, `ptyStart` is called with the session id, an
  honestly-idle terminal in mock mode — no more synthetic "Claude is working…" lines — and clean
  unmount/dispose); all 421 Vitest tests across all 35 files pass, `tsc --noEmit` is clean, and
  `pnpm build` (`tsc && vite build`) succeeds with `xterm.css` bundled into the output. (One test-harness
  wrinkle fixed along the way, unrelated to the pty logic itself: `SessionDetail.test.tsx`'s outer
  `afterEach(() => vi.restoreAllMocks())` was silently resetting the `vi.mock("../src/bridge/api", …)`
  factory's `mockResolvedValue`-configured fns back to a bare no-op after the first test in the file —
  invisible for every pre-existing fire-and-forget command mock, but `TerminalTab` is the first consumer
  to `.then()`-chain directly on a mocked async call's return; the fix re-applies the pty mocks' resolved
  values in the file's top-level `beforeEach`.)
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
