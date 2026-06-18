# D01/D02 — `apps/desktop` — working contract

Two blocks live here:

- **D01 `src-tauri/`** (Rust, Tier L1): the Tauri bridge over `marsad-core`. Tauri commands + an
  event bridge forwarding `marsad_core::VmUpdate`s to the UI; window/tray/notification wiring.
- **D02 `src/`** (React/TypeScript, UI tier): the desktop UI, faithful to the design.

**Design source of truth:** `_extracted/design_handoff_marsad/Marsad.dc.html` — recreate the look,
layout, states, and interactions exactly (tokens, typography, spacing, radii, glass). The
`renderVals()` logic is already ported into `marsad-viewmodel`; the UI should consume view-models
from the core, NOT re-derive them.
**Core API:** `docs/specs/core.md`. **Test plan:** `docs/specs/TESTPLAN.md`.

## Screens to build (D02)
Overview (command center: 4 stat tiles, "Needs your attention" gate grid, Active now, Idle &
finished) · Session detail (tabs Activity/Changes/Config/Usage/Terminal) · Gate overlay
(permission/plan/edit, approve / reject-with-reason) · New-session modal · Settings
(Notifications/Hosts/Session defaults/General) · Workspaces manager · header (workspace switcher,
search, theme + glass toggles, New session).

## What you may edit
- **D01 code-agent:** write `apps/desktop/src-tauri/src/**` + its `Cargo.toml`/`tauri.conf.json`.
- **D02 code-agent:** write `apps/desktop/src/**`.
- **D02 test-agent:** write `apps/desktop/tests/**` (Vitest + Playwright).
Do not edit other blocks. `src-tauri` is a standalone cargo package (not a workspace member).

## Definition of done
- `pnpm install && pnpm test` (Vitest) green; `pnpm test:e2e` (Playwright golden path) green.
- `cargo tauri dev` launches; Overview renders faithfully to `Marsad.dc.html` (manual UI pass by the
  owner — UI fidelity is owner-verified, functionality is test-verified).
- Interaction matrix in `docs/specs/TESTPLAN.md` fully covered (filters re-scope, gate routes to one
  session, slash runs top match on Enter, diff renders side-by-side, staging+commit, theme/glass).

## Rules
- Never parse terminal output; all session data comes from the core's view-models/events.
- Keep state in the core; the UI is a thin renderer + event dispatcher.
