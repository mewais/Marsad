# M2 — Typed IPC: `IpcMsg` as the single authoritative wire schema

**Blocks touched:** B01 `marsad-proto` (schema + parse/encode), B12 `marsad-core` (sender/receiver),
B13 `marsad-agent` (receiver/sender). **Coverage:** 100% line+region per crate (house gate:
`cargo llvm-cov --fail-under-lines 100 --fail-under-regions 100`).

## Purpose

Today `IpcMsg` covers only part of the GUI⇄agent traffic. The rest is hand-rolled JSON on both ends:

- **Core encodes-then-mutates** `LaunchSession` (`crates/marsad-core/src/lib.rs:851–953`): it calls
  `encode_ipc(&IpcMsg::LaunchSession{cwd,model,permission_mode})`, re-parses the string to a
  `serde_json::Value`, and injects `session_id / project / host_id / host_target / claude_bin /
  setup / env / posture / first_prompt / continue / resume_session_id / name / disallowed_tools`
  as raw JSON before `send_raw_line`.
- **Core sends raw lines** for `Interrupt` (`core:1264`), `SlashCmd` (`core:1315`), `SetWorkspace`
  (`core:1331`), because proto has no variants for them.
- **Core parses inbound via a private `IpcEnvelope`** (`core:2027`) and string-matches
  `"AgentEvent" | "TranscriptBackfill" | "ChangesUpdate" | "UsageUpdate"` (`core:1663–1669`) — three
  of those four types do not exist in `IpcMsg` at all.
- **Agent string-matches `handle_ipc`** (`crates/marsad-agent/src/lib.rs:774–786`), including
  `"Interrupt"` and `"Unsubscribe"` which are not `IpcMsg` variants, and parses `LaunchSession`
  through a private `LaunchSessionMsg` + `parse_posture_value` + `parse_launch_mode_value`.
- **Agent emits raw ack/broadcast lines** via `serde_json::json!` for `SessionLaunched`,
  `SessionResumed`, `SessionAttached`, `SessionStopped`, `PromptSent`, `Interrupted`, `GateAnswered`,
  `QuestionAnswered`, `Subscribed`, `Error`, `TranscriptBackfill`, `UsageUpdate`, `ChangesUpdate`
  (agent lines 818, 891, 907, 930, 988, 1019, 1076, 1118, 1125, 1614, 2835, 2874, 3009, 3037).

