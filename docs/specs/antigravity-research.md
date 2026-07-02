# Antigravity CLI (`agy`) research — MCP, elicitation, and headless-driver surface

**Status:** authoritative research, grounded in the real `agy` 1.0.15 binary and its on-disk state
(`~/.local/bin/agy`, already authenticated as the account in `~/.gemini/google_accounts.json`). NOT an
implementation plan — this is the evidence base a future Wasl-adapter / Marsad-driver phase (in the style of
`phase2-drivers.md`) would be scoped against. Every claim below is tagged with its evidence: a command
output, a real on-disk file, or a `strings`/`grep` hit against the binary. Claims with no such evidence are
listed in §6 as **needs live verification** — none were tested against a live model, per the zero-cost
constraint this doc was produced under (free-tier account).

## 0. What `agy` is

`agy` is the Antigravity CLI — "the terminal-first surface to interact with Antigravity agents"
(`agy changelog`, header line). v1.0.0 shipped as "Initial release"; this research is against 1.0.15
(`agy --version` → `1.0.15`). It is a Go/Bubble Tea TUI binary (164MB, statically linked) that reuses the
Gemini CLI's config home (`~/.gemini/`) and adds its own subtree at `~/.gemini/antigravity-cli/`. It talks to
Google's backend over Connect-RPC (`connect-go`) — model calls are not local, hence the hard no-live-call
constraint for this research.

Top-level `agy --help`:
```
Usage of agy:
  --add-dir                       Add a directory to the workspace (repeatable) (default [])
  -c                              Short alias for --continue
  --continue                      Continue the most recent conversation
  --conversation                  Resume a previous conversation by ID
  --dangerously-skip-permissions  Auto-approve all tool permission requests without prompting
  -i                              Short alias for --prompt-interactive
  --log-file                      Override CLI log file path
  --model                         Model for the current CLI session
  --new-project                   Create a new project for this session
  -p                              Short alias for --print
  --print                         Run a single prompt non-interactively and print the response
  --print-timeout                 Timeout for print mode wait (default 5m0s)
  --project                       Project ID for the current CLI session
  --prompt                        Alias for --print
  --prompt-interactive             Run an initial prompt interactively and continue the session
  --sandbox                       Run in a sandbox with terminal restrictions enabled

Available subcommands:
  changelog, help, install, models, plugin (alias plugins), update
```
This is the **complete** top-level flag surface — no hidden `--output-format`/`--json`/`--stream-json` flag
exists (unlike claude's `--print --output-format stream-json`). A stray `output-format`/`--json-schema`
string does exist in the binary (`strings ~/.local/bin/agy | grep output-format`), but it sits inside an
unrelated vendored-tool help string bundled in the binary's asset blob, not in agy's own flag parser — `-h`/
`--help` enumerates the real set and it isn't there. **Print mode has no streaming/event output — one line of
JSON at the end, full stop** (see §2).

`agy models` (no model call — this is a static/cached list, confirmed cheap by the task brief):
```
Gemini 3.5 Flash (Medium)
Gemini 3.5 Flash (High)
Gemini 3.5 Flash (Low)
Gemini 3.1 Pro (Low)
Gemini 3.1 Pro (High)
Claude Sonnet 4.6 (Thinking)
Claude Opus 4.6 (Thinking)
GPT-OSS 120B (Medium)
```
For any future live test, **`Gemini 3.5 Flash (Low)`** is the obvious cheapest pick.

`agy plugin list` → `No imported plugins.` (clean box; the owner hasn't installed any plugin/extension).

## 1. MCP server configuration

**File:** `~/.gemini/config/mcp_config.json` (currently empty on this machine — `cat` returns nothing, i.e.
no servers configured yet). This is the *migrated* location; changelog 1.0.3 fixed "a directory path mismatch
where pressing the `[Disable]` button wrote to the legacy `mcp_config.json` path instead of the migrated
`config/mcp_config.json` path" — so **do not** target `~/.gemini/mcp_config.json` (legacy, superseded).
`~/.gemini/config/.migrated` exists as the migration marker.

**Schema** — recovered directly from the binary's Go struct tags (authoritative; this is the literal
`encoding/json` unmarshal target, not a guess), via
`strings ~/.local/bin/agy | grep -o 'struct { Args \[\]string.*ServerUrl.*Url \*string.*'`:

```go
// top-level shape (from `struct { McpServers map[string]interface {} "json:\"mcpServers\"" }`)
type mcp_config struct {
    McpServers map[string]McpServerEntry `json:"mcpServers"`
}

// per-server entry (mcp.ConfigSchemaJsonMcpServersValue*)
type McpServerEntry struct {
    // stdio / local server
    Command          *string           `json:"command,omitempty"`
    Args             []string          `json:"args,omitempty"`
    Cwd              *string           `json:"cwd,omitempty"`
    Env              map[string]string `json:"env,omitempty"`   // ConfigSchemaJsonMcpServersValueEnv

    // remote / url server
    Url              *string           `json:"url,omitempty"`
    ServerUrl        *string           `json:"serverUrl,omitempty"`  // alt key seen alongside Url — see needs-verification
    Headers          map[string]string `json:"headers,omitempty"`    // ConfigSchemaJsonMcpServersValueHeaders
    AuthProviderType *AuthProviderType `json:"authProviderType,omitempty"`
    Oauth            *OauthConfig      `json:"oauth,omitempty"`      // ConfigSchemaJsonMcpServersValueOauth

    // shared
    Disabled         *bool             `json:"disabled,omitempty"`
    DisabledTools    []string          `json:"disabledTools,omitempty"`
    EnabledTools     []string          `json:"enabledTools,omitempty"`
    Tools            *ToolsConfig      `json:"tools,omitempty"`      // {background: {...}, eager: *bool}
}
```
This is verbatim Gemini-CLI's own `mcp_config.json` schema (agy reuses `gemini.GeminiMCPServerConfig` /
`mcp.ConfigSchemaJsonMcpServersValue*` types — same package family as upstream Gemini CLI), which is why
`agy help`/changelog talk about it as "the shared config directory" (`~/.gemini/config/`).

