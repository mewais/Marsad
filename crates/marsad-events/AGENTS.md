# B02 — `marsad-events` — working contract

**Purpose:** Pure reducer folding parsed messages + hooks into a derived SessionState.
**Tier:** L0 (pure)
**Full spec (read this first):** `../../docs/specs/events.md`
**Test plan & coverage regime:** `../../docs/specs/TESTPLAN.md`
**Design source of truth:** `_extracted/design_handoff_marsad/Marsad.dc.html` and `Marsad Mobile.dc.html` (port `renderVals()` / helper logic; do not invent behavior).

## What you may edit
- **code-agent:** write ONLY `crates/marsad-events/src/**` and `crates/marsad-events/Cargo.toml`.
  Read-only: `crates/marsad-events/tests/**` and `../../docs/specs/events.md`. No access to other blocks.
- **test-agent:** write ONLY `crates/marsad-events/tests/**`. Read-only: `../../docs/specs/events.md`.

## Definition of done
- The public API in `docs/specs/events.md` is implemented exactly (names, signatures, semantics).
- `cargo test -p marsad-events` is fully green.
- `cargo llvm-cov -p marsad-events --fail-under-lines 100 --fail-under-regions 100` passes.
- No `unsafe`, no panics on documented inputs, no IO beyond what the spec allows. Run `cargo fmt`
  and `cargo clippy -p marsad-events -- -D warnings` clean.

## Rules
- Tests are written BEFORE code and are read-only to the code-agent. Do not edit tests to pass.
- If a branch is unreachable from the public API, delete it — do not leave it uncovered.
- Keep within the dependency DAG declared in `Cargo.toml`; do not add new internal deps without
  reflecting them in ARCHITECTURE.md.