After M2, **every message either end sends or accepts is a typed `IpcMsg` variant**, encoded with
`encode_ipc` and parsed with `parse_ipc`. Hand-rolled JSON construction/mutation for IPC is
eliminated on both ends. This is the foundation for U1 and X2 (`PLAN.md` block M2: "X-blocks add
more IPC; they must land on a typed protocol, not hand-rolled JSON").

**Wire compatibility is REQUIRED both directions.** For every variant that exists today, the wire
shape is unchanged (additive-only): an old agent must parse a new core's messages and an old core
must parse a new agent's messages. The normative wire form for every new variant is *today's actual
raw JSON*, quoted verbatim below from the code that produces/consumes it.

---

## 1. Message inventory (full sweep result)

Sweep = every `serde_json::json!`/`Value`-mutation/`"type"`-string-match touching the IPC channel in
`marsad-core/src/lib.rs`, `marsad-agent/src/lib.rs`, `apps/desktop/src-tauri/src/lib.rs`, plus the
raw-JSON test corpus in `crates/{marsad-core,marsad-agent}/tests/`.

### GUI/core → agent

| Type string | Today | Producer | Consumer | Status |
|---|---|---|---|---|
| `LaunchSession` | typed-then-MUTATED | core:851–953 | agent:789 (`LaunchSessionMsg`) | **retype: full field set** |
| `ResumeSession` | typed (but agent reads extra raw `first_prompt`, agent:877) | core:1131 | agent:829 | **additive field** |
| `AttachSession` | typed | core:1144 | agent:895 | unchanged |
| `StopSession` | typed | core:1037,1158,1179 | agent:911 | unchanged |
| `SendPrompt` | typed | core:1245 | agent:934 | unchanged |
| `Interrupt` | **raw json!** | core:1264 | agent:992 (stringly arm) | **new variant** |
| `AnswerGate` | typed | core:1300 | agent:1023 | unchanged (parse-tolerance note §2) |
| `AnswerQuestion` | typed | core:1280 | agent:1081 | unchanged |
| `Subscribe` | typed / raw json! | core; desktop src-tauri:469 | agent:1121 | unchanged (parse-tolerance note §2) |
| `Unsubscribe` | **raw string, no schema** | (raw clients / tests only) | agent:784 (stringly arm, no-op) | **new variant** |
| `SlashCmd` | **raw json!** | core:1315 | agent: NONE → `Error` reply | **new variant** |
| `SetWorkspace` | **raw json!** (advisory, best-effort) | core:1331 | agent: NONE → `Error` reply | **new variant** |

### agent → GUI/core

| Type string | Today | Producer | Consumer | Status |
|---|---|---|---|---|
| `AgentEvent` | typed | agent:3054 (`encode_ipc`) | core:1664 (via `IpcEnvelope`) | unchanged |
| `TranscriptBackfill` | **raw json!** | agent:2835 | core:1665 | **new variant** |
| `ChangesUpdate` | **raw json!** | agent:3009 | core:1666 | **new variant** |
| `UsageUpdate` | **raw json!** | agent:2874, 3037 | core:1667 | **new variant** |
| `SessionLaunched` / `SessionResumed` / `SessionAttached` / `SessionStopped` / `PromptSent` / `Interrupted` / `GateAnswered` / `QuestionAnswered` | **raw json!** acks | agent:818,891,907,930,988,1019,1076,1118 | desktop reader skips (src-tauri:499–511); raw clients read them | **new variants** |
| `Subscribed` | **raw json!** ack | agent:1125 | same | **new variant** |
| `Error` | **raw json!** error reply | agent serve loop:1614 | same | **new variant** |

### Vestigial

| Type string | Notes |
|---|---|
| `VmUpdate` | In `IpcMsg` and proto tests; never sent by live code. **Keep unchanged** (wire compat; removal is out of scope). |

---

## 2. The `IpcMsg` v-next enum (normative)

Serde conventions (must hold for every variant):

- `#[serde(tag = "type")]` — internally tagged; the tag values are **exactly** today's strings
  (PascalCase variant names, no rename).
- **Unknown-field tolerance:** no `deny_unknown_fields` anywhere. serde's default (ignore unknown
  keys) is the forward-compat mechanism for additive fields; it is asserted by test (§6).
- **Additive-only:** existing variants keep their exact field names/types/optionality on the wire.
  The only permitted change to an existing variant is *parse tolerance* (`#[serde(default)]`) or a
  new `#[serde(default)]`+`skip_serializing_if` field — never a removed/renamed/retyped field.
- **JSON key order is NOT normative.** Today's mutation path and `json!` acks serialize through
  `serde_json::Value` (BTreeMap → alphabetical keys); typed `encode_ipc` emits the tag first.
  Both ends parse order-insensitively. Golden tests therefore compare **parsed values**, never raw
  bytes (except the encode→parse→re-encode stability check, which compares the crate's own output
  to itself).

