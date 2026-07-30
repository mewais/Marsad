# Wasl Backend — a Wasl-managed session runtime owner for Marsad (design plan)

**Status:** ✅ **Shipped.** The full feature — the trait/type contracts, the `FakeWaslSessionClient`,
the **real HTTP/SSE client**, and the **production wiring** (including the per-session observe thread) —
is implemented, tested (**100% line+region coverage on `marsad-wasl-session`**), and merged. This
document is retained as the **design record**: the sections below are the design-time contract that
was turned into per-crate specs (`docs/specs/*.md`) and code, **test-first**. Two post-v3 corrections
made during implementation are captured in the [Shipped implementation notes](#shipped-implementation-notes)
section so a future reader is not misled by any remaining v3-era planning language elsewhere in the doc.

**Revision log — v3** (revised after a rigorous comparison against the **real, shipped Wasl
implementation** at `/work/mewais/Wasl` — the earlier v1/v2 contract was written against an *assumed*
Wasl surface that has since been superseded by Wasl's actual code. All ten changes below are
**factual corrections against verified real-Wasl behavior**, not open design debates; each was
confirmed by reading `crates/wasl-runtime/src/lib.rs`, `crates/wasl-runtime-control/`, and the pinned
fixtures at `crates/wasl-runtime-control/fixtures/runtime-control-v1/`). Changes from v2:
1. **Native-gate identity is `(turn_id, seq)`, not a fictional `gate_id`.** Real `NativeGateDetail`
   is only `{summary, preview}` — no `gate_id`, no `tool`. Per-gate uniqueness comes from the event's
   own stream `seq` combined with `turn_id` (Wasl does *not* prevent two native gates per turn). §3, §4.4, §8.
2. **Reconnect compares `TargetSnapshot.last_seq`, not a fictional `created: bool`.** No `created` flag
   exists in real Wasl (Rust `target_for` returns `Arc<dyn DeliveryTarget>`; wire always returns a
   `TargetSnapshot`). Fresh-vs-resumed is decided by `last_seq < persisted seq ⇒ reset to 0`. §4.3, §6.1.
3. **`WaslTurnReceipt` gains the real `Duplicate { original: Box<WaslTurnReceipt> }` variant** (a resent
   idempotency key returns the wrapped original, no double-inject). §4.4.
4. **`Interrupting`/`Draining` carry `{active_turn, since}`** (already correct in v2; re-confirmed against real source). §3, §4.4.
5. **`WaslSessionEventKind` matches the real, richer `TurnEvent`**: added `TurnQueued`,
   `NativeGateResolved`, `TurnCancelled`, `TurnExpired`, `StatusChanged`; every kind gained `turn_id`;
   `TurnCompleted`/`TurnFailed` gained `certainty`. Marsad's synthetic `ObserveLost`/`Unknown` kept. §4.4.
6. **`StreamItem::Truncated{earliest, latest}` is now a distinct stream item**, not a `TurnEvent` —
   `WaslEventStream::next()` returns `WaslStreamItem { Event | Truncated }` so a retention-window
   truncation is an unmissable signal (re-snapshot + resubscribe from `latest`). §4.4.
7. **`WaslTurnRequest` now carries a `WaslCausation`** (`root_request, parent_request, path, hop`);
   Marsad always originates a fresh single-hop causation (fresh `root_request`, `parent_request: null`,
   empty `path`, `hop: 0`) since it only ever sends `LocalHuman` turns. §4.4.
8. **Transport is HTTP/TCP-only** (real control server uses `tokio::net::TcpListener`; no `UnixListener`
   anywhere). Marsad's real client targets HTTP; the "UDS vs HTTP undecided" framing is dropped. §2, §3, §7.
9. **No Wasl client crate ships** — Marsad hand-rolls an HTTP client against the fixture-pinned wire spec
   (`docs/specs/runtime-control-api.md`) and vendors fixtures from `runtime-control-v1/` (corrected from
   the earlier wrong `wasl-session-v1/` guess). §3.
10. **Notes-only (no action):** real Wasl added `answer_tool_call`/`ToolCallAnswerer` (mid-turn tool
    callback) with no Marsad analogue — a known future gap; and `interruptible`/`streaming_output` are
    genuinely `false` (not "unknown") for all shipped drivers (claude, opencode) today. §3, §11.

**Revision log — v2** (revised after an independent architecture review). Changes from v1:
1. **Idempotency key** is now a **UUID (v4/v7) minted once per human send action**, not a per-process
   counter (a counter can collide after a Marsad-core restart while the Wasl session survives). §4.4, §12 Q5.
2. **Native gate maps to the existing `Gate::Permission` variant**, not `Gate::Handoff` — a native CLI
   permission prompt is a *permission* gate (same domain as claude's and opencode's own tool-use prompts),
   not an agent-to-agent handoff. Reuses the existing gate UI/pipeline. §2, §6.3, §8.
3. **The Wasl session binding is now persisted** (`WaslSessionBinding`), so core/GUI can reconnect
   (re-`attach(after_seq)` + re-`status()`) to a still-alive Wasl session after a restart, rather than
   losing it; provider gains an idempotent `get_or_start`. §6.1.
4. **`RuntimeOwner` branching is centralized** in dedicated single-match routing methods
   (`send_session_prompt`/`attach_session_events`/`interrupt_session`/`stop_session`), not inlined per
   call site. §6.1.
5. **Lock hygiene is a numbered test requirement**: `marsad-agent` must never hold a registry/map/subscriber
   lock across a blocking `WaslSessionClient` call. §8, §10.3.
6. **Event/transcript-folding edge cases** are now explicit test scenarios (dedup, queued-vs-accepted,
   delta-append, single terminal event, post-terminal delta ignored). §10.4.
7. **Terminal-tab capability gap** noted: the raw PTY Terminal tab cannot attach to a `WaslManaged`
   session; flag to hide/disable it in the later UI phase. §11.

**Scope:** add a new **runtime-owner** kind to Marsad — a session whose underlying CLI process
Marsad does **NOT** launch or own. Instead Marsad is a **thin client** of a **Wasl-owned** live
session (Wasl's "Model A" managed runtime), prompting and observing it through Wasl's local
**session control API**. This is architecturally distinct from every session Marsad has today
(claude / codex / opencode under a tmux/ConPTY container that `marsad-agent` owns) and distinct from
the existing X1–X5 Wasl **operator** integration (which relays send-side Tier-3 gates + audit for a
session **Marsad still owns**). Here, the session belongs to Wasl end to end; Marsad only prompts,
observes, and surfaces its native gates.

**Hard constraint (locked):** Marsad's side is designed against the **real, shipped Wasl
implementation** at `/work/mewais/Wasl` — specifically the frozen type surface in
`crates/wasl-runtime/src/lib.rs` (the `DeliveryTarget` seam, `TargetStatus`, `TurnReceipt`,
`RuntimeCapabilities`, the public `TurnEvent` vocabulary, `NativeGateDetail`, `Causation`), the wire
contract in `crates/wasl-runtime-control/` (`TargetSnapshot`, `TruncatedFrame`, the SSE framing), and
the pinned golden fixtures at `crates/wasl-runtime-control/fixtures/runtime-control-v1/`. The transport
is **HTTP/TCP** (the real control server binds a `tokio::net::TcpListener`; there is **no** `UnixListener`
anywhere in the Wasl repo — the earlier "UDS vs loopback-HTTP undecided" framing was wrong and is
dropped). Wasl ships **no client crate**, so Marsad's real client is **hand-rolled** against the wire
spec + vendored fixtures (§3). Marsad is built and tested against a FAKE implementation of
this contract — mirroring how `crates/marsad-wasl` already ships `FakeWaslConnector` alongside the
real `WaslOperatorClient`. **[Shipped update — see [Shipped implementation notes](#shipped-implementation-notes):
the real HTTP/SSE client shipped as part of this same effort, not a later phase.]** It is swapped in
behind the transport-agnostic trait with **zero changes** to Marsad's internal abstraction, exactly as
this contract promised; the fake remains the substrate for the pure-logic tests.

**Relationship to `ARCHITECTURE.md`:** this feature is additive to the existing block map. It does
not modify any existing crate's contract. It introduces one new crate — provisionally
`marsad-wasl-session` (L1) — and additive-only edits to `marsad-proto` (new `IpcMsg` family),
`marsad-agent` (host the client), `marsad-core` (the `RuntimeOwner` distinction + status projection)
and **no change to** `marsad-gates` (native gates reuse the existing `Gate::Permission` variant — the
same gate a plain CLI permission prompt already produces). The block-map
delta is proposed in §9 for a later, separate `ARCHITECTURE.md` edit once specs land.

---

## Shipped implementation notes

The feature shipped ✅ as designed below, with two **post-v3 corrections** that surfaced during
implementation. They are recorded here for historical accuracy so a future reader is not confused by
any remaining v3-era planning language elsewhere in this document (the design sections are kept intact
as the design record; where they conflict with shipped reality, **this section is authoritative**).

1. **`WaslSessionProvider::get_or_start` returns a `WaslTargetSnapshot`, not a `created: bool`.** The
   v3 plan had already dropped the fictional `created` flag in favor of comparing a snapshot's
   `last_seq` (§4.3, §6.1); implementation confirmed and finalized that shape. `get_or_start` returns a
   `WaslTargetSnapshot { target_id, key, capabilities, status, last_seq, queued, native_session_id }`,
   and reconnect logic (`wasl_reconnect_after_seq` in `marsad-agent`) decides fresh-vs-resumed by
   comparing the snapshot's `last_seq` against the persisted `WaslSessionBinding.last_seq`: `snapshot.last_seq
   >= persisted` resumes with `attach(persisted)`; `snapshot.last_seq < persisted` (target replaced,
   journal restarted) resets local seq tracking to `0` and re-attaches from `0`. There is no `created`
   boolean anywhere in the shipped code.

2. **A per-session observe thread was added to `marsad-agent`'s `serve()`.** The design (§6.1) called
   for it; the shipped wiring spawns it on `LaunchWaslSession` success. The thread pumps `attach()` via
   the pure, unit-tested helpers `pump_wasl_observe` / `wasl_observe_should_reconnect` (in
   `crates/marsad-agent/src/wasl_session.rs`). It handles `WaslStreamItem::Truncated` **correctly** by
   re-attaching from `latest` (discarding the stale stream, then continuing to deliver events above the
   truncation floor) rather than treating it as an ordinary event, and it reuses `reader.rs`'s existing
   reconnect-backoff constants (`RECONNECT_INITIAL_BACKOFF_SECS`, `RECONNECT_MAX_BACKOFF_SECS`,
   `RECONNECT_PROBE_TIMEOUT_SECS`) for genuine disconnects. It **never** reconnects after a terminal
   `WaslSessionEventKind::SessionExited` — a stream that ended after a terminal exit means the session
   is truly gone (`wasl_observe_should_reconnect` returns `false`), so the thread exits cleanly instead
   of spinning on a dead target.

---

## Table of contents

1. [Context / why this is a new runtime owner](#1-context--why-this-is-a-new-runtime-owner)
2. [Scope for this first phase (explicit boundary)](#2-scope-for-this-first-phase-explicit-boundary)
3. [The frozen Wasl contract Marsad integrates against](#3-the-frozen-wasl-contract-marsad-integrates-against)
4. [The new Rust trait(s): a Wasl session control client](#4-the-new-rust-traits-a-wasl-session-control-client)
5. [`FakeWaslSessionClient` — the in-memory test double](#5-fakewaslsessionclient--the-in-memory-test-double)
6. [Integration into Marsad's session / runtime-owner model](#6-integration-into-marsads-session--runtime-owner-model)
7. [IPC / API surface (agent ⇄ core)](#7-ipc--api-surface-agent--core)
8. [Native-gate resolution wiring (reuse `marsad-gates`)](#8-native-gate-resolution-wiring-reuse-marsad-gates)
9. [Proposed block-map delta (not applied here)](#9-proposed-block-map-delta-not-applied-here)
10. [Test plan for this phase](#10-test-plan-for-this-phase)
11. [Non-goals for this phase](#11-non-goals-for-this-phase)
12. [Open design questions (decide before implementation)](#12-open-design-questions-decide-before-implementation)

---

## 1. Context / why this is a new runtime owner

Marsad's entire session model today rests on one invariant, stated in `ARCHITECTURE.md`:

> **Session liveness is independent of the GUI.** A detached supervisor (`marsad-agent`) **owns
> every** `claude` **process** so sessions survive GUI exit.

`marsad-agent` (B13) launches each session inside a `marsad-lifecycle::Container` (tmux / ConPTY),
holds the process, drives it via stream-json + hooks + the control protocol (`driver.rs`,
`reader.rs`), and serves IPC to one or more cores. `marsad-core` (B12) is the **client** of that
agent — it never spawns a CLI itself; it sends launch/resume/attach/stop/prompt/gate over
`marsad-transport` and folds the agent's stream back into `SessionState` via `marsad-events`.

There are already **two** kinds of Wasl-touching code in the tree, and it is essential not to
conflate this new work with either:

- **X1 `marsad-wasl` (operator connector).** An HTTP+SSE consumer of Wasl's **Operator API** that
  renders Wasl's own **send-side Tier-3 gates** and audit feed for a session **Marsad owns**
  (`WaslConnector`/`FakeWaslConnector`, `WaslOperatorClient`). The underlying CLI is still a
  Marsad-launched claude process; Wasl is a *peer supervisor* observing sends.
- **X4a launch-time attach.** A `LaunchSpec.wasl: bool` toggle that injects a `--mcp-config` so a
  **Marsad-owned** claude session gets Wasl's four A2A tools as an MCP server. Still Marsad's
  process.

**This new "Wasl Backend" is neither.** Here the session's process is spawned, owned, and driven by
**Wasl** (Wasl's Model A "managed runtime"): Wasl holds the sole input handle; Marsad can only
**prompt** it (`send` → a `TurnRequest{source: LocalHuman}`), **observe** it (`attach` →
`events(after_seq)`), **resolve its native gates** (`gate` → approve/deny the CLI's own permission
prompt), **interrupt**, and **stop** it. Marsad is a **caller of Wasl's `DeliveryTarget`**, not a
process owner. This is a genuinely new **runtime owner** and warrants a first-class distinction in
the model (§6), not a special case bolted onto the container-owning path.

Why this cannot be a flag on the existing path: the container-owning code (`driver.rs`, `reader.rs`,
`remote.rs`, `spawn_container`/`kill_container`, the tmux/ConPTY `Container`, `--resume` continuity,
the hook endpoint) is **all** predicated on Marsad holding the process. A Wasl-managed session has
**none** of it — no container to spawn, no argv to build, no stdin to write, no hook callback, no
`claude_argv`. Forcing it through the same seam would grow that path a pile of
`if wasl_managed { … }` branches — exactly the "weak, partially-implemented interface where every
caller must defensively probe what actually works" that Wasl's own plan (§3) forbids. A separate,
cleanly-typed client seam keeps both paths fully-implemented.

---

## 2. Scope for this first phase (explicit boundary)

This phase builds, **and proves entirely without any real Wasl process**:

1. A **transport-agnostic client trait** (`WaslSessionClient`, §4) representing Marsad's caller-side
   view of Wasl's `DeliveryTarget` control surface, living in a new L1 crate `marsad-wasl-session`.
2. A **`FakeWaslSessionClient`** (§5) implementing that trait fully in-memory — scriptable to
   simulate Idle/Busy/AwaitingGate transitions, queued turns, a native gate needing approval, and a
   crash/Offline/Faulted transition. **This is what the entire phase's test suite runs against.**
3. A **`RuntimeOwner` distinction** (`MarsadManaged` vs `WaslManaged`, §6) at the `marsad-core`
   layer, plus a pure **status-mapping layer** that projects `TargetStatus` → Marsad's existing
   `Phase` for UI compatibility, **without** adding any Wasl-specific variant to `Phase`.
4. A **new additive `IpcMsg` family** (§7) so `marsad-core` can drive a Wasl-backed session through
   `marsad-agent` (launch/prompt/attach/observe/gate/interrupt/stop), designed so the fake today and
   a real HTTP client later swap in **behind the same IPC** with zero IPC changes.
5. **Native-gate resolution** wired into Marsad's **existing** `marsad-gates` pipeline via the
   already-defined `Gate::Permission` variant (§8) — the same variant a plain CLI permission prompt
   already uses (claude's `can_use_tool`, opencode's ACP `session/request_permission`). Marsad's own
   gate UI is the resolution mechanism; no separate Wasl-specific gate UI, no new gate type.

**Explicit non-goal boundary (stated the way Wasl's own plan does, §11 in full).** This phase does
**NOT** integrate against a real Wasl process, does **NOT** implement the HTTP wire (that is the later
real-client phase — the transport itself is settled as HTTP/TCP, §3, but building the client is out of
scope here), does **NOT** build UI beyond the minimum needed to prove the backend works, does **NOT**
support cross-host Wasl backends (local-first), and does **NOT** add any A2A-triggered session
lifecycle. Real-Wasl integration is a distinct, later phase — and by construction (the trait boundary
of §4) it requires **no change** to anything built here except adding one new trait impl behind the
factory seam.

**Load-bearing optionality (mirrors X1 hard rule 1).** With no Wasl-backend session ever launched,
Marsad's behavior is bit-for-bit unchanged: no new threads, no new connections, no new `IpcMsg`
traffic, no new `VmUpdate`s, and every existing test passes untouched. The `RuntimeOwner` of every
existing session is `MarsadManaged`, which routes to exactly today's code.

---

## 3. The frozen Wasl contract Marsad integrates against

**Authoritative source (v3): the REAL, shipped Wasl implementation at `/work/mewais/Wasl`**, not the
earlier planning document. The v1/v2 versions of this plan were written against an *assumed* Wasl
contract; that contract has since been superseded by Wasl's actual code, and every type/shape below is
now grounded in — and must be verified against — these exact files:

- **`crates/wasl-runtime/src/lib.rs`** — the frozen, object-safe type surface: the `DeliveryTarget`
  trait and its supertraits `NativeGateResolver`/`ToolCallAnswerer`, `RuntimeProvider`,
  `RuntimeCapabilities`, `TargetStatus`, `TurnReceipt`, `BusyPolicy`, `TurnRequest`/`TurnSource`,
  `Causation`, the public `TurnEvent` vocabulary, `StreamItem`, `NativeGateDetail`, the identifier
  newtypes (`TargetId`, `TargetKey`, `TurnId`, `EventSeq`, `IdempotencyKey`), and `RuntimeError`.
- **`crates/wasl-runtime-control/src/wire.rs`** — the HTTP wire DTOs: `TargetSnapshot`,
  `TargetsSnapshotResponse`, `CreateTargetRequest`, `HelloFrame`, `TruncatedFrame`,
  `SubmitTurnRequest`, `CancelRequest`, `GateRequest`, `ShutdownRequest`, `ErrorBody`, `InfoResponse`.
- **`crates/wasl-runtime-control/src/lib.rs`** — the control server: HTTP/TCP (`tokio::net::TcpListener`),
  SSE framing including the `"truncated"` SSE event name.
- **`crates/wasl-runtime-protocol`** (`wasl_proto`) — the shared vocabulary re-used verbatim:
  `GateDecision`, `DeliveryCertainty`, `AgentId`, `RequestId`.
- **`crates/wasl-runtime/src/journal.rs`** — the event-journal retention model that produces
  `StreamItem::Truncated` when a replay cursor falls below the retained floor (default ring capacity
  **4096** events, `crates/wasl-runtime/src/config.rs`).
- **The pinned golden fixtures at `crates/wasl-runtime-control/fixtures/runtime-control-v1/`** — the
  executable form of the wire contract (`target.json`, `truncated.json`, `submit_request.json`,
  `receipt_*.json`, `turn_event_*.json`, `gate_request_*.json`, `hello.json`, `info.json`, …).

Future readers: verify any fact below by reading these live sources — do not trust this summary
blindly. Marsad depends on **only** the surface enumerated here and treats everything else (Wasl's
internal actor, FIFO queue, journal internals, `InteractiveProtocol`/`InteractiveTransport` drivers,
`ProtocolEvent`, idempotency map, `DeliveryCertainty` mechanics) as Wasl-internal and invisible.

- **The `DeliveryTarget` surface** (`wasl-runtime/src/lib.rs` §3) — the seam Marsad calls, one live
  session per target: `target_id() -> &TargetId`, `capabilities() -> RuntimeCapabilities`, `status()`
  (**advisory only** — never gate on it), `submit_turn(req, busy_policy) -> TurnReceipt`,
  `events(after_seq) -> TurnEventStream` (replayable/sequenced, yields `StreamItem`),
  `cancel_turn(turn_id, reason)`, `shutdown(mode)`. Native-gate resolution is the supertrait
  `NativeGateResolver::resolve_native_gate(turn_id, decision)` — **keyed by `TurnId`, not a gate id**
  (there is no gate id; see §8).
- **`TurnReceipt`** (real enum, `lib.rs` §3): `Started{turn_id, first_seq}` | `Queued{turn_id, position}`
  | `RejectedBusy{active_turn}` | `RejectedUnavailable{status}` | **`Duplicate{original: Box<TurnReceipt>}`**.
  The last variant is returned verbatim when a resend of an already-seen `(TargetKey, IdempotencyKey)`
  arrives — **no second injection** occurs (fixture `receipt_duplicate.json`). `submit_turn` is the
  **atomic admission decision** — it does not race with `status()`.
- **`TargetStatus`** (real enum, `lib.rs` §4.1): `Offline` | `Starting` | `Idle{since}` |
  `Busy{active_turn, source, phase, since, queued}` | `AwaitingGate{active_turn, detail, since, queued}`
  | **`Interrupting{active_turn, since}`** | **`Draining{active_turn, since}`** | `Faulted{error, recoverable}`
  | `Stopped`. `Interrupting`/`Draining` are **NOT** bare unit variants — they carry `{active_turn, since}`.
  `AwaitingGate` counts as **busy**. Wire tag is snake_case internally-tagged (`{"type":"idle","since":…}`).
- **`BusyPolicy`** (real enum, `lib.rs` §4.2): `Queue{max_wait}` | `Reject` | `QueueRelated{max_wait}`,
  durations as integer `max_wait_ms` on the wire.
- **Turn source & `Causation`** — `send` is a `TurnRequest{ delivery, source: LocalHuman,
  idempotency_key, causation }` (real `TurnRequest`, `lib.rs` §3.1) routed through the same
  single-writer actor as any A2A turn. Marsad only ever originates `LocalHuman` turns and always
  supplies a **fresh single-hop `Causation`**: `{root_request: <fresh RequestId>, parent_request: null,
  path: [], hop: 0}` — exactly the shape the `submit_request.json` fixture shows for a fresh non-chained
  turn. Marsad never participates in Wasl's cycle-rejection / `QueueRelated` relatedness beyond
  originating root turns (§4.4, §11).
- **`attach` = observe-only** via `events(after_seq)` (`lib.rs` §3). The stream yields **`StreamItem`**,
  which is **either** `Event{seq, event: TurnEvent}` **or** `Truncated{earliest, latest}` — the latter
  is a control message (not a `TurnEvent`) signaling the replay cursor fell below the retained ring
  floor; the caller must re-fetch a snapshot and resubscribe from `latest` (`journal.rs`; SSE event
  name `"truncated"`, `runtime-control/src/lib.rs`; fixture `truncated.json`).
- **The public `TurnEvent` vocabulary** (real enum, `lib.rs` §5) — richer than v2 assumed:
  `SessionReady{native}` | `TurnAccepted{turn_id, source}` | `TurnQueued{turn_id, position}` |
  `AssistantDelta{turn_id, text}` | `ToolStarted{turn_id, name}` | `ToolFinished{turn_id, name, ok}` |
  `NativeGateRequested{turn_id, detail}` | `NativeGateResolved{turn_id, decision}` |
  `TurnCompleted{turn_id, reply, certainty}` | `TurnFailed{turn_id, error, certainty}` |
  `TurnCancelled{turn_id, reason}` | `TurnExpired{turn_id}` | `StatusChanged{status}` |
  `ProcessExited{code}`. **Every** event correlates by `turn_id` (Wasl's rule: "correlate by `TurnId`,
  NEVER by `EventSeq`"); `TurnCompleted`/`TurnFailed` carry a `DeliveryCertainty`.
- **`gate` = native approve/deny** for a `TargetStatus::AwaitingGate` — the CLI's **own** permission
  prompt. The real `NativeGateDetail` is **only `{summary, preview}`** (`lib.rs` §4.1) — it has **no
  `gate_id` and no `tool` field**. Per-gate identity comes from the sibling `turn_id` on the
  `TurnEvent::NativeGateRequested{turn_id, detail}` / `TargetStatus::AwaitingGate{active_turn, detail, …}`,
  combined with the event's stream `seq` (see §8; resolution is `resolve_native_gate(turn_id, decision)`).
  This native gate is **distinct** from Wasl's send-side three-tier operator gate (the X1 gate) and
  does not merge with it.
- **`RuntimeCapabilities`** (`lib.rs` §2): the eight independent booleans (`persistent_process`,
  `persistent_conversation`, `structured_turns`, `streaming_output`, `attachable`, `interruptible`,
  `structured_provenance`, `resumable`). **Marsad treats these as purely informational** — never
  replicates any, never assumes one implies another. **Real-driver expectation (v3 note):** for every
  shipped driver today (claude, opencode), `interruptible` and `streaming_output` are genuinely
  **`false`** — not "unknown" — so Marsad's UI affordance logic should expect interrupt/streaming
  affordances **hidden** for now (this is an expectation, not a design change; e.g. `target.json` shows
  `interruptible:false, streaming_output:false`). Marsad may *display* the caps and may *decline* an
  operation the target doesn't claim, but implements no capability itself.
- **The wire endpoints** the real control server exposes over HTTP/TCP
  (`runtime-control/src/lib.rs`, spec `Wasl/docs/specs/runtime-control-api.md`): `GET /v1/info`,
  `POST /v1/targets` (create-or-reuse; **always** returns a `TargetSnapshot`), `GET /v1/targets`
  (list → `TargetsSnapshotResponse`), the per-target submit/attach(SSE)/cancel/gate/shutdown routes.
  Marsad's trait (§4) is the caller-side mirror of exactly these.

**Transport is settled: HTTP/TCP.** Despite Wasl's *prose* spec historically saying "UDS on unix,
HTTP elsewhere," the shipped control server implements **only** TCP/HTTP — `runtime-control/src/lib.rs`
binds a `tokio::net::TcpListener` and there is **no `UnixListener` anywhere in the Wasl repo**. Marsad's
real client therefore targets **HTTP**. The `WaslSessionClient` trait stays transport-abstracted (§4)
only so the fake and the real HTTP client swap cleanly — not because the transport is undecided.

**No Wasl client crate — hand-roll + vendored fixtures.** Wasl ships **no** client crate for the
control surface, so Marsad must **not** depend on a Wasl crate and instead **hand-rolls its own HTTP
client** against the documented, fixture-pinned wire spec (`Wasl/docs/specs/runtime-control-api.md`).
Following X1's vendored-fixture policy, this phase **vendors the real fixtures** from
`Wasl/crates/wasl-runtime-control/fixtures/runtime-control-v1/` into
**`fixtures/runtime-control-v1/`** (corrected from the earlier wrong `fixtures/wasl-session-v1/`
guess), and a conformance test asserts the fake and the real client both satisfy them. The trait's
rustdoc pins each method to the exact real-Wasl type/file it mirrors.

> **[Shipped]** The hand-rolled real HTTP/SSE client **shipped as part of this same effort** (it was
> *not* deferred to a later phase, contrary to the planning language above). It lives in
> `crates/marsad-wasl-session/src/{real,wire,wire_dto,sse}.rs` — `real.rs` is the `WaslSessionClient`/
> `WaslSessionProvider` HTTP implementation, `wire.rs`/`wire_dto.rs` are the request/response DTOs and
> their translation to/from the caller-side domain types, and `sse.rs` is the Server-Sent-Events attach
> stream parser (including the `"truncated"` event). It is driven to **100% line+region coverage** via
> `tests/real_*.rs` (`real_control.rs`, `real_prompt.rs`, `real_attach.rs`, `real_status.rs`,
> `real_provider.rs`, `real_timeout.rs`) plus `tests/wire_translation_completeness.rs`, all running
> against the vendored golden fixtures at `fixtures/runtime-control-v1/` — never against a live Wasl
> process. The fake still backs the pure-logic and projection tests.

---

## 4. The new Rust trait(s): a Wasl session control client

### 4.1 Where it lives — a new crate `marsad-wasl-session` (L1)

**Decision: a new crate, not inside `marsad-agent`.** Justification against the existing DAG
(`ARCHITECTURE.md`: `proto,events,transport,lifecycle,store,hosts,notify ← agent`;
`everything ← core`; `wasl ← agent,core`; **no `agent → core` edge**):

- It **mirrors the X1 precedent exactly.** `marsad-wasl` is an L1 crate sitting beside
  `marsad-transport`, depending on no other Marsad crate, consumed by `agent` and `core` via a
  trait + fake. `marsad-wasl-session` is the direct structural analogue for the *control* surface —
  keeping the two Wasl concerns (operator gates vs managed sessions) in **separate** crates so
  neither's coverage or dependency set contaminates the other.
- **Putting it inside `marsad-agent` would violate the DAG's spirit.** The fake and the trait must
  be reachable by **tests in both `agent` and `core`** (core needs the status-projection types;
  agent needs the client seam). If they lived in `agent`, `core` could not name them without an
  `agent → core`-adjacent edge or a duplicate — and `agent`'s coverage regime (`serve.rs` is
  `cfg(coverage)`-excluded) is a poor host for a pure, 100%-coverable trait+fake.
- **New DAG edges (acyclic):** `marsad-wasl-session ← agent, core`, identical in shape to
  `wasl ← agent, core`. `marsad-wasl-session` depends only on `serde`, `serde_json`, `thiserror`,
  plus **`marsad-proto`** for the shared status/receipt wire enums **if and only if** they must be
  wire-shared (see §7 — the preference is that the caller-facing domain types live in
  `marsad-wasl-session` and `marsad-proto` carries only `serde_json::Value`/string payloads, exactly
  as `marsad-proto` carries `WaslObservation.entry` as `Value` rather than depending on
  `marsad-wasl`). No `agent → core` edge is introduced.

The crate is a **pure, mockable-trait L1** (Tier L1), 100% line+region coverable against its own
fake and loopback-style unit tests — the same bar every touched crate holds.

### 4.2 The trait — caller-side mirror of `DeliveryTarget`

The trait is written from **Marsad's perspective as the client** — Marsad *calls* these; Wasl (or
the fake) *implements* them. Method names deliberately echo the control-surface verbs rather than the
raw `DeliveryTarget` method names, because Marsad is the operator side, not the scheduler side. Each
method's rustdoc pins the exact real-Wasl type/file (`wasl-runtime/src/lib.rs`,
`wasl-runtime-control/`) it mirrors.

```rust
/// Marsad's caller-side view of ONE Wasl-managed live session (Wasl's `DeliveryTarget`,
/// wasl-runtime/src/lib.rs §3). Marsad is the CLIENT: it prompts, observes, resolves native
/// gates, interrupts, and stops — it never owns the process, never writes raw stdin, never
/// implements any RuntimeCapability. Transport-abstracted so the in-memory fake and the real
/// HTTP client (Wasl's control server is HTTP/TCP — no UDS; see §3) swap cleanly behind the
/// same trait. Blocking + thread based to match the house style (marsad-wasl / marsad-transport
/// are std-blocking, no async runtime in the workspace).
pub trait WaslSessionClient: Send {
    /// Stable Wasl-side handle for this session (Wasl `target_id()`). This is a WASL target id,
    /// NOT a Marsad session id — core maps between them (§6).
    fn target_id(&self) -> &str;

    /// The session's honest capability set (Wasl `capabilities()`), constant for its lifetime.
    /// INFORMATIONAL to Marsad — used only to enable/disable UI affordances (e.g. hide interrupt
    /// when !interruptible), never to gate an admission decision.
    fn capabilities(&self) -> WaslCapabilities;

    /// A best-effort status snapshot (Wasl `status()`). ADVISORY ONLY — may be stale the instant
    /// it returns; NEVER gate a prompt on it (`prompt` is the atomic admission decision). Marsad
    /// uses it for the observe/status projection (§6) and for `list`.
    fn status(&self) -> WaslTargetStatus;

    /// Submit ONE human-originated prompt (Wasl `submit_turn` with source = LocalHuman,
    /// wasl-runtime/src/lib.rs §3). THE ATOMIC ADMISSION DECISION: returns immediately with a receipt
    /// (Started / Queued / RejectedBusy / RejectedUnavailable / Duplicate) — it does NOT block for the
    /// reply, which arrives via `events`. `busy_policy` governs the busy case. The `WaslTurnRequest`
    /// carries a fresh single-hop `WaslCausation` (§4.4) since Marsad only ever originates root turns.
    fn prompt(&self, req: WaslTurnRequest, busy_policy: WaslBusyPolicy) -> WaslTurnReceipt;

    /// Observe-only subscription to the replayable, sequenced event stream (Wasl `events(after_seq)`).
    /// `after_seq` resumes without gaps/dupes. Returns a pull-style iterator handle; the agent pumps
    /// it on a per-session thread (§7) exactly like `spawn_session_reader` pumps a claude NDJSON
    /// stream. Its `next()` yields a `WaslStreamItem` — either an `Event` or a distinct `Truncated`
    /// control item (§4.4) mirroring Wasl's `StreamItem`.
    fn attach(&self, after_seq: WaslEventSeq) -> Box<dyn WaslEventStream>;

    /// Resolve a NATIVE gate (Wasl `NativeGateResolver::resolve_native_gate(turn_id, decision)`,
    /// wasl-runtime/src/lib.rs §9 option (i)). Keyed by the **`turn_id`** of the blocked turn — the
    /// real gate has NO gate id; per-occurrence identity for Marsad's own ledger derives from
    /// `(turn_id, seq)` (§8). Only meaningful when status is AwaitingGate; routed through Wasl's
    /// single-writer actor, never a raw side-channel. This is the CLI's OWN permission prompt —
    /// DISTINCT from Wasl's send-side operator gate (X1).
    fn resolve_native_gate(&self, turn_id: &WaslTurnId, decision: WaslGateDecision)
        -> Result<(), WaslSessionError>;

    /// Best-effort interrupt of the active turn (Wasl `cancel_turn`, §3.1). Only meaningful if
    /// `capabilities().interruptible`; reflected in the event stream + status.
    fn interrupt(&self, reason: WaslCancelReason) -> Result<(), WaslSessionError>;

    /// Shut the session down (Wasl `shutdown(mode)`). Drain finishes the active turn then stops;
    /// Immediate stops now. Marsad does NOT own the process, so this is a REQUEST to Wasl, not a
    /// kill.
    fn shutdown(&self, mode: WaslShutdownMode) -> Result<(), WaslSessionError>;
}
```

### 4.3 A second trait — the provider/factory seam

Mirroring Wasl's `RuntimeProvider` (`wasl-runtime/src/lib.rs` §10) and Marsad's own
`set_wasl_connector_factory` seam (X1 §4.2), a tiny provider trait constructs (or reattaches to)
clients so `get_or_start` / `list` / swap-the-real-impl are one seam:

```rust
/// Constructs / recovers `WaslSessionClient`s. Config selects the impl (fake in tests; real HTTP
/// client later). Mirrors Wasl's RuntimeProvider (`RuntimeProvider::target_for`, wasl-runtime/src/
/// lib.rs §10) and Marsad's connector-factory seam. `get_or_start` corresponds to Wasl's
/// `POST /v1/targets` (create-or-reuse); `list` to `GET /v1/targets`.
pub trait WaslSessionProvider: Send {
    /// Idempotently obtain a client for the given Wasl target key. Corresponds to Wasl's
    /// `POST /v1/targets`, which is **create-or-reuse and ALWAYS returns a `TargetSnapshot`** — there
    /// is NO `created` flag anywhere in real Wasl (neither on the Rust `RuntimeProvider::target_for`
    /// signature, which returns `Arc<dyn DeliveryTarget>`, nor on the wire). Fresh-vs-resumed is
    /// therefore decided NOT by a boolean but by comparing the returned snapshot's `last_seq` against
    /// what Marsad has persisted (see below and §6.1).
    ///
    /// Returns the client PLUS the target's current `TargetSnapshot` (`snapshot.last_seq`, `status`,
    /// `capabilities`, …) so the caller can run the reconnect comparison. A reconnect must NEVER
    /// accidentally spawn a second competing target for the same key — the create-or-reuse contract
    /// guarantees it doesn't.
    fn get_or_start(
        &self,
        key: &WaslTargetKey,
    ) -> Result<(Box<dyn WaslSessionClient>, WaslTargetSnapshot), WaslSessionError>;

    /// List currently-known Wasl targets on this node (Wasl `GET /v1/targets` →
    /// `TargetsSnapshotResponse`).
    fn list(&self) -> Result<Vec<WaslTargetSnapshot>, WaslSessionError>;
}

/// Caller-side mirror of Wasl's `TargetSnapshot` (wasl-runtime-control/src/wire.rs; fixture
/// `target.json`). Returned by `get_or_start`/`list`. `last_seq` is the crux of the reconnect
/// decision (§6.1).
pub struct WaslTargetSnapshot {
    pub target_id: String,
    pub key: WaslTargetKey,
    pub capabilities: WaslCapabilities,
    pub status: WaslTargetStatus,
    /// The highest journalled event sequence the target has emitted. Compared against Marsad's
    /// persisted last-known seq to detect a fresh/replaced target (§6.1).
    pub last_seq: WaslEventSeq,
    pub queued: usize,
    pub native_session_id: Option<String>,
}
```

> **Reconnect note (see §6.1).** `get_or_start` is what a restarted Marsad-core calls, keyed by the
> persisted `WaslSessionBinding`, to re-obtain a handle to a still-alive Wasl session. Since real Wasl
> exposes **no `created` flag**, fresh-vs-resumed is decided by comparing `snapshot.last_seq` against
> the seq Marsad persisted for this binding: if `snapshot.last_seq < persisted_seq`, the real target
> was replaced/restarted (a fresh target's journal starts over), so Marsad **resets local seq tracking
> to 0** and re-`attach(0)`; otherwise it resumes normally, re-`attach(persisted_seq)`.

### 4.4 Domain types (caller-side mirror; no dependency on any Wasl crate)

All `#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]`, **no `deny_unknown_fields`**
(forward-compat, X1 §2.1 convention). Closed-set strings from Wasl that Marsad only relays/renders
(`phase`, `source`, `certainty` (the real `DeliveryCertainty`), the gate `decision`/`reason`, cancel
`reason`) are kept as **plain `String`** where Marsad never branches exhaustively — the X1
"render/relay, don't enum-lock a v1-additive server" rule. Where Marsad genuinely branches (status
kind, receipt kind, event kind, stream item) they are enums with an `Unknown(String)` /
forward-compat arm where applicable.

```rust
/// Mirror of Wasl RuntimeCapabilities (wasl-runtime/src/lib.rs §2). Eight independent booleans;
/// informational only. NOTE (v3): for all shipped drivers today (claude, opencode) `interruptible`
/// and `streaming_output` are genuinely `false` — Marsad's affordance logic should expect these
/// hidden (§3, §11).
pub struct WaslCapabilities {
    pub persistent_process: bool,
    pub persistent_conversation: bool,
    pub structured_turns: bool,
    pub streaming_output: bool,
    pub attachable: bool,
    pub interruptible: bool,
    pub structured_provenance: bool,
    pub resumable: bool,
}

/// Mirror of Wasl TargetStatus (wasl-runtime/src/lib.rs §4.1). Branched-on by the status projection
/// (§6), so it's an enum with a forward-compat Unknown arm. `since`/timestamps are RFC-3339 Strings
/// (Marsad displays, never does clock math — X1 §2.1 rule). `Interrupting`/`Draining` carry
/// `{active_turn, since}` — they are NOT bare unit variants in real Wasl.
pub enum WaslTargetStatus {
    Offline,
    Starting,
    Idle { since: String },
    Busy { active_turn: String, source: String, phase: String, since: String, queued: usize },
    AwaitingGate { active_turn: String, detail: WaslNativeGateDetail, since: String, queued: usize },
    Interrupting { active_turn: String, since: String },
    Draining { active_turn: String, since: String },
    Faulted { error: String, recoverable: bool },
    Stopped,
    /// Forward-compat: a status kind a future Wasl introduced. Projected to Marsad's Phase
    /// conservatively (§6.3). Carries the raw tag for logging.
    Unknown(String),
}

/// Mirror of Wasl TurnReceipt (wasl-runtime/src/lib.rs §3; fixtures `receipt_*.json`). Includes the
/// real `Duplicate` variant: a resend of an already-seen `(TargetKey, IdempotencyKey)` returns the
/// ORIGINAL receipt wrapped verbatim — Wasl does NOT double-inject (fixture `receipt_duplicate.json`).
pub enum WaslTurnReceipt {
    Started { turn_id: String, first_seq: WaslEventSeq },
    Queued { turn_id: String, position: usize },
    RejectedBusy { active_turn: String },
    RejectedUnavailable { status: WaslTargetStatus },
    /// A resent idempotency key: the wrapped original receipt, no second injection.
    Duplicate { original: Box<WaslTurnReceipt> },
}

/// Mirror of Wasl BusyPolicy (wasl-runtime/src/lib.rs §4.2). Marsad picks the policy per prompt
/// (default: Queue with a bounded wait for a human LocalHuman prompt — a late prompt is still useful;
/// see §12 Q4).
pub enum WaslBusyPolicy { Queue { max_wait_ms: u64 }, Reject, QueueRelated { max_wait_ms: u64 } }

/// One human-originated prompt (Wasl TurnRequest, wasl-runtime/src/lib.rs §3.1; wire
/// `submit_request.json`). `source` is implicitly LocalHuman — Marsad NEVER originates an Agent turn
/// (§11).
pub struct WaslTurnRequest {
    /// Idempotency key so a resend does not double-inject (Wasl returns `Duplicate` for a repeat). A
    /// **UUID (v4 or v7)** minted ONCE per logical human send action (i.e. at the moment the human
    /// hits send, before the first IPC attempt) and reused verbatim on every retry of that same send.
    /// It is NOT a per-process counter: a per-process monotonic counter can collide after a
    /// Marsad-core restart while the underlying Wasl session is still alive (Wasl's session state is
    /// independent of Marsad's process lifetime), which would make Wasl silently treat a genuine new
    /// prompt as a `Duplicate` of an old counter value and drop it. A fresh UUID per send costs
    /// nothing and eliminates that class of bug entirely. See §12 Q5 (DECIDED). v7 is preferred
    /// (time-ordered, debuggable) but v4 is acceptable; uniqueness, not ordering, is the requirement.
    pub idempotency_key: String,
    pub text: String,
    /// The synchronous/queued call-path metadata Wasl requires on every turn (Wasl `Causation`,
    /// wasl-runtime/src/lib.rs §8.2). Marsad only ever originates ROOT, non-A2A-chained `LocalHuman`
    /// turns, so this is ALWAYS a fresh single-hop causation per send (see `WaslCausation::fresh`):
    /// a fresh `root_request`, `parent_request = None`, empty `path`, `hop = 0` — exactly what the
    /// real Wasl code treats as a fresh non-chained turn (fixture `submit_request.json` shows this
    /// shape). Marsad never participates in Wasl's cycle-rejection / `QueueRelated` relatedness.
    pub causation: WaslCausation,
}

/// Mirror of Wasl Causation (wasl-runtime/src/lib.rs §8.2; wire fields per runtime-control-api.md).
/// Marsad always builds a fresh single-hop instance.
pub struct WaslCausation {
    /// The root request that started this call path. For Marsad, a freshly-minted id per send.
    pub root_request: String,
    /// The immediate parent request. ALWAYS `None` for Marsad (no chaining).
    pub parent_request: Option<String>,
    /// Agents already on this call path. ALWAYS empty for Marsad.
    pub path: Vec<String>,
    /// Distance from the root. ALWAYS 0 for Marsad (root turn).
    pub hop: u32,
}

impl WaslCausation {
    /// The only constructor Marsad uses: a fresh single-hop root causation (no parent, empty path,
    /// hop 0). Documented as the correct default for a fresh non-chained LocalHuman turn (§3, §11).
    pub fn fresh(root_request: String) -> Self {
        Self { root_request, parent_request: None, path: Vec::new(), hop: 0 }
    }
}

/// The CLI's own native permission prompt detail (Wasl NativeGateDetail, wasl-runtime/src/lib.rs
/// §4.1). The REAL type is ONLY `{summary, preview}` — there is NO `gate_id` and NO `tool` field.
/// Per-occurrence identity comes from the sibling `turn_id` (on the event / status) combined with the
/// event's stream `seq` (§8). Mapped into the existing `marsad_gates::Gate::Permission` variant and
/// rendered by Marsad's existing gate UI (§8); Marsad does not interpret it beyond display.
pub struct WaslNativeGateDetail {
    pub summary: String,
    pub preview: String,
}

/// A Wasl-side per-turn id (Wasl `TurnId`, wasl-runtime/src/lib.rs §1). This is what
/// `resolve_native_gate` is keyed by (there is no gate id) and what every `TurnEvent` correlates by.
pub struct WaslTurnId(pub String);
pub enum WaslGateDecision { Approve, Deny { reason: String } }
pub enum WaslShutdownMode { Drain, Immediate }
pub enum WaslCancelReason { Human, Superseded }
pub type WaslEventSeq = u64;

/// The public event vocabulary Marsad consumes from `attach` (Wasl `TurnEvent`, wasl-runtime/src/
/// lib.rs §5; wire fixtures `turn_event_*.json`). Field-for-field with the real enum: EVERY kind
/// correlates by `turn_id` (Wasl's rule: correlate by TurnId, never by seq), and
/// `TurnCompleted`/`TurnFailed` carry a `certainty` (`DeliveryCertainty`, kept as a String Marsad
/// only relays/renders). Marsad adds two client-synthesized kinds (`ObserveLost`/`Unknown`) with no
/// real-Wasl counterpart, by design.
pub struct WaslSessionEvent { pub seq: WaslEventSeq, pub kind: WaslSessionEventKind }
pub enum WaslSessionEventKind {
    SessionReady { native: Option<String> },
    TurnAccepted { turn_id: String, source: String },
    TurnQueued { turn_id: String, position: usize },
    AssistantDelta { turn_id: String, text: String },
    ToolStarted { turn_id: String, name: String },
    ToolFinished { turn_id: String, name: String, ok: bool },
    NativeGateRequested { turn_id: String, detail: WaslNativeGateDetail },
    NativeGateResolved { turn_id: String, decision: String },
    TurnCompleted { turn_id: String, reply: String, certainty: String },
    TurnFailed { turn_id: String, error: String, certainty: String },
    TurnCancelled { turn_id: String, reason: String },
    TurnExpired { turn_id: String },
    StatusChanged { status: WaslTargetStatus },
    /// The underlying Wasl session process exited (Wasl `ProcessExited`). Terminal.
    SessionExited { code: Option<i32> },
    /// Marsad-synthesized (NO real-Wasl counterpart, by design): the client lost its observe
    /// connection and will resubscribe. Analogous to marsad-wasl's `WaslEvent::Disconnected` —
    /// informational, drives the Detached projection.
    ObserveLost { error: String },
    /// Forward-compat (Marsad-synthesized): an event kind a future Wasl introduced. Logged + ignored
    /// (X1 tolerant-skip). NOTE: a stream `Truncated` is NOT an Unknown — it is a distinct
    /// `WaslStreamItem::Truncated` (below), an unmissable signal, never folded here.
    Unknown(String),
}

/// What `WaslEventStream::next()` yields — the caller-side mirror of Wasl's `StreamItem`
/// (wasl-runtime/src/lib.rs §3). It is EITHER a normal sequenced event OR a distinct `Truncated`
/// control item; the two must never be conflated (a truncation is NOT an `Unknown` event).
pub enum WaslStreamItem {
    /// A journalled event at its sequence.
    Event(WaslSessionEvent),
    /// The replay cursor (`after_seq`) fell below the retained ring floor (default capacity 4096
    /// events, wasl-runtime/src/config.rs; SSE event name `"truncated"`; fixture `truncated.json`).
    /// The caller MUST re-fetch a snapshot (`get_or_start`/`status`) and resubscribe from `latest`.
    /// This is an UNMISSABLE signal, distinct from any event — never folded into `Unknown`.
    Truncated { earliest: WaslEventSeq, latest: WaslEventSeq },
}

/// Pull-style stream (house style: std-blocking, thread-pumped — no async). Mirrors Wasl's
/// `TurnEventStream` of `StreamItem` from the caller side.
pub trait WaslEventStream: Send {
    /// Blocking next stream item; None on clean end-of-stream. Returns a `WaslStreamItem` so a
    /// `Truncated` control message is surfaced DISTINCTLY from a normal `Event` (never silently
    /// dropped or folded into `Unknown`). Bounded read timeout inside the real impl so a dead peer
    /// can't hang the pump forever (X4.3-amendment lesson from marsad-wasl).
    fn next(&mut self) -> Option<WaslStreamItem>;
}

pub enum WaslSessionError { Io(String), Protocol(String), Unavailable(String) }  // via thiserror
```

---

## 5. `FakeWaslSessionClient` — the in-memory test double

Mirrors `FakeWaslConnector`'s convention (`crates/marsad-wasl/src/fake.rs`): scriptable, inspectable,
zero IO, no real process anywhere. It implements **both** `WaslSessionClient` and a
`FakeWaslSessionProvider: WaslSessionProvider` that hands out fakes. **The entire phase's test suite
runs against these** — the real client is `#[cfg(not(coverage))]` and swapped in only in the later
real-Wasl phase (mirroring how `WaslConnector for WaslOperatorClient` is `cfg(not(coverage))` in
`marsad-wasl`).

`FakeWaslSessionProvider::get_or_start` is create-or-reuse per key: it keeps a keyed map of
already-handed fakes and returns `(client, snapshot)` — the same fake target (with its accumulated
`last_seq`) when the key is already known, or a fresh one (with `last_seq = 0`) otherwise. This proves
the §6.1 reconnect via the **`last_seq` comparison**, not a boolean: a scripted "core restart" (the
provider survives while the client-side registry is dropped and recreated) reattaches to the same fake
target whose `snapshot.last_seq >= persisted_seq`, so Marsad resumes; a scripted "target was replaced"
(the fake seeds a fresh target whose `snapshot.last_seq < persisted_seq`) drives Marsad to reset local
seq tracking to 0. A `script_pre_existing(key, target_id, last_seq)` knob seeds the map (with a chosen
`last_seq`) so both reconnect branches can be tested without a prior `get_or_start` in the same test.

Design (a scripted state machine, deterministic — no timing flakiness, matching Wasl §5.1's
in-repo-fake philosophy):

```rust
/// In-memory scriptable Wasl-managed session. Drives the full WaslSessionClient contract with NO
/// process and NO IO. Deterministic: transitions and events are advanced by explicit `script_*`
/// calls, not wall-clock. Inspectable: `prompts`, `resolved_gates`, `interrupts`, `shutdowns` are
/// pub logs (mirrors FakeWaslConnector.decisions).
pub struct FakeWaslSessionClient {
    pub prompts: Arc<Mutex<Vec<WaslTurnRequest>>>,
    pub resolved_gates: Arc<Mutex<Vec<(String, WaslGateDecision)>>>,
    // ... interrupts / shutdowns logs, current status, event queue, receipt script ...
}

impl FakeWaslSessionClient {
    pub fn new(target_id: &str, caps: WaslCapabilities) -> Self;

    // ── scripting knobs (what tests drive) ──────────────────────────────────
    /// Set the status `status()` returns next (Idle / Busy / AwaitingGate / Faulted / Offline / …).
    pub fn script_status(&self, status: WaslTargetStatus);
    /// Queue the receipt the NEXT `prompt` returns (Started / Queued / RejectedBusy /
    /// RejectedUnavailable), FIFO; default Started when exhausted.
    pub fn script_receipt(&self, receipt: WaslTurnReceipt);
    /// Enqueue one event onto the `attach` stream (seq assigned monotonically unless overridden).
    /// Tests build TurnAccepted → AssistantDelta* → TurnCompleted, or a NativeGateRequested, or a
    /// SessionExited, etc. Wrapped as `WaslStreamItem::Event` by the stream.
    pub fn script_event(&self, kind: WaslSessionEventKind);
    /// Enqueue a `WaslStreamItem::Truncated{earliest, latest}` onto the `attach` stream, so a test
    /// proves the caller re-snapshots + resubscribes from `latest` distinctly from any normal event.
    pub fn script_truncated(&self, earliest: WaslEventSeq, latest: WaslEventSeq);
    /// Convenience: script a full idle→busy→idle happy turn (accept, one delta, complete) so the
    /// common case is one call.
    pub fn script_happy_turn(&self, turn_id: &str, reply: &str);
    /// Convenience: drive a native gate — sets status AwaitingGate AND enqueues NativeGateRequested,
    /// so both the status projection and the gate-pipeline path (§8) fire.
    pub fn script_native_gate(&self, detail: WaslNativeGateDetail);
    /// Simulate a crash: status → Faulted{recoverable:false} (or Offline) AND enqueue SessionExited.
    pub fn script_crash(&self, code: Option<i32>);
    /// Error the next `prompt`/`resolve_native_gate`/`interrupt`/`shutdown` (scripted Err), FIFO.
    pub fn script_error(&self, err: WaslSessionError);
}
```

`attach` returns a `FakeWaslEventStream` draining the scripted item queue (its `next()` returns a
`WaslStreamItem` — `Event` or `Truncated` — or `None` when drained, or blocks on a channel if the test
wants a live-drip model — a `std::sync::mpsc` channel exactly like `FakeWaslConnector`'s `push`).
`resolve_native_gate` records (keyed by `turn_id`) and, by default, advances the scripted status from
`AwaitingGate` back to `Busy`, enqueues a `NativeGateResolved{turn_id, decision}`, and then a follow-on
`TurnCompleted` (so a test proves the full "gate blocks → Marsad resolves → turn completes" loop
without a real Wasl actor).

Every scriptable transition Wasl's contract calls out is reachable: Idle/Busy/AwaitingGate,
queued-turn (`Queued{position}`), reject-busy (`RejectedBusy`), unavailable (`RejectedUnavailable`),
duplicate resend (`Duplicate{original}`), native gate, crash → Offline/Faulted, replay-from-`after_seq`
(the stream honors `after_seq` by skipping earlier seqs), and a `Truncated` control item.

---

## 6. Integration into Marsad's session / runtime-owner model

### 6.1 The `RuntimeOwner` distinction — at `marsad-core`'s `SessionRecord`

The cleanest home for "who owns this session's runtime" is `marsad-core`'s per-session
`SessionRecord` (`crates/marsad-core/src/lib.rs:360`) — the same struct that already carries the
two-axis `live`/`conn_health` liveness model, `driver`, and `host_id`. Add one field:

```rust
/// Who owns this session's underlying runtime. `MarsadManaged` = today's model (the agent owns a
/// claude/codex/opencode process in a tmux/ConPTY container). `WaslManaged` = the NEW backend: the
/// process is Wasl's; Marsad is a thin client of a Wasl DeliveryTarget (this plan). Defaults to
/// `MarsadManaged` so every existing session and serialized record is unchanged (load-bearing
/// optionality, §2).
#[serde(default)]  // on the store-persisted mirror
runtime_owner: RuntimeOwner,

pub enum RuntimeOwner {
    #[default]
    MarsadManaged,
    /// Carries the PERSISTED binding to the Wasl session (see `WaslSessionBinding`) so core can map
    /// Marsad-session-id ⇄ Wasl-target-id, route prompt/attach/gate/interrupt/stop to the right
    /// client, AND reconnect to a still-alive Wasl session after a Marsad-core/GUI restart (§6.1
    /// reconnect).
    WaslManaged { binding: WaslSessionBinding },
}

/// The **persisted** identity of a Wasl-managed session's underlying target. Persisted (not merely
/// in-memory) because the Wasl session's lifetime is independent of Marsad's process lifetime: if
/// `marsad-core`/the GUI restarts while the Wasl session is still alive, Marsad must be able to
/// RECONNECT to it rather than losing track of it. Serialized on the store-persisted `SessionRecord`
/// mirror (`#[serde(default)]`, so old records without it load unchanged — load-bearing optionality).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaslSessionBinding {
    /// Wasl-side stable target id (Wasl `target_id()`) — the handle to reattach to.
    pub target_id: String,
    /// The node the target lives on. Local-first this phase (== SessionRecord.host_id, §11).
    pub host_id: String,
    /// The opaque Wasl target key (protocol id + context) used to re-obtain the client via
    /// `WaslSessionProvider::get_or_start` on reconnect. Value/opaque — the typed form lives in
    /// marsad-wasl-session; core relays it verbatim (same rule as LaunchWaslSession.wasl_target_key).
    pub provider_key: serde_json::Value,
    /// The highest event `seq` core folded for this session before it was last persisted. On
    /// reconnect this is compared against the returned `TargetSnapshot.last_seq` to decide fresh vs.
    /// resumed (there is NO `created` flag in real Wasl — §4.3). Persisted explicitly (not merely
    /// recovered from the transcript) so the comparison is unambiguous.
    #[serde(default)]
    pub last_seq: u64,
}
```

**Reconnect semantics (this phase, provable against the FAKE).** Real Wasl exposes **no `created`
flag** — neither on the Rust `RuntimeProvider::target_for` (returns `Arc<dyn DeliveryTarget>`) nor on
the wire (`POST /v1/targets` is create-or-reuse and always returns a `TargetSnapshot`). So reconnect
decides fresh-vs-resumed by comparing the returned snapshot's `last_seq` against Marsad's persisted
`binding.last_seq`. On a Marsad-core/GUI restart, for each persisted `WaslManaged` session core:
1. calls `WaslSessionProvider::get_or_start(binding.provider_key)`, which reattaches to the surviving
   target (create-or-reuse — never a second target) and returns its current `WaslTargetSnapshot`;
2. **compares `snapshot.last_seq` to `binding.last_seq`:**
   - if `snapshot.last_seq >= binding.last_seq` → the target is the same, still-alive session:
     **resume** by re-issuing `AttachWaslSession { after_seq = binding.last_seq }` so the observe
     thread resumes **from the last known sequence with no gap and no duplicate** (Wasl's
     `events(after_seq)` replay contract);
   - if `snapshot.last_seq < binding.last_seq` → the real target was **replaced/restarted** (a fresh
     target's journal starts over below what Marsad has on file): treat it as fresh — **reset local
     seq tracking to 0** and re-issue `AttachWaslSession { after_seq = 0 }`;
3. re-fetches `status()` (via a fresh `WaslSessionStatus` broadcast — or reads it straight off the
   returned snapshot) to resync the projected `Phase`.

The **behavior** (no gap, no dup, no second target, correct fresh-vs-resume via `last_seq`) is a
**hard test requirement** and is provable entirely against the fake: a scripted "core restarts"
scenario drops and recreates the client-side state, calls `get_or_start`, and asserts both branches —
`snapshot.last_seq >= persisted` resumes without repeating/skipping an event, and
`snapshot.last_seq < persisted` resets to `after_seq = 0` (§10.4).

**Centralized routing — the `RuntimeOwner` match happens in exactly ONE place per operation.** Rather
than sprinkling a `match runtime_owner { … }` branch inline at the top of every public `Core` method
(`send_prompt`/`attach_session`/`stop_session`/`interrupt`/`answer_gate`) — which drifts as the
codebase grows and invites one call site to forget the `WaslManaged` arm — each public method
delegates to a **dedicated internal routing method** whose *sole* job is one exhaustive match on
`RuntimeOwner` and nothing else:

```rust
// Each does EXACTLY ONE exhaustive `match runtime_owner` and nothing else. The public API
// (send_prompt, attach_session, interrupt, stop_session, answer_gate) calls into these; the branch
// lives here and ONLY here, so it can never drift across call sites.
fn send_session_prompt(&self, sid: &str, req: …)   -> …;   // MarsadManaged → SendPrompt; WaslManaged → SendWaslPrompt
fn attach_session_events(&self, sid: &str, …)      -> …;   // MarsadManaged → AttachSession; WaslManaged → AttachWaslSession
fn interrupt_session(&self, sid: &str)             -> …;   // MarsadManaged → Interrupt;     WaslManaged → InterruptWaslSession
fn stop_session(&self, sid: &str, …)               -> …;   // MarsadManaged → StopSession;    WaslManaged → StopWaslSession
```

`MarsadManaged` → exactly today's code (unchanged); `WaslManaged` → the corresponding new `IpcMsg`
(§7). (`answer_gate` similarly routes to `answer_wasl_native_gate` for a `WaslManaged` session, §8.)
This keeps the two paths cleanly separated — one exhaustive match per operation, never inline in
multiple call sites — rather than interleaving Wasl logic into the container-driving code. The
`driver` field for a `WaslManaged` session is a display string like `"wasl:<protocol>"`
(informational; the real driver is Wasl's).

The agent side (`marsad-agent`) holds the actual `WaslSessionClient`/`WaslSessionProvider` in a
registry keyed by Marsad session id — structurally identical to how it holds the per-session reader
threads and the X1 `wasl_connectors` registry (`Arc<Mutex<HashMap<…>>>`, always-compiled test seams
per the `WaslConnectorsWrap` precedent). A per-session **observe thread** pumps `attach`'s
`WaslEventStream` and broadcasts mapped `IpcMsg` lines to subscribers — byte-identical mechanics to
`spawn_session_reader` and to X1's forwarder threads.

### 6.2 Why the distinction lives in core (and a mirror in agent), not in `marsad-events`

`marsad-events` is L0-pure and must stay Wasl-agnostic (its `Phase`/`SessionState` are the shared UI
vocabulary for **all** sessions). The owner distinction is an **orchestration** fact (who to send
IPC to), which is `marsad-core`'s job as the agent client. The **agent** needs the same fact to pick
container-driving vs Wasl-client code, so it appears on both sides of the IPC (as a `LaunchWaslSession`
vs `LaunchSession` message family, §7) — never inside the pure reducer.

### 6.3 Status projection — `WaslTargetStatus` → Marsad `Phase`, no enum expansion

**Hard rule: do NOT add a Wasl-specific variant to `marsad_events::Phase`.** `Phase`
(`Starting | Working | Needs | Error | Done | Stale | Detached`) is the shared UI concept for every
session; polluting it with Wasl states would corrupt the model for real Marsad-owned sessions and
break every exhaustive `match` (`sessionStatus.ts`, `marsad-viewmodel`, `marsad-format`). Instead a
**pure mapping function** in `marsad-wasl-session` (or a thin core helper) projects Wasl status onto
the existing `Phase`, and separately maintains a Wasl-specific transcript/prompt-availability
projection alongside — never *inside* — `SessionState`.

The projection (pure, total, 100%-testable — the analogue of `wasl_event_lines`):

| `WaslTargetStatus`                | Marsad `Phase` | Rationale |
|-----------------------------------|----------------|-----------|
| `Offline`                         | `Detached`     | Not running / not connected; UI's "reconnect?" bucket. |
| `Starting`                        | `Starting`     | Direct match. |
| `Idle{..}`                        | `Done`         | Ready, no active turn — the calm "finished a turn, awaiting input" state (matches how a Marsad session sits at `Done` between prompts). |
| `Busy{..}`                        | `Working`      | Active turn (any `BusyPhase`). |
| `AwaitingGate{..}`                | `Needs`        | Native gate blocks — the attention bucket, exactly like a claude permission gate. |
| `Interrupting{..}`                | `Working`      | Still winding down an active turn. |
| `Draining{..}`                    | `Working`      | Finishing the active turn before stop. |
| `Faulted{recoverable:true, ..}`   | `Error`        | A recoverable fault surfaces as Error (with the fault string as the end-reason text). |
| `Faulted{recoverable:false, ..}`  | `Error`        | Same UI bucket; recoverability drives whether a "retry" affordance shows, not the Phase. |
| `Stopped`                         | `Done`         | Cleanly shut down. |
| `Unknown(tag)`                    | `Working`      | Conservative: don't claim Done/Needs on an unrecognized state; log the tag. |

Notes:
- `AwaitingGate` → `Needs` is what makes the **existing** gate UI light up (§8): the projected
  `Phase::Needs` + a `Gate::Permission` in the `SessionView.gate` slot is exactly what
  `sessionStatus.ts` and the dashboard already render as "needs attention."
- `Idle` → `Done` (not a new "Idle" phase) is the deliberate no-enum-expansion choice; see §12 Q2
  for the one open question this raises (whether a Wasl session's "ready and idle" is
  distinguishable enough from "finished" for the UI without a badge). The mapping is the fallback if
  Q2 resolves "no new concept." **Note:** even though `Idle` and `Stopped` both project to
  `Phase::Done`, the existing `live: bool` axis on `SessionRecord`/`SessionView` already keeps them
  distinct — an idle-and-ready Wasl session is `live: true`, a truly-stopped/exited one is
  `live: false` — so a still-live idle session is never confused with a dead one at the `live` field
  level, independent of the coarser `Phase` projection.
- `capabilities()` never affects the projection — it only gates UI affordances (hide interrupt when
  `!interruptible`).

### 6.4 What the UI sees (just enough to bound this phase)

The desktop's session model (`apps/desktop/src/bridge/useVm.ts`, `types.ts` `SessionView`,
`sessionStatus.ts`) consumes a `SessionView { status/phase, gate, transcript, sendable, … }`. A
Wasl-backed session projects into that shape via the mapping above:

- **status/phase** — from §6.3.
- **transcript** — the agent accumulates `AssistantDelta`/`TurnCompleted`/tool events from `attach`
  into `TranscriptEntry`s (Assistant / Tool), and Marsad's own outgoing prompts as `User` entries,
  exactly as `SessionRecord.transcript` does today for a claude stream. No new transcript type.
- **prompt/send availability** — `sessionStatus.ts`'s "can I send now?" derives from `live` + conn;
  for a Wasl session, `prompt` (the atomic admission) is authoritative, but the UI's *pre-check*
  reads the projected phase + a `sendable` hint from the advisory status (Busy/AwaitingGate ⇒ the UI
  may still allow a queued send per `BusyPolicy`; a hard reject surfaces as a toast from the
  `TurnReceipt`).
- **gate** — a `Gate::Permission` in the existing slot (§8).

Detailed UI work (a dedicated Wasl-session affordance set, capability badges) is **out of scope**
this phase (§11) — this phase proves the backend drives the *existing* projection correctly.

---

## 7. IPC / API surface (agent ⇄ core)

A **new, additive `IpcMsg` family** (not overloading `LaunchSession`/`SendPrompt`, which carry
container/argv semantics that make no sense for a Wasl session). It mirrors the existing
launch/prompt/attach/stop/interrupt/gate shapes so `marsad-core`'s branch (§6.1) is a thin
re-target, and — critically — it is **swappable**: the fake client today and a real HTTP client
later sit **behind the agent's registry**, so **the IPC layer never changes**. Core talks to the
agent; the agent talks to a `WaslSessionClient`; whether that client is fake or real is invisible to
core.

All variants follow `typed-ipc.md` conventions (internally tagged `"type"`, PascalCase, no
`deny_unknown_fields`, `#[serde(default)]` tolerance on non-identity fields, `serde_json::Value` for
payloads whose typed form lives in `marsad-wasl-session` — the same rule that keeps `marsad-proto`
from depending on `marsad-wasl`). `session_id`/`host_id`/`turn_id`/`gate_key` are identity fields
(required on parse → `SchemaMismatch` → `Protocol` error, like existing variants).

```rust
    // ── GUI/core → agent ────────────────────────────────────────────────────
    /// NEW: start a Wasl-managed session. The agent constructs a WaslSessionClient via the
    /// provider factory (fake in tests; real later) and registers it under `session_id`. No argv,
    /// no container — the process is Wasl's. `wasl_target_key` selects the Wasl target (protocol +
    /// context). Analogue of LaunchSession for the WaslManaged owner.
    LaunchWaslSession {
        session_id: String,
        host_id: String,
        /// Opaque Wasl target key (protocol id + context). Value ON PURPOSE (typed form lives in
        /// marsad-wasl-session). Relayed verbatim to WaslSessionProvider::get_or_start; also persisted
        /// into the WaslSessionBinding (§6.1) as provider_key for reconnect after a restart.
        #[serde(default)] wasl_target_key: serde_json::Value,
        #[serde(default)] name: Option<String>,
    },
    /// NEW: prompt a Wasl-managed session (Wasl `submit_turn`, source LocalHuman). Carries the busy
    /// policy and idempotency key so the agent builds a WaslTurnRequest. The agent mints the fresh
    /// single-hop `WaslCausation` (§4.4) — it is NOT carried on the IPC, since Marsad ALWAYS uses a
    /// fresh root causation per send (no chaining), so there is nothing for core to supply.
    /// Analogue of SendPrompt.
    SendWaslPrompt {
        session_id: String,
        text: String,
        #[serde(default)] idempotency_key: String,
        /// "queue" | "reject" | "queue_related" (+ max_wait_ms); default queue (§12 Q4).
        #[serde(default)] busy_policy: String,
        #[serde(default)] max_wait_ms: u64,
    },
    /// NEW: (re)subscribe to a Wasl session's observe stream from a sequence. Analogue of
    /// AttachSession; the agent (re)pumps `attach(after_seq)` on the per-session observe thread.
    AttachWaslSession { session_id: String, #[serde(default)] after_seq: u64 },
    /// NEW: resolve a native gate (Wasl `resolve_native_gate(turn_id, decision)`). See §8 — this is
    /// the SAME human decision the gate UI produces, routed to resolve_native_gate, KEYED BY
    /// `turn_id` (the real gate has no gate id). `gate_key` is Marsad's own per-occurrence identity
    /// `(turn_id, seq)` used for ledger idempotency; `turn_id` is what actually goes to Wasl.
    AnswerWaslNativeGate {
        session_id: String,
        /// The Wasl `turn_id` of the blocked turn — what `resolve_native_gate` is keyed by.
        turn_id: String,
        /// Marsad's per-occurrence gate identity `(turn_id, seq)` (§8), for DecisionLedger dedup.
        gate_key: String,
        /// "approved" | "denied" (relayed as WaslGateDecision).
        outcome: String,
        #[serde(default)] reason: Option<String>,
    },
    /// NEW: interrupt the active turn (Wasl `cancel_turn`). Analogue of Interrupt.
    InterruptWaslSession { session_id: String },
    /// NEW: stop the session (Wasl `shutdown`). `drain` = Drain else Immediate. Analogue of
    /// StopSession.
    StopWaslSession { session_id: String, #[serde(default)] drain: bool },

    // ── agent → GUI/core ─────────────────────────────────────────────────────
    /// NEW broadcast: a Wasl session's advisory status changed. Carries the raw status kind + fields
    /// as a Value; core projects it to Phase (§6.3). Emitted on every status transition the observe
    /// thread sees (and on connect).
    WaslSessionStatus {
        session_id: String,
        /// Verbatim WaslTargetStatus; Value ON PURPOSE (typed form in marsad-wasl-session).
        #[serde(default)] status: serde_json::Value,
    },
    /// NEW broadcast: one normalized observe event (assistant delta / tool / turn lifecycle /
    /// native-gate / exit / observe-lost). Value-carried; core folds it into the transcript +
    /// gate projection. This is the Wasl analogue of a claude StreamMsg line.
    WaslSessionEvent {
        session_id: String,
        seq: u64,
        #[serde(default)] event: serde_json::Value,
    },
    /// NEW ack (serve-loop reply to SendWaslPrompt): carries the TurnReceipt so the UI can show
    /// queued/rejected outcomes. NOT the reply (that arrives as WaslSessionEvent::TurnCompleted).
    WaslPromptReceipt {
        session_id: String,
        /// Verbatim WaslTurnReceipt; Value ON PURPOSE.
        #[serde(default)] receipt: serde_json::Value,
    },
    /// NEW ack (serve-loop reply to AnswerWaslNativeGate): the decision was RELAYED to the client
    /// (not proof the CLI accepted it — the follow-on WaslSessionStatus/Event, e.g.
    /// NativeGateResolved, is authoritative, mirroring X1's WaslGateAnswered semantics). Echoes the
    /// per-occurrence `gate_key` (§8).
    WaslNativeGateAnswered { session_id: String, gate_key: String },
```

`parse_ipc`'s `KNOWN` list grows by exactly these tags. `encode_ipc` unchanged. Backward-compat
(same discipline as X1 §3): old core ← new agent hits the tolerant-skip
(`UnknownDiscriminant` → logged no-op); old agent ← new core can only receive a `*Wasl*` command if
a Wasl session was first launched, which an old agent never does — a stray one gets the standard
`Error` reply.

**Why Value-carried, agent-registry-mediated = swappable.** The IPC only ever moves session ids,
text, sequences, and opaque Value payloads. The agent turns those into `WaslSessionClient` calls.
Replacing `FakeWaslSessionClient` with `RealWaslSessionClient` changes **only** the provider factory
registered in the agent's `serve()` (behind a `set_wasl_session_provider_factory` seam, the direct
analogue of `set_wasl_connector_factory`) — no `IpcMsg`, no core, no gate, no viewmodel change. The
HTTP wire (the hand-rolled client against `runtime-control-api.md` + vendored `runtime-control-v1/`
fixtures) lives entirely inside `RealWaslSessionClient`.

**The Value-carrying approach stays (reviewed and confirmed) — but with a mandatory drift guard.**
Keeping the `status`/`event`/`receipt`/`wasl_target_key` payloads as `serde_json::Value` at the
`marsad-proto` boundary (rather than typed DTOs) is the deliberate, reviewed choice: it keeps
`marsad-proto` free of a `marsad-wasl-session` dependency, exactly as it already carries
`WaslObservation.entry` as `Value`. The cost of Value-carrying is that a schema mismatch between the
`marsad-wasl-session` typed form and what actually crosses the wire would only surface at runtime — so
this phase **requires round-trip typed-conversion tests at the crate boundary**: for each Value-carried
payload, a test must assert `typed → Value (as the agent emits) → typed (as core parses) == typed`
(and the inbound direction where applicable), mirroring the existing **golden-fixture discipline**
(the operator-v1 fixtures, X1 §1). These live with the §10.2 `marsad-proto` quartet and the §10.1
`marsad-wasl-session` tests and are what catch typed↔Value drift **early**, at build time, rather than
against a real Wasl process later.

---

## 8. Native-gate resolution wiring (reuse `marsad-gates`)

**Confirmed conclusion: Marsad's OWN gate UI resolves native Wasl gates — no separate Wasl-specific
gate UI.** This reuses the **existing** `marsad-gates` pipeline and the already-defined
**`Gate::Permission`** variant — the **same** variant a plain CLI's own tool-use/permission prompt
already produces. A native Wasl gate is exactly that: **the underlying CLI's own permission prompt**,
surfaced through Wasl instead of through a Marsad-owned stdio stream. It is a *permission* gate, not
an *agent-to-agent handoff*.

**Why `Gate::Permission`, not `Gate::Handoff` (domain fit).** The actual `marsad_gates::Gate` enum
(`crates/marsad-gates/src/lib.rs`) has both. `Gate::Handoff` is documented and used specifically for
*agent-to-agent* handoff semantics surfaced by an external connector (X1's Wasl **operator** send-side
gates; `wasl-connector.md` §5.1 — "an agent-to-agent handoff gate surfaced by an external connector",
rendered as `"Wasl <kind> from <from>"` with the ⇄ icon and host-scoped `wasl:<host_id>` identity).
A native CLI permission prompt is **not** a handoff between agents — it is one agent's own tool-use
gate. Routing it through `Gate::Handoff` would be a domain-type mismatch and would visually mislabel it
(⇄ "handoff"). There is direct in-tree precedent for the correct choice: `opencode-acp.md` §4/§8
(`acp_permission_to_gate(params) -> marsad_gates::Gate`) reuses **`Gate::Permission`** for opencode's
own ACP `session/request_permission` prompt — the identical situation (another external CLI's own
permission prompt). A native Wasl gate is one more inhabitant of `Gate::Permission`, exactly as an
opencode ACP permission is.

Concretely:

1. When the observe thread sees `WaslSessionEventKind::NativeGateRequested { turn_id, detail }`
   (and/or the status projects to `AwaitingGate`), the agent broadcasts a `WaslSessionEvent` carrying
   it — **crucially with the event's own `seq`**. Core, on folding that event, derives a
   **per-occurrence gate identity** and constructs a `Gate::Permission`:

   ```rust
   // Per-occurrence identity. The REAL NativeGateDetail is only {summary, preview} — no gate_id.
   // Wasl's per-turn `turn_id` is NOT unique per gate occurrence (Wasl does NOT prevent two native
   // gates firing within one turn: approve tool A, the turn continues, tool B later needs approval —
   // SAME turn_id both times). The event's own stream `seq` IS guaranteed unique per event, so
   // combining it with turn_id yields a genuinely-unique-per-occurrence identity.
   let gate_key = native_gate_key(&turn_id, seq);   // e.g. format!("{turn_id}#{seq}")

   marsad_gates::Gate::Permission {
       // Identity slot: the per-occurrence (turn_id, seq) key. See the decision_id caveat below —
       // command/cwd MUST also encode gate_key so the derived DecisionId is unique per occurrence.
       request_id: gate_key.clone(),
       // What renders as the gate summary ("$ …" via gate_meta): detail.summary, with gate_key
       // encoded so the command+cwd pair (which decision_id hashes) is unique per gate occurrence.
       command: native_gate_command(&gate_key, &detail),  // e.g. format!("{gate_key}\x00{}", detail.summary)
       cwd: String::new(),                       // no cwd concept for a native gate; empty
       risk: String::new(),                      // Wasl supplies no risk score; empty (UI shows none)
       explanation: detail.preview.clone(),      // the CLI's own preview text
       is_elicitation: false,                    // a native permission prompt is a binary approve/deny,
       elicitation_choices: Vec::new(),          //   not an MCP elicitation with scope choices …
       permission_suggestions: Vec::new(),       //   and carries no claude-style "always allow" rules
   }
   ```

   and `enqueue`s it into the session's `SessionState.gate`/`gate_queue` slot — the **same** slot,
   one-at-a-time queue, and dedup-by-identity machinery that claude permission gates already use
   (`marsad-events::enqueue_gate`). The projected `Phase::Needs` (§6.3) lights up the existing "needs
   attention" UI, and `gate_meta`/`gate_vm`/the overlay render it as an ordinary permission gate
   (icon "⌘", lead "Wants to run", approve label "Approve & run"). **No new gate type, no new gate
   UI, no `marsad-gates`/`marsad-format` change.**

   **Native-gate identity — the v3 correction (load-bearing).** The v2 design assumed
   `NativeGateDetail` carried a `gate_id: String` unique per gate occurrence. **It does not** — the
   real type (`wasl-runtime/src/lib.rs` §4.1) is only `{summary, preview}`. The real per-gate handle
   is the sibling `turn_id` on `TurnEvent::NativeGateRequested{turn_id, detail}` /
   `TargetStatus::AwaitingGate{active_turn, detail, …}` — **but `turn_id` is unique per TURN, not per
   gate occurrence**, and Wasl's actor does **not** prevent two separate native gates within one turn
   (approve tool A → turn continues → tool B later gates → same `turn_id` again). So Marsad derives
   its gate identity / DecisionId-safety from **`(turn_id, seq)`**: the event's own journal `seq` is
   guaranteed unique per event (Wasl `EventSeq` is per-target monotonic and gap-free), so `(turn_id,
   seq)` is genuinely unique per occurrence. **Resolution back to Wasl still uses the bare `turn_id`**
   (that is what `resolve_native_gate(turn_id, decision)` takes) — the `seq` is only for Marsad's own
   per-occurrence ledger identity, never sent to Wasl.

   **Field-fit caveat (documented honestly).** `Gate::Permission` was shaped for claude's
   `can_use_tool` request, so three of its fields have no native-gate analogue and are set empty
   (`cwd`, `risk`, `elicitation_choices`/`permission_suggestions`, `is_elicitation:false`). Empty
   values render cleanly (the overlay already tolerates them — it derives the binary Approve/Reject
   when no suggestions/choices are present, per `opencode-acp.md` §7's "plain Allow/Deny is DERIVED"
   note). The one **non-cosmetic** mismatch is identity: `marsad_gates::decision_id` for
   `Gate::Permission` hashes **`command` + `cwd`** (not `request_id` — see `gate_canonical`), whereas
   the native gate's stable per-occurrence identity is the `(turn_id, seq)` `gate_key`. The mapping
   therefore **encodes `gate_key` into `command`** (via `native_gate_command`, a `\x00`-delimited
   `gate_key`-prefixed string that also carries the human-readable summary) so the derived
   `DecisionId` is unique and stable per gate occurrence exactly as `(turn_id, seq)` is.
   `native_gate_key` and `native_gate_command` are small pure helpers (in `marsad-wasl-session`'s
   projection module, alongside the status projection) with their own round-trip tests. This is the
   sole adaptation cost of reusing `Gate::Permission`, and it is far cheaper and more honest than
   either forcing the gate into `Gate::Handoff` (wrong domain) or adding a new gate variant
   (unnecessary — a native permission prompt IS a permission gate). No new `Gate` variant is
   introduced; if a future Wasl gate genuinely carried scope choices, it would map onto the existing
   `elicitation_choices`/`permission_suggestions` fields, still without a new variant.

2. The human answers in Marsad's existing gate UI. Core's handler — a small variant of
   `answer_wasl_gate` / `answer_gate` — records the decision in the **existing** `DecisionLedger`
   (idempotent: same `(scope, gate_key)` → dedup, exactly like X1, where `gate_key` is the
   `(turn_id, seq)` per-occurrence identity) and emits the new
   `IpcMsg::AnswerWaslNativeGate { session_id, turn_id, gate_key, outcome, reason }` (§7) — `turn_id`
   is what reaches Wasl's `resolve_native_gate`; `gate_key` is only Marsad's ledger identity.

   - **Scope difference from X1:** X1's `Gate::Handoff` (operator gates) is **host-scoped**
     (`wasl:<host_id>`) because operator gates belong to a node, not a session. A **native** gate
     belongs to a specific **Wasl-managed session**, so — like every ordinary `Gate::Permission` —
     its `marsad-gates` scope is the **Marsad session id** (`route()` returns exactly that session).
     This falls out naturally from choosing `Gate::Permission`: session-scoped permission gates are
     already the norm. It is called out so no one assumes native gates are host-scoped like the X1
     operator handoffs.

3. The agent's `AnswerWaslNativeGate` arm looks up the session's `WaslSessionClient` in its registry
   (clone the `Arc` handle out, **drop the lock before the call** — see the **lock-hygiene rule
   (§10.3 test requirement)**: `marsad-agent` must never hold the provider-registry lock, the
   session-client-map lock, or any subscriber-list lock while calling any potentially-blocking
   `WaslSessionClient` method, so a wedged Wasl peer can't freeze the serve loop or stall unrelated
   agent operations), calls `resolve_native_gate(&WaslTurnId(turn_id), decision)` (**keyed by
   `turn_id`** — the real gate has no gate id), and replies `WaslNativeGateAnswered { gate_key }`
   regardless of ack (the follow-on `NativeGateResolved` status/event is the authoritative resolution
   signal). The `resolve_native_gate` call is bounded (the same read-deadline discipline as
   `marsad-wasl`'s `read_batch` cap).

**Type/IPC shape carrying this:** the human decision travels as `IpcMsg::AnswerWaslNativeGate`
(§7, carrying `turn_id` for Wasl + `gate_key` for the ledger); the pending gate is a
`marsad_gates::Gate::Permission` (existing) identified per-occurrence by `(turn_id, seq)`; the
UI-facing pending-gate row reuses the existing `SessionView.gate` slot (a native gate is per-session,
so it does **not** use the host-scoped `WaslGateView`/`VmUpdate::WaslGates` list from X1 — that stays
exclusively for operator handoff gates). This keeps native-gate resolution entirely inside the
machinery that already exists, and keeps it visibly distinct from the X1 operator-gate path.

### 8.1 Lock hygiene (numbered requirement)

The lock-hygiene discipline is a **hard, testable requirement**, not just prose: `marsad-agent` MUST
NOT hold the provider-registry lock, the session-client-map lock, or any subscriber-list lock while
calling any potentially-blocking `WaslSessionClient` method — `prompt`, the `attach` stream's
`next()`, `resolve_native_gate`, `interrupt`, or `shutdown`. The pattern is always: take the lock,
clone the `Arc<dyn WaslSessionClient>` (or the target handle) out, **drop the lock**, then call. See
§10.3 for the scripted slow/blocking-fake test that proves a blocking call on one session does not
stall unrelated agent operations.

---

## 9. Proposed block-map delta (not applied here)

For a **later, separate** `ARCHITECTURE.md` edit once specs land — listed here for planning only:

| # | Crate | Tier | Status | Purpose |
|---|---|---|---|---|
| X6 | `marsad-wasl-session` | L1 | ✅ shipped | Caller-side Wasl **session control** client: `WaslSessionClient` + `WaslSessionProvider` traits, the caller-mirror domain types (`WaslTargetStatus`/`WaslTurnReceipt`/`WaslCapabilities`/…), the pure `WaslTargetStatus → Phase` projection, `FakeWaslSessionClient`/`FakeWaslSessionProvider`, **and the real HTTP/SSE client** (`src/{real,wire,wire_dto,sse}.rs`). No dep on any other marsad crate except (optionally) `marsad-proto` for shared wire enums. Tested **100% line+region** against the fake (pure logic) and the vendored `runtime-control-v1/` fixtures (real client). **Distinct from X1 `marsad-wasl`** (operator gates), analogous in structure. |

Additive edits to existing blocks (all src-only, no existing test/file edits — X1 §6/§8 discipline):
`marsad-proto` (the new `IpcMsg` family, §7; `KNOWN` grows); `marsad-core` (the `RuntimeOwner` field
+ per-method branch + the new inbound arms + `answer_wasl_native_gate` + the launch path for a
`WaslManaged` session); `marsad-agent` (the `WaslSessionProvider` registry + observe threads +
`serve()` wiring + `set_wasl_session_provider_factory` seam + the six new `handle_ipc` arms);
`marsad-gates` (**no change** — `Gate::Permission` already exists; native gates reuse it session-scoped,
exactly as opencode ACP permissions do);
`apps/desktop/src-tauri` (one mechanical mapping arm for the new `VmUpdate`s, if any are added,
mirroring X1 §5.3 `envelope_of`).

**Dependency additions (acyclic):** `marsad-wasl-session ← agent, core` (identical shape to
`wasl ← agent, core`). No `agent → core` edge. No existing edge changes.

---

## 10. Test plan for this phase

Follows every crate's `AGENTS.md`: **test-first** (tests define the contract before implementation;
tests are read-only to the code-agent), **black-box `tests/`** in buckets, **100% line+region** per
touched crate (`cargo llvm-cov --fail-under-lines 100 --fail-under-regions 100`), **no real Wasl
process anywhere**. The fake (§5) is the whole substrate; loopback transport is the tier for the
core/agent IPC tests (the house `LoopbackFake` pattern). Scenarios are the direct analogue of Wasl's
own §5.1 list, from the *caller's* side.

### 10.1 `marsad-wasl-session` — `tests/{happy,errors,branches,fakes,projection}.rs`

- **fake conformance** — `FakeWaslSessionClient` honors every method: `prompt` returns the scripted
  receipt (Started/Queued/RejectedBusy/RejectedUnavailable), default Started; `status` returns the
  scripted status; `attach` drains scripted events honoring `after_seq` (replay skips earlier seqs,
  no gaps/dupes); `resolve_native_gate`/`interrupt`/`shutdown` record + return scripted acks/errors;
  `stop`/drop idempotent. Mirrors `marsad-wasl/tests/fakes.rs`.
- **prompt-when-idle succeeds** — script Idle; `prompt` → `Started`; then a scripted
  `TurnAccepted → AssistantDelta → TurnCompleted` drains from `attach`.
- **prompt-when-busy per policy** — script Busy; with `Reject` → `RejectedBusy`; with
  `Queue{max_wait}` → `Queued{position}`; with `QueueRelated` → both branches.
- **prompt-when-unavailable** — script Offline/Faulted → `RejectedUnavailable{status}`.
- **duplicate resend** — script `Duplicate{original}`; assert the wrapped original receipt is returned
  verbatim and NO second entry lands in the `prompts` log (no double-inject).
- **causation is fresh single-hop** — every `WaslTurnRequest` the fake records carries
  `WaslCausation::fresh(...)`: `parent_request == None`, `path.is_empty()`, `hop == 0`, and a fresh
  `root_request` per send (two sends → two distinct `root_request`s).
- **attach replays correctly** — enqueue events seq 1..5, `attach(after_seq=2)` yields exactly 3..5;
  an `ObserveLost` mid-stream then a resubscribe from the last seq yields no dup/gap.
- **truncated is a distinct stream item** — `script_truncated(earliest, latest)` → `next()` yields
  `WaslStreamItem::Truncated{earliest, latest}` (NOT an `Event`, NOT folded into `Unknown`); the pump
  treats it as the re-snapshot+resubscribe-from-`latest` signal.
- **richer event vocabulary round-trips** — every real `WaslSessionEventKind` (incl. `TurnQueued`,
  `NativeGateResolved`, `TurnCancelled`, `TurnExpired`, `StatusChanged`, and the `turn_id`/`certainty`
  fields on the existing kinds) parses from its `turn_event_*.json` fixture and re-encodes identically;
  the synthetic `ObserveLost`/`Unknown` have no fixture (client-only, by design).
- **native gate loop** — `script_native_gate` → status `AwaitingGate` + `NativeGateRequested`;
  `resolve_native_gate(Approve)` recorded, status advances to Busy, follow-on `TurnCompleted`
  drains (proves "gate blocks → resolved → turn completes" with no real actor).
- **crash / offline** — `script_crash` → status `Faulted{recoverable:false}` (or Offline) +
  `SessionExited`; subsequent `prompt` → `RejectedUnavailable`.
- **status projection (pure, total)** — the §6.3 table, every row incl. both `Faulted`
  recoverabilities and `Unknown(tag) → Working`; capabilities never affect the projection.
- **capabilities informational** — a client with `interruptible:false`: `interrupt` still callable
  but the projection/affordance helper reports it unavailable; no branch asserts a capability the
  client didn't claim (the Wasl §5.2 gating-honesty rule, caller side).

### 10.2 `marsad-proto` — NEW `tests/wasl_session_ipc.rs`

The standard quartet per new variant (golden parse to exact typed value; encode→parse→re-encode
identity; golden encode Value-equality; tolerance: injected `x_future_field` + absent
`#[serde(default)]` fields parse, missing `session_id`/`host_id`/`turn_id`/`gate_key` →
`SchemaMismatch`).
Plus: `KNOWN` closure (all tags parse; `FutureMsg` still `UnknownDiscriminant`); Value-carried
`status`/`event`/`receipt`/`wasl_target_key` round-trip. **Plus (§7 drift guard): typed↔Value
round-trip conversion tests at the crate boundary** — for each Value-carried payload assert
`typed → Value → typed == typed` (both directions where applicable), mirroring the operator-v1
golden-fixture discipline (X1 §1), so typed↔Value drift is caught at build time, not at real-Wasl
integration.

### 10.3 `marsad-agent` — NEW `tests/wasl_session.rs` (fake provider; no sockets)

- **absent = inert** — no Wasl session ever launched ⇒ empty provider registry, zero observe
  threads, and every existing agent test passes untouched.
- **launch** — `LaunchWaslSession` registers a fake client via
  `set_wasl_session_provider_factory`; `list` reflects it; a second launch is independent
  (no cross-routing, i07-style).
- **prompt round-trip** — `SendWaslPrompt` → the fake's `prompts` log gets the request with the
  right idempotency key + busy policy; reply line parses as `WaslPromptReceipt` carrying the scripted
  receipt.
- **observe broadcast** — scripted events on the fake drain through the observe thread as
  `WaslSessionEvent` lines that re-parse to the exact typed events; status transitions broadcast as
  `WaslSessionStatus`.
- **native gate relay** — `AnswerWaslNativeGate` → fake's `resolved_gates` log gets
  `(turn_id, Approve/Deny{reason})` (**keyed by `turn_id`** — no gate id); reply parses as
  `WaslNativeGateAnswered { gate_key }`; scripted client error → `Protocol` error (serve loop would
  reply `Error`); a `resolve` failure can't poison the registry lock — the X1 `wasl_lock_safety.rs`
  analogue.
- **lock hygiene — no lock held across a blocking client call (numbered requirement, §8.1).** With a
  **scripted slow/blocking fake** (a `WaslSessionClient` whose `prompt` / stream `next()` /
  `resolve_native_gate` / `interrupt` / `shutdown` blocks on a test-controlled gate/channel), assert
  that while session **A**'s call is blocked, an unrelated operation on session **B** (e.g. a
  `SendWaslPrompt`/`status` broadcast, or a `LaunchWaslSession`) **completes without waiting** — i.e.
  the agent did NOT hold the provider-registry lock, the session-client-map lock, or any
  subscriber-list lock across A's blocking call. The test would deadlock/time out if any such lock
  were held across the call, and passes only under the "clone the `Arc` out, drop the lock, then
  call" pattern. Covers each blocking method.
