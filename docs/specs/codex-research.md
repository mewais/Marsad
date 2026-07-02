# R2 — `codex` CLI ground-truth research (codex-cli 0.142.5)

**Status:** ground truth, verified against the real installed binary on 2026-07-02. Supersedes the codex
claims in [`phase2-drivers.md`](./phase2-drivers.md) wherever they conflict (see the "corrections" callout
below). This doc answers, with cited local evidence, what a **Wasl** spawn-per-turn headless adapter and a
**Marsad** long-lived-session driver each need from `codex`.

## Method (what "verified" means here)

Evidence sources, in order of weight:

1. **Live invocations of the real binary** (`~/.local/bin/codex` → npm-installed
   `@openai/codex-linux-x64` 0.142.5, already authenticated via ChatGPT sign-in). Exactly **2** calls that
   reached the model API were made, both `codex exec`/`codex exec resume` with `-m gpt-5.4-mini -c
   model_reasoning_effort=low` (the cheapest model+effort combo — see "Model selection" below) and the
   trivial prompts specified in the task. Two *additional* invocations were attempted and failed **before**
   reaching the network (CLI arg-parse / local precondition errors, exit codes 2 and 1) — these are not
   counted as live calls and are noted inline where relevant.
2. **`--help` trees** for every subcommand (recursively, `codex <cmd> --help`, `codex <cmd> <sub> --help`).
3. **`codex app-server generate-json-schema --out <dir> --experimental`** — generates the *actual* JSON
   Schema for every app-server request/notification/response type from the compiled binary. This is
   authoritative, not inferred: 288 schema files in the `v2` protocol generation, plus top-level
   `ClientRequest.json` / `ClientNotification.json` / `ServerRequest.json` / `ServerNotification.json`
   enumerating every method name.
4. **`codex doctor --json`**, **`codex debug models`**, **`codex features list`**, **`codex mcp
   add/list/get/remove`** (a real MCP server was added and immediately removed to observe the resulting
   `~/.codex/config.toml` shape; net effect on the user's machine is zero).
5. **The session/rollout JSONL file** written to `~/.codex/sessions/` by the two live calls above — read
   directly, not inferred.
6. **`strings` on the compiled Rust binary** (`.../vendor/x86_64-unknown-linux-musl/bin/codex`, 273 MB) for
   event-type enums not exposed in a generated schema (e.g. the `codex exec --json` event vocabulary).
7. The npm package ships **no docs/ directory** — only a `README.md` (quickstart + links to
   `developers.openai.com/codex`, not fetched — out of scope per the "everything local" constraint) and a
   thin `bin/codex.js` launcher that execs the platform vendor binary. So "docs shipped in the npm package"
   contributed only the README's top-level pointers, nothing protocol-level.

### Corrections to `phase2-drivers.md`

- `phase2-drivers.md` says codex is "launched as `codex app-server`" and that config/task delivery is
  entirely protocol-driven — **true and confirmed** (§b). But it undersells `codex exec`: **headless
  one-shot with structured JSON output, `--output-last-message`, and session-id resume (`codex exec
  resume <id>`) all exist and were verified live** (§a). For Wasl's "spawn-per-turn, one prompt → one
  reply, session continuity across invocations" shape, `codex exec resume` is a much closer fit than
  standing up an app-server JSON-RPC client per turn — the doc's framing (JSON-RPC-only) should not block
  choosing `codex exec` for Wasl. See verdict in §g.
- The `codex app-server` transport is not "stdio-only": `--listen` accepts `stdio://` (default),
  `unix://PATH`, `ws://IP:PORT`, or `off`, and there's a whole `codex app-server daemon` subsystem for a
  **persistent background app-server** with a control socket, bootstrap-for-SSH, and remote-control pairing
  — directly relevant to Marsad's "launch/supervise long-lived sessions" (§g).

## Model selection for the 2 live calls

`codex debug models` (local, no network — reads the cached model catalog) lists exactly 3 models visible to
this account: `gpt-5.5` (priority 7, flagship), `gpt-5.4-mini` (priority 23, cheaper/smaller), and
`codex-auto-review` (hidden, review-only). `gpt-5.4-mini` was used for both live calls, with `-c
model_reasoning_effort=low` (confirmed applied: the session's `turn_context.effort` recorded `"low"` and
`collaboration_mode.settings.reasoning_effort` recorded `"low"`).

## (a) Headless one-shot: `codex exec`

### Exact invocation (verified live)

```
codex exec --json --skip-git-repo-check -m gpt-5.4-mini -c model_reasoning_effort=low \
  --sandbox read-only --output-last-message <FILE> "Reply with exactly: OK"
```

Exit code `0`. `--output-last-message <FILE>` wrote the plain-text final agent message (`OK`) to disk.
Stdout with `--json` was **newline-delimited JSON** (JSONL, one object per line — not a single JSON blob,
not framed with Content-Length headers):

```json
{"type":"thread.started","thread_id":"019f237a-f03d-7710-ab8f-7af32b222df8"}
{"type":"turn.started"}
{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"OK"}}
{"type":"turn.completed","usage":{"input_tokens":9065,"cached_input_tokens":3456,"output_tokens":15,"reasoning_output_tokens":8}}
```

This is a **separate, simpler event vocabulary** from the app-server JSON-RPC protocol (§b) — it is the
"exec JSON event" stream, not JSON-RPC (no `id`/`method`/`jsonrpc` envelope). `strings` on the binary
confirms the full enum beyond what one trivial run exercised:
`thread.started`, `turn.started`, `turn.completed`, `turn.failed`, `item.started`, `item.updated`,
`item.completed`; item `type`s include `agent_message`, `reasoning`, `command_execution`, `file_change`,
`mcp_tool_call`, `web_search`, `todo_list`. (Item sub-fields for the untriggered types are unconfirmed —
listed in the checklist §f.)

`codex exec --help` flags relevant to a Wasl adapter:

- `--json` — JSONL event stream to stdout (as above).
- `-o, --output-last-message <FILE>` — final agent message only, plain text.
- `--output-schema <FILE>` — JSON Schema file constraining the model's final response shape (structured
  output contract — not exercised live, but present and documented in `--help`; useful if Wasl wants typed
  replies instead of parsing prose).
