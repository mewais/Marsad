# I03 — Real claude + tmux lifecycle — integration scenario

**Blocks under test:** B09 lifecycle, B13 agent, B08 transport
**Kind:** Live
**Goal:** Launch a real claude in tmux (observe system/init); kill+--resume continuity; adopt a running container with NO duplication; client-disconnect liveness; terminal-over-tmux + stage/commit smoke.

**Spec:** `../../../docs/specs/TESTPLAN.md` (real-behavior coverage matrix). **Architecture:**
`../../../ARCHITECTURE.md`.

## What you may edit
- **integration-agent:** write ONLY `tests/integration/i03_lifecycle_real/**`. Read-only: every block's public
  API and `docs/specs/**`. No access to any block's `src/` for editing.

## Definition of done
- The scenario assertions in `tests/` pass via `cargo test -p marsad-it-i03_lifecycle_real`.
- For **Live** scenarios: the test detects `claude`/`tmux`/`ssh` availability and is part of the
  `claude`-gated CI job; it must genuinely exercise the real binary (no mocking the thing under test).
- For **Recorded-real** scenarios: assert against the checked-in `fixtures/` captures.

## Rules
- Compose blocks only through their public APIs; never reach into another crate's internals.
- A Live test that cannot find its real dependency must SKIP loudly (not silently pass).