- **interrupt / stop** — `InterruptWaslSession`/`StopWaslSession` reach the fake's logs; a
  `!interruptible` client still records the call (best-effort per contract).
- **crash reflected** — a scripted `SessionExited`/`Faulted` broadcasts a `WaslSessionStatus` that
  core will project to Error/Detached.
- **reconnect after core restart (§6.1) — both `last_seq` branches** — script a surviving target;
  drop and recreate the client-side registry state; on relaunch/reattach keyed by the persisted
  `WaslSessionBinding`, assert `get_or_start` returns the same target (no second one spawned) and,
  comparing `snapshot.last_seq` against `binding.last_seq`: (a) `snapshot.last_seq >= binding.last_seq`
  → the observe thread re-pumps `attach(binding.last_seq)` (resume); (b) a scripted replaced target
  with `snapshot.last_seq < binding.last_seq` → local seq resets to 0 and re-pumps `attach(0)` (fresh).

### 10.4 `marsad-core` — NEW `tests/wasl_session.rs` (loopback transport)

- **runtime-owner routing** — a `WaslManaged` session: `send_prompt` emits `SendWaslPrompt` (not
  `SendPrompt`); `attach_session`/`stop_session`/`interrupt` emit the `*Wasl*` variants; a
  `MarsadManaged` session emits exactly today's variants (regression: existing tests untouched-green).