- `-m/--model`, `-c key=value` (arbitrary config override, dotted-path + TOML-typed value),
  `-s/--sandbox {read-only|workspace-write|danger-full-access}`, `--dangerously-bypass-approvals-and-sandbox`,
  `-C/--cd`, `--add-dir`, `--ephemeral` (skip session persistence — see §d), `--skip-git-repo-check`
  (needed outside a trusted git repo — hit this live, see below), `-i/--image`.
- **No `-a/--ask-for-approval` flag on `codex exec`** (present on the top-level `codex`, `codex resume`,
  `codex fork` commands, absent from `codex exec --help` and `codex exec resume --help`). Confirmed live:
  the session's `turn_context.approval_policy` was `"never"` even though it was never requested — `codex
  exec` hardcodes non-interactive approval behavior. **Implication for Wasl:** a spawn-per-turn `codex exec`
  adapter cannot surface interactive approval prompts to a human; it can only choose a sandbox tier
  up-front (`--sandbox`) and let the model self-limit or fail. Real approvals-as-protocol-messages only
  exist in app-server mode (§b) — this is the key asymmetry between the two consumers.

### Session id / resume (verified live)

`codex exec` always assigns a thread/session id (UUID v7-shaped, e.g. `019f237a-f03d-7710-ab8f-7af32b222df8`)
printed as `thread.started`. **Headless resume works** via the documented subcommand:

```
codex exec resume <SESSION_ID> --json -m gpt-5.4-mini -c model_reasoning_effort=low \
  --skip-git-repo-check --output-last-message <FILE> "<next prompt>"
