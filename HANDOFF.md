# Handoff — integrate and harden Marsad (branch `claude/awesome-euler-r7x45r`)

## Done and verified

**Wasit** (pushed): gates carry the agent's own choices — `GateChoice {id, label, kind,
neutral_label, description, text}` on permission / plan / handoff gates, answered by
`GateDecision {choice, text}`:
- claude `can_use_tool`: allow / allow and remember (echoes every `permission_suggestions`
  entry as `updatedPermissions`, described from the rules) / deny (+message) / deny and stop
  the turn (`interrupt: true`); plans the same with plan wording; MCP elicitations from the
  schema. Labels neutral (protocol has none).
- ACP (OpenCode/Kilo): the request's `options[]` verbatim (id, name, kind).
- codex: the request's `availableDecisions` (accept / acceptForSession / execpolicy and network
  amendments / decline / cancel), else the method's decision set per the 0.169.1 schema.
- Question prompts: `id`, `allow_other`, `secret`; answers are lists per question id.
- `ResumeRequest::{New, Latest, Resume{id}}`; resumable sessions report `message_count`
  (claude, codex); `sessions.rename` → `display_name` in `session.json`; snapshots carry
  `usage_by_model` and the turn's `current_tool`; runtime refuses external-gate answers that
  pick no offered choice; discovery runs at most 4 probes at once.
- OpenCode/Kilo question tool: verified with a raw ACP client that kilo 7.8.27 offers it under
  ACP and the turn hangs (no request on the wire); opencode 1.19.6 hides it under ACP unless
  `OPENCODE_ENABLE_QUESTION_TOOL` is set (then hangs too). The renderer now always injects
  `tools.question=false`; the model then asks in plain text (verified on free models).

**Wasl** (pushed): handoff gates offer `handoff_choices()` (allow_once / always_allow / deny+reason).

**Marsad** (pushed): NodeLaunch resume; hub `gitwatch` (coalesced, rate-limited background
git summaries in the fleet topic, `fleet.git_refresh`); TS export of every wire type plus a
generated `names.ts` (method/topic names); CI checks generated TS and runs vitest. UI: agent's
own choices (no "Marsad standard choices"), continue-latest, message counts, rename, delete,
current tool, per-model usage, fleet git summaries, shell `navigate` event; fixed terminal
(base64 output written raw; keystrokes reordered), "checking…" agents while discovery runs,
inventory re-fetch while a project's probes run.

**Real-app e2e suite** `apps/desktop/e2e-real/` (WebDriver on the real Tauri app). Last full
default run: **26 passed, 0 failed** (launch ×4 personas, gates ×4, plan, interrupt,
model/mode/slash, rename, changes, terminal, resume + continue-latest, stop/delete, agent config,
workstream handoffs with allow once / always allow, daemon kill -9, ssh key host with provisioning,
remote session and link kill/reconnect, password host via the sign-in dialog, auth_needed after a
drop and Connect again).

Local checks green at last commit: Wasit/Wasl/Marsad `cargo test`, clippy `-D warnings`, fmt;
desktop tsc, vitest (75), Playwright (11), src-tauri clippy/test.

## Remaining (untouched goals)
- Goal 5: screenshots of the real app (`--shots <dir>` exists; set dark theme first via
  `store.settings.set {key:"theme",value:"dark"}` and capture every screen + light Inbox/Graph).
- Goal 6: CI iterations after the latest pushes (previous run: only Windows test build failed,
  fixed in 7ba4d82; not re-checked since).
- Not covered by the suite yet: kill -9 of the app mid-turn, notifications per prefs, agent-config
  override taking effect in a launched session, a live delegation through the MCP tool.
- Optional better fix for the question tool: an "ask the user" MCP tool (automatic item) that
  raises a real question gate.

## Build / run
```sh
cargo build -p marsad-agent -p marsad-fake-cli            # in Marsad (sibling patches in .cargo/config.toml)
pnpm --dir apps/desktop install && pnpm --dir apps/desktop build
(cd apps/desktop/src-tauri && cargo build --features custom-protocol)
MARSAD_AGENT_BIN=$PWD/target/debug/marsad-agent apps/desktop/src-tauri/target/debug/marsad-desktop
pnpm --dir apps/desktop test:real [-- --build] [--real] [--shots DIR] [--only a,b] [--keep]
```
Needs `tauri-driver` (cargo install), `webkit2gtk-driver`, xvfb, dbus; ssh scenarios need root +
openssh-server. Rebuild the app after `pnpm build` (the UI is embedded).

## Known failing / unverified
- The `--real` live scenarios (`live-*`: claude Haiku, opencode deepseek-flash-free, kilo
  auto-free, claude gate, resume of a terminal-started claude session) have not been seen passing:
  the first live run hit a stale-element bug in the harness and a discovery process storm (~100
  kilo processes; both fixed since). Rerun `pnpm --dir apps/desktop test:real -- --real` and fix
  what it shows.
- CI after the last pushes not re-checked (goal 6).
