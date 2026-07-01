# B14 — `marsad-workstream` — working contract

**Purpose:** Workstream/graph domain — profiles, workstreams, nodes, typed connections, and the pure
graph derivations (head-state, driver dots, host chips, dependency depth, connection validation) that the
board (Level 1) and graph (Level 2) views consume.
**Tier:** L0 (pure, stateless, no IO).
**Full spec (read this first):** `../../docs/specs/workstream.md`
**Design source of truth:** `../../docs/design/Marsad Workstreams.dc.html` + `../../docs/design/DESIGN_HANDOFF.md`
(port the domain model + head-state/aggregation behavior; do not invent new behavior).

## What you may edit
- **code-agent:** write ONLY `crates/marsad-workstream/src/**` and `crates/marsad-workstream/Cargo.toml`.
  Read-only: `crates/marsad-workstream/tests/**` and `../../docs/specs/workstream.md`. No other blocks.
- **test-agent:** write ONLY `crates/marsad-workstream/tests/**`. Read-only: `../../docs/specs/workstream.md`
  and the design source. Do not write to `src/`.

## Definition of done
- The public API in `docs/specs/workstream.md` is implemented exactly (names, signatures, semantics).
- `cargo test -p marsad-workstream` is fully green.
- `cargo llvm-cov -p marsad-workstream --fail-under-lines 100 --fail-under-regions 100` passes.
- No `unsafe`, no panics on documented inputs, no IO. `cargo fmt` and
  `cargo clippy -p marsad-workstream -- -D warnings` clean.

## Rules
- Tests are written BEFORE code and are read-only to the code-agent. Do not edit tests to pass.
- No test may be `#[ignore]`d, `#[cfg]`-disabled, stubbed, or reduced to a tautology. Every test runs.
- If a branch is unreachable from the public API, delete it — do not leave it uncovered.
- Only dependency allowed: `marsad-events` (for `Phase`) + `serde`. Do not add other internal deps.