```

or `codex exec resume --last` to pick the most recently recorded session without knowing its id.
`codex exec resume <id> -` reads the follow-up prompt from stdin (same `-`/stdin convention as the initial
`codex exec [PROMPT]`).

**Continuity was verified live**, not assumed: call 2 asked *"What word did I ask you to reply with in my
previous message?"* against the session created by call 1 (whose only prior content was "Reply with
exactly: OK") and the model answered `OK` — proving the resumed turn had the prior turn's context. The
`thread_id` in the resumed run's JSONL output was byte-identical to the original
(`019f237a-f03d-7710-ab8f-7af32b222df8`), and the **same rollout file on disk was appended to** (grew from
12 to 21 lines; see §d), not replaced.

Two local-only preconditions had to be satisfied before the resume call reached the network (surfaced as
CLI errors, not live/billed calls):
- `codex exec resume` does **not** accept `--sandbox` (arg-parse error: `unexpected argument '--sandbox'
  found`) — sandbox is apparently fixed by the original thread's config on resume, not overridable per the
  CLI surface exposed on `resume`. (Needs live verification: whether `-c sandbox_mode=...` on resume works
  where `--sandbox` doesn't — untested, see §f.)
- Outside a trusted/known git repo, `codex exec` (and `resume`) refuse to run unless
  `--skip-git-repo-check` is passed (`error: Not inside a trusted directory and --skip-git-repo-check was
  not specified.`).

### Verdict shape for Wasl (see full verdict in §g)

`claude -p --output-format json --resume <id>` maps to:

```
codex exec [--json] [-o FILE] [-m MODEL] [-s SANDBOX] "<prompt>"      # first turn, capture thread_id
codex exec resume <thread_id> [--json] [-o FILE] "<next prompt>"      # subsequent turns, same session
```

Both accept prompt-on-stdin (`codex exec -` / `codex exec resume <id> -`, or stdin is auto-appended as a
`<stdin>` block if a positional prompt is *also* given — this auto-append behavior was observed live: our
first call had piped stdin available and the CLI printed `Reading additional input from stdin...` to
stderr even though a positional prompt was supplied, per the documented `--help` behavior for `[PROMPT]`).

## (b) `codex app-server` — JSON-RPC protocol

### Protocol shape (verified from generated schema + `--help`, not live-exercised)

- **JSON-RPC 2.0**, four message shapes per `JSONRPCMessage.json`: `JSONRPCRequest {id, method, params,
  trace?}`, `JSONRPCNotification {method, params}`, `JSONRPCResponse {id, result}`, `JSONRPCError {id,
  error:{code, message, data?}}`. `id` is `string | integer`. Requests optionally carry a W3C
  `traceparent`/`tracestate` for distributed tracing.
- **Transport**: `--listen` accepts `stdio://` (default), `unix://` / `unix://PATH`, `ws://IP:PORT`, or
  `off`; `--stdio` is shorthand for `stdio://`. Over stdio this strongly implies **newline-delimited JSON**
  (one JSON-RPC message per line — consistent with every other codex/claude stdio protocol observed and
  with `JSONRPCMessage` having no length-prefix framing field), but the exact wire delimiter was **not
  live-verified** by spawning `codex app-server` and inspecting raw bytes — flagged in §f.
- **Handshake**: client sends `initialize` (`ClientRequest`, params = `InitializeParams {clientInfo:
  {name, version, title?}, capabilities?: {experimentalApi, mcpServerOpenaiFormElicitation,
  optOutNotificationMethods, requestAttestation}}`), server responds, client then sends `initialized`
  (`ClientNotification`, the *only* client→server notification type in the schema).
- **Thread/turn model**: a "thread" (= session) is started with `thread/start` or resumed with
  `thread/resume` (`ThreadResumeParams` supports resuming **by threadId, by history payload, or by rollout
  file path**, precedence: history > path > threadId for non-running threads; for a *running* thread the
  server rejoins it and treats a supplied path as a consistency check). Work is driven by `turn/start`
  (`TurnStartParams {threadId, input, approvalPolicy?, approvalsReviewer?, collaborationMode?,
  additionalContext?, clientUserMessageId?, ...}`), with `turn/steer` and `turn/interrupt` for mid-turn
  control.
- **~100 request methods** total under `ClientRequest` covering thread/turn lifecycle, filesystem
  (`fs/*`), process/PTY control (`process/*`, `command/exec*`), MCP server management (`mcpServer/*`,
  `mcpServerStatus/list`), plugin/skill/marketplace management, account/auth, config read/write
  (`config/read`, `config/value/write`, `config/batchWrite`), fuzzy file search, and realtime/voice
  (`thread/realtime/*`). Full method list captured verbatim from `ClientRequest.json` during research (not
  reproduced in full here — see the generated schema bundle referenced in §f for the authoritative list).
