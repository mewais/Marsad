# I07 — Core facade & concurrency — integration scenario

**Blocks under test:** B12 core over B01-B11, (B13)
**Kind:** Live
**Goal:** Drive the core/UniFFI surface end-to-end against a scripted/real agent: prompt + interrupt round-trip; N≥3 concurrent sessions advance independently with no cross-routing of gates/prompts.

**Spec:** `../../../docs/specs/TESTPLAN.md` (real-behavior coverage matrix). **Architecture:**
`../../../ARCHITECTURE.md`.

## What you may edit
- **integration-agent:** write ONLY `tests/integration/i07_core_facade/**`. Read-only: every block's public
  API and `docs/specs/**`. No access to any block's `src/` for editing.

## Definition of done
- The scenario assertions in `tests/` pass via `cargo test -p marsad-it-i07_core_facade`.
- For **Live** scenarios: the test detects `claude`/`tmux`/`ssh` availability and is part of the
  `claude`-gated CI job; it must genuinely exercise the real binary (no mocking the thing under test).
- For **Recorded-real** scenarios: assert against the checked-in `fixtures/` captures.

## Rules
- Compose blocks only through their public APIs; never reach into another crate's internals.
- A Live test that cannot find its real dependency must SKIP loudly (not silently pass).