```rust
/// GUI ↔ agent IPC messages (closed protocol — no Unknown variant; see §4 for
/// core's tolerant-skip disposition of unknown inbound types).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IpcMsg {
    // ── GUI/core → agent ────────────────────────────────────────────────────
    /// Full launch message. This variant IS the wire shape core:851–953 builds
    /// today by post-encode mutation; the mutation block is deleted (§3).
    /// Required on parse: `project`, `model`, `posture` (what the agent has
    /// always required). Everything else is parse-optional so the existing
    /// minimal test corpus (§7) keeps parsing.
    LaunchSession {
        /// Marsad session id, generated by core. Optional so raw clients that
        /// omit it still parse (the agent then generates one — agent:814–817).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        /// Project directory (the agent's cwd for claude). Required.
        project: String,
        /// Legacy mirror of `project`; kept for wire compat with old readers.
        /// Core always emits it (== project); defaulted on parse.
        #[serde(default)]
        cwd: String,
        /// Host registry id ("local" when absent). Core always emits it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host_id: Option<String>,
        /// Remote execution target (`user@host` / ssh alias / `wsl:<distro>`);
        /// absent for local. Emitted only when resolved (core:874–876).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host_target: Option<String>,
        /// Per-host claude executable; emitted only when configured (core:887–889).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        claude_bin: Option<String>,
        /// Per-host setup shell command; emitted only when configured (core:894–896).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        setup: Option<String>,
        /// Per-host env as [k,v] pairs; emitted only when non-empty (core:902–910).
        /// `Vec<(String,String)>` serializes to `[["K","V"],…]` — today's exact shape.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        env: Vec<(String, String)>,
        model: String,
        /// Authoritative permission posture ("Ask"|"AutoApproveSafe"|"FullAutonomy"
        /// — Posture's unit variants serialize to exactly these strings). Required;
        /// an unknown value is a SchemaMismatch (preserves agent's error on
        /// `posture:99` / `"Nonsense"`).
        posture: Posture,
        /// Legacy permission-mode mirror ("default"|"autoApprove"|"bypassPermissions");
        /// kept for back-compat per core:912. Core always emits it; defaulted on parse.
        #[serde(default)]
        permission_mode: String,
        /// ALWAYS serialized, `null` when absent — today core unconditionally
        /// inserts it (core:917–921). Plain Option (no skip attribute).
        #[serde(default)]
        first_prompt: Option<String>,
        /// LaunchMode::Continue on the wire (core:925–927). Absent otherwise.
        #[serde(default, rename = "continue", skip_serializing_if = "Option::is_none")]
        continue_: Option<bool>,
        /// LaunchMode::Resume(id) on the wire (core:928–930). Absent otherwise.
        /// Empty string == Fresh (agent:2409–2412) — preserved by `launch_mode()`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resume_session_id: Option<String>,
        /// Custom display name; emitted only when trimmed non-empty (core:941–943).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// --disallowedTools list; emitted only when non-empty (core:945–952).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        disallowed_tools: Vec<String>,
    },
    ResumeSession {
        session_id: String,
        /// ADDITIVE: the agent already reads this from the raw value (agent:877);
        /// core never sends it today. Absent → None; absent on the wire when None.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        first_prompt: Option<String>,
    },
    AttachSession { session_id: String },
    StopSession { session_id: String },
    SendPrompt { session_id: String, text: String },
    /// NEW (typed form of core:1264 / agent:992). Stops the current turn via
    /// claude's `interrupt` control_request; never kills the session.
    Interrupt { session_id: String },
    AnswerGate {
        session_id: String,
        request_id: String,
        /// PARSE-TOLERANCE (additive): `#[serde(default)]` preserves the agent's
        /// current lenient read `value["allow"].as_bool().unwrap_or(false)`
        /// (agent:1040) — a missing `allow` means deny, not a schema error.
        /// Emission unchanged (core always sets it).
        #[serde(default)]
        allow: bool,
        #[serde(default)]
        deny_reason: Option<String>,
    },
    AnswerQuestion {
        session_id: String,
        request_id: String,
        answers: serde_json::Value,
    },
    Subscribe {
        /// PARSE-TOLERANCE (additive): `#[serde(default)]` — the agent has never
        /// required it (handle_subscribe takes no args, agent:1121; the desktop
        /// sends `""`, src-tauri:469). Emission unchanged.
        #[serde(default)]
        session_id: String,
    },
    /// NEW (typed form of the stringly arm agent:784). Benign no-op today.
    Unsubscribe {
        /// Tolerated-absent for the same reason: today's arm ignores the payload.
        #[serde(default)]
        subscriber_id: String,
    },
    /// NEW (typed form of core:1315–1321). NOTE: the agent has NO handler today —
    /// it falls into the unknown-type arm and replies `Error`. §5 makes that an
    /// EXPLICIT arm with the same observable reply (behavior-preserving; real
    /// slash forwarding is U-series work, not M2).
    SlashCmd { session_id: String, cmd: String },
    /// NEW (typed form of core:1331–1337). Advisory broadcast; agent has no
    /// handler today (same explicit-arm treatment as SlashCmd).
    SetWorkspace { workspace_id: String },

    // ── agent → GUI/core ────────────────────────────────────────────────────
    VmUpdate { session_id: String, update: serde_json::Value },     // vestigial, unchanged
    AgentEvent {
        session_id: String,
        /// PARSE-TOLERANCE (additive): `#[serde(default)]` mirrors IpcEnvelope's
        /// `#[serde(default)] event: Value` (core:2034–2035) — an absent event
        /// parses as Null and flows to StreamMsg::Unknown, exactly as today.
        #[serde(default)]
        event: serde_json::Value,
    },
    /// NEW (typed form of agent:2835). Remote session history: the raw `.jsonl`
    /// text read on the remote host, shipped so core can backfill the transcript
    /// and aggregate model usage (core:1681).
    TranscriptBackfill {
        session_id: String,
        /// Absent/empty → core no-ops (core:1685,1690). Mirrors IpcEnvelope.
        #[serde(default)]
        jsonl: String,
    },
    /// NEW (typed form of agent:3009). Remote git-probe result.
    ChangesUpdate {
        session_id: String,
        /// Kept as Value ON PURPOSE: core parses it leniently
        /// (`from_value::<Vec<FileChange>>(…).unwrap_or_default()`, core:1732 —
        /// malformed → empty, never an error) and `FileChange` lives in
        /// marsad-viewmodel, which proto must not depend on (DAG). Element shape
        /// (informative): `{"path":"a.rs","status":"M","added":3,"removed":1}`.
        #[serde(default)]
        changes: serde_json::Value,
    },
    /// NEW (typed form of agent:2874/3037). Project-wide per-model usage from the
    /// remote python3 aggregation script.
    UsageUpdate {
        session_id: String,
        /// Kept as Value ON PURPOSE: core's lenient handling (missing → no-op,
        /// non-object → no-op; core:1753–1760, pinned by
        /// crates/marsad-core/tests/branches.rs:643–670) must survive. Shape
        /// (informative): `{"<model>":{"input":N,"output":N,"cache_read":N,"cache_creation":N}}`.
        #[serde(default)]
        usage: serde_json::Value,
    },
    // NEW: command acks (agent serve loop replies; today raw json!).
    SessionLaunched { session_id: String },
    SessionResumed  { session_id: String },
    SessionAttached { session_id: String },
    SessionStopped  { session_id: String },
    PromptSent      { session_id: String },
    Interrupted     { session_id: String },
    GateAnswered    { session_id: String },
    QuestionAnswered{ session_id: String },
    Subscribed      { subscriber_id: String },
    /// NEW (typed form of agent:1614): the serve loop's error reply.
    Error           { message: String },
}
```

`parse_ipc` keeps its closed-protocol contract (unknown type → `ProtoError::UnknownDiscriminant`,
per `docs/specs/proto.md`); its `KNOWN` list grows to exactly the tag set above. `encode_ipc` is
unchanged. Add one helper (replaces agent's `parse_launch_mode_value`, same precedence:
non-empty `resume_session_id` wins, then `continue == true`, else Fresh):

```rust
impl IpcMsg {
    /// LaunchSession only: derive the LaunchMode from the wire fields.
    pub fn launch_mode(&self) -> Option<LaunchMode>;
}
```

### Golden wire forms (normative, quoted from today's producers)

These exact JSON lines are the per-variant golden fixtures (§6). Optional-field rules as in the
enum; `<…>` marks sample values.

```json
{"type":"LaunchSession","session_id":"s-1","project":"/home/u/proj","cwd":"/home/u/proj","host_id":"hpc","host_target":"user@hpc.example.com","claude_bin":"genie","setup":"module load cuda","env":[["FOO","bar"]],"model":"claude-opus-4-8","posture":"Ask","permission_mode":"default","first_prompt":null,"resume_session_id":"uuid-xyz","name":"My run","disallowed_tools":["Bash"]}
{"type":"LaunchSession","project":"/home/user/proj","host_id":"local","model":"claude-opus-4-5","posture":"Ask"}
{"type":"LaunchSession","project":"/home/user/proj","host_id":"local","model":"claude-opus-4-8","posture":"Ask","continue":true}
{"type":"ResumeSession","session_id":"s-1"}
{"type":"AttachSession","session_id":"s-1"}
{"type":"StopSession","session_id":"s-1"}
{"type":"SendPrompt","session_id":"s-1","text":"do the thing"}
{"type":"Interrupt","session_id":"s-1"}
{"type":"AnswerGate","session_id":"s-1","request_id":"req-1","allow":true,"deny_reason":null}
{"type":"AnswerQuestion","session_id":"s-1","request_id":"req-q1","answers":{"Which approach?":"Option A"}}
{"type":"Subscribe","session_id":""}
{"type":"Unsubscribe","subscriber_id":"sub-1"}
{"type":"SlashCmd","session_id":"s-1","cmd":"compact"}
{"type":"SetWorkspace","workspace_id":"ws-1"}
{"type":"AgentEvent","session_id":"s-1","event":{"type":"result","subtype":"success"}}
{"type":"TranscriptBackfill","session_id":"s-1","jsonl":"{\"type\":\"user\"}\n{\"type\":\"assistant\"}"}
{"type":"ChangesUpdate","session_id":"s-1","changes":[{"path":"a.rs","status":"M","added":3,"removed":1}]}
{"type":"UsageUpdate","session_id":"s-1","usage":{"claude-opus-4-8":{"input":100,"output":50,"cache_read":0,"cache_creation":0}}}
{"type":"SessionLaunched","session_id":"s-1"}
{"type":"SessionResumed","session_id":"s-1"}
{"type":"SessionAttached","session_id":"s-1"}
{"type":"SessionStopped","session_id":"s-1"}
{"type":"PromptSent","session_id":"s-1"}
{"type":"Interrupted","session_id":"s-1"}
{"type":"GateAnswered","session_id":"s-1"}
{"type":"QuestionAnswered","session_id":"s-1"}
{"type":"Subscribed","subscriber_id":"sub-1"}
{"type":"Error","message":"unknown IPC type: Bogus"}
{"type":"VmUpdate","session_id":"abc-123","update":{}}
```

(Lines 2–3 are the agent-test minimal launch corpus — they MUST keep parsing; line 1 is the maximal
core-produced form. Key order in all goldens is non-normative.)

### Backward-compat matrix (must all hold)

| Direction | Why it holds |
|---|---|
| new core → old agent | `LaunchSession` emits a key-superset identical to today's mutated JSON (same keys, same value encodings; order differs, irrelevant). Old `LaunchSessionMsg` requires only `project`+`model` (+`posture` string) and ignores unknowns. `Interrupt`/`SlashCmd`/`SetWorkspace` are byte-shape identical to today's raw lines. All other variants: serde definition unchanged → identical output. |
| old core → new agent | Old mutated `LaunchSession` carries every new-required field (`project`,`model`,`posture` — core has always injected them); extras tolerated. Old raw `Interrupt` line matches the typed variant exactly. |
| new agent → old core | `TranscriptBackfill`/`ChangesUpdate`/`UsageUpdate`/`AgentEvent` emit the same keys `IpcEnvelope` reads; acks are skipped by the old desktop reader by type string, which is unchanged. |
| old agent → new core | Same shapes parse into the typed variants (all payload fields `#[serde(default)]`, `session_id` required exactly where core already required it). |