- **Notifications** (`ServerNotification`, ~70 types) stream turn/item progress: `thread/started`,
  `turn/started`/`turn/completed`, `item/started`/`item/completed`, delta notifications
  (`item/agentMessage/delta`, `item/reasoning/*Delta`, `command/exec/outputDelta`,
  `item/fileChange/outputDelta`), plus `thread/tokenUsage/updated`, `account/rateLimits/updated`,
  `hook/started`/`hook/completed`, `guardianWarning`, `configWarning`, `model/rerouted`, and more.

### Approval requests (verified from generated schema — the exact shape a Marsad supervisor answers)

`ServerRequest` (server→client, i.e. codex asking the supervisor for a decision) includes the approval
surface:

| method | params (required fields) | when |
|---|---|---|
| `execCommandApproval` / `item/commandExecution/requestApproval` | `callId, command[], conversationId, cwd, parsedCmd[]` (`parsedCmd` classifies the command as `read`/`list_files`/`search`/`unknown` with best-effort resolved path) | shell command needs approval under the active `AskForApproval` policy |
| `applyPatchApproval` / `item/fileChange/requestApproval` | `callId, conversationId, fileChanges{path: FileChange}` (`FileChange` = add/delete/update with diff content), `grantRoot?`, `reason?` | file write needs approval |
| `item/permissions/requestApproval` | `cwd, itemId, permissions: RequestPermissionProfile, startedAtMs, threadId, turnId, reason?` | broader permission-profile escalation (e.g. requesting extra filesystem/network access) |
| `item/tool/requestUserInput` | (tool-defined schema) | a tool is asking the user a question mid-turn |
| `mcpServer/elicitation/request` | see §c | an attached MCP server is running an MCP elicitation |
| `item/tool/call` | — | dynamic tool call needs supervisor-side execution |

The **response** shape for command/patch approvals is a `ReviewDecision` enum:
`"approved" | "approved_for_session" | "denied" | "abort" | "timed_out" | {approved_execpolicy_amendment:
{proposed_execpolicy_amendment[]}} | {network_policy_amendment: {action: allow|deny, host}}`. A supervisor
answers by sending a `JSONRPCResponse` with `result: {decision: <one of the above>}` correlated to the
request's `id`. This is a clean, already-typed mapping to the HITL gate `phase2-drivers.md` describes — no
guessing needed, the schema is authoritative.

### Daemon / persistent app-server (relevant to "long-lived sessions")

`codex app-server daemon` subcommands: `bootstrap` (install durable local app-server management for
SSH-driven use), `start`/`restart`/`stop`, `enable-remote-control`/`disable-remote-control`, `version`
(prints local CLI + running app-server versions as JSON). `codex app-server proxy --sock <PATH>` proxies
stdio bytes to a running daemon's Unix control socket — i.e. a supervisor can start one daemon and have
multiple thin stdio clients attach/detach via `proxy`, rather than spawning a fresh app-server process per
session. `codex remote-control {start,stop}` layers websocket auth (capability-token or signed-bearer-token
JWT) on top for non-loopback listeners. None of this was live-exercised (spawning a daemon and connecting
was out of scope for the 2-call budget) — flagged in §f.

## (c) MCP: is codex an MCP client?

**Yes**, confirmed multiple ways:

- `codex mcp {list, get, add, remove, login, logout}` — a full CLI surface for managing *external* MCP
  servers codex attaches to as a client. Verified live (zero-cost, no model call): ran `codex mcp add
  test-probe --env FOO=bar -- echo hello`, inspected `codex mcp list --json` / `codex mcp get --json`, then
  `codex mcp remove test-probe` to clean up. Resulting `~/.codex/config.toml` section (the authoritative
  on-disk format for external MCP servers):

  ```toml
  [mcp_servers.test-probe]
  command = "echo"
  args = ["hello"]

  [mcp_servers.test-probe.env]
  FOO = "bar"
  ```

  `codex mcp add <NAME> (--url <URL> | -- <COMMAND>...)` — stdio servers take a command; streamable-HTTP
  servers take `--url` plus optional `--bearer-token-env-var`, `--oauth-client-id`, `--oauth-resource`.
  `--env KEY=VALUE` (repeatable) sets env vars for stdio servers only. `codex mcp login`/`logout` handle
  OAuth for HTTP servers. The `codex mcp list --json` shape: `[{name, enabled, disabled_reason, transport:
  {type: "stdio"|..., command, args, env, env_vars, cwd}, startup_timeout_sec, tool_timeout_sec,
  auth_status}]`.
