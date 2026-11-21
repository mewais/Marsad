# Handoff — integrate and harden Marsad (branch `claude/awesome-euler-r7x45r`)

## State

Everything below is committed and pushed in Marsad, Wasit and Wasl.
Marsad's `Cargo.lock` files pin the Wasit/Wasl branch revisions (git sources); local builds use the
untracked `.cargo/config.toml` path patches, which rewrite the locks to path sources — never commit
those. Re-pin after pushing Wasit with: move `.cargo/config.toml` aside, `cargo update -p <crate>`
for the wasit/wasl crates in `.` and `apps/desktop/src-tauri`, move it back. Release builds use
`--locked`.

CI (`ci.yml`, Linux/Windows/macOS + `linux-helper` + `windows-e2e`): green at 43e90ba —
https://github.com/mewais/Marsad/actions/runs/44859855066 (later commits: see the branch).

### Windows first-run round (verified on a real Windows runner) + UI requests
- **Windows fixes** (Marsad 9500fdc..4ba256d, Wasit daba94c/6fd7b4d, Wasl bf7c305): every process
  via `wasit-process` (CREATE_NO_WINDOW; daemon detached with an invisible console; source scan
  forbids raw `Command::new`), helper per (os, arch) with any Linux libc → bundled musl build,
  WSL distro listing/picker (UTF-16LE decoded), configuration failures park a link as `Failed`.
- **Verification fixed**: the no-console tests' control (`cmd /C "<quoted path>"`) never ran the
  fake (std quoting), so "no visible window" proved nothing. Control is now the fake with
  CREATE_NEW_CONSOLE and must be reported `visible` (Wasit 4e53f0d, Marsad 72d8825).
- **CI `windows-e2e`** (windows-latest + Ubuntu-24.04 WSL distro): (a) console-less hub → probes,
  shim session, `git`, ssh transport, ConPTY all report no console window, control visible;
  (b) hub lists distros, connects the WSL host (musl helper provisioned), fake-CLI gate
  round-trip, link survives `wsl --terminate`; (c) **ssh from Windows** (Windows `ssh.exe`) to
  sshd inside the distro (key auth, `SetEnv MARSAD_BUILD_ID`): musl helper uploaded, gate
  round-trip. Green: https://github.com/mewais/Marsad/actions/runs/44859855066
- **Translucent window** (Settings → General → Window background + strength): tokens
  `[data-surface="translucent"]` in `theme/tokens.css` (alpha via `color-mix`, `--glass`),
  `lib/surface.ts` (hub setting `surface`/`surface.strength`, localStorage cache applied in
  `main.tsx` before render), shell `desktop.window_surface` (`src-tauri/src/surface.rs`: Windows
  Mica→Acrylic→Blur, macOS vibrancy, Linux alpha only if composited — else stays solid with a
  hint). `tauri.conf.json` no longer applies static effects; `macOSPrivateApi` on.
  Real-app scenario `translucent-window` (no compositor → solid; xcompmgr → see-through,
  `import` screenshots over a checkerboard window). **Native Windows/macOS effects untested on
  real machines.**
- **Overflow**: fixture `?long=1` (`bridge/fixture/long.ts`), Playwright `tests/e2e/overflow.spec.ts`
  (no element wider than its box / sticking out, 1024 + 1440) over agent config panel + table,
  session rail + Config tab, host details, workstream inspector, New session dialog. Panels
  use `overflow-wrap:anywhere`; `ui/Wrap` breaks paths after separators; HostChip ellipsizes.

### Hardening round (flaky tests, stale gates, spawn deadlines)
- **Flaky tests, root causes**: Wasit shim `echo_roundtrip_and_clean_exit` (Windows CI) — the endpoint now closes before the connections end; fake-cli `stderr_step` — read stderr to its end. Wasit `create_id` (Windows CI) — `rename` answered before
  `session.json` was written (coalescing writer); it now answers once the file holds the name.
  Wasl `ask_edge_gate_allow_once_then_result_returns` — `AuditLog::record` resolved before tokio's
  `File` reached the OS; entries are now flushed+synced before `record` returns and the node records
  every entry *before* the state change it describes (durability first; a gate decision is refused
  if the trail cannot be written). Found by stress runs: the runtime offered a record's events
  before publishing its snapshot (clients reading the snapshot on an event saw the old state) —
  snapshot first now; Wasl withdrawals run to the end even when the RPC caller disconnects; a
  spawn/handoff gate raised just before a daemon kill is adopted after restart.