Confirmed facts from the changelog (`~/.gemini/antigravity-cli/cache/CHANGELOG.md`, also reachable via
`agy changelog`):
- 1.0.5: "Added support for `url` in `mcp_config.json` to configure MCP servers directly via a URL."
- 1.0.7: "Added a configurable timeout for launching MCP servers, allowing users to specify a custom timeout
  or set it to `-1` to disable the timeout completely." (the `timeout` field surfaces on the *runtime*
  McpServerSpec / McpServerState proto pair — `GetMcpServerStates`/`RefreshMcpServers` RPCs — rather than the
  static config struct above; whether it's writable in `mcp_config.json` itself vs. only via `/permissions` UI
  is a needs-verification item, §6).
- 1.0.15: "Increased the MCP connection timeout to 60 seconds to improve reliability for slow-starting custom
  MCP servers." — i.e. the default connect timeout as of this version is 60s.
- 1.0.4: "Parallelized the MCP server initialization sequence, preventing slow or hanging custom MCP servers
  from blocking independent, fast-starting servers (like local plugins) from loading" — plugins can also
  register MCP servers (`agy plugin import`/`install` discovery), a second source of servers besides
  `mcp_config.json`.
- 1.0.2: plugin installation now targets `~/.gemini/config/` (the shared dir), not the private app-data
  folder, "making them instantly discoverable" — consistent with `mcp_config.json` living under
  `~/.gemini/config/`.

Both **stdio** (`command`/`args`/`cwd`/`env`) and **url/remote** (`url`/`headers`/`oauth`) server shapes exist
in the same struct (a Go "kitchen sink" struct with omitempty, not a tagged union at the JSON level) — this
matches the separate `codeium_common_go_proto.McpServerConfig_Local` / `..._Remote` oneof seen at the proto
layer (`McpServerConfig.GetLocal()` / `GetRemote()` / `GetServerId()`), i.e. internally it's normalized into a
Local/Remote oneof, but the JSON file itself is one flat object per server with both families of fields legal
(unused ones just absent).

**Attempted materialization via `agy plugin import claude`:** the task brief suggested trying
`agy plugin import` against a throwaway Claude-style config to observe the real schema. Tried this in an
isolated `$HOME` (scratchpad only, `/tmp/.../scratchpad/fakehome/.claude/settings.json` with a fabricated
`mcpServers` block) — result: `No claude extensions found.` `plugin import` targets Claude Code **plugins/
extensions** (skills, agents, marketplace bundles), not `.claude/settings.json`'s `mcpServers` key, so this
path does not import MCP server configs and was not useful for schema recovery; the Go-struct-tag evidence
above is the authoritative source instead. (This test made zero network/model calls — pure local file I/O
against a fake `$HOME`; no files outside the scratchpad were touched.)

**Hierarchy note (1.0.5 changelog):** "Integrated the permissioning system with the rest of Antigravity. CLI
permissions now merges project level permissions, permissions from user settings shared with Antigravity, and
permissions from the CLI `settings.json`" — the "three configs" are (1) `~/.gemini/config/projects/<id>.json`
(project-level), (2) `~/.gemini/settings.json` (shared with the Gemini CLI / Antigravity IDE), and (3)
`~/.gemini/antigravity-cli/settings.json` (agy-only). 1.0.12 changelog: "project-specific configurations
(located in `~/.gemini/config/projects/`) take precedence over global settings in
`~/.gemini/antigravity-cli/settings.json`."

## 2. Elicitation

MCP "elicitation" (server asks the user a structured question mid-tool-call, per the MCP spec's
`elicitation/create`) is wired end-to-end in the binary — this is real support, not vestigial:

- Proto types: `cortex_go_proto.ElicitationInteractionSpec`, `cortex_go_proto.ElicitationInteraction`,
  `cortex_go_proto.RequestedInteraction_Elicitation`, `cortex_go_proto.CascadeUserInteraction_Elicitation`.
- `ElicitationInteractionSpec` accessor methods recovered from the binary
  (`strings ~/.local/bin/agy | grep ElicitationInteractionSpec).Get`):
  `GetServerName() string`, `GetMode() <enum>`, `GetMessage() string`, `GetRequestedSchemaJson() string`,
  `GetUrl() string`. I.e. an elicitation carries: which MCP server asked, a mode, a human-readable message, a
  JSON-Schema string describing the requested structured input, and (for the URL mode) a URL.
- Two elicitation modes exist at the MCP-client-library layer: `mcp.FormElicitationCapabilities` and
  `mcp.URLElicitationCapabilities` — i.e. a form-based (JSON-Schema-driven) elicitation and a URL-redirect
  elicitation (consent/OAuth-style flows), matching the two non-empty fields on the spec (`requested_schema_json`
  vs `url`).