- App-server exposes the same as protocol methods: `mcpServer/oauth/login`, `config/mcpServer/reload`,
  `mcpServerStatus/list`, `mcpServer/resource/read`, `mcpServer/tool/call` — so a Marsad supervisor can
  manage/introspect MCP servers over the same JSON-RPC channel instead of shelling out to `codex mcp`.
- `codex mcp-server` (separate top-level subcommand) makes codex act as an MCP **server** itself (stdio) —
  the reverse direction, not relevant to "is codex a client."

**MCP elicitation: yes, and it's `stable: true`.** `codex features list` shows
`tool_call_mcp_elicitation | stable | true`, and `codex doctor --json`'s enabled-feature-flags list for this
account includes `tool_call_mcp_elicitation`. The protocol schema has a dedicated server→client request,
`mcpServer/elicitation/request` (`McpServerElicitationRequestParams`), supporting three elicitation modes:
`"form"` (standard MCP JSON-Schema-driven form, `requestedSchema: McpElicitationSchema`), `"openai/form"`
(OpenAI's extended form elicitation — gated behind the `mcpServerOpenaiFormElicitation` client capability
declared at `initialize` time), and `"url"` (redirect-style elicitation carrying an `elicitationId` +
`url`). Response type `McpServerElicitationRequestResponse` (not fully inspected — see §f). This confirms
codex can both *receive* MCP tool calls that trigger elicitation and *surface* that elicitation to whatever
is on the other end of the app-server connection — i.e. a Marsad supervisor sitting as the JSON-RPC client
would receive these as approvable/answerable requests, same pattern as command/patch approvals.

`codex exec` (headless one-shot, §a) was not tested with an MCP server attached, so **whether elicitation
requests surface at all in `--json` exec mode (vs. only in app-server mode) is unconfirmed** — flagged in
§f; this matters for Wasl if it ever needs MCP tools that elicit.

## (d) Session/transcript storage on disk

**Location** (verified live — both calls wrote to and appended the same file):

```
~/.codex/sessions/<YYYY>/<MM>/<DD>/rollout-<YYYY>-<MM>-<DD>T<HH>-<MM>-<SS>-<thread-id-uuid>.jsonl
```

Concretely: `~/.codex/sessions/2026/07/02/rollout-2026-07-02T11-38-07-019f237a-f03d-7710-ab8f-7af32b222df8.jsonl`.
The filename's timestamp is the thread's *creation* time; resuming a thread **appends to the same file**
(verified: 12 lines after call 1, 21 lines after `codex exec resume` call 2 — no new file was created).
`--ephemeral` (an `codex exec` flag) skips this persistence entirely — relevant if Wasl wants stateless
one-shot calls that leave no trace, though that forfeits headless resume.

**Line schema** (JSONL, one record per line, verified live): every line is
`{"timestamp": <ISO8601 UTC ms>, "type": <string>, "payload": {...}}`. Observed `type` values across the two
live runs:

- `session_meta` (first line only) — `{session_id, id, timestamp, cwd, originator: "codex_exec", source:
  "exec", thread_source: "user", cli_version, model_provider, base_instructions: {text}}`. `base_instructions`
  contains the full system prompt actually used (captured live — this is the real, current Codex system
  prompt for `gpt-5.5`-family models, several KB of text covering personality, formatting rules, autonomy
  posture, etc.).
- `turn_context` — one per turn: `{turn_id, cwd, workspace_roots[], current_date, timezone,
  approval_policy, sandbox_policy: {type}, permission_profile, model, comp_hash, personality,
  collaboration_mode: {mode, settings: {model, reasoning_effort, developer_instructions}},
  multi_agent_version, realtime_active, effort, summary}`.
- `response_item` — raw model I/O turned into rollout form: user `message` (role, content[]), assistant
  `message`, `reasoning` (with `encrypted_content` — opaque, provider-encrypted reasoning trace, not
  plaintext).