---

## 3. Core changes (`marsad-core`)

1. **Delete the encode-then-mutate block** (`launch_session_with_id`, core:850–953). Replace with
   direct typed construction. The **registry resolution logic stays in core** (it is business
   logic, not wire plumbing): resolve `host_target` (spec override → registry lookup for non-local
   host, core:867–873), `claude_bin`/`setup`/`env` (spec override → per-host config, core:882–901),
   trim-and-drop empty `name` (core:935–940), then build:

   ```rust
   let msg = IpcMsg::LaunchSession {
       session_id: Some(id.clone()),
       project: spec.project.clone(),
       cwd: spec.project.clone(),
       host_id: Some(spec.host_id.clone()),
       host_target, claude_bin, setup, env,
       model: spec.model.clone(),
       posture: spec.posture.clone(),
       permission_mode: posture_to_permission_mode(&spec.posture).to_string(),
       first_prompt: spec.first_prompt.clone(),
       continue_: matches!(spec.mode, LaunchMode::Continue).then_some(true),
       resume_session_id: match &spec.mode { LaunchMode::Resume(s) => Some(s.clone()), _ => None },
       name: custom_name,
       disallowed_tools: spec.disallowed_tools.clone(),
   };
   self.send_ipc(&msg)?;
   ```

2. **`interrupt()`** (core:1251–1266): `send_raw_line(json!…)` → `send_ipc(&IpcMsg::Interrupt{…})`.
3. **`run_slash()`** (core:1308–1322): raw line → `send_ipc(&IpcMsg::SlashCmd{…})`.
4. **`set_workspace()`** (core:1324–1339): raw line → `encode_ipc(&IpcMsg::SetWorkspace{…})`; keep
   the best-effort `let _ = self.transport.send_line(…)` semantics.
