# B01 — `marsad-proto` — working contract

**Purpose:** Wire types + serde for Claude stream-json and the GUI<->agent IPC protocol.
**Tier:** L0 (pure)
**Full spec (read this first):** `../../docs/specs/proto.md`; U1 dynamic capability discovery's
wire surface (`Capabilities`/`ModelCatalog`/`ModelInfo`/`ModelListSource`/`DiscoveredSession`/
`SessionScope` domain types + the `RefreshCapabilities`/`CapabilitiesRefreshQueued`/
`CapabilitiesUpdate` `IpcMsg` variants) is specified in
`../../docs/specs/dynamic-capabilities.md` §1-§2.
**Test plan & coverage regime:** `../../docs/specs/TESTPLAN.md`
**Design source of truth:** `_extracted/design_handoff_marsad/Marsad.dc.html` and `Marsad Mobile.dc.html` (port `renderVals()` / helper logic; do not invent behavior).

## What you may edit
- **code-agent:** write ONLY `crates/marsad-proto/src/**` and `crates/marsad-proto/Cargo.toml`.
  Read-only: `crates/marsad-proto/tests/**` and `../../docs/specs/proto.md`. No access to other blocks.
- **test-agent:** write ONLY `crates/marsad-proto/tests/**`. Read-only: `../../docs/specs/proto.md`.

## Definition of done
- The public API in `docs/specs/proto.md` is implemented exactly (names, signatures, semantics).
- `cargo test -p marsad-proto` is fully green.
- `cargo llvm-cov -p marsad-proto --fail-under-lines 100 --fail-under-regions 100` passes.
- No `unsafe`, no panics on documented inputs, no IO beyond what the spec allows. Run `cargo fmt`
  and `cargo clippy -p marsad-proto -- -D warnings` clean.

## Rules
- Tests are written BEFORE code and are read-only to the code-agent. Do not edit tests to pass.
- If a branch is unreachable from the public API, delete it — do not leave it uncovered.
- Keep within the dependency DAG declared in `Cargo.toml`; do not add new internal deps without
  reflecting them in ARCHITECTURE.md.