- Handler plumbing: `handleElicitation`, `callElicitationCompleteHandler`, `mcp.ScopedAccessor.
  RegisterElicitationCallback`/`UnregisterElicitationCallback`, `mcp.SystemMcpManager.
  RegisterElicitationCallback`, `pendingElicitations`/`pendingElicitationsMu` (a map + mutex — elicitations are
  tracked as in-flight, awaitable state, not fire-and-forget).

**What's known:** the wire shape of an elicitation request (server, mode, message, schema, url) and that it's
implemented as a registered-callback / pending-map pattern (so it *can* be observed/answered out-of-band in
principle). **What needs live verification (§6):** (1) whether an elicitation in **print mode** (`-p`) blocks
indefinitely, auto-declines, or is rejected outright (no TUI to render the form) — there is no `--elicit-*`
flag, so headless answering is not exposed at the CLI-flag layer today; (2) whether `--dangerously-skip-
permissions` has any effect on elicitation (elicitation is a distinct interaction type from tool-permission
prompts in the proto — `RequestedInteraction` has both a `Permission` and an `Elicitation` variant — so it
probably does *not*); (3) the exact `RequestedInteraction`/`CascadeUserInteraction` JSON encoding as it would
appear in a transcript step, since none of the two real transcript files on this machine contain an
elicitation (the one real conversation never hit an MCP server).

## 3. Drivable surface for a supervisor (Marsad)

### 3.1 Print-mode JSON contract

`-p`/`--print`/`--prompt` runs one prompt non-interactively and prints exactly one line of JSON on completion
(confirmed both from `--help` prose and from the literal field-name strings in the binary:
`conversation_id`, `status`, `response`, `error`, `duration_seconds`, `num_turns`, `usage`):
```json
{
  "conversation_id": "...",
  "status": "...",
  "response": "...",
  "error": "...",
  "duration_seconds": 0.0,
  "num_turns": 0,
  "usage": { "...": "..." }
}
```
`usage` sub-fields recovered from the binary's token-accounting structs (`json:"input_tokens"`,
`json:"output_tokens"`, `json:"cache_read_input_tokens"`/`cache_read_tokens`, `json:"cache_creation_input_
tokens"`, `json:"total_input_tokens"`, `json:"total_output_tokens"`, `json:"cached_tokens"`) — the exact key
set surfaced in the print-mode summary object (vs. internal-only accounting structs) is a needs-verification
item (§6), but at minimum expect input/output/cached token counts.

Behavioral strings found around print mode (`strings ~/.local/bin/agy | grep -i "print mode"`):
`"Print mode: empty prompt, exiting"`, `"Print mode: silent auth failed, triggering OAuth"`, `"Print mode:
silent auth succeeded"`, `"Print mode: SendUserMessage failed: %v"`. Auth is attempted silently first in `-p`
mode and only escalates to interactive OAuth if silent refresh fails — since this machine is already
authenticated, that path shouldn't trigger, but a supervisor spawning `agy -p` should treat "hangs waiting on
stdin" as a possible OAuth-prompt symptom on an unauthenticated box.

Changelog-confirmed print-mode fixes/behaviors relevant to a driver:
- 1.0.9: "Fixed a bug in headless print mode resumption (`--conversation`/`-c -p ...`) where the CLI would
  dump the entire historical conversation transcript instead of only printing the newly generated response" —
  i.e. as of 1.0.9+, `-p --conversation <id>` (or `-c -p`) prints **only the new turn's** JSON result, not the
  full history — good, this is what a spawn-per-turn adapter wants.
- 1.0.5: "Fixed a bug that metadata was written in the current directory as opposed to
  `~/.gemini/antigravity-cli/cache` when running using `-p`" — confirms `-p` writes cache metadata under
  `~/.gemini/antigravity-cli/cache/`, not cwd (matters if a driver spawns agy from a scratch dir).
  `~/.gemini/antigravity-cli/cache/last_conversations.json` on this machine is exactly
  `{"<absolute-workspace-path>": "<conversation-uuid>"}` — a workspace → most-recent-conversation-id map,
  which is how `--continue` resolves "most recent" without a flag.
- 1.0.6: "Fixed `--sandbox` flag propagation in headless print mode... ensuring sandbox isolation is correctly
  enforced during non-interactive execution" — `--sandbox` is honored in `-p` mode as of 1.0.6+.
- Windows-only (1.0.15): "Fixed a bug on Windows where print mode and other non-TUI command outputs were
  silently discarded when run in non-TTY environments (such as pipes or subprocesses)" — implies the general
  design intent is that print-mode output *is* meant to survive being piped/subprocessed (which is exactly the
  Marsad/Wasl spawn model); this specific bug was Windows-only, but it signals non-TTY/piped invocation is a
  supported, tested scenario on this code path.

### 3.2 Transcript schema — from real files on disk

Two real conversations exist as artifacts of today's session, at two *different* storage layers:

1. **Canonical store:** `~/.gemini/antigravity-cli/conversations/<conversation-id>.db{,-wal,-shm}` — a SQLite
   database (confirmed via `python3 -c "import sqlite3; ..."`, read-only, local file only — no model call).
   Tables: `trajectory_meta` (trajectory_id, cascade_id, trajectory_type, source), `steps` (idx, step_type,
   status, has_subtrajectory, metadata BLOB, error_details BLOB, permissions BLOB, task_details BLOB,
   render_info BLOB, step_payload BLOB, step_format), `gen_metadata`, `executor_metadata`,
   `parent_references`, `trajectory_metadata_blob`, `battle_mode_infos`. The payload columns are **raw
   protobuf**, not JSON (confirmed — `steps.step_payload` for row 0 starts with protobuf varint/length-
   delimited framing, not text). This is the TUI's source of truth but is **not practical for an external
   driver to parse** without the proto definitions (`cortex_go_proto`) linked in — not attempted here (would
   require pulling proto schemas out of the binary, out of scope for a docs pass; flagged as a
   needs-verification/needs-build-tooling item in §6 if Marsad ever wants to read `.db` directly instead of
   tailing JSONL).