5. **Delete `IpcEnvelope`** (core:2017–2052). `on_agent_line` becomes:

   ```rust
   pub fn on_agent_line(&mut self, line: &str) -> Result<Vec<VmUpdate>, CoreError> {
       let msg = match marsad_proto::parse_ipc(line) {
           Ok(m) => m,
           // Tolerant skip: a FUTURE inbound type (newer agent) must not error the
           // reader loop. The raw line is preserved via stderr for observability.
           Err(ProtoError::UnknownDiscriminant(t)) => {
               eprintln!("marsad-core: on_agent_line: skipping unknown IPC type '{t}': {line}");
               return Ok(vec![]);
           }
           // Malformed JSON / missing type / wrong-typed known fields stay HARD errors.
           Err(e) => return Err(CoreError::Protocol(e.to_string())),
       };
       match msg {
           IpcMsg::AgentEvent { session_id, event } => self.handle_agent_event(session_id, event),
           IpcMsg::TranscriptBackfill { session_id, jsonl } => self.handle_transcript_backfill(session_id, jsonl),
           IpcMsg::ChangesUpdate { session_id, changes } => self.handle_changes_update(session_id, changes),
           IpcMsg::UsageUpdate { session_id, usage } => self.handle_usage_update(session_id, usage),
           // Command acks legitimately arrive on the subscriber connection
           // (the desktop pre-filter at src-tauri:499–511 becomes optional).
           IpcMsg::SessionLaunched{..} | IpcMsg::SessionResumed{..} | IpcMsg::SessionAttached{..}
           | IpcMsg::SessionStopped{..} | IpcMsg::PromptSent{..} | IpcMsg::Interrupted{..}
           | IpcMsg::GateAnswered{..} | IpcMsg::QuestionAnswered{..} | IpcMsg::Subscribed{..}
           | IpcMsg::Error{..} => Ok(vec![]),
           // A GUI→agent message arriving inbound is a routing bug: hard error
           // (same observable class as today's "unknown IPC type" Protocol error).
           other => Err(CoreError::Protocol(format!(
               "misdirected IPC message on agent channel: {other:?}"))),
       }
   }
   ```

   Handler signatures change from `(&mut self, env: IpcEnvelope)` to typed params; bodies are
   otherwise unchanged (required `session_id` moves from `.ok_or(…)` checks into serde — a missing
   `session_id` on the four handled types is now a `SchemaMismatch` and still a
   `CoreError::Protocol`, preserving the three `*_missing_session_id_is_error` tests).

