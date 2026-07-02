# X1 — `marsad-wasl` + Wasl operator connector (additive spec)

**Blocks touched:** NEW crate `marsad-wasl` (L1); additive edits to B01 `marsad-proto` (new `IpcMsg`
variants), B03 `marsad-gates` (new `Gate` variant), B12 `marsad-core` (new inbound arms, new
`VmUpdate` variants, `answer_wasl_gate`), B13 `marsad-agent` (runtime-optional connector), plus one
mechanical compile-fix in `apps/desktop/src-tauri/src/events.rs`.
**Coverage:** 100% line+region per touched crate (house gate:
`cargo llvm-cov --fail-under-lines 100 --fail-under-regions 100`).

## Purpose

Wasl (separate repo, `/work/mewais/OpenSource/Wasl`) exposes a generic, consumer-agnostic
**Operator API** — an HTTP+SSE surface for watching a Wasl node and deciding its Tier-3
external-authority gates. The authoritative wire contract is
`Wasl/docs/specs/operator-api.md` plus 16 golden JSON fixtures under
`Wasl/crates/wasl-operator/fixtures/operator-v1/`. Marsad becomes **one** consumer of that surface:
it renders Wasl gates in the GUI, relays the **human's** decision back, and mirrors the observation
(audit) feed so the graph stays complete — the "B12 core / B13 agent (X)" block of
`docs/UPGRADE_PLAN.md` Phase 3.

### Hard rules (normative, non-negotiable)

1. **Strictly optional.** With no Wasl configuration present, Marsad's behavior is bit-for-bit
   unchanged: no threads, no connections, no new IPC traffic, no new `VmUpdate`s. **All existing
   test files pass untouched-green** — this spec adds test files and adds arms to existing `src/`
   match sites, but edits **zero** existing test files (the closed "must change" list of
   `typed-ipc.md` §7 has no analogue here: it is empty).
2. **Marsad relays human decisions only.** There is no auto-approval, no policy engine, no default
   answer, no timeout-answering on Marsad's side. Every `POST /v1/gates/{id}/decision` Marsad ever
   issues is the direct consequence of exactly one explicit GUI action. Unanswered gates are the
   node's problem (its own deadline dismisses them).
3. **No cross-repo code dependency.** Marsad never depends on a Wasl crate. The contract is
   spec + fixtures: the fixture directory is **vendored** (copied verbatim) into this repo (§1) and
   conformance tests run against the vendored copy. Per the Wasl fixture policy, the shared fixture
   directory — not shared code — is the compatibility boundary.
4. **Additive only**, per house rules: existing `IpcMsg` variants, `Gate` variants, `VmUpdate`
   variants, method signatures and wire shapes are untouched. Everything new follows the
   `docs/specs/typed-ipc.md` conventions (§3).

---

## 1. Vendored contract — `fixtures/operator-v1/`

Copy **all 16 files** of `Wasl/crates/wasl-operator/fixtures/operator-v1/` **verbatim** (byte
identical) into `fixtures/operator-v1/` in this repo, next to the existing `fixtures/streams/` and
`fixtures/hooks/`. Add `fixtures/operator-v1/README.md` (new file; the vendored JSON files
themselves get no header/edit of any kind) stating:

- **Origin:** vendored verbatim from the Wasl repo,
  `crates/wasl-operator/fixtures/operator-v1/`, contract doc `docs/specs/operator-api.md`
  (repo-relative pointers plus the upstream repo URL/path).
- **Version:** Operator API **v1** (frozen); captured against `wasl-operator` crate version
  `0.1.0` (the `version` field in `info.json`/`hello.json`); vendoring date.
- **Policy:** the upstream directory is append-only for the life of v1 — an edited or deleted
  fixture upstream is a breaking change (`/v2` + `fixtures/operator-v2/`). Re-vendoring is a
  whole-directory re-copy plus this README's version/date bump; never hand-edit a fixture here.
- **Consumers:** `crates/marsad-wasl/tests/fixtures.rs` asserts every file parses (§7.1); the SSE
  fixtures are `data:` payloads only (SSE `event:`/`id:` framing is prose-specified upstream).

The 16 files (inventory pinned here so a partial vendor is detectable):
`info.json`, `hello.json`, `gate_requested_external.json`, `gate_requested_elicitation.json`,
`gate_decided_approved_operator.json`, `gate_decided_denied_operator.json`,
`gate_decided_dismissed_timeout.json`, `gate_decided_approved_elicitation.json`,
`audit_decision.json`, `audit_message.json`, `gates_snapshot.json`,
`decision_request_approved.json`, `decision_request_denied.json`, `error_unknown_gate.json`,
`error_already_decided.json`, `error_malformed.json`.

---

## 2. New crate — `marsad-wasl` (L1)

`crates/marsad-wasl/{src,tests}`, workspace member, entry in `[workspace.dependencies]`
(`marsad-wasl = { path = "crates/marsad-wasl" }`). Dependencies: `serde`, `serde_json`,
`thiserror` only — **no other marsad crate** (it sits beside `marsad-transport` in the DAG).
New DAG edges: `wasl ← agent, core`. No `agent → core` edge is introduced (unchanged rule).

### 2.1 Typed frame structs (mirror the golden fixtures exactly)