- `event_msg` — the same higher-level lifecycle events as `codex exec --json`'s stdout stream plus more:
  `task_started`, `user_message`, `agent_message` (`{message, phase: "final_answer", memory_citation}`),
  `token_count` (`{info: {total_token_usage, last_token_usage, model_context_window}, rate_limits:
  {limit_id, primary: {used_percent, window_minutes, resets_at}, plan_type: "free", ...}}` — **confirms
  `plan_type: "free"` live**, matching the task's stated free-tier constraint), `task_complete`
  (`{turn_id, last_agent_message, completed_at, duration_ms, time_to_first_token_ms}`).

**Implication for Marsad**: this file is directly tailable — a supervisor can `tail -F` the day's rollout
file for a running thread and get a strict superset of what `codex exec --json` prints to stdout (plus the
system prompt, full turn context, and rate-limit telemetry), without needing to parse app-server JSON-RPC at
all. This is a legitimate second transcript source alongside the app-server `thread/read` /
`thread/turns/items/list` request methods (§b), useful as a resilience fallback if the app-server connection
drops mid-session.

An index/state store also exists (`~/.codex/state_5.sqlite`, `~/.codex/models_cache.json`,
`~/.codex/memories_1.sqlite`, `~/.codex/goals_1.sqlite`, `~/.codex/logs_2.sqlite` — all observed as
newly-created/touched by the live runs) but its schema was **not** inspected (out of scope, no sqlite
client run against it) — flagged in §f. The app-server `thread/list` / `thread/search` request methods are
the documented way to enumerate sessions; whether they read this sqlite state or re-scan the `sessions/`
tree is unconfirmed.

## (e) Sandbox / approval-policy flags for a supervisor

- **Sandbox** (`-s/--sandbox`, or `-c sandbox_mode=...`): `read-only | workspace-write |
  danger-full-access`. Verified live: `--sandbox read-only` produced `turn_context.sandbox_policy:
  {"type": "read-only"}` and a `permission_profile` of `{type: "managed", file_system: {type:
  "restricted", entries: [{path: {type: "special", value: {kind: "root"}}, access: "read"}]}, network:
  "restricted"}`.
- **Approval policy** (`AskForApproval`, from schema): `"untrusted" | "on-failure" (deprecated) |
  "on-request" | "never"`, or an object form `{granular: {sandbox_approval, rules, mcp_elicitations,
  request_permissions?, skill_approval?}}` for per-category control (schema-confirmed, not
  live-exercised). Exposed as `-a/--ask-for-approval` on `codex`, `codex resume`, `codex fork` — **absent**
  from `codex exec`/`codex exec resume` (§a). `codex exec` observed live to default to `"never"`.
- **`--dangerously-bypass-approvals-and-sandbox`** — skips all prompts and sandboxing entirely; present on
  every run-a-turn subcommand (`codex`, `exec`, `exec resume`, `resume`, `fork`, `review`). For a
  supervisor that wants to fully own risk decisions itself (e.g. Marsad's own gate already vetted the
  action), this is the flag that hands full control back to the harness instead of codex's sandbox.
- **`--dangerously-bypass-hook-trust`** — separate escape hatch specifically for the hooks system (see
  below); needed for automation that runs hooks without the normal interactive trust-persistence flow.
- **`-C/--cd`**, **`--add-dir`** — working root and additional writable roots, same shape as claude's
  `--add-dir`.
- **Hooks**: `features` shows `hooks | stable | true` (and it's in this account's enabled-flags list per
  `codex doctor`). Binary strings reveal a Claude-Code-shaped hook event vocabulary: `PreToolUse`,
  `PermissionRequest`, `PostToolUse`, `PreCompact`, `PostCompact`, `SessionStart`, `SubagentStart`,
  `SubagentStop`, plus a `HookHandlerConfig` with `matcher`/`command`/`timeout`/`async` fields (mirrors
  Claude Code's hook config shape closely). The app-server protocol has `hooks/list` (`ClientRequest`) and
  `hook/started`/`hook/completed` (`ServerNotification`) — so a supervisor sees hook execution as
  notifications, same channel as everything else. **Full hook config schema/registration flow was not
  live-tested** (no top-level `codex hooks` CLI subcommand exists; hooks appear to be config.toml/managed-
  config driven only) — flagged in §f.
- **`codex sandbox [COMMAND]...`** — a standalone subcommand to run an arbitrary command inside codex's own
  sandbox (Linux: seccomp + landlock/bwrap per binary strings; `-P/--permissions-profile` selects a named
  profile) without going through the agent loop at all — potentially useful for Marsad if it wants to reuse
  codex's sandboxing for its own tool execution, independent of spawning an agent turn.

## (f) Needs-live-verification checklist

Everything below is a real gap — either not reachable within the 2-call budget, or intentionally left
untested because it required mutating state beyond what was safe to leave behind:

1. **Raw app-server wire framing.** Never actually spawned `codex app-server --listen stdio://` and
   inspected raw bytes/newlines. Strongly implied to be newline-delimited JSON-RPC (no other framing field
   exists in the schema, and every other codex/claude stdio protocol observed is NDJSON) but not proven.
2. **Full `ClientRequest`/`ServerNotification` behavioral semantics** — the schema gives exact shapes for
   ~100 request methods and ~70 notifications, but only 2 live turns were run, so almost none of these were
   observed firing in practice (only `thread/started`-equivalent, `turn/started`/`completed`-equivalent,
   `item/completed`-equivalent were indirectly confirmed via the `exec --json` stream, which is a *different*
   simpler protocol from app-server proper — see §a/§b distinction).
3. **`item/tool/requestUserInput`, `item/permissions/requestApproval`, `mcpServer/elicitation/request`
   round-trip** — schema-confirmed shapes, but no live approval/elicitation was ever triggered (the test
   prompts were trivial by design and touched no tools requiring approval).
4. **`codex app-server daemon` lifecycle** (`start`/`bootstrap`/`enable-remote-control`) — not started;
   daemon-mode multi-client behavior (via `codex app-server proxy`) is documented from `--help` only.
5. **`codex exec` behavior when a command actually needs approval** — since `exec` hardcodes
   `approval_policy: never` and exposes no `-a` flag, what happens when the model proposes a
   sandbox-escaping command is unconfirmed: does it auto-deny and let the model route around it (consistent
   with the `never` policy's documented semantics: "Execution failures are immediately returned to the
   model"), or does `--sandbox workspace-write`/`danger-full-access` simply widen what's allowed without any
   approval gate ever firing? Trivial prompts never exercised this path.
6. **`codex exec resume` + `--sandbox`/`-c sandbox_mode=...` interaction** — `--sandbox` errors as an
   unrecognized arg on `resume`; whether `-c sandbox_mode=...` (generic config override) works around this
   was not tested.
7. **`~/.codex/state_5.sqlite` / `memories_*.sqlite` / `goals_*.sqlite` schemas** — created by the live
   runs, never opened with a sqlite client. Whether `thread/list`/`thread/search` (app-server) read from
   here or re-scan `sessions/` on disk is unknown.
8. **`codex exec --output-schema <FILE>`** (structured JSON output contract) — flag exists, not exercised;
   unknown how strictly it's enforced or how violations surface in the `--json` event stream.
9. **`codex exec-server`** (EXPERIMENTAL standalone service, `--listen ws://IP:PORT` default) — a third
   headless surface beyond `exec` and `app-server`; help text read, nothing else. Unclear how it differs
   from `app-server` in practice or whether it's stable enough to depend on.
10. **MCP elicitation inside `codex exec --json`** — confirmed to exist in app-server's protocol; whether
    (and how) it surfaces in the simpler `exec --json` event stream is unknown (no MCP server was attached
    during the live runs).
11. **Hook config file schema and registration** (`.rules`/`config.toml` shape for `[hooks]`, matcher
    syntax) — only inferred from binary strings, no working example constructed or tested.
12. **Windows-specific paths/behavior** (`windowsSandbox/*` methods exist in the schema) — this research
    was done entirely on Linux; irrelevant to Wasl/Marsad's current target but noted for completeness.

## (g) Verdict per consumer

### Wasl — spawn-per-turn headless adapter

**Recommended shape: wrap `codex exec` / `codex exec resume`, not `app-server`.** This is a direct
functional match for the claude equivalent (`claude -p --output-format json --session-id/--resume`,
prompt on stdin):

- First turn: `codex exec --json -o <last-message-file> [-m MODEL] [-s SANDBOX] [-c k=v ...] "<prompt>"`
  (or prompt via stdin). Parse the JSONL stream for `thread.started` to capture the session id for later
  resume; parse `item.completed` (type `agent_message`) / read `-o`'s file for the final reply;
  `turn.completed.usage` gives token accounting for free.
- Subsequent turns: `codex exec resume <thread_id> --json -o <file> "<prompt>"` (same id from
  `thread.started`, byte-stable across resumes — verified live).
- Structured reply: `--output-last-message <FILE>` for the plain final message (parity with claude's
  last-message extraction), or parse the `item.completed`/`agent_message` JSONL event directly if richer
  structure is wanted; `--output-schema <FILE>` exists for a stricter JSON-Schema-constrained final answer
  (untested, §f item 8) if Wasl ever needs guaranteed-typed replies.
- **Known limitation vs. claude**: no interactive approval channel in this mode (§a) — `codex exec` always
  runs with `approval_policy: never`. If Wasl's use case needs a human-in-the-loop gate mid-turn, `codex
  exec` cannot provide it; only sandbox-tier selection (`--sandbox`) is available as a pre-turn risk
  control. This is a real behavioral gap versus claude's `-p` mode (which also has no interactive approval,
  so arguably parity — but worth flagging since Wasl's docs elsewhere describe wanting approval surfacing).
- Every turn should pass `--skip-git-repo-check` if Wasl may run outside a git-trusted directory (hit this
  live) and should pick sandbox/approval flags matching Wasl's existing `LaunchSpec.posture`/`mode`
  vocabulary the same way `claude_argv` maps posture → claude flags.

### Marsad — long-lived session supervisor

**Recommended shape: `codex app-server` JSON-RPC client, exactly as `phase2-drivers.md`'s P2b/P2c blocks
already plan — now schema-backed instead of assumed.** Concretely:

- Run `codex app-server generate-json-schema --out <dir> --experimental` as a **build-time or
  fixture-generation step** (it's free, local, deterministic per codex version) and generate a typed client
  from the resulting schema bundle instead of hand-writing request/response structs — this removes the
  "needs the real codex schema" caveat P2b called out, since the schema is now mechanically obtainable.
- Handshake: send `initialize` with `clientInfo` + `capabilities` (set `experimentalApi: true` to get
  experimental methods/fields if Marsad wants the full surface; set `mcpServerOpenaiFormElicitation` if it
  wants to support OpenAI's extended elicitation form), then `initialized`.
- Start/resume: `thread/start` (new) or `thread/resume` (existing, by `threadId` — same ids as `codex
  exec`'s session ids, confirmed same UUID format and same `~/.codex/sessions/` storage, so a thread
  started via `codex exec` could in principle be picked up by an app-server client and vice versa — this
  cross-surface resume was **not tested** but is directly implied by `ThreadResumeParams`'s description).
  Drive turns with `turn/start`/`turn/steer`/`turn/interrupt`.
- Approvals → HITL gate: handle `execCommandApproval`, `applyPatchApproval`,
  `item/permissions/requestApproval`, `item/tool/requestUserInput`, `mcpServer/elicitation/request` as
  `ServerRequest`s; respond with the typed `ReviewDecision` (or elicitation-specific response) — this maps
  1:1 onto the existing gate pipeline `phase2-drivers.md` already designed for, no redesign needed.
- Transcript/event stream: subscribe to the ~70 `ServerNotification` types and normalize into
  `marsad_events` (P2c) — `item/agentMessage/delta`, `item/reasoning/*Delta`, `command/exec/outputDelta`
  give streaming granularity claude's `stream-json` also provides. As a resilience fallback (or a cheaper
  read-path for history), tail `~/.codex/sessions/<Y>/<M>/<D>/rollout-*.jsonl` directly (§d) — same
  `event_msg`/`response_item` shapes, no protocol round-trip needed for passive observation.
- For "launch/supervise long-lived sessions" specifically: prefer `codex app-server daemon start` (or
  `bootstrap` for SSH-driven hosts) over spawning a bare `codex app-server --listen stdio://` per session —
  the daemon persists across Marsad restarts and multiple threads can share one process, with
  `codex app-server proxy --sock <PATH>` as the thin per-connection stdio bridge. This wasn't live-tested
  (§f item 4) but is a direct `--help`-documented feature and matches Marsad's supervision model better than
  a 1:1 process-per-session app-server.
- MCP: Marsad can manage external MCP servers for a codex session either via `codex mcp add/...` (shell out,
  writes `~/.codex/config.toml`) or natively over the app-server protocol (`mcpServer/*` methods) —
  prefer the latter for a running daemon so config changes don't require a restart (`config/mcpServer/reload`
  exists for exactly this).
