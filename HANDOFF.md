# Handoff — integrate and harden Marsad (branch `claude/awesome-euler-r7x45r`)

## State

Everything below is committed and pushed in Marsad, Wasit and Wasl.
Marsad's `Cargo.lock` files pin the Wasit/Wasl branch revisions (git sources); local builds use the
untracked `.cargo/config.toml` path patches, which rewrite the locks to path sources — never commit
those. Re-pin after pushing Wasit with: move `.cargo/config.toml` aside, `cargo update -p <crate>`
for the wasit/wasl crates in `.` and `apps/desktop/src-tauri`, move it back. Release builds use
`--locked`.

CI (`ci.yml`, Linux/Windows/macOS + `linux-helper` + `windows-e2e` + the Windows translucency
screenshot step in `desktop (windows-latest)`): see the branch's latest run.

### Shared-home / environment / translucency round (owner's Windows 11 reports)
- **Two SSH hosts sharing a network home** (one failed with `bridge_unavailable … Connection
  refused … Attempt 11`). Cause confirmed: everything lived in `~/.marsad` — host A's daemon
  owned `marsad.sock` + its (NFS-wide) lock; host B's bridge saw a socket nobody on its kernel
  served, its daemon lost the lock and exited, the link retried forever. Fix (Wasit fa82296,
  1ee15aa; Marsad 54045cd, a854df7): `wasit_runtime::machine` (hashed machine key, owner-checked
  0700 runtime dir chosen from machine facts only), shim `--run` (lock + socket there),
  `RuntimeConfig::run_dir`, `marsad_proto::layout` (data in `~/.marsad/machines/<key>/`, sockets
  and locks in the runtime dir, helper cache `bin/<os>-<arch>/` installed via unique temp +
  rename), `marsad-agent/src/migrate.rs` (flat layout adopted), `Hello.machine` →
  `HostEntry.machine` (twin hosts warned in the Hosts screen, never restarted away from each
  other), `bridge_unavailable` ×3 → `Failed` with a plain message, `start_detached` reports a
  daemon that exits before ready. Layout: Wasit ARCHITECTURE §4.2. Overrides
  `MARSAD_MACHINE_KEY`, `MARSAD_RUNTIME_DIR`, `--run-dir`.
  Tests: Marsad `tests/shared_home.rs` (two sshd "machines", one HOME: concurrent provisioning,
  own daemons/sessions, restart re-adopts only its own, twin alias refused; flat layout with a
  live session adopted and still answering; foreign flat layout left alone), Wasit shim
  `runtime_dir_holds_lock_and_endpoint`, `machine` units (permission check incl. foreign owner),
  control `supervised_parks_failed_after_repeated_bridge_unavailable`.
  Not verified on a real NFS pair; remaining risk: with NFS `nolock` a foreign machine's live
  legacy shim looks dead — its directory is only claimed when untouched for 10 min.
- **WSL found Claude/Codex but not Kilo/OpenCode.** Cause: probes/sessions ran with the
  daemon's bare non-login env; tools on rc-added paths (nvm in `.bashrc`) were invisible.
  Fix (Wasit c56a09d, 19fde2f; Marsad 6100625): `wasit_runtime::environment::HostEnv` (login +
  interactive shell → login → own env; Windows registry), installed as
  `wasit_process::set_base_env` for every spawn, terminals too; `host.environment[.refresh]`;
  Hosts screen "Environment" section + "Refresh environment". Test sshds use a profile-free
  login shell (`bin/loginsh`) so `/etc/profile` does not put a real CLI first. windows-e2e
  installs a fake `kilo` under `/root/.nvm/...` added only by `.bashrc` and asserts discovery.
