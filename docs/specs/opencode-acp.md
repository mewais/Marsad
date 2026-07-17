# opencode backend via ACP (Agent Client Protocol) — implementation contract

Status: in development (TDD, subagent-orchestrated). This is the FROZEN contract the
tests and the implementation are both written against. Module names + public
function signatures here are authoritative; coders fill bodies, they do not change
signatures; testers test against these, they do not add implementation.

## 0. Why ACP (not `opencode run`)

`opencode run --format json` is one-shot + output-only: no back-channel to answer a
mid-turn permission prompt, and an `ask` permission hangs forever headless. Interactive
HITL needs a bidirectional channel. `opencode acp` speaks the Agent Client Protocol —
JSON-RPC 2.0, newline-delimited, over stdin/stdout — which has a structured permission
request/response. Architecturally this mirrors the **claude** driver (persistent,
bidirectional), NOT the codex eval-loop.

## 1. Process / durable-session model

- Launch a PERSISTENT `opencode acp` process under the existing durable-session wrapper
  (setsid + keepalive + FIFO), exactly like claude: `stdin ← in.fifo`, `stdout → out.log`
  (regular file, appended), pids tracked for liveness. NOT an eval-loop.
- The reader tails `out.log` and parses newline-delimited JSON-RPC messages.
- The writer writes newline-delimited JSON-RPC to `in.fifo`.
- Binary resolved from `LaunchSpec.claude_bin` (default `"opencode"`), same as codex reuses it.
- Driver discriminator string: `"opencode"`.

## 2. ACP handshake + lifecycle (client = Marsad)

