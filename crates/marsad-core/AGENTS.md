# B12 — `marsad-core` — working contract

**Purpose:** Facade tying B01-B11 into the desktop/phone API; UniFFI surface; agent client.
**Tier:** L1 (facade+FFI)
**Full spec (read this first):** `../../docs/specs/core.md`; U1 dynamic capability discovery's
core-side contract (`capabilities: HashMap<(driver, host_id), Capabilities>` state,
`VmUpdate::Capabilities`, the `on_agent_line` `CapabilitiesUpdate`/`CapabilitiesRefreshQueued`
arms, `Core::capabilities()`/`Core::refresh_capabilities()`) is specified in
`../../docs/specs/dynamic-capabilities.md` §3.5. `VmUpdate::Capabilities` wraps
`marsad_viewmodel::CapabilitiesView` (§3.6, a sibling block's pure `From<Capabilities>` mirror
type) — read-only from here. X3's Wasl inbound surface (`wasl_gates:
BTreeMap<(host_id, gate_id), WaslGateView>` state, `VmUpdate::{WaslGates, WaslGateDecided,
WaslObservation}`, the `on_agent_line` `WaslGateRequested`/`WaslGateDecided`/`WaslObservation`/
`WaslGateAnswered` arms, `CoreError::UnknownWaslGate`, `Core::answer_wasl_gate` — host-scoped, NOT
`require_session`) is specified in `../../docs/specs/wasl-connector.md` §5.2. `answer_wasl_gate`
routes through `marsad_gates::Gate::Handoff` with `session_id = "wasl:<host_id>"` (§5.1); pending
gates are node-authoritative (added by `WaslGateRequested`, removed ONLY by `WaslGateDecided` —
`answer_wasl_gate` never optimistically removes).
**Test plan & coverage regime:** `../../docs/specs/TESTPLAN.md`
**Design source of truth:** `_extracted/design_handoff_marsad/Marsad.dc.html` and `Marsad Mobile.dc.html` (port `renderVals()` / helper logic; do not invent behavior).

## What you may edit
- **code-agent:** write ONLY `crates/marsad-core/src/**` and `crates/marsad-core/Cargo.toml`.
  Read-only: `crates/marsad-core/tests/**` and `../../docs/specs/core.md`. No access to other blocks.
- **test-agent:** write ONLY `crates/marsad-core/tests/**`. Read-only: `../../docs/specs/core.md`.

## Definition of done
- The public API in `docs/specs/core.md` is implemented exactly (names, signatures, semantics).
- `cargo test -p marsad-core` is fully green.
- `cargo llvm-cov -p marsad-core --fail-under-lines 100 --fail-under-regions 100` passes.
- No `unsafe`, no panics on documented inputs, no IO beyond what the spec allows. Run `cargo fmt`
  and `cargo clippy -p marsad-core -- -D warnings` clean.

## Rules
- Tests are written BEFORE code and are read-only to the code-agent. Do not edit tests to pass.
- If a branch is unreachable from the public API, delete it — do not leave it uncovered.
- Keep within the dependency DAG declared in `Cargo.toml`; do not add new internal deps without
  reflecting them in ARCHITECTURE.md.