- **status projection into `SessionView`** — feed `WaslSessionStatus` lines for each
  `WaslTargetStatus`; assert the projected `Phase` per §6.3 and that no `Phase` variant was added.
- **transcript folding** — `WaslSessionEvent` deltas/tool/completed fold into `TranscriptEntry`s;
  Marsad's own prompt appears as a `User` entry.
- **transcript folding — edge cases (explicit, each its own assertion):**
  - **duplicate events are not double-folded** — the same `WaslSessionEvent { seq }` delivered twice
    (e.g. a replay overlap after an `ObserveLost`/reattach) folds **once**: the transcript is
    identical whether the event arrives once or twice (dedup by `seq`).
  - **queued ≠ accepted** — a prompt that received a `Queued{position}` receipt is represented as a
    **queued/pending** prompt distinctly from one whose turn has been `TurnAccepted`; a queued
    prompt's content is **not** shown as already-delivered. Only on `TurnAccepted` (or `Started`)
    does it become an in-flight turn.
  - **`AssistantDelta` appends, not re-creates** — a run of `AssistantDelta { text }` for one
    in-progress turn **appends** to the *same* Assistant `TranscriptEntry` (concatenating text),
    rather than creating a new entry per delta.
  - **exactly one terminal event closes a turn** — a `TurnCompleted` (or `TurnFailed`) closes out the
    turn's transcript entry exactly once; the entry transitions to its final/complete form a single
    time (no double-close, no reopen).
  - **post-terminal delta is ignored, not folded** — an `AssistantDelta` (or any non-terminal event)
    arriving for a turn that already saw its terminal `TurnCompleted`/`TurnFailed` is **logged and
    ignored**, never folded — Marsad does not corrupt or reopen a closed entry. This is defensive:
    Wasl's own contract says it shouldn't happen, but Marsad must not mangle state if it does.