## 4. Unknown-type disposition (normative summary)

| Layer | Unknown `type` | Rationale |
|---|---|---|
| `parse_ipc` (proto) | `Err(UnknownDiscriminant)` | unchanged closed-protocol contract (proto.md) |
| core `on_agent_line` | **tolerant skip**: log line to stderr, `Ok(vec![])` | a newer agent must never wedge an older GUI's reader loop; observability preserved (line is logged verbatim; the desktop reader already logs-and-continues, src-tauri:521–530) |
| agent `handle_ipc` | `Err(AgentError::Protocol)` → serve loop replies `IpcMsg::Error` | unchanged observable behavior; the requester gets a diagnosable reply (agent test `handle_ipc_unknown_type_returns_protocol_error` keeps passing) |

## 5. Agent changes (`marsad-agent`)

1. `handle_ipc` (agent:759–787) parses with `marsad_proto::parse_ipc(line)`; `MalformedJson` /
   `MissingDiscriminant` / `SchemaMismatch` / `UnknownDiscriminant` all map to
   `AgentError::Protocol` (same observable class as every existing error test). The stringly
   `match type_str` and ALL raw-`Value` field reads in handlers are removed.
2. **Delete** `LaunchSessionMsg` (agent:2473–2498), `parse_posture_value` (agent:2424–2435 — serde
   on `Posture` now yields the schema error), and `parse_launch_mode_value` (agent:2408–2418 —
   replaced by `IpcMsg::launch_mode()`, identical precedence incl. empty-string-resume → Fresh).
