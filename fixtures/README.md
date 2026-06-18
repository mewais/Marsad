# Fixtures — recorded-real Claude Code captures

These checked-in captures make the "Recorded-real" tests in `docs/specs/TESTPLAN.md` deterministic
and runnable anywhere (no `claude` binary needed at test time).

## `streams/` — stream-json NDJSON

Each file is the raw `--output-format stream-json --verbose` output of one real `claude` run.

| File | Scenario | Status |
|---|---|---|
| `qa.ndjson` | plain Q&A (init → assistant → result/success, real usage+cost) | **captured** |
| `permission.ndjson` | a permission gate (`can_use_tool`) | TODO(fixture) — capture on a toolchain machine |
| `plan.ndjson` | a plan-mode gate | TODO(fixture) |
| `edit.ndjson` | an edit-approval gate | TODO(fixture) |
| `subagent.ndjson` | a subagent spawn | TODO(fixture) |
| `compaction.ndjson` | a compaction event | TODO(fixture) |

Regenerate / extend with `scripts/capture_fixtures.sh [/path/to/claude]`.

## `hooks/` — hook payload samples

JSON captured from a temporary Claude Code hook (one file per event: Notification, Stop,
SubagentStop, PreToolUse, PostToolUse, SessionStart, SessionEnd, PreCompact). See `hooks/README.md`
(created by the capture script) and code.claude.com/docs/en/hooks.

## Note

The gate/subagent/compaction captures require driving interactive control-protocol flows; capture
them alongside the first run of the `i02`/`i03` live integration tests on a toolchain machine, then
commit. `qa.ndjson` is sufficient to bring up B01/B02 recorded-real tests for init/assistant/result,
usage, cost, and context%.
