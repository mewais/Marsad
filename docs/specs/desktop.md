# D01/D02 — `apps/desktop`

**D01 `src-tauri`** Tier L1 (Rust) · **D02 `src`** Tier UI (React/TS). Full design in
`_extracted/design_handoff_marsad/Marsad.dc.html`; core API in `docs/specs/core.md`; interaction
matrix + UI test tooling in `docs/specs/TESTPLAN.md`. Working contract in `apps/desktop/AGENTS.md`.

## D01 — Tauri bridge
Wrap `marsad-core` behind Tauri commands and an event stream:
- Commands: `launch_session, resume_session, attach_session, stop_session, send_prompt, interrupt,
  answer_gate, run_slash, set_workspace`, plus host/notification/settings ops — each a thin call into
  `marsad_core::Core`.
- Event bridge: forward `marsad_core::VmUpdate`s to the webview via Tauri events; the UI re-renders
  from view-models, never re-deriving state.
- Platform glue: window, system tray, OS notifications (delegating content to `marsad-notify`).
- Standalone cargo package (own `[workspace]` table) so `cargo tauri` builds it with the Tauri
  profile; depends on `marsad-core` by path.

## D02 — React UI (faithful port)
Screens: Overview (4 stat tiles · "Needs your attention" glowing gate grid · Active now rows with
live action line + ctx/token bars + elapsed · Idle & finished grid) · Session detail
(Activity transcript with prompt/response/tool/result/error/working/subagent/gate styles + composer
with `/` slash popover; Changes with side-by-side diff + staging + commit; Config; Usage with ≥75%
compaction warning; Terminal escape hatch) · Gate overlay (permission/plan/edit, approve /
reject-with-reason, routes to exactly one session) · New-session modal · Settings
(Notifications with pop-up preview / Hosts / Session defaults / General with theme + background +
glass) · Workspaces switcher + manager. Honor all design tokens, glass mode, dark/light.

Consume view-models from `marsad-viewmodel` via the core; keep zero business logic in the UI.

## Tests
- Vitest + React Testing Library: the interaction matrix (filters re-scope; gate approve/reject
  routes to one session; slash runs top match on Enter; diff renders; staging + commit; theme/glass
  toggle; workspace switch re-scopes).
- Playwright: golden-path e2e against a mocked core bridge.
- UI visual fidelity is owner-verified; functionality is test-verified.
