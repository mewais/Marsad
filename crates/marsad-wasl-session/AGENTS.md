# X6 — `marsad-wasl-session` — working contract

**Purpose:** Marsad's caller-side client of a **Wasl-managed** live session (Wasl's "Model A"
managed runtime): the `WaslSessionClient` / `WaslSessionProvider` traits, the caller-mirror domain
types (`WaslTargetStatus` / `WaslTurnReceipt` / `WaslCapabilities` / …), the pure
`WaslTargetStatus → marsad_events::Phase` projection, the in-memory `FakeWaslSessionClient` /
`FakeWaslSessionProvider`, **and** the real, hand-rolled HTTP/1.1 + SSE client
(`RealWaslSessionClient` / `RealWaslSessionProvider` / `WaslSessionClientConfig`) against a live Wasl
runtime-control node. Consumed by `marsad-agent` (B13) and `marsad-core` (B12) via the trait seam.
**Tier:** L1 (`marsad-wasl-session ← agent, core`, no `agent → core` edge — the exact structural
analogue of X1 `marsad-wasl`, and **distinct** from it).
**Full design (read first):** `../../docs/design/wasl-backend-plan.md` (v3).
**Wire spec (cross-repo, authoritative):** `/work/mewais/Wasl/docs/specs/runtime-control-api.md`.
**Vendored wire contract:** `../../fixtures/runtime-control-v1/` — the compatibility boundary is the
fixture directory, not shared code (no dependency on any Wasl crate).

## What you may edit
- **code-agent:** write ONLY `crates/marsad-wasl-session/src/**` and `crates/marsad-wasl-session/Cargo.toml`.
  Read-only: `crates/marsad-wasl-session/tests/**`, `../../fixtures/runtime-control-v1/**`, and the
  design/spec docs.
- **test-agent:** write ONLY `crates/marsad-wasl-session/tests/**`. Read-only: the spec + vendored
  fixtures.

## Module map (`src/`)
Flat `marsad_wasl_session::*` public API via `pub use` re-exports in `lib.rs`; all `mod` declarations
are private.

- **`lib.rs`** — crate doc + `mod` declarations + every `pub use` re-export.
- **`types.rs`** — the §4.4 caller-mirror domain structs/enums (`WaslCapabilities`,
  `WaslTargetStatus`, `WaslTurnReceipt`, `WaslBusyPolicy`, `WaslTurnRequest`, `WaslCausation`,
  `WaslNativeGateDetail`, `WaslTurnId`, `WaslSessionEvent`/`WaslSessionEventKind`, `WaslStreamItem`,
  `WaslTargetKey`, …). Plain, externally-tagged serde — **frozen** (used elsewhere; do NOT change
  their derives).
- **`error.rs`** — `WaslSessionError` (`Io` / `Protocol` / `Unavailable`).
- **`client.rs`** — the `WaslSessionClient` / `WaslEventStream` / `WaslSessionProvider` traits, plus
  the `Arc<T>` forwarding impl. `WaslSessionClient: Send + Debug` (the `Debug` supertrait is required
  by the frozen `real_timeout.rs`, which calls `.expect_err()` on `get_or_start`'s
  `(Box<dyn WaslSessionClient>, bool)` result).
- **`projection.rs`** — the pure, total `project_status_to_phase` (§6.3) + `status_is_live`,
  `native_gate_key` / `native_gate_command` (§8), and `new_idempotency_key` (§12 Q5). Fake-path,
  100% covered by black-box tests.
- **`fake.rs`** — `FakeWaslSessionClient` / `FakeWaslSessionProvider` (§5), the in-memory double the
  whole fake-only suite runs against. 100% covered.
- **`sse.rs`** — a FRESH SSE parser (`SseFrame` / `SseParser`) for the real client. Hand-rolled here,
  NOT imported from `marsad-wasl` (its SSE primitives are `pub(crate)`).
- **`wire.rs`** — the FRESH hand-rolled HTTP/1.1 client for the real client: `CONNECT_TIMEOUT`,
  `READ_TIMEOUT` / `MAX_READ_RETRIES` read-deadline discipline, chunked-transfer decoding
  (`ChunkedDecoder`), line splitting (`LineSplitter`), the streaming `BodyPump`, and the buffered
  (`get_buffered` / `post_json_buffered`) + streaming (`connect_sse` / `read_stream`) helpers. Mirrors
  `marsad-wasl/src/wire.rs`'s patterns/discipline; **no external HTTP crate** (no reqwest/hyper).
- **`wire_dto.rs`** — the private wire-DTO translation layer: internally-tagged snake_case DTOs
  (`#[serde(tag="type", rename_all="snake_case")]`) matching the real Wasl wire shapes, converting
  to/from the frozen public types. Mirrors how `marsad-wasl/src/wire.rs` translates between wire and
  public types. See "The wire-DTO translation layer" below.
- **`real.rs`** — `WaslSessionClientConfig`, `RealWaslSessionProvider`
  (`POST /v1/targets` + `GET /v1/targets`), `RealWaslSessionClient`
  (`status`/`prompt`/`attach`/`resolve_native_gate`/`interrupt`/`shutdown` over HTTP), and the
  SSE-backed `RealWaslEventStream`.

