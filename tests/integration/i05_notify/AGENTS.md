# I05 — Notifications — integration scenario

**Blocks under test:** B02 events, B10 notify
**Kind:** Recorded-real + fake sink
**Goal:** Real events + hook payloads → correct notification content/priority; DND mutes; bursts group; captured via the fake Notifier.

**Spec:** `../../../docs/specs/TESTPLAN.md` (real-behavior coverage matrix). **Architecture:**
`../../../ARCHITECTURE.md`.

## What you may edit
- **integration-agent:** write ONLY `tests/integration/i05_notify/**`. Read-only: every block's public
  API and `docs/specs/**`. No access to any block's `src/` for editing.

## Definition of done
- The scenario assertions in `tests/` pass via `cargo test -p marsad-it-i05_notify`.
- For **Live** scenarios: the test detects `claude`/`tmux`/`ssh` availability and is part of the
  `claude`-gated CI job; it must genuinely exercise the real binary (no mocking the thing under test).
- For **Recorded-real** scenarios: assert against the checked-in `fixtures/` captures.

## Rules
- Compose blocks only through their public APIs; never reach into another crate's internals.
- A Live test that cannot find its real dependency must SKIP loudly (not silently pass).