- **native gate → existing pipeline** — a `NativeGateRequested{turn_id, detail}` event (with its
  `seq`) upserts a `Gate::Permission` (session-scoped) into `SessionView.gate` with `Phase::Needs`,
  identified per-occurrence by the `(turn_id, seq)` `gate_key`; `answer_wasl_native_gate(Approve)` →
  exactly one `AnswerWaslNativeGate` line **carrying `turn_id` (to Wasl) + `gate_key` (ledger)**,
  ledger-idempotent (second call → no-op, one line ever); unknown gate → `UnknownWaslGate`-class
  error; the host-scoped `VmUpdate::WaslGates` list is **not** touched (native gates are per-session —
  regression pin). The derived `DecisionId` is stable per gate occurrence (the `gate_key`-into-
  `command` encoding of §8 makes `command`+`cwd` unique).
- **two native gates in one turn are distinct (v3 regression pin)** — two `NativeGateRequested`
  events with the **same `turn_id`** but **different `seq`** produce two distinct gate occurrences
  (distinct `gate_key`, distinct `DecisionId`), resolvable independently — proving the identity fix:
  Wasl does not prevent two native gates per turn, so `turn_id` alone must not collapse them.
- **prompt-busy surfaces** — a `RejectedBusy`/`Queued` receipt line surfaces as a toast / queued
  indicator, not a silent drop.