3. Typed arms: `LaunchSession` builds the `LaunchSpec` from variant fields (`host_id
   .unwrap_or("local")` preserved); `Interrupt`/`Unsubscribe` become real variants (`Unsubscribe`
   stays `Ok(vec![])`); **`SlashCmd` and `SetWorkspace` get explicit arms** returning
   `Err(AgentError::Protocol("SlashCmd not implemented"))` / `("SetWorkspace is advisory —
   ignored")`… with the SetWorkspace arm instead returning `Ok(vec![])` (it is an advisory
   broadcast; erroring it would spam `Error` lines at every workspace switch — today's reply IS an
   `Error` line, but it is fire-and-forget and unread; the no-op is the intended semantics and is
   direction-compatible). `SlashCmd` keeps the error reply so behavior is observably unchanged
   until U-series implements it.
4. All ack and broadcast lines are built with `encode_ipc(&IpcMsg::…)` — `SessionLaunched`,
   `SessionResumed`, `SessionAttached`, `SessionStopped`, `PromptSent`, `Interrupted`,
   `GateAnswered`, `QuestionAnswered`, `Subscribed`, `Error` (serve loop), `TranscriptBackfill`,
   `UsageUpdate` (×2 sites), `ChangesUpdate`. `parse_remote_changes` keeps returning the JSON array
   it returns today; it becomes the `changes` Value of the typed variant.

## 6. Test plan (house style: black-box `tests/`, 100% line+region)

### marsad-proto (`tests/happy.rs`, `tests/errors.rs`, `tests/branches.rs`)

For **EVERY** variant (all 29 tags listed in §2 — existing 10 + 19 new):

1. **Golden parse:** pin the §2 golden JSON string as a `r#"…"#` literal; `parse_ipc` → assert the
   exact typed value (full field equality, not just the variant arm).
2. **Encode→parse identity:** construct the typed value → `encode_ipc` → `parse_ipc` →
   `assert_eq!` with the original; plus re-encode stability
   `assert_eq!(encode_ipc(&reparsed), encoded)` (extends the existing
   `encode_ipc_all_variants_produce_valid_json` loop, proto tests/happy.rs:532).
3. **Golden encode:** `encode_ipc` output and the golden string are equal **as
   `serde_json::Value`** (key order non-normative, §2).
4. **Cross-version tolerance:** (a) golden + injected `"x_future_field":123` still parses to the
   same typed value — every variant, loop-driven; (b) unknown type
   (`{"type":"FutureMsg","session_id":"s"}`) → `UnknownDiscriminant` (existing errors.rs:146–164
   stays authoritative).
5. **LaunchSession-specific:** maximal golden (all optionals) + minimal golden (agent-corpus line)
   + `continue` golden; absent optionals stay absent on re-encode (`skip_serializing_if` proof:
   encode the minimal value, assert the JSON object has no `host_target`/`claude_bin`/`setup`/
   `env`/`continue`/`resume_session_id`/`name`/`disallowed_tools`/`session_id` keys and DOES have
   `first_prompt: null`); `launch_mode()` precedence: resume non-empty → Resume; resume `""` +
   `continue:true` → Continue; resume `""` alone → Fresh; neither → Fresh; both set → Resume.
6. **Schema errors:** `posture: 99` and `posture: "Nonsense"` → `SchemaMismatch`; missing
   `project`/`model` → `SchemaMismatch`; `AnswerGate` without `allow` parses with `allow == false`
   (the §2 tolerance).

Coverage bar: the crate keeps `--fail-under-lines 100 --fail-under-regions 100`; unreachable
branches deleted, not waived (ARCHITECTURE.md coverage gate).

### marsad-core / marsad-agent (updated suites)