2. **Textual projection:** `~/.gemini/antigravity-cli/brain/<conversation-id>/.system_generated/logs/
   transcript.jsonl` and `transcript_full.jsonl` — **this is the practical integration point for a driver**:
   plain JSONL, one step object per line, human/LLM-readable. `<conversation-id>` here is
   `97197d46-2091-40e2-b36f-0dcedd34bcc4`, and `~/.gemini/antigravity-cli/cache/last_conversations.json` maps
   the workspace path to it (`{"/work/mewais/OpenSource/Marsad": "97197d46-2091-40e2-b36f-0dcedd34bcc4"}`).
   `brain/<id>/` is additionally a **git repository** (`.git/`, `.gitignore` excluding `*.db*`,
   `.system_generated/tasks/`, `.system_generated/terminals/`, `.tempmediaStorage/`, `browser/`), with 4 real
   commits (`Initial commit with .gitignore`, then 3 `Snapshot` commits) — i.e. agy snapshots the workspace
   into a private git history per conversation, presumably backing `/diff`/checkpoint/revert features.

   **Real schema, from the actual 5-line file** (verbatim, only content is a smoke-test prompt `"x"`):
   ```json
   {"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","created_at":"2026-07-02T15:23:36Z","content":"<USER_REQUEST>\nx\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\nThe current local time is: 2026-07-02T11:23:36-04:00.\n</ADDITIONAL_METADATA>\n<USER_SETTINGS_CHANGE>\n...\n</USER_SETTINGS_CHANGE>"}
   {"step_index":1,"source":"SYSTEM","type":"CONVERSATION_HISTORY","status":"DONE","created_at":"2026-07-02T15:23:36Z"}
   {"step_index":2,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-07-02T15:23:36Z","tool_calls":[{"name":"list_dir","args":{"DirectoryPath":"/home/mewais/.gemini/antigravity-cli","toolAction":"Listing application directory","toolSummary":"List App Data Directory"}}]}
   {"step_index":4,"source":"SYSTEM","type":"CHECKPOINT","status":"DONE","created_at":"2026-07-02T15:23:37Z","content":"{{ CHECKPOINT 0 }}\n **The earlier parts of this conversation have been truncated...**\n...\n- /home/mewais/.gemini/antigravity-cli/brain/<id>/.system_generated/logs/transcript.jsonl\n\n**IMPORTANT: ...DO NOT ACKNOWLEDGE THIS CHECKPOINT MESSAGE...**"}
   {"step_index":5,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-07-02T15:23:37Z","content":"It looks like your request was just the letter \"x\". Could you please clarify..."}
   ```
   Fields observed: `step_index` (int, not always contiguous — 3 is skipped, presumably an internal step
   filtered out of this projection), `source` (`USER_EXPLICIT`|`SYSTEM`|`MODEL` seen), `type`
   (`USER_INPUT`|`CONVERSATION_HISTORY`|`PLANNER_RESPONSE`|`CHECKPOINT` seen), `status` (`DONE` seen — others
   presumably `RUNNING`/`ERROR`/etc., not observed), `created_at` (RFC3339 UTC), `content` (string, present on
   user/checkpoint/final-answer steps), `tool_calls` (array of `{name, args}`, present on steps that invoked
   tools — `args` is a flat string-keyed map). **`content` on `USER_INPUT` wraps the raw prompt in
   `<USER_REQUEST>...</USER_REQUEST>` plus injected `<ADDITIONAL_METADATA>`/`<USER_SETTINGS_CHANGE>` blocks —
   a driver parsing this file for "what did the user actually say" must strip this wrapper.**

   `transcript.jsonl` vs `transcript_full.jsonl`: byte-identical except one line (`diff` output, both real
   files) — `transcript.jsonl`'s `tool_calls[].args` values are **JSON-string-escaped again**
   (`"DirectoryPath":"\"/home/mewais/.gemini/antigravity-cli\""`, i.e. a JSON string containing a
   double-quoted string), while `transcript_full.jsonl`'s are clean (`"DirectoryPath":"/home/mewais/.gemini/
   antigravity-cli"`). **A driver should prefer `transcript_full.jsonl`** for this reason — one less
   quoting trap. Only 5 lines exist because the "2-3 prompts" the owner ran today were interactive-TUI turns
   ending in `/exit` (confirmed via `~/.gemini/antigravity-cli/history.jsonl`:
   `{"display":"/exit","timestamp":...,"workspace":"/work/mewais/OpenSource","type":"slash_command"}`), not
   `-p` invocations — **whether `-p` mode writes to this same `brain/<id>/.../transcript*.jsonl` path is a
   needs-verification item (§6)**, since the only real evidence captured here is from an interactive run.

### 3.3 Hooks