- **absent = inert (core side)** — a fresh `Core` with only `MarsadManaged` sessions emits no
  `*Wasl*` session update from any existing flow (spot-check over the existing corpus).

### 10.5 Cross-cutting

Coverage bars per touched crate; existing integration `i01–i11` run unmodified (no Wasl-backend
session in their environments — they double as the inertness proof). Live Marsad↔real-Wasl e2e is
the **later** phase's rung, parked out of this phase (requires Wasl's control surface to exist).

---

## 11. Non-goals for this phase

Stated the way Wasl's own plan states its non-goals (§9), so no reader assumes more than is built:

- **No real Wasl process integration.** The entire phase runs against `FakeWaslSessionClient`. The
  real, hand-rolled `WaslSessionClient` HTTP impl (§3 — no Wasl client crate exists) is a **separate,
  later phase**. By the trait boundary of §4, that phase adds exactly one new client impl behind the
  provider factory and changes **nothing** built here.
- **No wire-transport ambiguity.** The transport is settled — **HTTP/TCP** (the real Wasl control
  server binds a `TcpListener`; there is no `UnixListener` anywhere in the Wasl repo, §3). Marsad's
  client trait stays transport-abstracted only so the fake and the real HTTP client swap cleanly.
- **No UI work beyond proving the backend.** A Wasl session projects into the **existing**
  `SessionView`/`Phase`/gate UI (§6.4, §8). A dedicated Wasl-session affordance set (capability
  badges, a distinct observe view, queue-position UI) is a later phase.
