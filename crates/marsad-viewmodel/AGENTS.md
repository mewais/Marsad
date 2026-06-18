# B11 — `marsad-viewmodel` — working contract

**Purpose:** Pure dashboard aggregation — the Rust port of renderVals().
**Tier:** L0 (pure)
**Full spec (read this first):** `../../docs/specs/viewmodel.md`
**Test plan & coverage regime:** `../../docs/specs/TESTPLAN.md`
**Design source of truth:** `_extracted/design_handoff_marsad/Marsad.dc.html` and `Marsad Mobile.dc.html` (port `renderVals()` / helper logic; do not invent behavior).

## What you may edit
- **code-agent:** write ONLY `crates/marsad-viewmodel/src/**` and `crates/marsad-viewmodel/Cargo.toml`.
  Read-only: `crates/marsad-viewmodel/tests/**` and `../../docs/specs/viewmodel.md`. No access to other blocks.
- **test-agent:** write ONLY `crates/marsad-viewmodel/tests/**`. Read-only: `../../docs/specs/viewmodel.md`.

## Definition of done
- The public API in `docs/specs/viewmodel.md` is implemented exactly (names, signatures, semantics).
- `cargo test -p marsad-viewmodel` is fully green.
- `cargo llvm-cov -p marsad-viewmodel --fail-under-lines 100 --fail-under-regions 100` passes.
- No `unsafe`, no panics on documented inputs, no IO beyond what the spec allows. Run `cargo fmt`
  and `cargo clippy -p marsad-viewmodel -- -D warnings` clean.

## Rules
- Tests are written BEFORE code and are read-only to the code-agent. Do not edit tests to pass.
- If a branch is unreachable from the public API, delete it — do not leave it uncovered.
- Keep within the dependency DAG declared in `Cargo.toml`; do not add new internal deps without
  reflecting them in ARCHITECTURE.md.
