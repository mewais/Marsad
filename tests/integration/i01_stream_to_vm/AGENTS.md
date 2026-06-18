# I01 — Stream → view-model — integration scenario

**Blocks under test:** B01 proto, B02 events, B11 viewmodel
**Kind:** Recorded-real
**Goal:** Fold a recorded real claude stream into SessionState and assert the resulting Overview/Detail view-models (counts, groups, usage/cost/context%, diff fidelity).

**Spec:** `../../../docs/specs/TESTPLAN.md` (real-behavior coverage matrix). **Architecture:**
`../../../ARCHITECTURE.md`.

## What you may edit
- **integration-agent:** write ONLY `tests/integration/i01_stream_to_vm/**`. Read-only: every block's public
  API and `docs/specs/**`. No access to any block's `src/` for editing.

## Definition of done
- The scenario assertions in `tests/` pass via `cargo test -p marsad-it-i01_stream_to_vm`.
- For **Live** scenarios: the test detects `claude`/`tmux`/`ssh` availability and is part of the
  `claude`-gated CI job; it must genuinely exercise the real binary (no mocking the thing under test).
- For **Recorded-real** scenarios: assert against the checked-in `fixtures/` captures.

## Rules
- Compose blocks only through their public APIs; never reach into another crate's internals.
- A Live test that cannot find its real dependency must SKIP loudly (not silently pass).