## The wire-DTO translation layer (`wire_dto.rs`) — the load-bearing design finding
Marsad's public types derive **plain, externally-tagged** serde, but the real Wasl runtime-control
wire is **internally-tagged snake_case** (`{"type":"started",...}`, `{"type":"idle",...}`). The public
types are frozen (used elsewhere), so `wire_dto.rs` holds a PRIVATE DTO per wire shape with the correct
internal tagging and converts to/from the public types — exactly the pattern `marsad-wasl`'s `wire.rs`
uses. Key translations that are NOT 1:1:
- **`source` / `busy_policy`** are internally-tagged objects on the wire
  (`{"type":"local_human"}`, `{"type":"queue","max_wait_ms":N}`) but the caller-mirror keeps `source`
  a plain relay `String` and `busy_policy` a typed enum; the DTO layer maps between them.
- **`TurnEvent::process_exited`** (wire) maps to `WaslSessionEventKind::SessionExited` (caller).
- **`WaslTurnRequest` is NOT the wire body.** The real `POST .../turns` body is a
  `{delivery, source:{type:local_human}, idempotency_key, busy_policy, causation}` shape
  (`submit_request.json`); `submit_request_body` builds it from `WaslTurnRequest` + the chosen busy
  policy + the target key (for `delivery.context_id`), always stamping `source: local_human` and the
  fresh single-hop causation the request already carries.
- An unrecognized wire `TargetStatus` `type` projects to the forward-compat `WaslTargetStatus::Unknown(tag)`;
  an unrecognized `turn_event` `type` is tolerantly skipped (returns `None`, never surfaced).

## Definition of done
- Every test in `tests/**` passes — BOTH the frozen fake-only suite
  (`{happy,errors,branches,fakes,projection,native_gate}.rs`) AND the real client suite
  (`real_{provider,status,prompt,control,attach,timeout}.rs` + `real_support/mod.rs`), plus the
  coverage-completion suite `wire_translation_completeness.rs` (see below).
- `cargo test -p marsad-wasl-session` fully green.
- `cargo clippy -p marsad-wasl-session --all-targets -- -D warnings` clean (see the frozen-test-file
  note below), and `cargo fmt -p marsad-wasl-session -- --check` clean on `src/**`.
- `cargo llvm-cov -p marsad-wasl-session --fail-under-lines 100 --fail-under-regions 100` passes.
- No `unsafe`.

## Coverage regime — `cfg(coverage)` residuals (read before touching `wire.rs`/`real.rs`)

Per the house `cfg(coverage)` pattern (`marsad-transport` / `marsad-wasl` precedent): production code
paths that this crate's own test suite structurally cannot exercise get a coverage-mode-only
alternative, not a waived gap. `cargo-llvm-cov` sets `cfg(coverage)`; `Cargo.toml` declares the
`check-cfg` for it.

**Unlike X1 `marsad-wasl`, the real client here is NOT `#[cfg(not(coverage))]`-gated as a whole.** The
frozen `real_*.rs` tests reference `RealWaslSessionProvider` / `RealWaslSessionClient` /
`WaslSessionClientConfig` **unconditionally**, so those symbols must exist under `cfg(coverage)` too
(gating the module out breaks compilation of the frozen tests under `llvm-cov`). The real code is
therefore **always compiled and IS counted**, and is driven to 100% by the `real_*.rs` suite — the
same way `marsad-wasl`'s real `WaslOperatorClient` is driven to 100% by that crate's own direct tests.
Only the genuinely-untestable-here IO error/retry branches carry per-site `cfg(coverage)` residuals:

1. **`wire::connect`'s `set_read_timeout`** — a just-connected socket's `set_read_timeout` has no
   test-inducible failure mode (`connect_timeout`'s own Err IS covered, via the connection-refused
   tests).
