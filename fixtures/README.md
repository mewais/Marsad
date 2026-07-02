# Fixtures — recorded-real Claude Code captures

These checked-in captures make the "Recorded-real" tests in `docs/specs/TESTPLAN.md` deterministic
and runnable anywhere (no `claude` binary needed at test time).

## `streams/` — stream-json NDJSON

Each file is the raw `--output-format stream-json --verbose` output of one real `claude` run.

| File | Scenario | Status |
|---|---|---|
| `qa.ndjson` | plain Q&A (init → assistant → result/success, real usage+cost) | **captured** |
| `permission.ndjson` | a permission gate (`can_use_tool`, real `git push --force`) | **captured** (real claude 2.1.198) — parses RED, see "Known gaps" |
| `plan.ndjson` | a plan-mode gate (`ExitPlanMode`) | **captured** (real claude 2.1.198) — parses RED, see "Known gaps" |
| `edit.ndjson` | an edit-approval gate (real `Edit` tool on NOTES.txt) | **captured** (real claude 2.1.198) — parses RED, see "Known gaps" |
| `subagent.ndjson` | a subagent spawn + completion | **captured** (real claude 2.1.198) — parses RED, see "Known gaps" |
| `compaction.ndjson` | a compaction event (real `/compact`, `compact_result: success`) | **captured** (real claude 2.1.198) — parses RED, see "Known gaps" |

Regenerate / extend with `scripts/capture_fixtures.sh [/path/to/claude]` (note: as shipped, that
script does not send the control-protocol `initialize` handshake nor `--permission-prompt-tool
stdio`, so it will not actually produce `can_use_tool` control_requests on a real claude 2.1.x —
the gate/subagent/compaction fixtures above were captured with a purpose-built driver instead; see
"How these were captured" below).

## `hooks/` — hook payload samples

JSON captured from a temporary Claude Code hook (one file per event). Status:

| File | Status |
|---|---|
| `PreToolUse.json` | **captured** (real claude 2.1.198, a Bash call) |
| `PostToolUse.json` | **captured** |
| `Stop.json` | **captured** |
| `SubagentStop.json` | **captured** |
| `SessionStart.json` | **captured** |
| `SessionEnd.json` | **captured** |
| `PreCompact.json` | **captured** |
| `Notification.json` | **not captured** — tried twice against a real claude 2.1.198 session with every hook wired to a temporary `.claude/settings.json`: (1) a normal permission-gated Bash run, answered immediately — no `Notification` fired; (2) a second run where the `can_use_tool` control_request was deliberately left unanswered for 65s (the documented "idle ≥ 60s" trigger) — still nothing. In this environment permission gates are routed entirely through the `--permission-prompt-tool stdio` control-protocol channel; `Notification` (an interactive-TTY nudge) does not appear to fire in headless `--print` mode for either documented trigger condition. `hook_notification_is_valid_json` in `crates/marsad-proto/tests/fixtures.rs` is left `#[ignore]`-d with this explanation rather than captured with synthesized content. |

See code.claude.com/docs/en/hooks for the hook payload schema.

## How the gate/subagent/compaction/hooks fixtures were captured