- Core: new tests — unknown inbound type is a tolerant no-op (`Ok(vec![])`); ack types inbound are
  no-ops; misdirected `SendPrompt` inbound is a Protocol error; launch line asserted **through
  `parse_ipc`** (typed equality) instead of `line.contains(…)` where practical (existing
  `contains`-style tests stay as-is — they pass unchanged because the emitted keys/values are
  identical).
- Agent: existing raw-JSON corpus is now ALSO the compat corpus — no edits needed (see §7);
  add one arm test each for typed `SlashCmd` (Error reply preserved) and `SetWorkspace` (no-op).

### Integration

`tests/integration/i09_gui_agent_live` / `i10_core_e2e_live` exercise the real socket path and must
pass unrun-modified (live, per M3).

## 7. Existing tests that assert on raw JSON and MUST keep passing unchanged

These are the behavioral compat oracle; a green run of these without edits is the M2 done-gate.

**marsad-agent** (raw wire lines fed to `handle_ipc`):
- `tests/happy.rs:409,440,489` — minimal `LaunchSession` lines.
- `tests/errors.rs:138` (launch), `:224` (unknown type → Protocol), `:230–236` (truncated JSON),
  `:246` (`posture:99` schema error), `:439` (posture error), `:504–506` (`Interrupt` missing
  session_id → error), `:709,730,756` (launch variants).
- `tests/branches.rs:41,76` (launch + explicit session_id), `:115` (`continue:true`), `:132`
  (`resume_session_id`), `:145` (empty resume id → Fresh), `:351` (`Interrupt` happy),
  `:787,833,867` (`Unsubscribe`), `:1084,1101,1119` (postures incl. `"Nonsense"` → error).

**marsad-core** (raw wire lines through `on_agent_line` / raw assertions on outbound lines):
- `tests/happy.rs:94` launch emits IPC, `:134` host_target forwarding, `:171` mode+posture,
  `:229` first_prompt, `:438` interrupt_sends_ipc, `:879` all postures, `:1361` per-host
  claude_bin/setup/env in IPC, `:1405` env override, `:2022,:2104` restart Stop→Launch with
  same id / resume_session_id / model, `:2558` custom name, `:3294` disallowed_tools,
  `:4003,:4079` restart carries persisted cfg.
- TranscriptBackfill suite `:2381–2560` and model-usage backfill `:2863–3060`.
- ChangesUpdate suite `:2698–2806` (incl. missing session_id → error).
- UsageUpdate suite `:3515–3776` and `tests/branches.rs:643–670` (missing/array `usage` → no-op).
- `tests/errors.rs`: non-JSON, truncated JSON, missing `session_id` on AgentEvent → all still
  `CoreError::Protocol`.

**Tests that MUST change (the only ones — each change is required by this spec, list is closed):**
- `crates/marsad-core/tests/errors.rs:359–369`
  (`on_agent_line_unknown_ipc_type_returns_protocol_error`) → becomes
  `on_agent_line_unknown_ipc_type_is_tolerant_noop` asserting `Ok(vec![])` (§4).
- `crates/marsad-proto/tests/happy.rs:423` (`parse_ipc_launch_session`) — its golden line
  (`cwd`/`model`/`permission_mode` only) never matched the real wire (the agent has always required
  `project`+`posture`); replace with the §2 minimal + maximal goldens.
- `crates/marsad-proto/tests/happy.rs:532` (`encode_ipc_all_variants_…`) — LaunchSession
  constructor gains the new fields; loop extended to all 29 variants.
- `crates/marsad-proto/tests/errors.rs` UnknownDiscriminant probes: any probe string that is now a
  real type (none today — `FutureMsg`/`FutureType` stay unknown) — verify, no change expected.

## 8. Non-goals

- No behavior change for `SlashCmd` execution (still unimplemented in the agent; U-series).
- No removal of `VmUpdate` or the legacy `cwd`/`permission_mode` mirrors (compat).
- No desktop/src-tauri edits required (its type-string pre-filter keeps working; migrating it to
  typed parse is optional follow-up, parked with M9).
- No change to the claude stream protocol (`StreamMsg`), hooks, or the control protocol.

## Edit scope

proto test-agent: `crates/marsad-proto/tests/`; proto code-agent: `crates/marsad-proto/src/`.
Core/agent agents: their crate's `src/` + listed test updates only, per ARCHITECTURE.md's
directory-access contract.
