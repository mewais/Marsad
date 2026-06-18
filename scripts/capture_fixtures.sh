#!/usr/bin/env bash
# capture_fixtures.sh — record one real `claude` stream-json session per scenario into
# fixtures/streams/*.ndjson, plus hook payloads into fixtures/hooks/*.json. These checked-in
# captures feed every "Recorded-real" test (B01/B02/B06/i01/i05) deterministically.
#
# Run on any machine that has the `claude` CLI. It does NOT need tmux/SSH (those are exercised
# live by the integration tests, not captured here).
#
# Usage:  scripts/capture_fixtures.sh [/path/to/claude]
set -euo pipefail

CLAUDE="${1:-claude}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
STREAMS="$ROOT/fixtures/streams"
HOOKS="$ROOT/fixtures/hooks"
mkdir -p "$STREAMS" "$HOOKS"

if ! command -v "$CLAUDE" >/dev/null 2>&1; then
  echo "TODO(fixture): claude CLI not found ('$CLAUDE'). Run this on a machine with claude installed." >&2
  exit 2
fi

# The flags Marsad uses in production (see crates/marsad-agent: claude_argv).
COMMON=(--print --input-format stream-json --output-format stream-json --verbose --dangerously-skip-permissions=false)

# Drive a single prompt and capture the raw NDJSON stream to a fixture file.
capture () {
  local name="$1"; shift
  local prompt="$1"; shift
  echo ">> capturing $name"
  # stream-json input is one JSON user turn per line on stdin.
  printf '%s\n' "$(jq -cn --arg t "$prompt" '{type:"user",message:{role:"user",content:[{type:"text",text:$t}]}}')" \
    | "$CLAUDE" "${COMMON[@]}" > "$STREAMS/$name.ndjson" 2>/dev/null || true
  echo "   -> $STREAMS/$name.ndjson ($(wc -l < "$STREAMS/$name.ndjson") lines)"
}

# Scenario set (one of each behavior the recorded tests assert against):
capture qa            "In one short sentence, what is 2+2? Do not use any tools."
capture permission    "Run the shell command 'echo hello' to demonstrate a permission gate."
capture plan          "Make a short plan (do not execute) to add a README to this directory, then stop."
capture edit          "Propose an edit to a file named NOTES.txt adding one line; show the change."
capture subagent      "Spawn a subagent to list files, then summarize what it found."
capture compaction    "Briefly summarize our conversation so far to demonstrate context handling."

cat > "$HOOKS/README.md" <<'EOF'
Hook payload samples used by B02/B10 recorded-real tests. Capture these by configuring a temporary
Claude Code hook that appends its JSON stdin to a file, for each event type:
Notification, Stop, SubagentStop, PreToolUse, PostToolUse, SessionStart, SessionEnd, PreCompact.
See code.claude.com/docs/en/hooks. Save each as fixtures/hooks/<event>.json.
EOF

echo "Done. Review fixtures/streams/*.ndjson and capture fixtures/hooks/*.json per the README."