- **No cross-host Wasl backend.** Local-first: a Wasl session's `host_id` is the local node this
  phase. Cross-host Wasl targets (and any tunneling to a remote Wasl control surface) are deferred —
  the same posture as X1's remote-operator deferral (`wasl-connector.md` §9).
- **No A2A-triggered session lifecycle.** Marsad only ever originates `LocalHuman` prompts (Wasl
  §1.1). Marsad does not start/stop Wasl sessions in response to A2A traffic, does not schedule
  agent-originated turns, and does not participate in Wasl's `Causation`/recursion protection — those
  are Wasl-internal.
- **No replication of any `RuntimeCapability`.** Marsad implements none of `persistent_process`,
  `structured_turns`, `interruptible`, etc. — it reads them as informational only. **Expectation note
  (v3):** for all shipped drivers today (claude, opencode), `interruptible` and `streaming_output` are
  reported as **`false`** (genuinely false, not "unknown"), so Marsad's affordance logic should expect
  the interrupt/streaming affordances hidden for now — no design change required, just expectation
  setting (§3).
- **No `answer_tool_call` / bidirectional mid-turn tool-callback support (known future gap, note
  only).** Real Wasl ships a `ToolCallAnswerer::answer_tool_call(turn_id, value)` supertrait on
  `DeliveryTarget` (`wasl-runtime/src/lib.rs` §9 — the tool-channel counterpart of
  `resolve_native_gate`) for a mid-turn bidirectional tool callback. **No current real driver
  (claude, opencode) uses it** — it is a default no-op on the unidirectional stdin/ACP drivers — and
  Marsad has no analogue. This is recorded as a known future gap; **no action this phase.** If a
  future driver actually blocks a turn behind a tool-shaped callback, Marsad would need a new
  IPC/client method mirroring the native-gate resolution path — a later, additive extension.
