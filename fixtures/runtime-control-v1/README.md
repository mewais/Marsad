# Vendored Wasl runtime-control-API v1 golden fixtures

Origin: vendored VERBATIM from the Wasl repo
(crates/wasl-runtime-control/fixtures/runtime-control-v1/ at commit 4a4d0b7;
the fixtures themselves last changed at Wasl commit 92dffde,
"feat(runtime): live-runtime Phase 3.5 — real R3/R4: control plane + wasl-session").
These files are the canonical v1 wire contract per Wasl's
docs/specs/runtime-control-api.md. Append-only for the life of v1 — never edit;
re-vendor from upstream if v1 gains additive fixtures. Marsad's conformance
tests (marsad-wasl-session, design docs/design/wasl-backend-plan.md §3/§10.1)
assert the fake and the real RealWaslSessionClient both satisfy these copies.

This is the session-control analogue of ../operator-v1/ (the send-side operator
gates X1 vendors): the compatibility boundary is this fixture directory, not
shared code — marsad-wasl-session depends on NO Wasl crate (Wasl ships no client
crate for the control surface; the real client is hand-rolled HTTP/1.1 over
std::net against the fixture-pinned wire spec).

27 files (the exact set present upstream at the vendored commit):

| file | is |
|---|---|
| `info.json` | `GET /v1/info` body |
| `targets_snapshot.json` | `GET /v1/targets` body (one target) |
| `target.json` | `GET /v1/targets/{id}` + `POST /v1/targets` body |
| `create_request.json` | `POST /v1/targets` request body |
| `hello.json` | SSE `hello` frame payload |
| `turn_event_session_ready.json` | SSE `turn_event`, session_ready |
| `turn_event_turn_accepted.json` | SSE `turn_event`, turn_accepted (carries source) |
| `turn_event_assistant_delta.json` | SSE `turn_event`, assistant_delta |
| `turn_event_tool_started.json` | SSE `turn_event`, tool_started |
| `turn_event_native_gate_requested.json` | SSE `turn_event`, native_gate_requested |
| `turn_event_turn_completed.json` | SSE `turn_event`, turn_completed (carries certainty) |
| `turn_event_turn_failed.json` | SSE `turn_event`, turn_failed (carries certainty) |
| `turn_event_process_exited.json` | SSE `turn_event`, process_exited |
| `truncated.json` | SSE `truncated` frame payload |
| `submit_request.json` | `POST /v1/targets/{id}/turns` request body (source local_human, with causation) |
| `receipt_started.json` | `TurnReceipt` started |
| `receipt_queued.json` | `TurnReceipt` queued |
| `receipt_rejected_busy.json` | `TurnReceipt` rejected_busy |
| `receipt_duplicate.json` | `TurnReceipt` duplicate (wraps original) |
| `cancel_request.json` | `POST /v1/targets/{id}/cancel` request body |
| `gate_request_approve.json` | `POST /v1/targets/{id}/gate` request body, approve |
| `gate_request_deny.json` | `POST /v1/targets/{id}/gate` request body, deny + reason |
| `shutdown_request_drain.json` | `POST /v1/targets/{id}/shutdown` request body |
| `error_unknown_target.json` | 404 error body |
| `error_admission_refused.json` | 429 error body |
| `error_unsupported.json` | 409 error body (cancel when !interruptible) |
| `error_malformed.json` | 400 error body |

## Provenance notes (fixtures vs. the spec doc table)

- The spec doc's fixture table (`Wasl/docs/specs/runtime-control-api.md`)
  enumerates the SAME 27 files. There is no `receipt_rejected_unavailable.json`
  and no `turn_event_*` fixtures for `turn_queued`/`tool_finished`/
  `native_gate_resolved`/`turn_cancelled`/`turn_expired`/`status_changed`
  upstream — those variants exist in the wire vocabulary but ship no golden
  fixture. Marsad's real-client tests therefore exercise the shipped fixtures
  verbatim and construct the un-fixtured variants inline (documented in the
  test files) rather than inventing new fixture files here (append-only policy).
- All enum bodies are internally-tagged snake_case (`{"type":"started",...}`),
  matching the real Wasl serde form. Marsad's caller-mirror types
  (`WaslTurnReceipt`/`WaslTargetStatus`/`WaslSessionEventKind`) are
  externally-tagged by derive, so the real client maps wire↔public via a
  private wire-DTO layer — these fixtures are the machine-checkable pin for that
  mapping.