**File:** `~/.gemini/config/hooks.json` (shared config dir — same tier as `mcp_config.json`). 1.0.8
changelog: "Fixed a bug where the `/hooks` command wrote configurations to
`~/.gemini/antigravity-cli/hooks.json` instead of the shared `~/.gemini/config/hooks.json`, ensuring hooks
remain synchronized between the TUI and the backend" — confirms the *shared* path is correct/current; the
antigravity-cli-local path was the pre-1.0.8 bug. No `hooks.json` exists on this machine yet (`find ~/.gemini
-iname '*hooks*'` → nothing) — the owner hasn't configured any.

Binary evidence for the hook engine (package `google3/third_party/jetski/cortex/customization/hooks/jsonhook`):
`ParseHooksFile`, `JSONHookSpec` (a `map[string]jsonhook.JSONHookSpec`, i.e. hook definitions are keyed —
by event name, matching the pattern below), `MatchesToolName`, `PreToolHookConverter`,
`HookHandler.Execute`, `expandHome` (paths in hook commands support `~` expansion), `setDefaults`. JSON field
names recovered directly (`strings ~/.local/bin/agy | grep 'json:"'`): `hooks`, `matcher`, `type`, `command`,
`timeout`, `stop`. Event-name symbols present in the binary: `PreToolUse`, `PostToolUse`, `SessionStart`,
`SessionEnd` (each appears as a distinct symbol/string, not just substring noise). Runtime log line:
`"skipping hooks.json at %s: %v"` / `"hooks.json found at %s"` / `"hooks.json from %s: %d named hooks"` /
`"failed to write hooks.json to %s: %v"` — hooks are loaded per-discovered-path (project + shared + built-in
tiers, same customization-discovery machinery as skills) and merged, and load failures are logged, not fatal.

Putting the field names and event names together, the shape is almost certainly Claude-Code-hooks-compatible:
```json
{
  "hooks": {
    "PreToolUse": [
      { "matcher": "shell", "hooks": [ { "type": "command", "command": "~/bin/audit.sh", "timeout": 30 } ] }
    ],
    "PostToolUse": [ ... ],
    "SessionStart": [ ... ],
    "SessionEnd": [ ... ]
  }
}
```
**This exact nesting (top-level `"hooks"` wrapper vs. bare event-keyed map; whether `Stop`/`Notification`/
`UserPromptSubmit`/`PreCompact` also exist as events; whether `matcher` is a tool-name string, a regex, or a
list) is inferred from field/symbol names, not observed in a real file — needs-verification, §6.** The
`json:"stop"` field is real but its owning struct wasn't isolated; it may be a per-hook "stop propagation"
flag rather than a `Stop` lifecycle event — ambiguous from strings alone.

### 3.4 Permission behavior in `-p` mode

No live test was run (would require sending a prompt to a model). From static evidence only:
- `--dangerously-skip-permissions` exists and its own log string confirms the semantics:
  `"--dangerously-skip-permissions: auto-approving all tool permissions"` and (permission-prompt copy)
  `"No, and always deny '%s' in this conversation (overridden by dangerously-skip-permissions)"` — so even a
  saved "always deny" rule is overridden by this flag. This is the honest equivalent of claude's
  `--dangerously-skip-permissions`.
- `types.PermissionMode` exists with at least modes `"default"` and `"plan"` as literal strings recovered from
  the binary, plus `"proceed-in-sandbox"` (1.0.1 changelog: "Added `proceed-in-sandbox` tool permission mode.
  Auto-approves terminal commands that run inside the secure sandbox, requesting manual approval only when a
  command attempts to bypass the sandbox.") — i.e. there is a middle ground between full-manual and
  skip-all-permissions when combined with `--sandbox`.
- Permission decisions are governed by a 3-tier merge (§1, "Hierarchy note") of project / shared-user /
  agy-local settings, with the interactive `/permissions` TUI panel able to add "Always Approve" rules
  (1.0.13: matching is exact-string by default, `regex:`-prefixed for opt-in regex).
- `google3/.../permissions.(*BattleModePermissionManager).PromptUser` exists — i.e. the permission manager has
  an explicit interactive-prompt code path, implying there is a **separate, non-interactive path it must fall
  back to when there's no TTY** (as in `-p` mode) — but what that fallback resolves to (auto-deny? consult
  saved rules only and error out on a miss? hang?) is **not evidenced in strings** and is the single most
  important needs-verification item for driving agy headlessly (§6, test 1).

## 4. Headless session continuity flags

| Flag | Effect | Evidence |
|---|---|---|
| `--print` / `-p` / `--prompt` | Run one prompt non-interactively, print final-result JSON, exit | `--help` |
| `--conversation <id>` | Resume a specific past conversation by UUID | `--help`; UUIDs match `conversations/*.db` and `brain/*/` dir names |
| `--continue` / `-c` | Resume the most recent conversation (per-workspace, via `cache/last_conversations.json`) | `--help` + `cache/last_conversations.json` shape |
| `--conversation <id> -p ...` or `-c -p ...` | As of 1.0.9, prints **only the new turn**, not full history replay (a pre-1.0.9 bug did the latter) | CHANGELOG 1.0.9 |
| `--print-timeout` | Timeout for waiting on the print-mode response, default `5m0s` | `--help` |
| `--project <id>` / `--new-project` | Pin or create the project a session belongs to (project ≠ conversation; a project groups conversations/resources — see `config/projects/<id>.json`) | `--help`; CHANGELOG 1.0.12 |
| `--add-dir <path>` (repeatable) | Extra workspace directories visible to the agent | `--help` |
| `--model <name>` | One of the `agy models` strings | `--help` + `agy models` |
| `--sandbox` | Terminal-restricted sandbox; honored in `-p` mode since 1.0.6 | `--help` + CHANGELOG 1.0.6 |
| `--dangerously-skip-permissions` | Auto-approve all tool permissions, overriding even saved "always deny" rules | `--help` + binary log strings |
| `-i` / `--prompt-interactive` | Send one initial prompt, then drop into the normal interactive TUI (not headless — no print-mode JSON at the end; not useful for a supervisor) | `--help` |