All types: `#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]`, **no
`deny_unknown_fields` anywhere** (the contract's unknown-field tolerance is mandatory). Closed-set
strings from the contract (`kind`, `mode`, `outcome`, `decided_by`, `resume`, `error`) are kept as
**plain `String`** — Marsad renders and relays them; it never branches on an exhaustive enum that a
v1-additive server could outgrow. Timestamps stay RFC 3339 `String`s (Marsad displays them; it
never does clock math against the node's clock).

```rust
/// GET /v1/info body — fixture info.json.
pub struct OperatorInfo { pub agent_id: String, pub host_id: String, pub version: String }

/// `hello` frame payload — fixture hello.json.
pub struct Hello {
    pub api_version: u64,
    pub agent_id: String,
    pub host_id: String,
    pub version: String,
    pub last_seq: u64,
    pub resume: String,          // v1: always "snapshot"
}

/// Embedded GateRequest — verbatim Wasl wire form (inside gate_requested_* and gates_snapshot).
pub struct GateRequest {
    pub id: String,              // the initiating tool-call id — MAY repeat; never correlate by it
    pub kind: String,            // "consult" | "command" | "message" | "transfer"
    pub from: String,
    pub summary: String,
    pub preview: String,
}

/// `gate_requested` frame payload — fixtures gate_requested_{external,elicitation}.json.
pub struct GateRequested {
    pub gate_id: String,
    pub mode: String,            // "elicitation" | "external" ("prepare_commit" reserved)
    pub decidable: bool,
    pub request: GateRequest,
    pub requested_at: String,
    pub timeout_ms: u64,
    pub deadline: String,
}

/// `gate_decided` frame payload — fixtures gate_decided_*.json.
pub struct GateDecided {
    pub gate_id: String,
    pub outcome: String,         // "approved" | "denied" | "dismissed"
    pub decided_by: String,      // "operator" | "elicitation" | "timeout"
    pub reason: Option<String>,  // string iff denied; fixture shows explicit null otherwise
    pub decided_at: String,
}

/// Embedded AuditEntry + the `audit` frame wrapper — fixtures audit_{decision,message}.json.
pub struct AuditEntry {
    pub seq: u64,                // the audit log's own sequence, independent of SSE id
    pub kind: String,            // "consult"|"command"|"message"|"transfer"|"decision"
    pub actor: String,
    pub targets: Vec<String>,
    pub summary: String,
    pub decision: Option<AuditDecision>,   // null on auto-approved sends
}
pub struct AuditDecision {
    pub decision: String,                  // "approve" | "deny"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,            // present on deny
}
pub struct AuditFrame { pub entry: AuditEntry }

/// GET /v1/gates body — fixture gates_snapshot.json.
pub struct GatesSnapshot { pub last_seq: u64, pub gates: Vec<PendingGate> }
pub struct PendingGate {
    pub gate_id: String,
    pub request: GateRequest,
    pub requested_at: String,
    pub age_ms: u64,
    pub deadline: String,
}

/// POST body — fixtures decision_request_{approved,denied}.json.
pub struct DecisionRequest {
    pub outcome: String,                              // "approved"|"denied"|"dismissed"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,                       // only meaningful with "denied"
}

/// 4xx body — fixtures error_*.json.
pub struct ErrorBody { pub error: String, pub detail: String }
```

`DecisionRequest` serialization must reproduce the two POST-body fixtures exactly as
`serde_json::Value` (key order non-normative): approve serializes with **no** `reason` key
(`skip_serializing_if`), deny carries it.

### 2.2 SSE reader (subset sufficient for the contract)

```rust
/// One parsed SSE frame: `event:` type, optional `id:` (absent on hello), raw `data:` payload.
pub struct SseFrame { pub event: String, pub id: Option<u64>, pub data: String }

/// Incremental SSE parser: feed lines, get frames at blank-line dispatch.
/// - `event: X` sets the type; `data: Y` appends payload (single-line in practice; multiple
///   data lines join with '\n' per the SSE spec); `id: N` parses as u64 (unparsable → None).
/// - Comment lines (leading ':', e.g. the ~15 s `: ka` keep-alives) are ignored.
/// - Unknown field names are ignored. A dispatched frame with no `event:` is dropped.
pub struct SseParser { /* private */ }
impl SseParser {
    pub fn new() -> Self;
    pub fn push_line(&mut self, line: &str) -> Option<SseFrame>;
}
```

Unknown **event types** (anything other than `hello`/`gate_requested`/`gate_decided`/`audit`) are
skipped by the client with a stderr log — mandatory forward-compat (mirrors core's tolerant-skip
disposition for unknown IPC types). Their `id:` still advances gap tracking (they occupy sequence
numbers).

### 2.3 Minimal HTTP/1.1 client (std only)

No new third-party deps (the workspace has no HTTP crate; house style is hand-rolled minimal IO
over `std::net`, like `marsad-transport`). `http://host:port` URLs only (v1 defines no TLS/auth;
the node binds loopback/trusted interfaces — deployment concern, quoted from the contract).

- `GET` streaming (for `/v1/events`): sends `GET <path> HTTP/1.1`, `Host`, `Accept:
  text/event-stream`; **never sends `Last-Event-ID`** (the server ignores it; v1 has no replay).
  Handles `Transfer-Encoding: chunked` and raw/`Content-Length`-less streaming bodies, yielding
  decoded lines to `SseParser`.
- `GET` buffered (for `/v1/gates`, `/v1/info`): chunked or `Content-Length` bodies.
- `POST` JSON (for decisions): sends `Content-Type: application/json`; returns
  `(status: u16, body: Vec<u8>)`.
- Read timeouts so a dead peer can't hang the connector forever; connect failures and mid-stream
  EOF/IO errors surface as `WaslError::Io` and trigger reconnect (§2.5).

### 2.4 Domain output — `WaslEvent`

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum WaslEvent {
    /// hello parsed and api_version verified (== 1).
    Connected { hello: Hello },
    GateRequested(GateRequested),
    GateDecided(GateDecided),
    Audit(AuditEntry),
    /// A tracked pending gate is absent from a reconcile snapshot and no gate_decided
    /// frame was seen: it resolved while this connection lagged/was down. The outcome is
    /// UNKNOWN — the connector never invents one (hard rule 2 adjacence: we do not
    /// fabricate node decisions, we only report that the gate is gone).
    GateVanished { gate_id: String },
    /// Connection lost; the client will retry with backoff. Informational (agent logs it).
    Disconnected { error: String },
}
```

### 2.5 `WaslOperatorClient` — connect / reconnect / gap / resnapshot (normative algorithm)

```rust
pub struct WaslClientConfig {
    pub base_url: String,          // e.g. "http://127.0.0.1:8787"
    pub initial_backoff_ms: u64,   // production default 1_000; tests use 1
}
pub const WASL_BACKOFF_MAX_MS: u64 = 60_000;
/// Pure: double, capped (mirrors marsad_agent::reconnect_backoff_next, which marsad-wasl
/// cannot import — the DAG points the other way).
pub fn wasl_backoff_next(current_ms: u64) -> u64;

pub struct WaslOperatorClient { /* private */ }
impl WaslOperatorClient {
    pub fn new(cfg: WaslClientConfig) -> Self;
    /// Blocking loop; returns only when `stop` is set. Runs on a dedicated thread.
    pub fn run(
        &mut self,
        events: std::sync::mpsc::Sender<WaslEvent>,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    );
    /// One-shot decision POST on a fresh connection (callable from any thread).
    pub fn post_decision(&self, gate_id: &str, outcome: &str, reason: Option<&str>)
        -> Result<DecisionAck, WaslError>;
}

#[derive(Debug, Clone, PartialEq)]
pub enum DecisionAck {
    /// 204 — decision DELIVERED to the gate (contract: not proof any send succeeded;
    /// the gate_decided frame + audit feed are the observables).
    Delivered,
    /// 409 already_decided — benign race (an earlier POST, elicitation, or timeout won).
    AlreadyDecided,
    /// 404 unknown_gate — never a decidable pending gate on this node.
    UnknownGate,
}
// 400 malformed and transport failures are WaslError (Marsad only ever sends
// schema-correct bodies, so 400 is a bug, not an ack).
```

`run()` per connection (exactly the contract's prescribed reconnect semantics):

1. Open `GET /v1/events`; read frames via `SseParser`.
2. First frame MUST be `hello` (no `id:`). `api_version != 1` → log, drop the connection, back
   off, retry (never guess at a future major). Emit `Connected`. Set `last_id = hello.last_seq`.
3. **Resnapshot (mandatory on every connect/reconnect):** `GET /v1/gates`, then reconcile against
   the client's `known_pending: HashMap<gate_id, PendingGate-ish>` set:
   - snapshot gate not in `known_pending` → track it, emit `GateRequested` **synthesized** from
     the `PendingGate` (`mode: "external"`, `decidable: true` — the snapshot only ever contains
     decidable external gates; `timeout_ms: 0` — unknown from a snapshot; `deadline` is
     authoritative for any countdown).
   - `known_pending` gate absent from snapshot → untrack, emit `GateVanished`.
4. Stream loop: for each subsequent frame, if `id != last_id + 1` → **gap**: the connection missed
   frames. Per the contract, do **not** reconnect — keep the stream open, set `last_id = id`,
   process the frame, then re-run step 3 (resnapshot + reconcile). Otherwise `last_id = id`.
   - `gate_requested` → track iff `decidable` (observe-only gates are never pending), emit.
   - `gate_decided` → untrack, emit (also for gates never tracked — e.g. decided-while-lagged
     races after a reconcile already vanished them; downstream handling is idempotent).
   - `audit` → emit `Audit(frame.entry)`.
   - keep-alive comments: consumed silently by `SseParser`.
5. IO error / EOF → emit `Disconnected`, sleep current backoff, `wasl_backoff_next`, go to 1.
   A successful step 2 resets backoff to `initial_backoff_ms`. `stop` is checked between reads
   (read timeout bounds the check latency).

Frame-order guarantees relied on (and nothing more): ascending `id` per connection;
`gate_requested` precedes its `gate_decided`. Marsad never assumes where a gate's audit `decision`
entry falls relative to its `gate_decided` frame. Correlation is by `gate_id` **only** (never
`request.id`).

### 2.6 Trait + fake (the agent's seam)

```rust
pub trait WaslConnector: Send {
    /// Spawn the streaming side; events flow into `events` until `stop()`/drop.
    fn start(&mut self, events: std::sync::mpsc::Sender<WaslEvent>);
    /// Relay one human decision (blocking, bounded by the HTTP timeouts).
    fn decide(&self, gate_id: &str, outcome: &str, reason: Option<&str>)
        -> Result<DecisionAck, WaslError>;
    fn stop(&mut self);
}

/// Real impl: start() spawns a thread running WaslOperatorClient::run; decide() = post_decision.
impl WaslConnector for WaslOperatorClient { … }

/// Test fake: `push(event)` feeds the events channel; `decide` records
/// `(gate_id, outcome, reason)` into an inspectable log and returns a scripted ack
/// (default Delivered; `script_ack(…)` overrides, incl. Err).
pub struct FakeWaslConnector { /* public inspection: pub decisions: Arc<Mutex<Vec<…>>> */ }
```

### 2.7 Config types (shared shape for the agent's config file)

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaslOperatorsConfig { pub operators: Vec<WaslOperatorEntry> }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaslOperatorEntry {
    /// Marsad host-registry id this node belongs to ("local" for the local host).
    pub host_id: String,
    /// Operator API base URL, reachable FROM THE AGENT (loopback for a local node; a
    /// user-managed tunnel/trusted bind for a remote one — see Non-goals §9).
    pub url: String,
}
```

`WaslError`: `Io(String)`, `Http { status: u16, body: String }`, `Protocol(String)`
(bad hello / unparsable known frame / api_version mismatch), via `thiserror`.

---

## 3. B01 `marsad-proto` — additive `IpcMsg` variants

Five new variants, exactly per the `typed-ipc.md` conventions: internally tagged `"type"`,
PascalCase tags, no `deny_unknown_fields`, `#[serde(default)]` parse tolerance on every
non-identity field, `serde_json::Value` for payloads whose typed form lives in another crate
(precedent: `ChangesUpdate.changes` ↔ `marsad-viewmodel::FileChange`; here `request`/`entry` ↔
`marsad-wasl::{GateRequest, AuditEntry}` — proto must not depend on wasl, DAG). `host_id` and
`gate_id` are identity fields: required on parse (missing → `SchemaMismatch` →
`Protocol` error, same class as `session_id` on existing variants).

```rust
    // ── GUI/core → agent ────────────────────────────────────────────────────
    /// NEW: relay ONE explicit human decision on a Wasl gate to the node's
    /// operator endpoint. Marsad-originated only — never synthesized (hard rule 2).
    AnswerWaslGate {
        host_id: String,
        gate_id: String,
        /// Wasl outcome string, relayed verbatim: "approved" | "denied"
        /// ("dismissed" is wire-valid for forward compat; core never emits it in X1).
        outcome: String,
        /// Denial reason. ALWAYS serialized (null when absent), like LaunchSession.first_prompt.
        #[serde(default)]
        reason: Option<String>,
    },

    // ── agent → GUI/core ────────────────────────────────────────────────────
    /// NEW broadcast: a Wasl gate surfaced (or re-surfaced via resnapshot reconcile).
    WaslGateRequested {
        host_id: String,
        gate_id: String,
        #[serde(default)] mode: String,
        #[serde(default)] decidable: bool,
        /// Verbatim Wasl GateRequest object; Value ON PURPOSE (typed form is
        /// marsad_wasl::GateRequest; core parses leniently — §5.2).
        #[serde(default)] request: serde_json::Value,
        #[serde(default)] requested_at: String,
        #[serde(default)] timeout_ms: u64,
        #[serde(default)] deadline: String,
    },
    /// NEW broadcast: a Wasl gate resolved.
    WaslGateDecided {
        host_id: String,
        gate_id: String,
        /// Wasl outcome verbatim, PLUS "unknown" for connector-synthesized
        /// GateVanished (Marsad-wire-only value; never sent to Wasl).
        outcome: String,
        /// Wasl decided_by verbatim, PLUS "resync" for GateVanished.
        #[serde(default)] decided_by: String,
        #[serde(default)] reason: Option<String>,   // always serialized, null when absent
        /// RFC 3339 from the frame; "" for GateVanished (no node timestamp exists).
        #[serde(default)] decided_at: String,
    },
    /// NEW broadcast: one audit-feed entry (the observation feed — EVERY durable
    /// audit append, including auto-approved sends).
    WaslObservation {
        host_id: String,
        /// Verbatim Wasl AuditEntry; Value ON PURPOSE (same rationale as `request`).
        #[serde(default)] entry: serde_json::Value,
    },
    /// NEW ack (serve-loop reply to AnswerWaslGate): the decision was RELAYED to the
    /// connector — not that the POST succeeded (the WaslGateDecided broadcast is the
    /// authoritative resolution signal, mirroring the contract's own 204 semantics).
    WaslGateAnswered { host_id: String, gate_id: String },
```

`parse_ipc`'s `KNOWN` list grows by exactly these 5 tags (29 → 34). `encode_ipc` unchanged.

### Golden wire forms (normative; values lifted from the vendored fixtures)

```json
{"type":"AnswerWaslGate","host_id":"local","gate_id":"gate-1","outcome":"approved","reason":null}
{"type":"AnswerWaslGate","host_id":"local","gate_id":"gate-1","outcome":"denied","reason":"not during business hours"}
{"type":"WaslGateRequested","host_id":"local","gate_id":"gate-1","mode":"external","decidable":true,"request":{"id":"call-9","kind":"message","from":"wasl-a","summary":"message to 1 peer(s)","preview":"hi bob"},"requested_at":"2026-07-02T12:00:00.000Z","timeout_ms":120000,"deadline":"2026-07-02T12:02:00.000Z"}
{"type":"WaslGateDecided","host_id":"local","gate_id":"gate-1","outcome":"approved","decided_by":"operator","reason":null,"decided_at":"2026-07-02T12:00:03.500Z"}
{"type":"WaslGateDecided","host_id":"local","gate_id":"gate-1","outcome":"denied","decided_by":"operator","reason":"not during business hours","decided_at":"2026-07-02T12:00:03.500Z"}
{"type":"WaslGateDecided","host_id":"local","gate_id":"gate-7","outcome":"unknown","decided_by":"resync","reason":null,"decided_at":""}
{"type":"WaslObservation","host_id":"local","entry":{"seq":3,"kind":"decision","actor":"wasl-a","targets":["wasl-b"],"summary":"gate Message","decision":{"decision":"approve"}}}
{"type":"WaslGateAnswered","host_id":"local","gate_id":"gate-1"}
```

Backward compat: **old core ← new agent** — the three broadcast tags hit `on_agent_line`'s
tolerant-skip (`UnknownDiscriminant` → logged no-op), and the old desktop reader's type-string
pre-filter likewise skips them; nothing wedges. **old agent ← new core** — a new core only emits
`AnswerWaslGate` when a Wasl gate is pending, which an old agent can never have broadcast; if it
somehow arrives, the agent's unknown-type arm replies `Error` (fire-and-forget, unchanged class).

---

## 4. B13 `marsad-agent` — runtime-optional connector

Compile-time dep on `marsad-wasl` (add to `Cargo.toml`); **zero runtime behavior without config**.

### 4.1 Configuration — `<data_root>/wasl.json` (per host)

`AgentConfig` is **not** touched (existing tests construct it as a struct literal; a new field
would break them — forbidden). Instead the agent reads an optional file at serve-time:

```json
{ "operators": [ { "host_id": "local", "url": "http://127.0.0.1:8787" } ] }
```

```rust
/// Pure-ish, unit-testable: read <data_root>/wasl.json.
/// Absent file → None (THE inert path). Malformed JSON / empty `operators` → None + one
/// stderr line (config problems must never take the agent down). Duplicate host_id →
/// first entry wins + stderr line.
pub fn load_wasl_config(data_root: &str) -> Option<marsad_wasl::WaslOperatorsConfig>;
```

One entry per Wasl node (= per host, matching the daemon-mesh topology); `host_id` ties frames to
Marsad's host registry ids for the GUI. The URL must be reachable from the agent (§9 for remotes).

### 4.2 Serve wiring (guarded entirely by `Some(config)`)

In `serve()`, after bind, `if let Some(cfg) = load_wasl_config(&data_root)` — the whole block below
is inside this guard (the optionality proof point):

- For each entry, build a connector via the factory seam
  (`Box<dyn WaslConnector>`; default factory constructs
  `WaslOperatorClient::new(WaslClientConfig { base_url: entry.url, initial_backoff_ms: 1000 })`):

  ```rust
  /// Additive test seam; default = real client. Setter so no constructor changes.
  pub fn set_wasl_connector_factory(
      &mut self,
      f: Box<dyn Fn(&marsad_wasl::WaslOperatorEntry) -> Box<dyn marsad_wasl::WaslConnector> + Send>,
  );
  ```

- Register the connector in `wasl_connectors: Arc<Mutex<HashMap<String /*host_id*/, …>>>` (shared
  with the serve thread for `AnswerWaslGate`; an `opaque_debug_wrap` field like the other
  registries).
- Spawn one forwarder thread per entry: `connector.start(tx)`; the thread drains `rx` and pushes
  `wasl_event_lines(&entry.host_id, &event)` into the existing broadcast `subscribers` list —
  byte-identical mechanics to the session FIFO reader threads.

```rust
/// Pure mapping (unit-testable without sockets), house style like reconnect_backoff_next:
/// WaslEvent → zero or more encode_ipc'd broadcast lines.
///   Connected{..}        → []               (stderr log only)
///   GateRequested(g)     → [WaslGateRequested{ host_id, …g, request: to_value(g.request) }]
///   GateDecided(d)       → [WaslGateDecided{ host_id, …d }]
///   Audit(e)             → [WaslObservation{ host_id, entry: to_value(e) }]
///   GateVanished{gate_id}→ [WaslGateDecided{ host_id, gate_id, outcome:"unknown",
///                            decided_by:"resync", reason:None, decided_at:"" }]
///   Disconnected{..}     → []               (stderr log only)
pub fn wasl_event_lines(host_id: &str, ev: &marsad_wasl::WaslEvent) -> Vec<String>;
```

### 4.3 `handle_ipc` — the decision relay (new arm)

`IpcMsg::AnswerWaslGate { host_id, gate_id, outcome, reason }`:

1. Look up `host_id` in `wasl_connectors`. Absent (incl. the no-config case, where the map is
   empty) → `Err(AgentError::Protocol("no wasl operator configured for host '<id>'"))` → the serve
   loop's standard `Error` reply. **This is the entire no-config surface of the new arm.**
2. Present → `connector.decide(&gate_id, &outcome, reason.as_deref())`, then reply
   `IpcMsg::WaslGateAnswered { host_id, gate_id }` regardless of ack value:
   - `Delivered` — done; the node's `gate_decided` frame is the resolution signal.
   - `AlreadyDecided` (409) — benign race; stderr log; the frame/resnapshot already covers the UI.
   - `UnknownGate` (404) — stderr log (a stale UI answered a dead gate; nothing to do).
   - `Err(WaslError)` (400/transport) → `Err(AgentError::Protocol(…))` → `Error` reply (the GUI
     learns the relay itself failed).

  `decide()` is a short bounded HTTP POST (connect+read timeouts, §2.3); it runs on the serve
  thread like every other command — acceptable because it is strictly bounded, and command
  ordering for gate answers is desirable.

The agent never originates a decision: the only call site of `decide()` is this arm (rule 2).

---

## 5. B03 `marsad-gates` + B12 `marsad-core`

### 5.1 `marsad-gates` — the `Gate::Handoff` variant (define it)

Additive variant (the P1 "handoff/consult/command" gate family from UPGRADE_PLAN Phase 3, first
inhabited by Wasl-originated gates):

```rust
pub enum Gate {
    …existing Permission / Plan / Edit / Question unchanged…,
    /// An agent-to-agent handoff gate surfaced by an external connector (X1: Wasl).
    Handoff {
        /// Connector-scope gate id — THE identity field (Wasl: gate_id, unique for the
        /// node's process lifetime; NEVER request.id, which may repeat).
        gate_id: String,
        kind: String,        // "consult" | "command" | "message" | "transfer"
        from: String,
        summary: String,
        preview: String,
    },
}
```

- `gate_canonical` gains: `Gate::Handoff { gate_id, .. } => format!("handoff\x00{session_id}\x00{gate_id}")`
  — identity = (scope, gate_id) only; `kind/from/summary/preview` are display payload, exactly like
  `Gate::Question`'s `request_id`-only identity.
- The `session_id` slot carries the **Wasl scope string** `wasl:<host_id>` (Wasl gates belong to a
  host's node, not to a Marsad session; `route()` still returns exactly one scope — the invariant
  "a decision routes to exactly one owner" is preserved, the owner is just a node scope).
- `approve`/`reject`/`decision_id`/`DecisionLedger` work unchanged on the new variant.

Downstream exhaustive matches that MUST gain a (small, additive) arm to compile:
- `marsad-format::gate_meta` (`crates/marsad-format/src/lib.rs:176`): `Gate::Handoff { kind, from,
  preview, .. }` → `GateMeta { kind: "Handoff", icon: "⇄", lead: "Wasl <kind> from <from>",
  summary: preview (fallback to summary when preview is empty), approve_label: "Approve & send" }`.
- `marsad-viewmodel::gate_icon` (`crates/marsad-viewmodel/src/lib.rs:453`):
  `Gate::Handoff { .. } => "\u{21c4}"` (⇄). (The `matches!`/`if let` sites in `gate_vm` are
  non-exhaustive and need no edits: a Handoff gate yields all-false flags + empty diff rows,
  which is correct — the dedicated Wasl approval UI is X4.)
- `marsad-events` constructs but never exhaustively matches `Gate` — no edit.

Each new arm gets additive tests (new test files) to hold the 100/100 bar (§7).

### 5.2 `marsad-core` — inbound arms, `VmUpdate`s, `answer_wasl_gate`

New dep: `marsad-wasl` (core already depends on everything). New private state:

```rust
/// Pending Wasl gates, keyed by (host_id, gate_id). Node-authoritative: entries are
/// added by WaslGateRequested and removed ONLY by WaslGateDecided (incl. synthesized
/// "unknown"/"resync" ones) — answer_wasl_gate does NOT optimistically remove.
wasl_gates: std::collections::BTreeMap<(String, String), WaslGateView>,
```

```rust
/// UI-shaped pending-gate row (BTreeMap keeps VmUpdate::WaslGates deterministically ordered).
#[derive(Debug, Clone, PartialEq)]
pub struct WaslGateView {
    pub host_id: String,
    pub gate_id: String,
    pub kind: String,
    pub from: String,
    pub summary: String,
    pub preview: String,
    pub requested_at: String,
    pub deadline: String,       // authoritative for countdown UI (timeout_ms may be 0 on resync)
}

pub enum VmUpdate {
    …existing variants unchanged…,
    /// Full pending Wasl-gate list (Hosts-style full-list semantics: idempotent to re-render,
    /// resync-friendly — duplicate WaslGateRequested collapses to the same list).
    WaslGates(Vec<WaslGateView>),
    /// One gate resolved (toast/history material). outcome/decided_by verbatim from the wire
    /// (incl. "unknown"/"resync" for lag-resync synthesis).
    WaslGateDecided { host_id: String, gate_id: String, outcome: String,
                      decided_by: String, reason: Option<String> },
    /// One observation-feed entry (typed; graph/feed material for X4).
    WaslObservation { host_id: String, entry: marsad_wasl::AuditEntry },
}
```

New `CoreError` variant (additive, core-ext precedent):
`#[error("unknown wasl gate: {0}")] UnknownWaslGate(String)`.

**`on_agent_line` additive arms** (before the misdirected-catchall; the catchall keeps rejecting
`AnswerWaslGate` inbound as a routing bug automatically since it is a GUI→agent tag):

- `IpcMsg::WaslGateRequested { .. }` → parse `request` Value via
  `serde_json::from_value::<marsad_wasl::GateRequest>`; malformed → **tolerant skip** (stderr log,
  `Ok(vec![])` — observational data must never wedge the reader loop; ChangesUpdate-leniency
  precedent). `decidable == false` → tolerant skip too (observe-only gates are stream-visible but
  never pending/answerable; they surface via the audit feed instead). Else upsert into
  `wasl_gates` and emit `vec![VmUpdate::WaslGates(current full list)]` (upsert of an existing key
  re-emits the same list — idempotent).
- `IpcMsg::WaslGateDecided { .. }` → remove `(host_id, gate_id)` if present; emit
  `VmUpdate::WaslGateDecided{…}` always, plus `VmUpdate::WaslGates(list)` iff a removal happened
  (unknown gate decided → decided event only; already-cleared races are silent-consistent).
- `IpcMsg::WaslObservation { .. }` → parse `entry` leniently (malformed → tolerant skip); emit
  `vec![VmUpdate::WaslObservation{…}]`.
- `IpcMsg::WaslGateAnswered { .. }` → joins the existing ack no-op list (`Ok(vec![])`).

**`answer_wasl_gate`** — the human decision path, through the ledger:

```rust
/// Relay ONE explicit human decision on a pending Wasl gate. NO session lookup
/// (Wasl gates are host-scoped, not session-scoped — deliberately not require_session).
pub fn answer_wasl_gate(
    &mut self,
    host_id: &str,
    gate_id: &str,
    decision: marsad_gates::Decision,      // Approve | Reject{reason} — the human's verdict
) -> Result<Vec<VmUpdate>, CoreError> {
    let view = self.wasl_gates.get(&(host_id.into(), gate_id.into()))
        .ok_or_else(|| CoreError::UnknownWaslGate(format!("{host_id}/{gate_id}")))?;
    let gate = marsad_gates::Gate::Handoff { gate_id: …, kind: …, from: …, summary: …, preview: … };
    let scope = format!("wasl:{host_id}");
    let gd = match &decision {
        Decision::Approve         => marsad_gates::approve(&scope, &gate),
        Decision::Reject{reason}  => marsad_gates::reject(&scope, &gate, reason),
    };
    // Idempotency: identical to answer_gate — seen → silent no-op, no duplicate IPC.
    if self.ledger.seen(&gd.id) { return Ok(vec![]); }
    self.ledger.record(&gd).unwrap();   // infallible: seen() was false
    let (outcome, reason) = match gd.decision {
        Decision::Approve        => ("approved".to_string(), None),
        Decision::Reject{reason} => ("denied".to_string(),   Some(reason)),
    };
    self.send_ipc(&IpcMsg::AnswerWaslGate {
        host_id: host_id.into(), gate_id: gate_id.into(), outcome, reason })?;
    Ok(vec![])   // node-authoritative: the pending list changes only on WaslGateDecided
}
```

Notes: `decision_id` is content-stable over `(wasl:<host_id>, gate_id)`, so approve-then-deny of
the same gate is ALSO deduped (one decision per gate per core lifetime — the node would 409 the
second POST anyway; the ledger just saves the wire trip and matches `answer_gate` semantics
exactly). `"dismissed"` is deliberately not producible here (no `Decision` maps to it) — a human
who ignores a gate lets the node's own deadline dismiss it (rule 2).

### 5.3 Desktop mechanical compile-fix (NOT the Wasl UI — that is X4)

`apps/desktop/src-tauri/src/events.rs` has an exhaustive `impl From<VmUpdate> for
VmUpdateEnvelope`; new variants break its compile. Mechanical change, behavior-preserving for all
existing variants: replace the `From` impl with `fn envelope_of(u: VmUpdate) ->
Option<VmUpdateEnvelope>` (same arms, wrapped in `Some`; the three `Wasl*` variants → `None`) and
have `emit_updates` skip `None`. The desktop thus compiles and ignores Wasl updates until X4 wires
the approval modal/graph (which will also reuse the agent-spawn `adopt_or_spawn_agent` path
unchanged — the connector lives in the agent, so the GUI needs no new process plumbing). No
`src-tauri` tests exist to update; `apps/desktop/src` (React) is untouched.

---

## 6. Optionality proof (normative)

**Definition of inert:** no `fixtures/…wasl.json`-style config anywhere ⇒ observable behavior of
every existing surface is unchanged, and no Wasl code path executes beyond `load_wasl_config`
returning `None`.

Mechanisms (each pinned by a test in §7):
1. Agent: `AgentConfig` unchanged; the entire connector block in `serve()` is guarded by
   `load_wasl_config(...) == Some(…)`; absent `<data_root>/wasl.json` → `None` → zero threads,
   zero connections, empty `wasl_connectors` map.
2. Agent `handle_ipc`: the only new arm requires an `AnswerWaslGate` line, which nothing emits
   unless a Wasl gate was first broadcast; if hand-fed anyway, the empty connector map yields the
   standard `Error` reply (no side effects).
3. Core: new `on_agent_line` arms require inbound tags no configless agent ever sends;
   `answer_wasl_gate` is a new method nothing calls; `wasl_gates` stays empty; no new `VmUpdate`
   is ever constructed.
4. Proto/gates: new enum variants alter no existing tag/shape/id derivation
   (`gate_canonical` for existing variants is byte-identical; `KNOWN` only grows).
5. **Zero existing test files change.** All existing suites — including the raw-JSON compat corpora
   in `crates/marsad-{agent,core}/tests/` and integration `i01–i11` — must pass **without any
   edit** (contrast: typed-ipc.md §7 had a closed must-change list; X1's list is EMPTY). The only
   existing-file edits in the whole block are `src/` (new arms/variants/methods), `Cargo.toml`s,
   and the §5.3 desktop mapping; CI proof = green run of the untouched suites on the X1 branch.

---

## 7. Test plan (house style: black-box `tests/`, buckets, 100% line+region per crate)

### 7.1 `marsad-wasl` — `tests/{fixtures,happy,errors,branches,fakes}.rs`

**Fixture conformance (the vendored-contract half; `tests/fixtures.rs`):** for every one of the 16
files in `fixtures/operator-v1/` (path via `concat!(env!("CARGO_MANIFEST_DIR"),
"/../../fixtures/operator-v1/…")`):
- parse into its typed struct (`info→OperatorInfo`, `hello→Hello`,
  `gate_requested_*→GateRequested`, `gate_decided_*→GateDecided`, `audit_*→AuditFrame`,
  `gates_snapshot→GatesSnapshot`, `decision_request_*→DecisionRequest`, `error_*→ErrorBody`) and
  assert the exact field values pinned in §2.1/§1 (full equality, not just is-ok — e.g.
  `gate_decided_dismissed_timeout`: `outcome=="dismissed" && decided_by=="timeout" &&
  reason==None`; `audit_message`: `decision==None`; `gate_requested_elicitation`:
  `decidable==false`).
- unknown-field tolerance: every fixture re-parsed with an injected `"x_future_field":123`
  (top-level and inside `request`/`entry`) parses to the same value — loop-driven.
- `DecisionRequest` golden **encode**: serialize the two constructed values and compare to the two
  POST-body fixtures as `serde_json::Value` (approve emits no `reason` key).
- inventory pin: exactly 16 `.json` files exist in the vendored dir (a partial/extended vendor
  fails loudly).

**SSE + client against a fake server (`tests/happy.rs` / `tests/errors.rs`):** an in-test
`std::net::TcpListener` on `127.0.0.1:0` serving scripted HTTP (this is the loopback-real tier —
the real client code path end to end; `initial_backoff_ms: 1` keeps it fast):
- hello-then-frames: scripted `/v1/events` (chunked encoding) emitting `hello` (fixture payload),
  `: ka` comments, then `gate_requested_external` (`id: 43`), `gate_decided_approved_operator`
  (`id: 44`) — client emits `Connected`, `GateRequested`, `GateDecided` with fixture-equal
  payloads; `/v1/gates` served empty (`{"last_seq":42,"gates":[]}`) for the connect resnapshot.
- connect-resnapshot: `/v1/gates` returns `gates_snapshot.json` → `GateRequested` synthesized
  (`mode=="external"`, `decidable==true`, `timeout_ms==0`, request/deadline fixture-equal).
- **gap → resnapshot, stream stays open:** frames with ids 43 then 45 (44 skipped); assert the
  server sees a second `GET /v1/gates` and the TCP stream is NOT reopened; a tracked gate absent
  from that snapshot yields `GateVanished`; a snapshot gate already tracked yields no duplicate.
- reconnect with backoff: server drops the connection after hello; assert a fresh `GET /v1/events`
  arrives, `Disconnected` was emitted, and the retry delays follow `wasl_backoff_next`
  (pure-function table test: 1→2→…→cap 60_000, plus 0 and cap inputs).
- decision POST: scripted `204` (no body) → `Delivered`; `409` + `error_already_decided.json` →
  `AlreadyDecided`; `404` + `error_unknown_gate.json` → `UnknownGate`; `400` +
  `error_malformed.json` → `Err(WaslError::Http{status:400,…})`; request-side assertions: path
  `/v1/gates/gate-1/decision`, `Content-Type: application/json`, body Value-equal to the matching
  `decision_request_*` fixture; connection refused → `Err(Io)`.
- errors/branches: non-hello first frame; `api_version: 2` hello → drop+retry; unknown event type
  skipped but id advances gap tracking; `id:` unparsable → `None`; multi-`data:` join;
  chunked + Content-Length body variants; observe-only `gate_requested` not tracked (its later
  absence from a snapshot yields no `GateVanished`).

**`tests/fakes.rs`:** `FakeWaslConnector` — pushed events arrive on the channel; `decide` records
args and returns scripted acks/errors; `stop` idempotent.

Coverage bar: `cargo llvm-cov -p marsad-wasl --fail-under-lines 100 --fail-under-regions 100`
(all IO is loopback-testable; unreachable branches deleted, not waived).

### 7.2 `marsad-proto` — NEW `tests/wasl_ipc.rs` (existing proto tests untouched)

For each of the 5 new variants, the standard M2 quartet against the §3 goldens: golden parse
(exact typed value), encode→parse→re-encode identity, golden encode (Value equality), tolerance
(injected `x_future_field` + absent `#[serde(default)]` fields parse; missing `host_id`/`gate_id`
→ `SchemaMismatch`). Plus: `KNOWN` closure — all 34 tags parse, `FutureMsg` still
`UnknownDiscriminant`; `AnswerWaslGate.reason: None` serializes as literal `null` (present key);
the `outcome:"unknown"/decided_by:"resync"/decided_at:""` synthesized golden round-trips.

### 7.3 `marsad-gates` — NEW `tests/handoff.rs`

`decision_id` stability (same `(scope, gate_id)` twice → equal across constructions);
distinct `gate_id` / distinct scope → distinct ids; payload fields (`kind/from/summary/preview`)
do NOT affect identity; `approve`/`reject`/`route` on Handoff; ledger: record → seen →
`DuplicateDecision` on re-record; `to_control_response` still functions on a Handoff-derived
decision (id passthrough). Existing gates tests untouched (existing canonicals unchanged).

### 7.4 `marsad-format` / `marsad-viewmodel` — NEW `tests/handoff.rs` each

`gate_meta(Handoff)` full-field assertions incl. the empty-`preview` fallback branch;
`gate_icon(Handoff) == "⇄"`; `gate_vm` with a Handoff gate → all `is_*` false, empty diff rows.
(Closes the new arms' regions; existing files untouched.)

### 7.5 `marsad-agent` — NEW `tests/wasl.rs` (fake connector; no sockets)

- **absent-config = inert:** `load_wasl_config` on a temp data_root with no `wasl.json` → `None`;
  malformed JSON → `None`; `operators: []` → `None`; valid file → exact entries; duplicate
  host_id → first wins. With no config, `handle_ipc` of a valid `AnswerWaslGate` line →
  `AgentError::Protocol` (empty connector map) and **no other observable effect**.
- broadcast round-trips: `wasl_event_lines` for every `WaslEvent` variant → each line re-parsed
  with `parse_ipc` equals the exact §3 typed value (incl. `Connected`/`Disconnected` → `[]`, and
  `GateVanished` → the synthesized `unknown/resync/""` golden).
- decision relay: register a `FakeWaslConnector` (via `set_wasl_connector_factory` + a config
  file in the temp data_root, or direct registry seam), feed `AnswerWaslGate` lines → fake's
  decision log gets `("gate-1","approved",None)` / `("gate-1","denied",Some(…))`; reply line
  parses as `WaslGateAnswered`; scripted `AlreadyDecided`/`UnknownGate` still ack (benign);
  scripted `Err` → `Protocol` error (serve loop would reply `Error`).

### 7.6 `marsad-core` — NEW `tests/wasl.rs` (loopback transport, house pattern)

- reduce arms: `WaslGateRequested` golden line → `[VmUpdate::WaslGates([view])]` with exact
  `WaslGateView` fields; duplicate line → same single-entry list (idempotent); second gate →
  two-entry deterministic order; `decidable:false` → `Ok(vec![])`; malformed `request` Value →
  tolerant `Ok(vec![])`; missing `host_id`/`gate_id` → `CoreError::Protocol`;
  `WaslGateDecided` for a pending gate → `[WaslGateDecided, WaslGates(shrunk)]`; for an unknown
  gate → `[WaslGateDecided]` only; synthesized `unknown/resync` variant clears too;
  `WaslObservation` (both audit fixtures' entries) → typed `VmUpdate::WaslObservation`; malformed
  `entry` → tolerant skip; `WaslGateAnswered` inbound → `Ok(vec![])`; `AnswerWaslGate` inbound →
  misdirected `Protocol` error.
- `answer_wasl_gate`: pending gate + Approve → exactly one transport line, `parse_ipc`-typed-equal
  to the approve golden; Reject{reason} → denied golden; **ledger idempotency:** second call
  (same or different verdict) → `Ok(vec![])`, still exactly one line ever sent; unknown gate →
  `CoreError::UnknownWaslGate`; no `require_session` interaction (works with zero sessions);
  pending list NOT shrunk by answering (only by the decided broadcast).
- **absent-config = inert (core side):** a fresh `Core` emits no `Wasl*` `VmUpdate` from any
  existing flow — pinned implicitly by every existing core test passing untouched (§6.5) plus an
  explicit `snapshot`/`on_agent_line`-of-existing-corpus spot-check asserting no `Wasl*` variant
  appears.

### 7.7 Cross-cutting

- Coverage bars: every touched crate keeps/reaches
  `cargo llvm-cov --fail-under-lines 100 --fail-under-regions 100`.
- Integration `i01–i11`: run unmodified, must pass (no Wasl config exists in their environments —
  they double as the end-to-end inertness proof).
- Live Marsad↔Wasl e2e (real node, real HITL gate) is the Phase-3 ladder's final rung — out of X1
  scope, parked with X4 (UI) since a human approval surface is required to exercise it honestly.

---

## 8. Edit scope (directory-access contract)

- NEW: `crates/marsad-wasl/{src,tests,Cargo.toml,AGENTS.md}`, `fixtures/operator-v1/` (16 vendored
  files + README), new test FILES listed in §7 under existing crates' `tests/`.
- Existing-file edits (src-only, additive): root `Cargo.toml` (member + workspace dep);
  `crates/marsad-proto/src/lib.rs` (5 variants + KNOWN); `crates/marsad-gates/src/lib.rs`
  (Handoff + canonical arm); `crates/marsad-format/src/lib.rs`, `crates/marsad-viewmodel/src/lib.rs`
  (one arm each); `crates/marsad-core/{src/lib.rs,Cargo.toml}`;
  `crates/marsad-agent/{src/lib.rs,Cargo.toml}`; `apps/desktop/src-tauri/src/events.rs` (§5.3);
  `ARCHITECTURE.md` DAG line (`wasl ← agent, core`).
- **Forbidden:** edits to any existing file under any `tests/` directory, `fixtures/streams/`,
  `fixtures/hooks/`, `apps/desktop/src/`, or any vendored fixture.

## 9. Non-goals

- No Wasl-side changes; no shared crate; no `/v2` anticipation beyond tolerance rules.
- No auto-approval/policy/defaults; no Marsad-emitted `"dismissed"`; no answering on timeout.
- No GUI rendering (approval modal, graph edges, observation feed UI) — X4, which consumes the
  `Wasl*` `VmUpdate`s defined here over the existing `adopt_or_spawn_agent` bridge unchanged.
- No auth/TLS (v1 contract defines none; loopback/trusted-bind deployment note quoted in §2.3) and
  no automatic SSH tunneling to remote nodes' loopback-bound operators — X1 requires an
  agent-reachable URL; tunneling is a follow-up (candidate X4/X5) noted in `wasl.json` docs.
- No persistence of Wasl gates/observations (`marsad-store` untouched); pending gates are live
  rendezvous state, rebuilt by resnapshot on every (re)connect — same reasoning as the upstream
  snapshot-over-replay decision.