- **Gate lifecycle** (Wasit runtime + Wasl node): a gate lives as long as the request that raised
  it. Withdrawal = `gate_resolved { reason: "withdrawn" }`, gone from the snapshot, late answer →
  `CommandError::GateWithdrawn` (`gate_withdrawn`), id never re-raisable; process exit withdraws
  external gates. Handoff: task cancelled/expired, sending process ended, delegating turn
  interrupted (a normal turn end keeps it). Spawn: asking turn ended or interrupted, MCP call
  cancelled (`wasl.spawn.cancel`), client disconnected (no `wasl.spawn` caller for
  `spawn_reattach_grace`, 30 s), process ended, deadline passed → `spawn_withdrawn` / `spawn_expired`.
- **Spawn deadlines**: `SpawnSpec.deadline_secs` (MCP `spawn_agent.deadline_secs`); refused at once
  if the first task's or the caller's own task's deadline passed; refused at approval if it passed
  or the caller's task ended; the first task gets the remainder.
- **UI**: an open gate (Inbox row, session/inspector card) that disappears without our answer shows
  a "No longer needed" toast; a late answer's `gate_withdrawn` is that toast, not an error.
- Tests: Wasit `external_gate` (withdraw + exit, typed late answer), Wasl `handoff`
  (interrupt/cancel/expiry/exit) and `spawn` (interrupt, MCP cancel + disconnect, deadlines), Marsad
  `spawn_gate_is_withdrawn_when_the_asking_turn_is_interrupted`, vitest "a gate that goes away".

### Previous round (spawning workers + UI defects)
- **UI defects**: edge labels are placed on a free stretch of their own edge, never on a node
  or another label (beside the edge only when it has no room; `totalLabelOverlaps` tested);
  agent config: an item added by the selected layer offers "remove", "reset" only for inherited
  items it changed / turned off (no more "reset at Global" on added items); ACP
  permission-with-options that carry `questions` read as questions (`lib/gatewords.ts`: only
  command executions say "Run"); no raw ids: nameless sessions fall back to prompt / folder
  (UI + notifications), gate rows show host names, handoff payload line drops the task id.