No `--fork`/`--branch` or multi-turn-in-one-invocation flag was found — the continuity model is: **one process
per turn**, `--conversation <id>` (or `-c`) to thread turns together, exactly like claude's `--resume`/
`--continue`. This is good news for a spawn-per-turn adapter (§5).

## 5. Config / env surface

**Config file tree** (all under `~/.gemini/`, real paths on this machine):
```
~/.gemini/settings.json                          shared user settings (security.auth.selectedType=oauth-personal)
~/.gemini/projects.json                           workspace-path -> project label, e.g. {"/home/mewais": "mewais"}
~/.gemini/trustedFolders.json                     workspace-path -> "TRUST_FOLDER"
~/.gemini/google_accounts.json                    {"active": "<email>", "old": []}
~/.gemini/oauth_creds.json                        OAuth token store (0600 perms; do not touch/log)
~/.gemini/installation_id                         stable per-machine install UUID
~/.gemini/config/config.json                      {"userSettings": {"remoteControlHostname": "..."}}
~/.gemini/config/mcp_config.json                  MCP servers, see §1 (empty here)
~/.gemini/config/hooks.json                       hooks, see §3.3 (absent here — never configured)
~/.gemini/config/.migrated                         migration marker
~/.gemini/config/projects/<project-id>.json        per-project resources, e.g. default-cli-project.json = {"id":"default-cli-project","name":"Default Project","projectResources":{}}
~/.gemini/antigravity-cli/settings.json            agy-local settings: {"colorScheme","enableTelemetry","trustedWorkspaces":[...]}
~/.gemini/antigravity-cli/jetski_state.pbtxt       onboarding-step tracking (protobuf text format)
~/.gemini/antigravity-cli/installation_id          (duplicate of the top-level one)
~/.gemini/antigravity-cli/history.jsonl            slash-command history: {"display","timestamp","workspace","type"}
~/.gemini/antigravity-cli/cache/CHANGELOG.md       cached changelog (source for `agy changelog`)
~/.gemini/antigravity-cli/cache/last_conversations.json   workspace-path -> most-recent-conversation-id
~/.gemini/antigravity-cli/cache/default_project_id.txt    e.g. "default-cli-project"
~/.gemini/antigravity-cli/cache/onboarding.json    {"consumerOnboardingComplete","enterpriseOnboardingComplete","onboardingComplete"}
~/.gemini/antigravity-cli/conversations/<id>.db{,-wal,-shm}   SQLite conversation store, see §3.2
~/.gemini/antigravity-cli/brain/<id>/              per-conversation git-tracked workspace snapshot, see §3.2
~/.gemini/antigravity-cli/log/cli-<timestamp>.log  per-launch log file (overridable via --log-file)
~/.gemini/antigravity-cli/builtin/skills/          builtin skills (e.g. antigravity_guide, added 1.0.10)
~/.gemini/antigravity-cli/updater/update.lock       self-update lock file
```

**Env vars** (all `AGY_CLI_*`, recovered by `strings ~/.local/bin/agy | grep -o 'AGY_CLI_[A-Z_]*'`, each
cross-checked against a changelog line where possible):
| Var | Effect | Evidence |
|---|---|---|
| `AGY_CLI_DISABLE_LATEX` | Disables LaTeX math rendering in the TUI | CHANGELOG 1.0.4 |
| `AGY_CLI_HIDE_ACCOUNT_INFO` | Hides email/plan tier from the header | CHANGELOG 1.0.2 |
| `AGY_CLI_CMD_OUTPUT_PERCENTAGE` | Max height of command output blocks, as % of terminal height | CHANGELOG 1.0.11 |
| `AGY_CLI_DISABLE_AUTO_UPDATE` | Disables the self-updater | symbol only, name is self-explanatory |
| `AGY_CLI_DISABLE_NEW_USAGE` | Reverts to the legacy usage/quota page | symbol only, cf. CHANGELOG 1.0.8 "Models & Quota" redesign |
| `AGY_CLI_EXPERIMENTAL_RENDERING` | Opts into an experimental TUI rendering path | symbol only |
| `AGY_CLI_FORCE_OSC` | Forces OSC terminal escape sequences (hyperlinks etc.) | symbol only, cf. CHANGELOG 1.0.12 OSC8 hyperlinks |
| `AGY_CLI_LOGO_STYLE` | Alternate CLI logo rendering | symbol only, cf. CHANGELOG 1.0.3 "Redesign CLI logo on Apple Terminal" |
| `AGY_CLI_NEW_HARNESS` | Opt-in flag for a newer agent harness | symbol only |

None of these look relevant to a headless/JSON-only driver except possibly `AGY_CLI_DISABLE_AUTO_UPDATE` (a
supervisor probably wants updates disabled so its pinned binary/flag-set doesn't shift mid-fleet) —
speculative, unconfirmed effect, so treat as a needs-verification nice-to-have, not a requirement.

**Auth:** OAuth-personal (`~/.gemini/settings.json`: `security.auth.selectedType = "oauth-personal"`),
already authenticated as `mohammad.a.ewais@gmail.com` (`~/.gemini/google_accounts.json`). No API-key /
service-account env var was found in the binary's string table — this build appears to be OAuth-only, which
matters for a supervisor: **there's no obvious non-interactive credential-injection path**; a driver would
need to either reuse this machine's already-authenticated `~/.gemini/oauth_creds.json` (fine for a
single-owner fleet-node setup like this one) or find/verify a service-account flag (not found — flagged §6).