- **Translucency** (Marsad 414e87c): Acrylic before Mica; one tint layer (`.app`), chrome and
  panels light tints, dialogs/menus near-opaque + backdrop blur; `data-backdrop`;
  `MARSAD_SURFACE` test hook (`MARSAD_WINDOW_EFFECT` since removed); the shell reports a hint when Windows'
  Transparency effects are off (Acrylic is then drawn solid — the runner's default, and a
  likely cause of the owner's "varying shade"). CI desktop (windows) job screenshots over
  magenta (`e2e-real/translucency.ps1`, artifact `translucency-windows`). Observed on the
  hosted runner: Acrylic applied + window active + effects on → solid fallback grey
  (220,221,222 vs solid 246,247,250) — no GPU composition; once, without an effect, the
  content showed the magenta through the whole stack (250,139,252), top bar/rail 253,208,254.
  CI asserts the Acrylic answer and that content/top bar/rail/status bar differ from solid.
  Real blur over other windows is unverified on real Windows 11 hardware; macOS unverified.
- **WSL capture fix found by CI**: under `wsl.exe` the daemon has a controlling terminal, so
  `bash -l -i` in a background group stopped on SIGTTOU (6 s deadline, `.bashrc` never read);
  the capture shell now starts with `setsid` (`wasit_process::new_session`). windows-e2e then
  found the fake `kilo` under `/root/.nvm/...` (added only by `.bashrc`).

### Per-host environment round (setup commands, variables, agent commands)
- **Behaviour** (Wasit `environment.rs`, ARCHITECTURE §3.5; Marsad `hub/hostenv.rs`): per host
  only. Order: login capture → setup commands → variables → agent commands. Setup: Unix = the
  login shell in the capture's mode (`-l -i`, so `module` from rc files exists), lines passed in
  `__WASIT_SETUP` and run with `eval` inside `{ …; } </dev/null 2>&1` between nonce markers,
  deadline 30 s, process group killed; the environment it leaves is taken (capture's PATH
  entries kept after its own); fish runs them as fish, csh/tcsh/nu/elvish/xonsh/pwsh login
  shells via `/bin/sh`. **Native Windows: PowerShell `-NoProfile -NonInteractive` in the
  captured environment, lines in `& { }`; `$env:` changes taken; exit status = a thrown error
  (1) or `$LASTEXITCODE`; non-terminating cmdlet errors do not count as failure; a parse error
  means nothing runs ("the shell did not run the commands" + PowerShell's message).** Failure ⇒
  `SetupReport {ok:false, exit_code, error, output tail, applied}`: a failing last command keeps
  the environment it left (`applied: true`); a hang, `exit` or a dead shell keeps the login
  environment. Variables are literal. Agent commands (`AgentCommands`, shared by discovery and
  launches, read at use time, win over `--driver-bin`) stand in for the CLI with the same
  argv; the driver's version probe must answer like that CLI (parsers now reject output that
  is not a version; codex needs `codex-cli <v>`), else `DriverInventory.warning`.
  `DriverInventory.command` names the command in use. Discovery: a refresh asked for while one
  runs runs again (`again` set), env changes refresh every cached inventory, probes get the
  base environment (was the daemon's own for `HOME`/`CODEX_HOME`).
- **Hub**: `hosts.environment.{get,set,test}`. Store: `host-environments.json` (settings, secret
  values absent) + `secrets.json` (values only); every store file is now created 0600. Secrets:
  OS keychain NOT used (0600 file chosen: no native deps, headless Linux CI); Windows relies on
  the profile directory ACL. A secret is sent to its host only inside
  `host.environment.configure/test` (kept in memory by that daemon), never returned to the UI
  (`get` → `value: null`; `set` with `null` keeps it), scrubbed from setup output/notes, hidden
  by `EnvVar`/`HostVar`/`SecretValue` `Debug`, and request decoding errors do not echo params.
  Applied (`configure` with the stored random revision; no-op when the host reports it): on
  save (awaited, reply carries the report or `not_applied`), whenever a link becomes
  Connected (a restarted daemon has none), at hub start for the local host, and before every
  launch (`launch` → `envs.ensure`). Terminals and git use the base environment, so they get
  it from the next process on; a continued session keeps the command stored at its creation.
- **UI**: Hosts → Environment: setup textarea, variables (name / value / Secret / remove; a saved
  secret shows as `••••••••` with Replace), agent command per driver (placeholder = default),
  Save, Test (saved settings only; per agent command → resolved path, version or error,
  warning), setup failure as a red gate with exit status and output; Agent CLIs list shows
  `command →` and warnings. New session dialog / Config tab do NOT show the command yet.
- **Tests**: Wasit `tests/host_setup.rs` (bash login shell + rc-defined `module`; kilo/genie/
  lister; order; secrets scrubbed and absent from session.json/journal; failing/exit/malformed/
  hanging setups), `tests/host_setup_windows.rs` (PowerShell, Windows CI), version parser units.
  Marsad `tests/host_env.rs` (real sshd: module system, genie session, terminals, local host,
  secret only in `secrets.json` 0600 and in no log/journal/reply, failing setup, daemon restart
  ⇒ re-applied), windows-e2e WSL (setup puts a genie wrapper on PATH; Test finds it), vitest
  (editing, write-only secret, failure, warning), real-app scenario `host-environment`.
- **Also this round**: `MARSAD_WINDOW_EFFECT` removed (shell + CI script). Flaky Playwright
  "html preview": cause found (Chromium kept the first `srcdoc` document when `srcdoc` changed
  while loading); the preview frame is keyed per document. Wasit `SessionManager::delete` now
  waits for the aborted actor, loader and `session.json` writer before removing the directory
  (macOS CI "Directory not empty").
- **Open CI failure (stopped after 4 pushes)**: Wasit `test (macos-latest)`,
  `host_setup.rs` malformed-setup case. macOS `/bin/bash` 3.2 in `-l -i` mode prints the syntax
  error of `eval "if then fi ("` and returns status 0 (bash 5 returns 2; a sentinel for an
  abandoned group did not catch it), so such a setup is reported `ok` with the error in its
  output. Fix candidates: check the lines first with the shell's `-n` (POSIX shells), or run
  them non-interactively with the interactive capture's functions exported. The other three
  macOS findings were fixed (eval from a variable, delete racing the session.json writer,
  status taken inside the group).
- **Not done / unverified**: OS keychain; fish setup (no fish here); csh family; native
  Windows setup over SSH end to end (PowerShell runner covered only by the Wasit unit-level
  Windows test); setup output encoding of non-ASCII on Windows; showing the agent command in
  the New session dialog / Config tab; a terminal opened while a save is still applying gets
  the previous environment.

### Media round: images and files (Wasit b58753e+, Wasl fb52161, Marsad this branch)
- **Verified with the real CLIs** (fixtures in `wasit-claude/tests/fixtures/claude-{image-prompt,read-image}`,
  `wasit-opencode/tests/fixtures/{opencode-image-prompt,opencode-read-image,kilo-image-prompt}`):
  claude 2.1.327 — `image` blocks in a stream-json user message are seen (Haiku said "Red"); a `Read`
  of a PNG returns a `tool_result` image block (+ base64 in `tool_use_result`). opencode 1.19.6 /
  kilo 7.8.27 — `initialize` reports `promptCapabilities {image, embeddedContext}`; ACP `image`
  and embedded `resource` work; the read tool returns `image` content (and a data URL in
  `rawOutput.attachments`). Model matters: opencode `deepseek-v4.1-flash-free` and
  `nemotron-3.5-super-vl-free`, kilo `nvidia/nemotron-3.5-super-vl:free` see images;
  `opencode/big-pickle` and `kilo/kilo-auto/free` answer that they cannot. codex 0.169 — from
  the app-server schema only (no login here): `localImage {path}` input; `imageView` /
  `imageGeneration` items.
- **Model** (`wasit-core::content`, ARCHITECTURE §3.4): `ContentPart`, `MediaSource`,
  `Attachment` on `Command::Prompt`, `PromptInput` on the descriptor (claude: images + files by
  path; ACP: images + text files embedded / others as `resource_link`; codex: `localImage` + path).
  The pure core moves inline media into `<session>/blobs/<sha256>` (dims sniffed), scrubs base64
  from raw JSON; a missing blob is rebuilt by replaying the journal.
- **Methods** (wasit-runtime, routed by host over the existing link): `session.attachment.put`
  (chunks ≤ 1 MiB, files ≤ 50 MiB, into `<session>/attachments/<upload>/`), `session.file.read`
  (blob | path, ranges ≤ 4 MiB, host-sniffed mime, canonical paths inside cwd/attachments/blobs
  unless `outside`), `session.files.stat`. Prompt attachments must be uploads of that session.
- **UI** (`features/media/`): inline thumbnails + lightbox (fit / actual size), file chips,
  `FileRefs` (tool write targets, resource links, prose paths that exist), `PreviewHost`
  (image, SVG via `<img>`, PDF via pdf.js chunk, text, markdown without HTML/remote images, JSON,
  CSV, HTML in a sandboxed frame with a no-network CSP and a scripts toggle, audio/video,
  metadata + open/reveal (local) or download through the link (remote)). Composer: paste, drop,
  attach (shell `desktop.pick_files`/`read_picked`; file input in a browser), chips with limits
  from `prompt_input`. Shell `desktop.download_begin/chunk` (Downloads, created files only).
- **Tests**: Wasit replay fixtures above, unit tests (ACP fallbacks, codex input), runtime
  `tests/media.rs` (blobs across restart, uploads + limits, traversal/symlink/ranges); Marsad
  `tests/media.rs` over a real ssh link; vitest `features/media.test.tsx`; Playwright
  `media.spec.ts` (`?media=1` fixture); real app `media` scenario (fake CLI `image` /
  `write_file` steps) and `--real` `live-claude-media` (Haiku described the attached chart and
  wrote `live_chart.svg`, opened in the preview). Shots: `media-*.png`.
- **Not done**: codex image input untested live (the earlier gaps — first-prompt media, relative
  HTML resources, highlighting — are closed in the polish round below).

### Polish round: WSL files, HTML resources, first-prompt attachments, highlighting
- **WSL = local files (Windows GUI).** `marsad-proto::paths`: the ONE pure translation
  (`wsl_to_windows` → `\\wsl.localhost\<distro>\…`, `/mnt/<d>/…` → `D:\…`, Windows paths pass
  through, non-absolute / backslash / bad distro rejected; `windows_to_wsl` inverse; `resolve_wsl`
  adds the "distro not running" vs "not readable" messages). `HostEntry.local_files` (hub-derived:
  local host, and WSL when the hub runs on Windows; UI reads `HostView.localFiles`, no host-kind
  string checks). Shell `desktop.open/reveal/pick_folder` take `distro`; ProjectField offers a
  "Windows picker…" rooted at the distro (answers a Linux path; `/mnt/c/...` shown as is in the
  browser). windows-e2e asserts a file created inside the distro is readable from Windows at the
  translated path, `resolve_wsl` (what open/reveal use) accepts it and refuses missing/relative
  paths, and the WSL host reports `local_files`. No GUI app is launched in CI.
- **HTML previews** (`media/htmlResources.ts`): relative images/`srcset`/media/CSS (`url()`,
  `@import`, fonts)/scripts (only with "Run scripts") are read via `session.file.read` (host roots,
  4 workers, 200 files / 24 MB budget) and inlined as `data:` URLs; remote refs are removed and
  counted ("Remote content blocked: N"); links, iframes, `<meta http-equiv>`, `<base>` stripped.
- **First prompt attachments** (`newsession/launch.ts`): create (no first prompt) → upload each
  file → send the prompt with them, with progress in the footer; limits from `host.info`'s driver
  `prompt_input`, re-validated on driver/host change (`revalidate`, dropped files explained). A
  failure after creation opens the session with text + files in its composer (`session/draft.ts`).
  Workstream "Add agent" (design mode) does not launch from the dialog, so no attachments there.
- **Highlighting**: highlight.js core + 29 languages in ONE lazy chunk (`features/highlight/`,
  149 kB / 46 kB gz, off the main bundle); output parsed into tokens (never `innerHTML`); colours
  from design tokens (`highlight.css`); per-block cache; skipped > 256 KB / 6000 lines; auto-detect
  only without tag/extension and ≤ 16 KB. Used by file previews (line numbers, wrap, `path:line`
  focus, Rendered/Source for Markdown, pretty JSON), the shared `Markdown` (fenced blocks, tables,
  nested lists, task lists), tool bodies (edit diffs coloured per language, written files by
  extension, shell commands, JSON, unified diffs). Changes-tab diffs are NOT syntax-coloured.
- **Path chips** (`media/pathRefs.ts`): relative / absolute / `path:line[:col]` / Windows forms,
  existence via debounced, batched (50), cached `session.files.stat`. Reads stay confined to the
  session roots (an earlier grant design was removed at the owner's request): a path outside the
  folder is plain text and the host refuses to read it.
- **Tests**: Rust paths/hosts units, windows.rs; vitest `polish.test.tsx` (languages, limits,
  token sanitising, markdown, html resources incl. traversal/remote/budget, chip forms, batching,
  launch flow success/failure, dialog flow, WSL open buttons, outside paths never chips);
  Playwright `media.spec.ts` additions; real app scenario `polish`. Shots `polish-*.png`.

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
Default (fake CLIs): 35 scenarios (incl. `spawn`, `translucent-window`, `media`, `polish`, `host-environment`), last full run 33/33 (an earlier round; `host-environment` was run on its own this round) — incl. notifications per prefs (Settings
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