Client→Agent requests we SEND:
- `initialize` { protocolVersion:"1", clientInfo, clientCapabilities:{ fs:{readTextFile:false,
  writeTextFile:false}, terminal:false } } → response carries agentCapabilities
  (loadSession, promptCapabilities, sessionCapabilities), used to gate resume.
  We DECLINE fs+terminal so opencode does its own IO and never sends fs/*, terminal/*.
- `session/new` { cwd, mcpServers:[…], additionalDirectories? } → { sessionId, modes?,
  configOptions? }. Fresh launch. `sessionId` captured for follow-up turns + resume.
- `session/load` { sessionId, cwd, mcpServers } → same result. Used for RESUME (only if
  agentCapabilities.loadSession).
- `session/prompt` { sessionId, prompt:[{type:"text",text}] } → { stopReason:
  "completed"|"cancelled"|"auth_required"|"request_limit_exceeded" }. One per turn.
- `session/cancel` { sessionId } (notification) → interrupt the current turn.

Agent→Client requests we ANSWER (only one, because fs/terminal declined):
- `session/request_permission` { sessionId, toolCall:{toolName,toolUseId,input},
  options:[{optionId,name,kind}] } where kind ∈ allow_once|allow_always|reject_once|
  reject_always. We reply { outcome:"selected", optionId } or { outcome:"cancelled" }.

Agent→Client notifications we RECEIVE:
- `session/update` { sessionId, update:{ type, … } } — streaming. Variants handled:
  - `agent_message_chunk` { content: ContentBlock } → assistant text (accumulate).
  - `agent_thought_chunk` { thinking } → thinking/reasoning (rendered as thought, not a msg).
  - `user_message_chunk` { content } → echoed user content (skip or record).
  - `tool_call` { toolName, toolUseId, input } → assistant tool_use.
  - `tool_call_update` { toolUseId, toolName, input, content:[text|diff|terminal] } →
    tool_use progress / result (diff → file change, text → output).
  - `plan` { plan:{ entries:[{content, priority:critical|high|medium|low}] } } → PLAN MODE
    display (the live plan/todo list).
  - `available_commands_update` { availableCommands:[{name,description,input?}] } → slash
    commands catalog (feeds the composer's slash popover).
  - `current_mode_update` { currentModeId } → track current session mode (plan/build/etc.).
  - `config_option_update` { configOptions } → session config options (tracked).

## 3. Translation → Marsad stream-json (module `acp_translate.rs`)

Mirror `codex.rs`: turn ACP JSON-RPC messages into the claude-stream-json `Value`s the
rest of Marsad already understands, so the reader/reducer/UI work unchanged.

- On `initialize` result + first `session/new`|`session/load` result → synthesize a
  `system/init` (model, cwd, permissionMode, slash_commands seeded empty; mcp_servers).
- `agent_message_chunk` (text) → `assistant` with a text content block. Chunks with the
  same messageId accumulate into one assistant message (the reducer dedups by message id).
- `agent_thought_chunk` → an `assistant` thinking block (or a dedicated thought event the
  reducer already tolerates); MUST NOT count as a normal assistant message.
- `tool_call` → `assistant` with a `tool_use` content block (id=toolUseId, name=toolName,
  input parsed from the ACP `input` string).
- `tool_call_update` with `diff` content → drives the Changes probe (file change); with
  `text` → tool result/output. Maps to the same tool_use/tool_result shapes claude emits.
- `plan` → CANONICAL stream event `{type:"system", subtype:"plan_update", session_id,
  plan_entries:[{content,priority}]}` (flatten ACP `plan.entries` to top-level
  `plan_entries`). The events reducer consumes this → `SessionState.plan_entries`. See §5.
- `current_mode_update` → CANONICAL `{type:"system", subtype:"mode_update", session_id,
  current_mode:<id>}` → `SessionState.current_mode`.
- `available_commands_update` → CANONICAL `{type:"system", subtype:"slash_update",
  session_id, slash_commands:[{name,description,argument_hint}]}` (map ACP
  availableCommands {name,description,input?} to the SlashCommandInfo shape) → reducer
  updates `SessionState.slash_commands` mid-session.
- ACP compaction signal → CANONICAL: emit the EXISTING `{type:"system",
  subtype:"compact_boundary", session_id}` shape the events reducer already increments
  `compactions` on (do NOT invent a new subtype). `detect_compaction` stays for the
  reader's separate use.

CANONICAL-SHAPE RULE: every stream event acp_translate emits MUST be a shape the events
reducer actually consumes (not a bespoke `acp_*` subtype that reduces to Unknown and is
silently ignored). Emitting Unknown-only shapes is an integration gap, not forward-compat.
- `session/prompt` result `stopReason` → `result` event: `completed` → result/success;
  `cancelled` → result with interrupt semantics (Done + Cancelled); `auth_required` /
  `request_limit_exceeded` → result/error with a real reason.
- Compaction signal (opencode emits a compaction notification via `session/update` `_meta`
  or a dedicated update; detect it) → increment `SessionState.compactions` +
  `last_compaction`. See §5.
- Unknown update types → skip (forward-compatible), exactly like codex.rs.

Every synthesized Value MUST pass `marsad_proto::parse_stream_line` (unit-tested, like codex).

## 4. Gates (module: `acp_translate.rs` gate mapping + agent wiring)

`session/request_permission` → a Marsad `Gate` captured as pending (keyed by session_id,
in the gate VecDeque like claude), broadcast to the UI:
- The gate carries the toolCall (toolName + input, shown as the action) and the ACP
  `options[]` rendered VERBATIM (name shown on each button; optionId + kind carried).
- Reuse `Gate::Permission` with `permission_suggestions` holding the ACP options
  (each option object: {optionId,name,kind}). The plain Allow/Deny is DERIVED from the
  option kinds present — no invented buttons; if opencode offers allow_once/allow_always/
  reject_once, exactly those show.
- Answer: the human picks an option → the agent replies to the pending JSON-RPC request
  id with { outcome:"selected", optionId } (or {outcome:"cancelled"} on interrupt/deny-
  with-no-reject-option). The wire `request_id` in AnswerGate routes to the pending ACP
  request id (reuse the gate-queue mechanism).
- This SINGLE path covers HITL permissions, MCP elicitations, and questions — whatever
  opencode surfaces as request_permission with options.

Interrupt: on user cancel, send `session/cancel` and answer any pending permission with
`{outcome:"cancelled"}`, then drain the gate queue (mirror the claude interrupt path).

## 5. Feature coverage (the explicit list)

- HITL permission gates → §4 (request_permission).
- MCP elicitation → §4 (surfaces as request_permission with options; MCP servers passed
  via session/new.mcpServers so opencode connects to them).
- Questions (AskUserQuestion-style) → §4 (request_permission with the choice options).
- Plan mode → `session/update{plan}` entries rendered as the live plan; session `modes` +
  `current_mode_update` tracked and surfaced (current mode shown; mode list available).
- Context compaction → detect opencode's compaction notification and bump
  `SessionState.compactions`/`last_compaction` (same fields claude uses).
- Slash commands → `available_commands_update` feeds the composer slash catalog.

## 6. Capabilities / discovery (`capabilities.rs`)

`RealOpencodeProbe` + `build_opencode_discovery_script(command, setup)`:
- availability: `command -v <opencode>`.
- models: `opencode models` → lines `provider/model`; `--verbose` adds cost. Parser
  `parse_opencode_models(raw) -> Vec<ModelInfo>` (source = Listed; free_text allowed).
- sessions (resume list): `opencode session list --format json` → parser
  `parse_opencode_sessions(raw) -> Vec<(session_id, Option<project>)>`.
- Registered in the probes list in `serve.rs` alongside claude/codex.

## 7. Frontend

- `apps/desktop/src/screens/NewSession.tsx` DRIVERS += { id:"opencode", label:"opencode" }.
- `Driver` type (types.ts) += "opencode".
- Data-driven: availability + models + resumable sessions come from the capabilities probe.
- Gate overlay already renders permission_suggestions as verbatim buttons — ACP options
  flow through that. Plan-mode plan entries render in the transcript/activity (reuse plan
  rendering). No new bespoke screens.

## 8. Module / API contract (Rust, marsad-agent unless noted)

NEW `crates/marsad-agent/src/acp.rs` — pure ACP protocol (transport-agnostic, unit-tested):
- `enum AcpMessage { Request{id,method,params}, Response{id,result|error}, Notification{method,params} }`
  with `parse_acp_line(&str) -> Result<AcpMessage, AcpError>` and
  `encode_request(id,method,params)->String`, `encode_response(id,result)->String`,
  `encode_notification(method,params)->String` (all newline-free; caller adds `\n`).
- Typed constructors for the client requests: `initialize_params(...)`, `session_new_params(...)`,
  `session_load_params(...)`, `session_prompt_params(session_id,text)`,
  `session_cancel_params(session_id)`, `request_permission_response(option_id: Option<&str>)`.
- `fn is_permission_request(method:&str)->bool`, helpers to read fields.

NEW `crates/marsad-agent/src/acp_translate.rs` — ACP→stream-json + gate mapping (unit-tested):
- `fn acp_update_to_stream_json(update:&Value, session_id:&str, model:&str) -> Option<Value>`
- `fn acp_init_to_system_init(init_result:&Value, session_result:&Value, model,cwd,project)->Value`
- `fn acp_stop_reason_to_result(stop_reason:&str)->Value`
- `fn acp_permission_to_gate(params:&Value)->marsad_gates::Gate` (Permission gate w/ options)
- `fn acp_options_to_suggestions(options:&Value)->Vec<serde_json::Value>` (verbatim)
- `fn detect_compaction(update:&Value)->bool`

Wiring (existing files, coders extend; signatures/branches added per contract):
- `driver.rs`: `opencode_argv(spec)->Vec<String>` (`["acp"]` + model/agent flags as available);
  dispatch `"opencode"` in `session_argv`/`build_spawn_cmd` → PERSISTENT process (claude-shaped),
  not eval-loop.
- `lib.rs`: `launch_with_id` `"opencode"` branch → persistent remote/local spawn (claude-shaped)
  + cache spec + capture ACP sessionId map (generalize `codex_threads` → also opencode, or a
  parallel `acp_sessions` map).
- `ipc.rs`: `handle_send_prompt` `"opencode"` branch → `session/prompt`; `handle_interrupt`
  → `session/cancel` + cancel pending permission; `handle_answer_gate` → ACP
  `request_permission` response by optionId when driver==opencode.
- `reader.rs`: `"opencode"` branch → parse ACP lines; notifications→translate→broadcast;
  `request_permission` (a REQUEST with id) → capture as pending gate (store the JSON-RPC id
  to answer later) + broadcast the gate; capture sessionId from session/new result.
- `capabilities.rs`: `RealOpencodeProbe`, `build_opencode_discovery_script`,
  `parse_opencode_models`, `parse_opencode_sessions`.
- proto: driver string only; reuse gate/AnswerGate types. Any ACP option carrier reuses
  `permission_suggestions` (Vec<Value>) + `permission_updates` (the chosen option object).

## 9. Test plan (written FIRST, all RED, real, good coverage)

- `acp.rs`: parse/encode round-trips; request vs response vs notification discrimination;
  malformed lines; id correlation. (unit)
- `acp_translate.rs`: each session/update variant → correct stream-json (asserted through
  `parse_stream_line`); stopReason mapping; permission→gate with verbatim options; compaction
  detection; unknown-update skip. (unit)
- `capabilities.rs`: `parse_opencode_models`, `parse_opencode_sessions` on real-shaped
  fixtures; discovery-script content. (unit)
- driver/ipc/lib: opencode argv; send_prompt builds session/prompt; interrupt builds
  session/cancel; answer_gate builds request_permission response by optionId. (unit +
  the fake-container path)
- core/events: an ACP permission surfaces as a Needs gate + board attention card; a plan
  update surfaces plan entries; compaction bumps the counter. (integration-style, existing
  reduce pipeline)
- frontend vitest: opencode appears as a driver; gate overlay renders ACP options verbatim
  and answers by optionId; plan entries render.
- An i-series live seam test (opt-in, skips without `opencode`) mirroring i02: real
  `opencode acp` handshake → prompt → permission round-trip.

## 10. Out of scope
- Client-side fs/terminal ACP methods (declined in initialize).
- The opencode HTTP `serve` mode (not used).
- Remote ACP transport (ACP is local-subprocess; we run it on the host under the durable wrapper).