## 6. Needs-live-verification checklist

Every item below requires exactly one real `agy -p ...` call to resolve — **none were run** (zero-cost
constraint). Cheapest model for all of these: **`Gemini 3.5 Flash (Low)`** (from `agy models`); use a
single-word/no-tool-call prompt wherever the test doesn't specifically require a tool call, to minimize turns
and tokens.

| # | Question | Why it matters | Cheapest test |
|---|---|---|---|
| 1 | Does a tool-permission prompt in `-p` mode (no `--dangerously-skip-permissions`) hang until `--print-timeout`, auto-deny, or auto-allow against saved rules? | Determines whether a Marsad driver *must* pass `--dangerously-skip-permissions` for any prompt that might touch a tool, or can rely on pre-seeded "always allow" rules for a gated flow | `agy -p --model "Gemini 3.5 Flash (Low)" "run the shell command: echo hi"` **without** `--dangerously-skip-permissions`, on a workspace with zero pre-existing permission rules; observe whether it returns within seconds or stalls to `--print-timeout` |
| 2 | Does `-p` mode write to `brain/<id>/.system_generated/logs/transcript{,_full}.jsonl`, same as interactive mode? | Confirms the JSONL tailing strategy in §3.2 works for headless-spawned turns, not just interactive ones (only interactive evidence exists today) | `agy -p --model "Gemini 3.5 Flash (Low)" "say hi"`, then check `~/.gemini/antigravity-cli/cache/last_conversations.json` for the new id and `ls`/`cat` its `brain/<id>/.system_generated/logs/` |
| 3 | Exact top-level JSON key set of the `-p` result object (is `usage` present with which sub-keys; is `error` `null` vs. absent on success)? | Needed to write a strict deserializer in the Wasl adapter | Same call as #2; `jq .` the one output line |
| 4 | What `status` enum values exist (`"success"`? `"error"`? `"timeout"`?) and how errors surface (`error` populated + `status` non-success, or thrown non-zero exit + stderr instead)? | Needed for adapter error handling | Run #2's happy path, then a deliberately-invalid one, e.g. unknown `--model "nonexistent-model"` in `-p` mode, and inspect exit code + stdout/stderr split |
| 5 | Real `hooks.json` schema — top-level `"hooks"` wrapper vs. flat map; which event names actually fire (`PreToolUse`/`PostToolUse` confirmed as symbols, `Stop`/`Notification`/`SessionStart`/`SessionEnd`/others unconfirmed as *file* keys); `matcher` semantics (tool-name exact match? regex?) | Needed before Marsad can rely on hooks as an approval-gate integration point (the phase2-drivers.md guardrail: "Approvals must route to the gate") | Write a minimal `~/.gemini/config/hooks.json` with a `PreToolUse` hook that just appends to a local file, then `agy -p --model "Gemini 3.5 Flash (Low)" "run: echo hi"` and check whether/how the hook fired |
| 6 | Real `mcp_config.json` end-to-end: does a stdio server with `command`/`args` actually get invoked (validates §1's schema deserializes and connects), and does a `disabled: true` server get skipped? | Confirms the Go-struct-tag-derived schema is not just what unmarshals but what's *acted on* | Point `mcp_config.json` at a trivial local echo-stdio MCP server (no model call needed for the *connection* itself — but exercising it via the agent does require one turn); `agy -p` a prompt that would need to list MCP tools |
| 7 | Elicitation end-to-end shape as it appears in `transcript_full.jsonl` (what `type`/`source` values, does it block the turn, can `-p` mode even complete a turn that raises one) | §2 — this is the biggest unknown for HITL-via-Marsad | Requires an MCP server that actually calls `elicitation/create`; higher setup cost than the others — lowest priority to resolve first |
| 8 | Does `--dangerously-skip-permissions` also silently answer elicitations, or only tool-permission prompts? | Correctness of the Marsad gate mapping | Combine #7's server with `--dangerously-skip-permissions` |
| 9 | Whether `ServerUrl` vs `Url` in the MCP schema are aliases or distinct (one legacy?) | Avoids writing a config with the wrong key | Configure a URL-based MCP server using each key separately across two `-p` runs and see which one the CLI actually connects with (log-based confirmation, e.g. `--log-file`, may not even require a full model turn — worth trying the connection-only, no-turn behavior first, i.e. does `agy --print --print-timeout=5s` even attempt server discovery before the model responds — needs checking without a full paid turn) |

## 7. Verdict / recommendation per consumer

### Wasl adapter (spawn-per-turn, like the existing `claude` adapter)
**Feasible with today's flag surface**, structurally very close to the existing claude adapter:
- Argv shape: `agy -p --model "<name>" [--conversation <id> | (first turn, no flag)] [--dangerously-skip-
  permissions] [--sandbox] "<prompt>"`, same one-process-per-turn model as claude's `--print --resume`.
- Continuity: `--conversation <id>` from the previous turn's `conversation_id` field in the print-mode JSON —
  simpler than claude's transcript-path threading, since agy returns the id directly in-band.
- Parsing: single JSON line on stdout (§3.1) — simpler than claude's `stream-json` line-delimited event
  stream (nothing to demux mid-turn; the tradeoff is **zero visibility into tool calls/thinking as they
  happen** — a Wasl adapter built against `-p` alone is call-and-wait, not call-and-observe).
- Gap vs. claude adapter: no in-band streaming means tool-call-level events for the UI/board must come from
  tailing `transcript_full.jsonl` **in parallel** with the `-p` subprocess (§3.2) — this is a second I/O
  source the claude adapter doesn't need (claude gets it all on one stdout stream). Whether this file updates
  live during a running `-p` turn or only flushes at the end is itself unconfirmed (needs-verification, not
  listed separately above but implied by #2/#3 — worth checking incrementally, i.e. `cat` the file *while*
  `-p` is still running).
- **Recommendation:** build it, but scope it as "claude-adapter parity minus mid-turn streaming" rather than
  full parity, and resolve needs-verification #1–#4 first (permission-hang behavior is a launch-blocker: if a
  bare `-p` call can hang forever on a permission prompt, the adapter must default to
  `--dangerously-skip-permissions` plus lean on the hooks/gate system (needs-verification #5) for approvals
  instead of the interactive prompt path).

### Marsad driver (launch/observe/gate, per `phase2-drivers.md`'s pattern)
**Not yet at parity with codex/gemini-ACP's protocol-native approach**, and structurally different again from
both: agy has no JSON-RPC/ACP mode — it's print-mode-JSON *or* full TUI, nothing in between. For a Marsad
driver (which wants live observe + gate, not just launch/collect):
- **Observe:** tail `transcript_full.jsonl` (§3.2) for step-level events, normalized into `marsad_events` the
  same way P2c/P2e do for codex/gemini — feasible in principle, but the schema is thinner than a purpose-built
  protocol (no explicit tool-call-start/tool-call-end pairing observed, no clear approval-request event type
  in the one real sample) and was captured from a *single, toolcall-light* real conversation — before betting
  the normalizer on this shape, capture a **richer** real fixture (multi-tool-call, at least one denied
  permission, ideally one elicitation) the same way `phase2-drivers.md` insists on for codex/gemini
  ("Capture real fixtures... Do NOT implement a normalizer from docs alone" — that guardrail applies here with
  even more force, since this doc's transcript evidence is thinner than what P2c/P2e will have).
- **Gate:** hooks (`~/.gemini/config/hooks.json`, §3.3) are the only discovered mechanism that could intercept
  before a tool runs and is the natural mapping to Marsad's HITL gate (`PreToolUse` hook → gate check) — but
  its schema is inferred, not observed (needs-verification #5), and it's a fire-a-command mechanism, not a
  request/response protocol call a driver can synchronously answer the way codex/ACP approval requests work —
  a `PreToolUse` hook can presumably only allow/block by exit code, not carry rich context back into the gate
  UI the way a JSON-RPC approval request would. This is a materially weaker gate integration than codex/gemini
  will get from P2c/P2e.
- **Recommendation:** treat an agy/Marsad driver as **lower priority and lower fidelity than codex/gemini**
  until needs-verification #1, #2, #5, #7, #8 are resolved with real fixtures — the permission/elicitation
  gate story is the crux of Marsad's value proposition (HITL), and today it's the least-evidenced part of this
  whole surface. If/when pursued, scope it as its own phase-2-style block list (argv + print-mode client +
  JSONL-tail-based events-normalizer + hooks-based gate integration), explicitly flagging the hooks-as-gate
  approach as weaker than the JSON-RPC approval-request pattern codex/gemini get, and re-evaluate once a real
  `hooks.json` + real elicitation fixture exist.

## Sources
- `agy --version`, `agy --help`, `agy help <sub>`, `agy models`, `agy changelog`, `agy plugin list`, `agy
  plugin --help` (all local, zero-cost, no model call).
- `~/.gemini/antigravity-cli/cache/CHANGELOG.md` (== `agy changelog` output; read in full, 214 lines, v1.0.0
  through v1.0.15).
- On-disk config/state files under `~/.gemini/` and `~/.gemini/antigravity-cli/` (enumerated in §5), read
  directly — all real files on this machine, not fabricated.
- Real conversation artifacts: `~/.gemini/antigravity-cli/conversations/97197d46-2091-40e2-b36f-
  0dcedd34bcc4.db` (inspected read-only via `python3`/`sqlite3` module — table list + schema only, no write),
  and `~/.gemini/antigravity-cli/brain/97197d46-2091-40e2-b36f-0dcedd34bcc4/.system_generated/logs/
  transcript.jsonl` + `transcript_full.jsonl` (read in full, 5 lines each, `diff`'d against each other).
- `strings -a ~/.local/bin/agy | grep ...` against the installed binary, for: MCP config Go struct tags,
  elicitation proto accessors, hooks JSON field/event names, print-mode JSON field names, usage/token field
  names, `AGY_CLI_*` env vars, permission-mode strings. All quoted symbol/string hits are literal binary
  contents, not inferred.
- One throwaway local-only test: `agy plugin import claude` against a fabricated `$HOME` under the task
  scratchpad directory (`/tmp/claude-1001/.../scratchpad/fakehome/`), to check whether it materializes an
  MCP config schema (it doesn't — it's for plugin/extension import, not `mcpServers`). No files outside the
  scratchpad were touched; no network/model call was made.
- Explicitly **not** done: any `agy -p`/`--print`/`--prompt-interactive` invocation, or anything else that
  would reach a live model — per the hard zero-live-call constraint. Every such gap is listed in §6.