- **Spawning (ARCHITECTURE §5 `spawn_agent`)**:
  - Wasit: `GateRequest::Spawn { request_id, spawn: Box<SpawnGate>, choices }` (flattened on the
    wire); fake CLI `mcp_call` step (claude persona calls tools of its `--mcp-config` servers,
    `save_as` + `{{name.path}}` placeholders).
  - Wasl: MCP `spawn_agent` / `dismiss_agent`, `list_peers.spawnable`; node `wasl.spawn` asks
    its `SpawnAuthority` (`wasl.supervisor.spawn_plan` → gate on the calling session unless
    allowed → `wasl.supervisor.spawn_launch`, idempotent by request id) and delegates the first
    task; journaled per request id; non-hub nodes forward to the hub (`UplinkSpawnAuthority`);
    standalone ⇒ `spawn_unavailable`. Choices: Allow once / Always allow spawning in this
    workstream / Always allow spawning and handoffs to spawned agents / Deny (+reason).
  - Marsad hub (`hub/spawn.rs`, methods `workstreams.spawn.*`): `Workstream.spawn` settings
    (policy ask/always_allow/never, `handoffs_always_allow`, max_agents 8, max_depth 2,
    allowed_hosts besides the asker's, allowed_drivers), budget; nodes carry `spawned_by` and
    `spawn_request`; `NodeLaunch.session_id` / `LaunchRequest.id` pre-chosen so a repeated
    launch finds the session; standalone asker ⇒ "<name> + workers" on approval; dismiss only
    by the spawner; archive stops spawned sessions; view lists `pending_spawns`.
  - UI: `SpawnGate` (Inbox/session/inspector), banner, "by <spawner>" marker, inspector
    "Spawning" section, spawned count in the list; fixture `startSpawnScenario()`.
  - Tests: Wasl `tests/spawn.rs` (test supervisor), Marsad `tests/spawn.rs` (local + ssh, hub
    restart while asking, standalone, fake agent through its MCP tools), vitest, e2e `spawn`.
  - Live (`MARSAD_LIVE=1 … --test live_workstream live_claude_spawns -- --ignored`): claude haiku
    spawned an opencode worker (deepseek-v4.1-flash-free) and replied "42". First attempt timed
    out: the harness did not answer the worker's own gates; second attempt passed — the free
    opencode worker itself spawned a sub-worker (depth 2) which answered.
- **CI** (Windows): `git` paths `/x` and `C:x` were accepted as repository-relative (fixed);
  daemon / git+pty / local workstream tests now run on Windows and macOS too (portable
  `kill_pid` / `stop_pid` / `pkill` helpers; only the ssh scenarios stay Linux-only). Product bug:
  a terminal never reported its exit on Windows (ConPTY keeps the pipe open) — a waiter thread now
  closes the master after the shell exits.
- **Discovery storm** (Wasit + UI): the New session dialog asked for the effective config on every
  keystroke of the folder field and every prefix was probed with the real CLIs; launches waited
  minutes. Debounced; folders that do not exist are answered at once without probes.
- **OpenCode/Kilo** (Wasit): they read their project from `$PWD` — probes and sessions now get
  `PWD`; the sessions probe runs in the project; the ACP modes handshake no longer leaves an empty
  session in the project (it hijacked "continue latest"); tool updates carry late arguments
  (`ToolCallUpdated.input`).
- **claude modes** (Wasit): sessions offer the modes discovery found (`ModeHints`), since claude
  reports only the current mode.
- **`ask_user`** (Marsad): automatic MCP server `marsad` (`marsad-agent ask-mcp`) injected only for
  drivers whose descriptor says `questions: false`; tool `ask_user` (single / multiple / text /
  yes-no) → host method `ask.user` → runtime external question gate on the calling session →
  answer returned. Verified live with opencode and kilo.
- **Desktop shell**: notifications now honor "Do not disturb" and "Only when not focused";
  `$MARSAD_NOTIFY_LOG` test hook; `MARSAD_AGENT_BIN_DIR` defaults to `<resources>/agents`
  (`ClientConfig::daemon_env`); `@tauri-apps/*` 2.12 to match the crate.
- **New session dialog**: no longer replaces the user's agent pick while a folder's inventory
  resolves, and uses the host-wide entry meanwhile (Start was disabled).

- **Fixes from the screenshot pass**: node sessions named after their node; handoff gates name
  agents (Wasl) and show hop/deadline instead of ids/epochs; resume list without native ids;
  model picker selects the alias a CLI resolved; claude's default mode leads and is offered as
  `default` (Wasit); local host platform known; second sign-in factor not shown as a retry;
  this machine first in host lists and as the dialog default; graph lanes with agents first.

### Real-app suite `apps/desktop/e2e-real/` (`pnpm --dir apps/desktop test:real [-- --real] [--shots DIR]`)
Default (fake CLIs): 32 scenarios (incl. `spawn`, `translucent-window`), last run 32/32 — incl. notifications per prefs (Settings
page → shell log), kill -9 of the app mid-turn + relaunch, daemon kill -9, ssh key / password
hosts, and a **real second factor**: sshd keyboard-interactive through PAM (pam_unix +
pam_oath TOTP; needs `libpam-oath`, the run swaps `/etc/pam.d/sshd` and restores it).
`--real` adds per CLI (claude Haiku, opencode deepseek-flash-free, kilo auto-free): launch, every
gate choice of the CLI's own (outcome checked on disk; no prompt after "always"), interrupt,
model+mode switch and back, slash command, continue-latest and resume-by-id of terminal-started
conversations, `ask_user` (opencode, kilo), a claude config override (inherited MCP server
disabled + one added, checked against claude's own init report), a live claude → opencode
delegation with the handoff gate answered in the UI. `--shots` adds the `showcase` scenario
(dark screens + light Inbox/Graph).

## Build / run
```sh
cargo build -p marsad-agent -p marsad-fake-cli            # in Marsad (sibling patches in .cargo/config.toml)
pnpm --dir apps/desktop install && pnpm --dir apps/desktop build
(cd apps/desktop/src-tauri && cargo build --features custom-protocol)
pnpm --dir apps/desktop test:real [-- --build] [--real] [--shots DIR] [--only a,b] [--keep]
```
Needs `tauri-driver`, `webkit2gtk-driver`, xvfb, dbus; ssh scenarios need root + openssh-server;
`translucent-window`'s composited half needs `xcompmgr` + imagemagick (skipped otherwise).
Rebuild the app after `pnpm build` (the UI is embedded).

Screenshots: `--real --shots DIR` (dark set via the UI's localStorage preference).

## Not done / known
- Free models are slow/flaky: `opencode run` occasionally stalls (retried); a /review on the
  free model can run for minutes (the slash scenario interrupts after a minute). A failing live
  scenario can cascade into the next ones on the same session — rerun with `--only`.
- Spawning: templates saved from a workstream keep spawned nodes as ordinary nodes; `--real` e2e
  has no live spawn scenario (Rust live test only); the hub view does not say *why* a spawn gate
  went away (the asking agent gets the reason). A delegated task does not inherit its parent
  task's deadline (only spawns cap the first task by it).
- `ask_user` (Marsad) treats any turn end after its cursor as the end of the question, including a
  turn that ended before the question was raised (same pattern Wasl now guards against).
- The fake CLI's `mcp_call` step is synchronous, so the interrupt-while-a-tool-blocks tests drive
  `wasl.spawn` over RPC during a `wait_for_interrupt` turn instead of through the fake agent.
