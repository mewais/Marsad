# B13 — `marsad-agent` — working contract

**Purpose:** Detached supervisor daemon owning claude processes; the thing that outlives the GUI.
**Tier:** L2 (integration-bound)
**Full spec (read this first):** `../../docs/specs/agent.md`
**Test plan & coverage regime:** `../../docs/specs/TESTPLAN.md`
**Design source of truth:** `_extracted/design_handoff_marsad/Marsad.dc.html` and `Marsad Mobile.dc.html` (port `renderVals()` / helper logic; do not invent behavior).

## What you may edit
- **code-agent:** write ONLY `crates/marsad-agent/src/**` and `crates/marsad-agent/Cargo.toml`.
  Read-only: `crates/marsad-agent/tests/**` and `../../docs/specs/agent.md`. No access to other blocks.
- **test-agent:** write ONLY `crates/marsad-agent/tests/**`. Read-only: `../../docs/specs/agent.md`.

## Module map (`src/`)
The crate is one logical unit split across files purely for readability — this is
NOT a public module hierarchy (all `mod` declarations are private); the flat
`marsad_agent::*` public API is preserved via `pub use` re-exports at the top of
`lib.rs`. When adding code, put it in the file matching its seam below; when in
doubt, prefer the file the majority of its callers already live in.

- **`lib.rs`** — root. `AgentError`/`ListenSpec`/`AgentConfig`, the opaque-Debug
  wrapper types, the `Agent` struct itself, and its core lifecycle impl
  (`new`/`with_fakes`/`with_real_io`, `active_container`, `container_calls`,
  `next_session_id`, `launch`/`launch_with_id`, `reconcile_all`/`reconcile_one`).
  Also hosts the `mod` declarations and every `pub use` re-export, so this file
  is the map of the crate's public surface.
- **`ipc.rs`** — `handle_ipc` (decode + dispatch) and every `handle_*` method it
  routes to (`handle_launch_session`, `handle_resume_session`,
  `handle_attach_session`, `handle_stop_session`, `handle_send_prompt`,
  `handle_interrupt`, `handle_answer_gate`, `handle_answer_question`,
  `handle_subscribe`), plus hook ingestion (`handle_hook`/`parse_hook_event`).
- **`driver.rs`** — builds the `claude` invocation and drives the session
  container: `claude_argv`/`posture_to_mode`, `build_spawn_cmd`, the
  `spawn_container`/`kill_container` builders (local vs. remote routing), and
  the store lookups they share with `serve.rs`
  (`session_host_target`/`session_transcript_project_uuid`).
- **`reader.rs`** — `spawn_session_reader`: the per-session thread that connects
  the transport, pumps its NDJSON stream, and broadcasts to subscribers
  (including the remote auto-reconnect loop), plus the small pure-logic helpers
  it leans on (`reconnect_backoff_next`, `assistant_has_file_modifying_tool_use`,
  the `RECONNECT_*` constants).
- **`remote.rs`** — SSH/WSL/plink machinery: `exec_prefix_for`,
  `run_remote_script`/`run_remote_script_bounded`, the remote script builders
  (`remote_transcript_script`, `remote_usage_script`, `remote_changes_script` +
  `parse_remote_changes`, `remote_durable_launch_script`,
  `remote_liveness_script`, `remote_kill_script`), the durable-session argv
  builders (`remote_writer_argv`/`remote_reader_argv`), and
  `build_remote_spawn_cmd`/`fetch_remote_session_list`.
- **`serve.rs`** — `Agent::serve`, the blocking real-IO event loop (bind, accept,
  command loop). Entirely `#[cfg(not(coverage))]`.

Cross-file calls between these use `pub(crate)` where the callee isn't already
`pub`; nothing in this list is reachable from outside the crate except through
the `pub use` re-exports in `lib.rs` (or as an `Agent`/`AgentConfig`/`AgentError`/
`ListenSpec` method or variant, which don't need re-exporting since they're
defined in `lib.rs` itself).

## Definition of done
- The public API in `docs/specs/agent.md` is implemented exactly (names, signatures, semantics).
- `cargo test -p marsad-agent` is fully green.
- `cargo llvm-cov -p marsad-agent --fail-under-lines 100 --fail-under-regions 100` passes.
- No `unsafe`, no panics on documented inputs, no IO beyond what the spec allows. Run `cargo fmt`
  and `cargo clippy -p marsad-agent -- -D warnings` clean.

## Rules
- Tests are written BEFORE code and are read-only to the code-agent. Do not edit tests to pass.
- If a branch is unreachable from the public API, delete it — do not leave it uncovered.
- Keep within the dependency DAG declared in `Cargo.toml`; do not add new internal deps without
  reflecting them in ARCHITECTURE.md.
