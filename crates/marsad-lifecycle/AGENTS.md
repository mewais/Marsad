# B09 — `marsad-lifecycle` — working contract

**Purpose:** Reconnect-or-resume algorithm + container abstraction (tmux/ConPTY).
**Tier:** L1 (trait-backed)
**Full spec (read this first):** `../../docs/specs/lifecycle.md`
**Test plan & coverage regime:** `../../docs/specs/TESTPLAN.md`
**Design source of truth:** `_extracted/design_handoff_marsad/Marsad.dc.html` and `Marsad Mobile.dc.html` (port `renderVals()` / helper logic; do not invent behavior).

## What you may edit
- **code-agent:** write ONLY `crates/marsad-lifecycle/src/**` and `crates/marsad-lifecycle/Cargo.toml`.
  Read-only: `crates/marsad-lifecycle/tests/**` and `../../docs/specs/lifecycle.md`. No access to other blocks.
- **test-agent:** write ONLY `crates/marsad-lifecycle/tests/**`. Read-only: `../../docs/specs/lifecycle.md`.

## Definition of done
- The public API in `docs/specs/lifecycle.md` is implemented exactly (names, signatures, semantics).
- `cargo test -p marsad-lifecycle` is fully green.
- `cargo llvm-cov -p marsad-lifecycle --fail-under-lines 100 --fail-under-regions 100` passes.
- No `unsafe`, no panics on documented inputs, no IO beyond what the spec allows. Run `cargo fmt`
  and `cargo clippy -p marsad-lifecycle -- -D warnings` clean.

## Rules
- Tests are written BEFORE code and are read-only to the code-agent. Do not edit tests to pass.
- If a branch is unreachable from the public API, delete it — do not leave it uncovered.
- Keep within the dependency DAG declared in `Cargo.toml`; do not add new internal deps without
  reflecting them in ARCHITECTURE.md.