2. **`wire::parse_base_url`'s two malformed-URL error arms** — this crate's tests always pass a
   well-formed `http://ip:port` (from the mock's `base_url()` / `unused_base_url()`).
3. **`get_buffered` / `post_json_buffered` / `connect_sse`'s `parse_base_url` + request-writer `?`
   arms** — never fail against the cooperating mock. For the two buffered helpers, their own `connect`
   and `read_head` Err arms ARE covered (via the refused and wedged-peer tests). For `connect_sse`,
   its `connect` Err arm is ALSO folded: the SSE path only ever targets the cooperating mock, which
   always accepts the connection (the dead-stream attach test connects OK, then the peer closes
   mid-stream — a read-side end, not a connect-side one), so `connect`'s Err is unreachable at that
   site; the genuine connect-refused Err is proven at the buffered call sites instead. `connect_sse`'s
   `read_head` Err IS still live and covered (the dead-stream attach test drives it).
4. **`wire::read_body_buffered`'s CHUNKED branch + mid-body `Retry`/`Ended` break arms** — the
   `MockRuntimeServer` sends every buffered JSON body with `Content-Length` (never chunked) and always
   finishes it (the chunked path is exercised only by the streaming SSE `BodyPump`, a different call
   site).
5. **`wire::read_stream`'s internal `Retry` re-loop** — the mock writes-then-closes its SSE stream, so
   reads see `Data` then `Ended` with no idle timeout tick between.
6. **`ChunkedDecoder`'s terminal-0-chunk handling (+ `finished` field)** — reached only by a chunked
   buffered body (never sent) — a live SSE stream ends by TCP close, not a 0-chunk.
7. **`LineSplitter`'s CRLF-tolerance pop** and **`BodyPump`'s non-chunked branch** — this crate's SSE
   producers emit bare-`\n` lines and the mock serves SSE `Transfer-Encoding: chunked`.
8. **`real.rs`: the malformed-2xx-body / non-2xx arms of `list` / `try_prompt` / `try_status` /
   `get_or_start`, and `frame_to_item`'s malformed-payload `?` skips** — a 2xx body in this crate's
   tests always parses to the expected shape (the malformed-BODY disposition is proven for the sibling
   `list_malformed_body_is_protocol_error`; the transport-failure disposition is proven for the
   connection-refused/wedged tests; `wire_dto`'s own `None` cases are proven by
   `wire_translation_completeness.rs`).

Each is handled with a `#[cfg(not(coverage))] … #[cfg(coverage)] …` pair at the call site: the real
build keeps full `Result`/`?` propagation (or the real chunked/CRLF handling), and the coverage build
folds the untested-here branch into a **std-internal** panic (`assume_ok` / `Result::expect`) or drops
the unreachable-under-coverage handling, so 100% reflects this crate's genuinely-tested surface. Every
one is commented in place with its specific reason and points back here.

**If you add a new IO call site:** first check whether a test can actually drive its failure path
before reaching for `cfg(coverage)`. Only use the residual pattern when the *frozen* test file
genuinely cannot (not "doesn't currently") exercise it — the code-agent cannot edit the frozen tests.

### `tests/wire_translation_completeness.rs` — the coverage-completion suite (spec-derived, test-agent-authored)
The frozen black-box `real_*.rs` suite deliberately exercises **one shipped fixture per wire shape**,
so it does not drive the remaining closed-set wire variants that ship no golden fixture (the extra
`TargetStatus` kinds offline/starting/interrupting/draining/stopped + the `Unknown` fallback; the
extra `TurnEvent` kinds turn_queued/tool_finished/native_gate_resolved/turn_cancelled/turn_expired/
status_changed; the `agent` turn source; the `queue_related` busy policy; the `superseded` cancel
reason; an unmodelled SSE field; a non-JSON error body; and a deterministic SSE connect-refused).
`wire_translation_completeness.rs` drives exactly those — still **black-box, through
`RealWaslSessionClient` + the mock** — so the always-compiled real path reaches 100% without gating
real business logic. Like every other test in `tests/**`, it is authored by the test-agent from the
authoritative wire shapes in `runtime-control-api.md` and is read-only to the code-agent; it
complements, never replaces, the frozen contract suite. (This is why `src/` carries NO in-module
`#[cfg(test)]` unit
tests — an in-`src` test's own `assert`/`unwrap` panic arms would themselves count as uncovered
regions against the `src` file; keeping coverage-completion assertions in a `tests/` file, whose
regions are excluded from the `llvm-cov` report, avoids that.)

## Known clippy findings in the frozen test files (not this crate's `src/`)
The frozen `tests/real_control.rs` module-doc (its lead-in list) trips `clippy::doc_lazy_continuation`
under `cargo clippy --all-targets -- -D warnings`, through no fault of `src/`. `Cargo.toml` therefore
carries a crate-wide `[lints.clippy] doc_lazy_continuation = "allow"` (a pure doc-formatting lint that
`src/**` never triggers) so `--all-targets` stays green without masking any real finding. `cargo clippy
--lib` and `cargo fmt --check` on `src/**` are clean.

## Rules
- Tests are written BEFORE code by the test-agent from the spec, and are read-only to the code-agent.
  Do not edit tests to pass. This includes the coverage-completion suite
  `tests/wire_translation_completeness.rs` — it too is spec-derived, test-agent-authored, and
  read-only to the code-agent. The code-agent closes coverage by editing `src/**` (see the
  `cfg(coverage)` residual regime above), never by writing or shaping a test. `#[cfg(not(coverage))]`
  residuals are reserved strictly for genuinely-untestable real-IO paths (mirroring
  `crates/marsad-wasl/AGENTS.md`'s convention) — never as a substitute for a missing test.
- Do NOT change the frozen public types' serde derives (`types.rs`) — they are used elsewhere. Wire
  differences live entirely in the private `wire_dto.rs` layer.
- Keep within the dependency DAG declared in `Cargo.toml` (`serde`, `serde_json`, `thiserror`,
  `marsad-events` only — no other Marsad crate, no Wasl crate, no HTTP crate). Do not add new deps
  without reflecting them in `ARCHITECTURE.md`.