`scripts/capture_fixtures.sh`'s naive one-shot pipe (`echo user-turn | claude ... > file`) cannot
produce a `can_use_tool` control_request on a modern claude CLI: the control protocol requires (a)
an `initialize` control_request sent on stdin before any user turn, and (b)
`--permission-prompt-tool stdio` on argv (both are what `marsad-agent`'s real `claude_argv` /
session-attach code actually does — see `crates/marsad-agent/src/lib.rs`). Capturing these fixtures
therefore used a small driver (not committed, lived in a scratch dir) that: sends the `initialize`
handshake, sends one `user` stream-json turn, tees claude's raw stdout to the fixture file
untouched, and — only for permission/plan/edit turns — answers any `can_use_tool` /
`elicitation` control_request it observes on stdout with an `allow` (echoing the original tool
input back, exactly as `marsad-agent`'s `handle_answer_gate` does) so the run can finish. The
control_response we send back is written to claude's **stdin**, never to the fixture file — every
byte in each `fixtures/streams/*.ndjson` file is claude's own real stdout.

Concretely: `permission.ndjson` was captured by pre-creating a local git repo + bare "remote",
amending a pushed commit, and asking claude (default permission mode) to `git push --force` it —
a real `soft_deny`-classified action that reliably requires approval (see `claude auto-mode
defaults`), unlike e.g. a bare `echo` which this CLI's auto-mode classifier now auto-approves even
under `--permission-mode default`. `plan.ndjson` used `--permission-mode plan` and a one-line
planning prompt. `edit.ndjson` pre-created `NOTES.txt` and asked claude to edit it. `subagent.ndjson`
asked claude to use its subagent-spawning tool (`--permission-mode bypassPermissions`, to keep the
capture to one turn). `compaction.ndjson` sent four trivial Q&A turns followed by a literal
`/compact` user turn — the first attempt (one Q&A turn, then `/compact`) got a real but *unsuccessful*
compaction (`"compact_result":"failed","compact_error":"Not enough messages to compact."`); the
committed fixture is the second attempt, with enough turns for a real
`"compact_result":"success"` + `system/compact_boundary`. Hook payloads were captured by writing a
project-level `.claude/settings.json` with a `command` hook per event
(`[ -f OUT ] || cat > OUT`, so only the first real invocation of each event type is kept) and
driving one session that exercises Bash (Pre/PostToolUse), a subagent (SubagentStop), and
`/compact` (PreCompact), alongside the implicit SessionStart/SessionEnd/Stop of the session itself.

## Known gaps — real captures the current parser/reducer cannot handle

The five gate/subagent/compaction stream fixtures are genuine, unmodified real captures against
`claude` 2.1.198, but that CLI's real stream-json shape has drifted from what
`marsad_proto::parse_stream_line` (and, transitively, `marsad_events::reduce`) expects. This is
**not** a bug in the fixtures — `crates/marsad-proto/tests/fixtures.rs` and
`crates/marsad-events/tests/fixtures.rs` un-ignore these tests and assert the literal documented
expectations from the original `#[ignore]`d stubs, and several now fail (panic) for real, specific
reasons:

1. **No `ContentBlock::Thinking` variant.** This account has `alwaysThinkingEnabled: true`, so real
   assistant messages include a `{"type":"thinking","thinking":"...","signature":"..."}` content
   block before any text/tool_use block. `ContentBlock` (`crates/marsad-proto/src/lib.rs`) only
   defines `Text`/`ToolUse`/`ToolResult`, so any assistant message that reasoned first fails with
   `SchemaMismatch { field: "assistant", detail: "unknown variant \`thinking\`, ..." }`. Affects
   `permission.ndjson`, `plan.ndjson`, `edit.ndjson`, `subagent.ndjson`.

2. **`parse_stream_line` dispatches every `"type":"system"` line to `SystemInit`.** Real claude
   2.1.198 emits many other `system` subtypes on a live stream: `thinking_tokens` (running
   thinking-token estimate), `status` (e.g. `"status":"compacting"`, or a plan-mode
   `permissionMode` change), `task_started`/`task_progress`/`task_updated`/`task_notification`
   (subagent lifecycle), `compact_boundary`, `hook_started`/`hook_response`. None of these carry
   `SystemInit`'s required `model`/`cwd`/`tools`/`permissionMode` fields, so each fails with
   `SchemaMismatch { field: "system_init", detail: "missing field \`model\`" }`. `parse_stream_line`
   would need to branch on `subtype` for `"system"` messages the way it already does for
   `"control_request"`. Affects `permission.ndjson`, `plan.ndjson`, `subagent.ndjson`,
   `compaction.ndjson` (and would affect any fixture with tool use / a subagent / compaction).

3. **`UserMessage.content` assumes `Vec<ContentBlock>`, but real post-`/compact` user messages carry
   a bare string.** After a successful compaction, claude synthesizes a `user` message whose
   `content` is the plain-string compaction summary (and a second one,
   `"<local-command-stdout>Compacted </local-command-stdout>"`), not a content-block array. Fails
   with `SchemaMismatch { field: "user", detail: "invalid type: string ..., expected a sequence" }`.
   Affects `compaction.ndjson` only.

4. **The real subagent-spawning tool in this environment is named `"Agent"`, not `"Task"`,** and
   its input carries `subagent_type` (matching what `reduce_assistant` already looks for) but under
   a tool_use block whose `name` field is `"Agent"`. `marsad-events::reduce_assistant`
   (`crates/marsad-events/src/lib.rs`) only special-cases `name == "Task"` to start tracking a
   subagent. `subagent.ndjson`'s real capture never triggers that branch, so even once gap #1/#2
   above are fixed, `fixture_subagent_spawn_recorded_in_state` would still fail its
   `!state.subagents.is_empty()` assertion (a second, independent mismatch, only reachable once
   parsing itself succeeds). Note the `system/init` `tools` list *does* advertise `"Task"` — the
   discrepancy is between the advertised tool name and the tool_use block's actual `name`.

Per the capture task's contract: these are reported as genuine red tests caused by real captured
data outrunning the current parser/reducer, not weakened or hidden. Fixing `marsad-proto`/
`marsad-events` to handle shapes 1-4 is out of scope for this fixture-capture task.

`qa.ndjson` (captured earlier, presumably against an older claude build or a differently-configured
account) predates gaps 1-2 — it's a 3-line, no-tool-use, no-thinking session, so it never exercises
any of them, which is why it alone parses clean.
