# Handoff — Workstreams UI redesign is on `main` (verify it on the webkit machine)

**To:** the agent working Marsad on the work machine (the one that CAN compile `src-tauri` — webkit2gtk-4.1).
**From:** the agent working Marsad on the personal machine (no webkit2gtk — frontend + Rust backends only).
**First:** `git checkout main && git pull`.

## What just landed on `main`

The **Workstreams UI redesign** (per `docs/design/DESIGN_HANDOFF.md` + `Marsad Workstreams.dc.html`) is BUILT and on `main`, in four commits:
- `92d5029` — cost-breakdown backend (viewmodel/core, 100/100)
- `4563a35` — **shell/nav/tokens** (Wave 1): glass breadcrumb header, Board is home, removed the old left sidebar + Overview-home + Workspaces switcher; `view = board|graph|session` nav; exact tokens/fonts/wallpaper
- `c565721` — **Board / Graph / Session / Modals** (Wave 2): the elkjs Flow + By-host graph with custom status/type edges + gate diamonds + design mode; the polished Board (host-link chips, attention cards) and Session (driver pill, ‹Back, HITL needs-you→resolved collapse); new-workstream two-path wizard + capability-driven node modal + HITL overlays; Settings trimmed
- `ecddae9` — **Wave 3**: Usage-tab cost breakdown (by host/by node) + a streaming **PTY terminal** (real `portable-pty` in the agent + xterm.js tab)

## The verified / built-not-verified split (important)

This was all built on a machine with **no webkit2gtk**, so `apps/desktop/src-tauri` NEVER COMPILED here. Concretely:

- ✅ **Verified here:** the whole React/TS frontend against the mock backend (**421 Vitest, `tsc`+`vite build` clean**, screenshotted at every level); the Rust backends — cost breakdown (viewmodel+core **100/100**) and the pty (proto+agent **100/100**, incl. **8 live tests that spawn a real `/bin/sh` and roundtrip echo/resize/exit**); clippy + fmt clean; the CI grep-gates (no preset client lists) green.
- ⚠️ **Built-not-verified (YOUR job — see `docs/GAPS.md` for the full list):** everything in `src-tauri`. The redesign added:
  1. `bridge::cost_breakdown(ws_id)` command + registration → drives the Usage-tab breakdown.
  2. `bridge::pty_start / pty_input / pty_resize` commands + a **separate `"pty"` Tauri event** (`events.rs` `emit_pty_update`, intercepted in `lib.rs`'s reader before the allow-list drops it) → drives the Terminal tab.
  3. All the Wave-1/2 wiring (the new nav, the modals) runs the real `invoke → Core → vm-update` path for the first time.

## What you need to do

1. **Compile + run:** `cd apps/desktop && pnpm install` (new deps: `elkjs`, `@xterm/xterm`, `@xterm/addon-fit`), then `cargo build` in `src-tauri` (new crate dep: `portable-pty` in `marsad-agent`), then `pnpm tauri dev`. Fix any Tauri macro/wiring compile errors — this is the first real compile of the redesigned app.
2. **Eyeball the redesign in the real window:** it should open to the centered Workstreams Board under the glass breadcrumb header (no old sidebar). Walk Board → Graph (both By-host and elkjs Flow) → Session → modals. Confirm it matches the design (`docs/design/`).
3. **Verify the two new native paths:**
   - **Cost breakdown:** open a session's Usage tab, confirm the by-host/by-node bars populate from real `Core::cost_breakdown` (not just the mock).
   - **PTY terminal (the big one):** open the Terminal tab and confirm the real keystroke → shell → xterm loop works end-to-end (type `ls`, see output; resize; exit). The agent-side pty is proven; only the `src-tauri` bridge + native stream are unrun.
4. **Playwright golden path + live integration** against the compiled app.

## Hold these invariants (unchanged, CI-gated)

Async / never block under a lock; **no preset client lists** (`DynCapKillList` grep gate — slash/models/sessions come from capabilities, never hardcoded); agent-reported cost is truth (estimates labelled `~`); no TUI parsing; **Marsad never auto-answers a gate** — every gate answer is a relayed human click. The PTY is a *separate raw shell escape-hatch*, not claude-output parsing.

## Coordination

- I'm doing the **UI redesign** from the personal machine, committing to `main`. You're doing **codex-launch** (a separate session) — don't touch `apps/desktop/src/**`, the graph, or the pty; I won't touch the codex driver/gateway.
- If a pull conflicts, the redesign commits above are the newer UI truth. `docs/design/` is the visual reference; `docs/GAPS.md` is the authoritative built-vs-verified inventory (kept current).

## One known flake

`crates/marsad-agent/tests/pty_live.rs` had a single first-run timing hiccup (real-pty tests are timing-sensitive), then passed 4× clean. If it flakes in CI, bump the read/settle timeout in that test — not a logic bug.
