# D01/D02 — `apps/desktop` — working contract

Two blocks live here:

- **D01 `src-tauri/`** (Rust): a thin Tauri v2 shell over `marsad-client` — exactly the generic
  bridges `rpc(method, params?, host?, trace?)`, `rpc_subscribe(topic, host?, from?, params?, channel)`
  (channel of `{type:"event",cursor,payload}` / `{type:"link",state}` / `{type:"end"}`),
  `rpc_unsubscribe(id)`, `connection_state()` (+ `connection` event), `reconnect_daemon()`, and
  `diag_log(events)`; `smoke_report(report)` exists only in debug builds run with `MARSAD_SMOKE`.
  Native extras as `rpc` methods `desktop.open|reveal|pick_folder|pick_files|read_picked|download_begin|download_chunk|export_save|window_surface`;
  notifications, tray, single instance, window state; the `navigate {host, session?}` event after
  a notification brings the app forward. The app's own log is `<data>/machines/<key>/diag/app.ndjson`
  (`src-tauri/src/diag.rs`, typed `AppEvent`s only); the webview may only log typed `UiEvent`s
  through `diag_log`. See the Marsad README ("Desktop app").
- **D02 `src/`** (React/TypeScript, UI tier): the desktop UI, faithful to the design.

**Design source of truth:** `design/marsad-mockups.html` — recreate the look,
layout, states, and interactions exactly (tokens, typography, spacing, radii, glass). The
The wire types and method/topic names are generated from the Rust crates into
`src/bridge/generated/` (`marsad-ts-export`; CI checks they are current); never hand-type them.
Real-app end-to-end suite: `pnpm test:real` (see the Marsad README).

## Screens to build (D02)
Inbox (the landing page: sessions grouped Needs you, Failed, Finished not seen yet, Working, Idle, Ended) · Session detail (tabs Activity/Changes/Config/Terminal, plus Diagnostics in developer mode; usage is in the session sidebar) · Gates (permission/question/plan/handoff/spawn, drawn in the Inbox row or the session view; approve / reject-with-reason) · New-session modal · Settings
(General/Notifications/Keyboard/Diagnostics/About; theme and window background are under General) · Workstreams manager · header (search, New session).

## What you may edit
- **D01 code-agent:** write `apps/desktop/src-tauri/src/**` + its `Cargo.toml`/`tauri.conf.json`.
- **D02 code-agent:** write `apps/desktop/src/**`.
- **D02 test-agent:** write `apps/desktop/tests/**` (Vitest + Playwright).
Do not edit other blocks. `src-tauri` is a standalone cargo package (not a workspace member).

## Definition of done
- `pnpm install && pnpm test` (Vitest) green; `pnpm test:e2e` (Playwright golden path) green.
- `cargo tauri dev` launches; the UI renders faithfully to `design/marsad-mockups.html` (manual UI pass by the
  owner — UI fidelity is owner-verified, functionality is test-verified).
- `pnpm test:real` (real app, real backend) green; interactions covered (filters re-scope, gate routes to one
  session, slash runs top match on Enter, diff renders side-by-side, staging+commit, theme/glass).

## Rules
- Never parse terminal output; all session data comes from the core's view-models/events.
- Keep state in the core; the UI is a thin renderer + event dispatcher.