- **No Wasl-specific `Phase` variant** and **no change to the `marsad-events` reducer's** Wasl-
  agnostic model — the owner distinction and status projection live in core/agent, never in L0.
- **No merging with the X1 operator-gate path.** Native gates (this plan) and operator send-side
  gates (X1) stay distinct: different gate type (native = `Gate::Permission`, operator =
  `Gate::Handoff`), different scope (session vs host), different IPC, different meaning — they only
  *share* the gate **UI/pipeline** (the `SessionView.gate` slot, enqueue/dedup/ledger machinery), as
  designed.
- **No Terminal-tab support for a Wasl-managed session (forward-note — flag, don't build).** Marsad's
  existing raw-PTY **Terminal tab** (the shell escape-hatch that attaches to a Marsad-owned session's
  process/PTY) **cannot** operate against a `WaslManaged` session: there is no Marsad-owned
  process/PTY to attach it to (the process is Wasl's, and Marsad only prompts/observes it through the
  control API — there is no raw stdio handle). This is a genuine **capability gap**, not a bug. This
  phase builds no UI, so nothing is done now; but the note is recorded here so the later UI phase
  **hides/disables the Terminal tab for `WaslManaged` sessions** rather than silently leaving it
  broken (a dead tab attaching to nothing). No workaround (e.g. a synthetic PTY) is in scope.

---

## 12. Open design questions (decide before implementation)

Flagged explicitly — not silently guessed. Each is architecturally significant enough to want an
owner decision before specs are frozen.

- **Q1 — Crate placement final call.** §4.1 argues for a **new crate** `marsad-wasl-session` (mirror
  of X1's `marsad-wasl`), *not* folding into `marsad-agent`. This is the recommendation and it fits
  the DAG cleanly, but it does add a workspace member. Confirm the new-crate decision (vs. a
  `session` module *inside* `marsad-wasl` — rejected here because X1's `marsad-wasl` is frozen at
  100/100 against operator fixtures and its `AGENTS.md`/coverage regime is operator-specific; adding
  an unrelated control-surface trait+fake into it would entangle two independently-evolving
  contracts). **Recommended: new crate.**

- **Q2 — Does `Idle` need a UI concept, or is `Idle → Done` (§6.3) enough?** A Wasl-managed session
  that is *ready and waiting for the next prompt* projects to `Phase::Done` under the no-enum-
  expansion rule. For a claude session, `Done` means "turn finished." For a live Wasl session, "idle
  and ready" is arguably a *stronger, ongoing* readiness than a finished one-shot. If the UI must
  distinguish them (e.g. a "live · idle" badge vs a "done" card), that is a **projection-side
  presentational hint** (a boolean alongside the phase, NOT a new `Phase` variant) — decide whether
  this phase needs it or whether `Idle → Done` is acceptable until the later UI phase. **Recommended:
  `Idle → Done` now; add a presentational `wasl_idle: bool` hint only if the UI proof demands it.**
  Note that the existing `live: bool` axis on `SessionRecord`/`SessionView` **already** correctly
  distinguishes `Idle` (`live: true`) from `Stopped`/exited (`live: false`) independent of the `Phase`
  projection — so even while both project to `Phase::Done`, a Wasl session that is idle-and-ready is
  never confused with a truly-exited one at the `live` field level, and any "live · idle" affordance
  keys off the existing `live` flag rather than needing a new one.

- **Q3 — Where does the `WaslTargetStatus → Phase` projection physically live?** §6.3 puts the pure
  function in `marsad-wasl-session` (so both agent and core can call it and it's 100%-coverable
  there). The alternative is a private helper in `marsad-core`. Placing it in the shared crate is
  cleaner but means `marsad-wasl-session` must know `marsad_events::Phase` — introducing a
  `marsad-wasl-session → marsad-events` edge (acyclic, but a new edge). Confirm this edge is
  acceptable, or keep the projection private to core. **Recommended: shared crate + the new
  `→ marsad-events` edge** (it's L0-pure and the projection genuinely belongs to the Wasl domain).

- **Q4 — Default `BusyPolicy` for a human prompt.** Wasl's defaults are per delivery *kind*
  (consult→Queue, command→Reject, message→Queue-with-expiry). A Marsad human prompt has no A2A kind;
  it is a `LocalHuman` turn. Pick Marsad's default when a Wasl session is Busy: **Queue with a
  bounded wait** (a human's late prompt is usually still wanted) vs **Reject** (surface "busy, try
  again"). **Recommended: `Queue{max_wait}` with a UI-surfaced `Queued{position}` receipt**, with the
  policy overridable per send in a later UI phase.

- **Q5 — Idempotency-key derivation. ✅ DECIDED (v2).** `WaslTurnRequest.idempotency_key` is a
  **UUID (v4 or v7) minted once per logical human send action** and reused verbatim on every retry of
  that same send (§4.4). **Rejected** alternative: a per-process monotonic counter (the v1 proposal) —
  a counter can **collide after a Marsad-core restart while the underlying Wasl session survives**
  (Wasl's session state is independent of Marsad's process lifetime), which would make Wasl silently
  treat a genuine new prompt as a duplicate resend of an old counter value and drop it. A per-send
  UUID costs nothing, needs no persisted counter/store touch, and eliminates that entire class of bug;
  uniqueness (not ordering) is the requirement, so v4 is sufficient and v7 is preferred only for
  time-ordered debuggability.

- **Q6 — Native-gate scope: session-id vs a `wasl-session:<id>` prefix.** §8 scopes native gates by
  the **Marsad session id** (reusing ordinary session-scoped gate routing). X1 operator gates use a
  `wasl:<host_id>` scope. Confirm native gates should use the *plain* session id (cleanest — they
  *are* session-scoped) rather than a distinguishing prefix like `wasl-session:<id>`. A prefix would
  make them visually separable in logs/ledger but complicate the reuse of the existing session-gate
  routing. **Recommended: plain session id** (a native gate is a session's gate, full stop).

- **Q7 — Observe-thread reconnect semantics.** For a real client, the observe stream can drop
  (`ObserveLost`). The fake models it, but the real reconnect/backoff policy (resubscribe from
  `after_seq`, backoff schedule) is the real client's concern in the later phase. Confirm this phase
  only needs to prove the **fake's** replay-from-`after_seq` correctness and the agent's re-pump
  wiring (the `AttachWaslSession{after_seq}` path), leaving the real backoff to mirror
  `marsad-wasl`'s `wasl_backoff_next` later. **Recommended: yes — prove replay + re-pump now, defer
  real backoff.**
