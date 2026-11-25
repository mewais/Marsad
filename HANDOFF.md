# Handoff — integrate and harden Marsad (branch `session-flows`)

## State

The current work is the branch `session-flows` in Marsad, Wasit and Wasl, each started from that
repo's `origin/main` (Marsad 090dee9, Wasit 6127d6e, Wasl 6ddfab9; `main` is the tip of the previous
branch `claude/awesome-euler-r7x45r`, whose rounds are described further down). Heads when this
round was written down: Wasit 7adfba4, Wasl fef0b92, Marsad d929132 plus this note. All three
branches are pushed; nothing of this round is merged to `main` (the owner has not asked for it), and
its commits are authored as `Claude <noreply@anthropic.com>` by the owner's instruction.
Marsad's `Cargo.toml` names `branch = "session-flows"` for every Wasit and Wasl git dependency
(Wasl's manifests do the same for Wasit). The two Marsad `Cargo.lock` files pin the revisions of
that branch (git sources), in the root lock and in the shell's `apps/desktop/src-tauri/Cargo.lock`
(which also has `wasl-proto`: re-pin both crates' families in both locks); they are pinned to
Wasit 3cdfd46 / Wasl a5fb553. Local builds use the untracked,
git-ignored `.cargo/config.toml` path patches (they must list every Wasit crate Marsad uses,
including the new `wasit-diag`), which rewrite the locks to path sources — never commit those.
Re-pin after pushing Wasit or Wasl with: move `.cargo/config.toml` aside, `cargo update -p <crate>`
for the wasit/wasl crates in `.` and `apps/desktop/src-tauri`, move it back. Release builds use
`--locked`.

CI (`ci.yml`, Linux/Windows/macOS + `linux-helper` + `windows-e2e` (WSL, ssh to WSL, native
Windows over ssh) + the Windows WebDriver translucency step in `desktop (windows-latest)` + a
Secret Service step on Linux) runs on every push; this round did not change `.github/`, and
whether it ran green on `session-flows` is not verified here. Pin helper: replace the Wasit/Wasl
hash in both Cargo.lock files (index only), or re-resolve with the patches moved aside.

### Session-flows round (diagnostics, session lifecycle, turns, hosts; owner's work order of 2026-11-22)
The owner's work order had three parts, in this order: (1) diagnostics, built first as the tool for
(2) nine bugs seen with Claude Code, mostly on a WSL host (B1–B9), and (3) agreed changes to session
control, the session view, hosts and workstreams. The owner tests with the real Claude CLI on
Windows + WSL + ssh hosts and sends a diagnostics export when something fails; the automated tests
use stand-in CLIs, so a green run is not proof that a flow works in the real app. The work order,
the manager's log, the root-cause investigations (written before the fixes: where they differ from
the code, the code is right) and the test audits are NOT in the repos, they are under
`/work/mewais/Marsad/.notes/` on the development box: `ASKS-session-flows.md`,
`PLAN-session-flows.md`, `investigation/{turns,lifecycle,hosts,ui}.md`, `design/diagnostics.md`,
`checkpoint2/tests-changed-{wasit,wasl,marsad}.md`. Status: diagnostics done; B1, B2, B3, B5, B6, B7
(cause a), B8, B9 fixed in code; B4 not established; section 3 done except reusing one
authenticated ssh connection. This round's code was not run on Windows or macOS (compile check only).

**Diagnostics** (Wasit `ARCHITECTURE.md` §9; Marsad `ARCHITECTURE.md` "Diagnostics", `docs/INSTALL.md`)
- *What is recorded:* typed records of starts and stops, every step of a host connect and why it
  failed, session state changes with their cause, the path of every prompt, failures with their
  codes, sizes and timings. *Never:* prompt or response text, thinking, tool input or output, file
  contents, attachment names, session or workstream names, secrets. Project folders appear hashed;
  an agent CLI's error text only as code, length and fingerprint; what ssh or a host printed on a
  failed connect is kept shortened with registered secrets and project paths masked. The owner
  accepted these defaults.
- *Where:* on every machine that runs a Marsad process, under `<root>/machines/<key>/` (root `~/.marsad`
  on Linux and macOS, `%LOCALAPPDATA%\Marsad` on Windows; a remote or WSL host has its own root and
  files): `diag/app.ndjson` (the desktop shell, 4 MiB, two older generations;
  `apps/desktop/src-tauri/src/diag.rs`), `diag/daemon.ndjson` (8 MiB, three older;
  `crates/marsad-agent/src/diag.rs`), `diag/aux.ndjson` (bridge, `stop`, askpass; the daemon rotates it
  over 1 MiB), `sessions/<id>/shim.log` (256 KiB, one older; Wasit `wasit-shim/src/diag.rs`),
  `diag/exports/`. `marsad-agent.log` is the daemon's raw stderr: never a record, never exported.
  Directories 0700, files 0600 on Unix.
- *Bounds:* one sink per process (`wasit-diag`): emitting is a budget check, serialisation, a ring
  push and a non-blocking queue push; a full queue drops and counts (`diag.dropped`); one writer thread
  rotates by size. Records are lifecycle-level (no per-delta, per-line logging); nothing awaits.
- *Trails:* the session trail (`wasit-runtime/src/trail.rs`, topic `session.trail:<id>`, method
  `session.trail.page`) is computed on the host from the journal and never written to a file. A prompt's
  path in it: `InputApplied` (status sent / queued / rejected / not_running …), `InputSent` (left the
  driver queue), `InputWritten` or `InputWriteFailed`, `InputAcknowledged` (the CLI said so), `InputDropped`
  with the reason; the first stage, "accepted by the app", is the app's `ui.prompt` record in
  `app.ndjson`; both carry the request's trace id (`u-` + 16 hex digits). Also: turn start/end with
  trigger, tools, tasks, gates, phase changes with cause, process spawn/exit, earlier-history state.
  The connection trail is the `boot.*`, `auth.*`, `hostenv.*` records (`marsad-proto/src/diag.rs`) and
  the link and transport records (`wasit-control/src/diag.rs`), each with host and attempt number.
- *Developer mode* (Settings → Diagnostics, hub setting `developer.mode`, off by default) adds a
  Diagnostics tab to each session (`features/diagnostics/SessionDiagnostics.tsx`) and a connection log
  to each host (`HostDiagnostics.tsx`), in time order. `MARSAD_LOG=<level>` separately prints tracing text.
- *Export* (Settings → Diagnostics, no Developer mode needed; CLI `marsad-agent diagnostics export`
  when the app will not start): the hub's `Exports` actor (`marsad-agent/src/hub/export/`) collects the
  app, the local daemon and every connected host into
  `diag/exports/marsad-diagnostics-<host>-<id>.ndjson.gz` and the UI saves it where the user chooses.
  Window 24 h (3 d, 7 d offered; CLI up to 31 d),
  20 MiB total, 16 MiB per host, 180 s per host, newest 3 exports kept; an unreachable host is listed
  as such. Session trails are included.
- *Leak tests* (the owner's marked prompt and secret): `crates/marsad-agent/tests/diagnostics.rs`
  `no_content_in_any_log_or_export` (Linux, a test sshd host: about twenty markers through every
  content channel, searched raw, JSON-escaped, base64 and UTF-16LE in both machines' `diag/`, shim logs,
  stderr logs and the export; the same test checks the markers ARE in the journal) and
  `no_session_content_reaches_any_diagnostic_file`; opt-in `live_diagnostics.rs` with the real Claude CLI
  (`MARSAD_LIVE=1 cargo test -p marsad-agent --test live_diagnostics -- --ignored`; it passed once mid-round
  with claude 2.1.293, not recorded as re-run on the final heads); Wasl `crates/wasl/tests/diag_content.rs`;
  Wasit `wasit-runtime/tests/{diag_daemon,diag_service,trail_wire}.rs`.
- *Adding an event:* declare it in the crate's `diag_events!` enum (`MarsadEvent`, `HubEvent`, `ClientEvent`,
  `StoreEvent`, Wasit's `ShimEvent`/runtime events, Wasl's `WaslEvent`); field types must be `wasit-diag`
  value types (`Ident`, `Code`, `Bytes`, `Ms`, `ErrorInfo`, …), never `String`; data types use
  `diag_data!`, enums `diag_enum!`. Free text exists only through `DiagText::new` and `ErrorInfo::ours`,
  and every call site is on an allow-list in a source-scan test that fails on a new one until it is
  listed with a reason: Wasit `crates/wasit-diag/tests/value_sites.rs`, Marsad
  `crates/marsad-agent/tests/diag_sites.rs`, Wasl `crates/wasl/tests/diag_sites.rs`. A new `AgentEvent`
  or `SessionEventKind` forces a decision in `trail.rs` (exhaustive match). The webview may only send `UiEvent` variants
  (`marsad-proto/src/diag.rs`); the shell refuses anything else.

**Bugs** (root cause → change → checks → what still needs the owner's real CLI)
- **B1 first prompt missing.** The dialog's first prompt went to the agent at start outside the input
  path, so no user message was recorded. `Core::on_start` (`wasit-runtime/src/core.rs`) now splits it off
  the spawn spec and applies it as an ordinary input in the same journal note as `start` (its id is that
  note's seq); drivers no longer read `first_prompt`; old journals replay it the same way. Checks: core
  `the_first_prompt_is_an_ordinary_input_applied_with_the_start`, `wasit-runtime/tests/drivers.rs`
  `*_first_prompt_is_an_input` (four drivers). Owner: the first prompt showing with the real Claude CLI.
- **B2 no earlier transcript on continue.** (a) The app's Continue launched a new Marsad session;
  (b) the engine never brought in the agent's own conversation. Now the app continues the existing
  session (`sessions.continue`: `ContinueButton` in `features/common/SessionBits.tsx`; a prompt typed
  into the dialog for an ended session continues it, `NewSessionDialog.tsx` `resume`). The engine
  imports the earlier conversation (Wasit §3.8): Claude through a parser its driver describes
  (`DriverDescriptor::history_storage`/`history_parser`, `wasit-claude/src/history.rs`), read-only from the
  agent's own storage by the runtime (`wasit-runtime/src/history.rs`); codex and ACP agents through what
  their protocols replay. History is a list kept apart from the live events (paged with
  `session.history` and `earlier: true`), shown above them (`features/session/EarlierPart.tsx`,
  `earlierWords.ts`) with loading, gaps (omitted older messages, cut items, unreadable lines, missing files) and an
  "unavailable" reason. "Continue most recent" is resolved to a real conversation id at create or
  refused with `nothing_to_continue` (`manager.rs`). Bounds (`wasit-core/src/history.rs`,
  `wasit-runtime/src/history.rs`): the most recent 64 MiB of the conversation's files, items cut at
  256 KiB, an event above 4 MiB replaced by a notice, a main file above 1 GiB not imported at all
  (`unavailable: too_large`); older messages are counted and not loadable (no load-older).
  Checks: `wasit-runtime/tests/earlier_history.rs`, `wasit-fake-cli/tests/history_wire.rs`, `wasit-claude/src/history_tests.rs` (sanitised fixtures with
  the structure of real claude 2.1.293 transcripts), opt-in `live_earlier.rs`; vitest `earlier.test.tsx`,
  `earlierTranscript.test.tsx`, `nothingToContinue.test.tsx`. Owner: continuing on a WSL host and an ssh
  host with real Claude and a conversation of real size; no test continued a session on a WSL host.
- **B3 Idle while working.** Turn boundaries were inferred from what we wrote, and background tasks and
  subagents were not tracked. Now turn start/end are the CLI's own (claude: `system/init` … `result`),
  background tasks are tracked (`wasit-core/src/tasks.rs` `apply`), `Working` means a turn is open or a
  task runs, subagent output arrives as `AgentEvent::Scoped` and never lands in the session's own
  transcript, `Interrupt` stops background work when no turn runs, `StopTask` stops one task
  (Wasit §3.6). Wasl follows a delivered task through its own turn and waits for background work
  (`287c8e6`). The app lists tasks and subagents in Activity and opens a subagent's transcript
  (`SessionWork.tsx`, `SubagentView.tsx`); the shell raises "finished" only after a completed,
  not interrupted turn with no background work (`src-tauri/src/notify.rs`). Checks: recordings of the
  real claude 2.1.293 (`wasit-claude/tests/fixtures/{background-*,agent-turn-*,lifecycle,stop-task}.ndjson`),
  `wasit-runtime/tests/background.rs`, `wasit-fake-cli/tests/{background,reference}.rs`; vitest
  `tasks.test.ts`, `subagents.test.tsx`, `transcriptScope.test.ts`. Owner: a long real run with subagents.
- **B4 prompt after a finished turn never arrived — NOT established.** In the owner's own journal the
  prompt was written to the agent; the CLI stayed silent and later exited. No Marsad delivery fault was
  found. The prompt trail (accepted, queued, written, acknowledged, dropped with reason) and the export
  exist to catch it next time; it needs the owner's export. The app shows a user message as sent,
  queued, or not sent with the reason (`features/session/transcript.ts`).
- **B5 queued prompts not cleared.** Prompts sent mid-turn are queued inside the agent CLI, which
  Marsad neither cancelled nor tracked. Now every input is tracked to acknowledgement; `Interrupt` and
  `End session` drop the pending ones and report each (`InputDropped`: interrupted, shutdown, unencodable,
  refused, process_ended); for Claude the messages in the CLI's own queue are cancelled
  (`cancel_async_message`). Checks: `drivers.rs` `*_queued_prompt_dropped_by_{interrupt,stop}` (claude,
  opencode, kilo), `*_prompt_is_acknowledged_by_the_cli`; vitest `prompts.test.tsx`, `lifecycle.test.tsx`.
  Owner: the cancel against the real CLI.
- **B6 no Continue after End.** Marsad did not reliably hold the agent's conversation id and the app had
  no continue path for an ended session. Now the id is chosen at start where the agent allows it
  (`DriverDescriptor::chosen_session_id`; claude `--session-id`), the snapshot says `continuable` or
  `not_continuable` with the reason, the app shows Continue, and a prompt or slash command to an ended
  session continues it and then delivers it as one operation (`wasit-runtime/src/continuation.rs`).
  Checks: `wasit-runtime/tests/lifecycle.rs` (four drivers: never prompted, after a turn, prompt to an
  ended session, two prompts during a continue, a process that dies at once), `drivers.rs`
  `*_crash_mid_turn`; vitest `lifecycle.test.tsx`. Owner: End then Continue with the real CLI.
- **B7 not findable from the user's own CLI.** (a) A working directory given as `~` was not normalised, so
  the conversation was filed under another project folder: `wasit-runtime/src/cwd.rs` now resolves it once
  at create (home, absolute, canonical; refused naming the path as given); test
  `lifecycle.rs::a_tilde_working_directory_is_the_home_and_is_used_everywhere`. (b) Claude's interactive
  picker does not list conversations started through its non-interactive entrypoint; changing that would
  mean pretending to be the interactive CLI, which was decided against. `claude --resume <id>` works: the
  snapshot carries `resume_command` (`DriverDescriptor::user_resume_argv`), the session facts show the
  agent's conversation id and a copyable command (`SessionFacts.tsx`), and the session's name is passed as
  the conversation title (`--name`, only for a conversation the session starts, never a resumed one).
  Owner: run that command in a terminal, while the session runs and after it ended.
- **B8 model menu wrapping, B9 Markdown in user prompts:** fixed in the app (`ui/Select.tsx`; user
  prompts render as Markdown with their line breaks, `features/session/Markdown.tsx`). Checks: vitest
  `chat.test.tsx`, `ui/primitives.test.tsx`; jsdom has no layout, so the menu's look needs the owner's eyes.

**Agreed changes** (all in the app unless noted)
- Interrupt cancels the turn and keeps the agent; Esc interrupts a running turn only, never background
  work (`SessionPage.tsx`, `escapeInterrupts`). "Stop" is "End session" (shuts the process down, keeps
  the session). Ended sessions show Continue and form their own group (`state/selectors.ts`). A prompt to
  an ended session continues it (B6). A session can be named in the new-session dialog (`LaunchRequest.name`).
- Usage tab removed; cost, cost per model and the context window are in the sidebar (`SessionFacts.tsx`);
  subagents in Activity; a run of consecutive tool calls is one expandable row (`toolRuns.ts`); copy
  buttons on prompts, responses and code (`ui/CopyButton.tsx`); Up/Down recall earlier prompts, also those
  of an earlier conversation (`usePromptRecall.ts`; a recalled one-line prompt can be walked from any caret
  position, otherwise only at the start/end of the text — a rule the owner accepted).
- Hosts: csh, tcsh and bsd-csh run setup commands in the user's own shell and syntax, from a private file
  (Wasit `wasit-runtime/src/environment.rs`); a csh user must write csh. ssh keep-alive was already there
  (`hub/transport.rs`: `ServerAliveInterval=15`, `ServerAliveCountMax=3`, `ConnectTimeout=15`).
  **Shared ssh connection (Linux/macOS hubs, ssh hosts):** one authenticated connection per host, opened by the
  attempt's sign-in step (`ssh -M -N -f`, `ControlPersist=yes`, where every prompt happens) and ridden by the
  link step (`BatchMode=yes`, can never prompt); it is ssh's own process and outlives the hub daemon, so a
  restarted hub or app reconnects without asking; it ends at *Sign out* (`hosts.signout`), host removal, an
  edit of the host's route, or when it dies (`hub/master.rs`, `hub/transport.rs`, `hub/provision.rs`; owner's
  decisions: forever, key hosts too, security trade-off accepted; a Windows hub keeps one command per attempt
  with `ControlPath=none`). A host that needed interactive or multi-factor login and whose shared connection is
  gone is held: no ssh is started, a notification (preference `sign_in_needed`) and one Sign in click start
  one attempt; its sessions keep running and reattach (`hub/auth.rs`, `hub/fleet.rs`, `wasit-control`
  `supervised.rs` hold hook (now async), `src-tauri/src/notify.rs`, `state/signin.ts`; tests
  `marsad-agent/tests/interactive_hosts.rs`). A host needs the user when a connect asked anything besides
  ssh's own host-key confirmation.
- "Allow and remember": each thing a choice will also apply is listed under the buttons before the
  click (`GateChoice.effects`, Wasit `d103a5e`; `features/gates/parts.tsx`).
- Workstreams: why a spawn request went away is shown (Wasl's `EndCode` in `wasl-proto/src/end.rs`, topic
  `wasl.spawns`, the hub's `recent_spawns`, `workstreams/Spawning.tsx`); a template holds designed agents
  only (`hub/ws_methods.rs`, test `template_leaves_out_spawned_agents_and_session_identity`); a node's
  session is created with the node's name. Terminal: one pty per session, reaped (`host/pty.rs`,
  `tests/pty_lifecycle.rs`).

**Engine concepts added** (a newcomer needs these)
- *Input id and acknowledgement:* every command is an input whose id is its journal seq (`InputId`,
  `wasit-core`); drivers report `InputSent`, `InputAcknowledged` (only on the CLI's own word; claude:
  `command_lifecycle` frames), `InputDropped` (`wasit-core/src/event.rs`); the core adds written / write-failed.
- *Turn trigger:* `TurnStarted{input, trigger}` is `input`, `agent` (the CLI started it itself) or `unreported`.
- *Background tasks, scoped events:* `tasks.rs`, `AgentEvent::Scoped`, `session.history {scope}` (§3.6).
- *`continuable`, auto-continue, chosen id:* §3.3 and `continuation.rs`.
- *Earlier history:* `history.rs` in core, runtime and `wasit-claude` (§3.8).
- *Turn-bound external gates:* `raise_gate_for_turn` with `TurnBinding` (`wasit-core/src/event.rs`); the
  session's `Core` resolves the gate when the turn is interrupted or ends, in journal order; Wasl's handoff
  and spawn gates and Marsad's `ask_user` (`host/ask.rs`) only observe it. A gate carries a typed
  `GateReason{code, detail}` (§3.7).
- *Held hosts / sign-in:* a link can be held until the user clicks (`Supervised` hold hook, `hub/auth.rs`).
- *Spawn end codes:* `EndCode` in `wasl-proto/src/end.rs`, one vocabulary for how a spawn or handoff ended.
- *Link failures:* `helper_failed` (a remote program that ends before the handshake) parks the link; one
  classifier in `wasit-control/src/command.rs`, used by Marsad's `hub/provision.rs`.

**Tests.** Integrated run, 2026-11-23: Wasit 863 passed / 0 failed / 21 ignored; Wasl 56 / 0 / 3; Marsad
169 / 0 / 6; desktop vitest 728 in 51 files (729 after the small changes below); desktop shell (`src-tauri`) 36; clippy `-D warnings` and fmt
clean; `cargo xwin check` (Windows) clean, 0 warnings. Not verified anywhere: behaviour on Windows and
macOS, the cross-built installer (never run by its builder), real Codex and OpenCode, the real-app suite
`e2e-real` (cannot run on the dev box; its scripts were only updated as text), and everything with the
real Claude CLI except what the opt-in `live*.rs` tests cover (`#[ignore]`d; Wasit
`cargo test -p wasit-runtime --test live -- --ignored`, `--test live_earlier`; they fail loudly without
the CLI). After that run only tests, the fixture backend and documents changed (Wasit 3f4e237, 7adfba4; Marsad
a758318, d929132); those were run on their own (stand-in CLI scenarios 85 passed, `flow.spec.ts` 3 runs,
vitest 729 passed). Run everything with `/work/mewais/Marsad/.notes/verify-all.sh` (see Build / run). Tests run real
processes (daemons, shims, the stand-in CLI), so after a Wasit change run
`cargo build --workspace --all-targets` in Wasl and Marsad before their tests, or stale binaries give
false results. Hygiene: tests end every process they start (`wasit_fake_cli::harness::stop_all_under`, from `TempDir::drop`), leave no
directory behind (guards `no_leftovers.rs` in the three repos) and give every test daemon an explicit
runtime directory under its temp dir (`--run-dir`, `ClientConfig::run_dir`).
Test audits (existing tests that were changed; every finding below is from the audit files, which read
diffs and ran nothing; nothing was re-audited here): *Wasit* — `lifecycle.rs` `a_new_process_that_dies_at_once` and
`a_resume_the_agent_refuses_fails_the_continue_with_its_reason` accept `Err(ContinueFailed)` or
`Ok(Queued)` (a real race between two processes; a unit test in `actor.rs` pins each order); `recovery.rs`
`continue_starts_a_second_generation…` and the live helper `live_turn` compare the snapshot at the exit,
not while running; `host_setup_shells.rs` no longer asserts "a syntax error runs nothing" for csh and
bsd-csh (product change); `drivers.rs` `crash_mid_turn` accepts a prompt after a crash as queued or sent
(it auto-continues); the stand-in's `claude_resume_argument_sets_session_id` was replaced (the stand-in now
follows the real storage rules) and one `model == "claude-haiku-5-5"` line was dropped
and restored afterwards (Wasit 3f4e237, in `claude_stores_a_conversation_with_its_first_prompt_and_resume_needs_it`); the two new reference recordings (claude lifecycle, codex steer) are compared more loosely; the
stand-in CLI changed a lot, so every test that uses it runs against a changed stand-in; allow-lists in
`value_sites.rs` grow by design. *Wasl* — no loosening; assertions became stricter (typed reason codes); one
gap: no test reads an old journal whose gate reason is the old "withdrawn". *Marsad* — two vitest tests
about the Usage tab were deleted and replaced by `usage.test.tsx`; the screens e2e now sees a fixture
terminal; the unreachable-host gates test was rewritten with a stricter replacement; the `flow.spec.ts`
prompt step first asserted less than its comment said and was strengthened afterwards (Marsad a758318: the
prompt to an agent waiting on a question shows as sent, the question stays pending, no reply is added); `store.test.tsx` triggers on an interrupt instead of a
prompt; `notify.rs` `transitions_become_notes` follows the new "finished" rule; 24 trailing stop calls
were removed because `TempDir::drop` ends everything; `stop_hub` is strict and waits for the exit; two
`live_workstream` tests that returned silently without their environment variable now fail with
instructions. `e2e-real/run.mjs` changes (End session, Sign in) were matched against the product by
grep only.

**Layering rule (owner's).** Agent-CLI specifics live only in Wasit's driver crates or behind the driver
descriptor (`DriverDescriptor`: chosen ids, storage and parser of history, the user's resume command,
conversation listing); agent-to-agent lives in Wasl; Marsad only displays and forwards. No agent names in
Marsad's control flow, no workarounds for one agent in the hub or the UI. A search of the lines this round
added to Marsad's non-test sources found an agent name only in a comment and in unit-test code; audit new
diffs the same way.

### Gap-closing round (11 items; Wasit, Wasl, Marsad on this branch)
1. **Save vs. launch ordering** (Marsad 253db74): the per-host lock now spans storing *and*
   applying a save; `pty.start`, `sessions.create`, `sessions.continue` go through
   `hub/hostenv.rs::EnvGated` (exact routes in the hub's mux), which `ensure`s the host runs the
   stored revision before the routed call, as `sessions.launch` did. No sleeps; nothing in the
   UI waits. Test `host_env.rs::terminal_asked_for_during_a_save_gets_the_saved_environment`
   (setup held open on a FIFO; failed before the fix).
2. **Agent command in the UI** (Wasit eb1089e, Marsad 418485a): `SessionSnapshot.command` (the
   stored spec's binary; a continued session keeps it). New session dialog: the agent picker
   shows the command (warning-styled when it does not answer like the CLI), the discovery line
   names it, the warning is spelled out; Config tab: "Command" field. vitest + host_env asserts.
3. **fish / tcsh / csh / zsh** (Wasit cfcde7a, 0a23d2d): real shells (CI installs them).
   Found: tcsh refuses `-l` with `-c`, so every tcsh capture fell back to the daemon's own env;
   csh has no `2>` (the capture's `env -0 2>/dev/null` broke). csh-family now captures with
   `-c` (reads `.cshrc`/`.tcshrc`) sourcing `/etc/csh.login` + `~/.login`; fish setup lines are
   parsed with `fish -n` first (a syntax error used to be "applied" as status 123). Setup lines
   for csh-family logins stay POSIX, run by `/bin/sh` (documented; a csh `module` alias is not
   available there — source the module system's sh init) (changed in the session-flows round: csh,
   tcsh and bsd-csh now run the setup lines in their own syntax, so a csh user writes csh). Test
   `tests/host_setup_shells.rs`.
4. **Native Windows over SSH** (Marsad d559836..1392f62, Wasit 3f916dd): windows-e2e runs Win32
   OpenSSH Server on the runner (port 2223: 127.0.0.1:22 is the WSL distro's sshd via WSL
   localhost forwarding) and `windows.rs::native_windows_host_over_ssh_end_to_end` connects it
   through Marsad's real ssh link: helper provisioning, discovery, PowerShell setup with
   non-ASCII output and `$env:` values, variables (one secret), agent command (the fake CLI,
   claude only through a host variable), a claude session, cmd terminals. Product bugs found:
   a never-connected Windows host got the POSIX bootstrap (Git's `sh` reports MINGW; cmd.exe
   mangles the line) — now recognised / one PowerShell attempt for a first-contact host whose
   POSIX bootstrap printed no report (locale-independent); PowerShell read the redirected stdin
   itself (`-InputFormat None`); PowerShell capture/setup output came back in the OEM code page
   (now a UTF-8 writer on raw stdout; Wasit Windows test covers a non-ASCII registry variable,
   setup output and `$env:` value). **Upload stall — root cause (diagnosed with a throwaway
   matrix workflow on windows-latest, Marsad 6366e1d..5a6374c, since deleted):** OpenSSH for
   Windows gives the remote command a stdin pipe opened for overlapped I/O (`NtQueryInformationFile`
   mode 0x0: no `FILE_SYNCHRONOUS_IO_*`). Windows PowerShell 5.1 and any .NET Framework console
   stream read it synchronously: the first read returns what is already buffered, the next never
   completes. Evidence: the old script read `UPLOAD n` and the first 6129 bytes then hung at
   1 MB/30 MB, with and without `-InputFormat None`, even with stdin closed; 1 KB passed; pwsh 7
   passed 30 MB; `$input` never yielded a line while stdin stayed open; a .NET Framework probe
   hung with `Console.OpenStandardInput()` and read 30 MB in 3 s through an asynchronous
   `FileStream` on the same handle. Also found: first install always failed in `& $b stop`
   (helper not there yet, `$ErrorActionPreference='Stop'`), and errors came back as CLIXML.
   sshd itself closes the channel when the command exits (not a cause). **Fix (Marsad 19a6cb2):**
   the Windows bootstrap is one cmd.exe line — `set MARSAD_BOOT_CS=<deflated C#>&& powershell
   … -EncodedCommand <compile once> <NUL && set MARSAD_BOOT_CS=&& "%LOCALAPPDATA%\Marsad\bin\
   marsad-boot-<hash>.exe" && "…\marsad-agent.exe" bridge --host-id <id>`. PowerShell never
   touches the connection; the C# boot program (`hub/windows_boot.cs`, compiled by Add-Type,
   cached by source hash) speaks the same line protocol on an asynchronous unbuffered stdin;
   failures are one plain `Marsad bootstrap: …` stderr line + exit 1 and nothing further starts
   (verified on the runner: unknown command, connection closed mid-upload, an invalid helper).
   A host whose OpenSSH DefaultShell is Windows PowerShell 5.1 cannot work this way (its shell
   would read stdin itself); cmd.exe is OpenSSH's default. With it, the native test got through
   provisioning, discovery, PowerShell setup (non-ASCII), variables, the agent command and the
   claude session on the first CI run; the cmd terminal then timed out because the test's
   terminal never answered ConPTY's cursor-position query (`ESC[6n`), which the app's xterm.js
   does — test fixed to answer like `git_pty.rs` already did (a test gap, not a product bug).
   Two latent races surfaced in `workstreams.rs::three_nodes_across_local_and_ssh_host` with the
   new timing (3/144 locally): the edge view's gates (session snapshot feed) and in-flight tasks
   (task feed) are two feeds — the test now waits for both before asserting both; and a session
   holding handoff gates read `starting` while its CLI handshake was still pending — product
   fix (Wasit 31fa6b5): pending gates take precedence (`needs_you`), test
   `core.rs::a_gate_raised_while_starting_needs_you` (failed before). After: 160/160.
   `git_pty.rs::pty_echo_and_scrollback_replay` failed once on `test (windows-latest)`: after
   resuming from a cursor it asserted the old line's text was absent, but ConPTY may repaint the
   screen (old lines sent again as new bytes). The test was wrong; it now checks the resumed
   stream byte-exactly (first chunk at the cursor, contiguous after), which is what the cursor
   promises.
5. **Secrets in the OS credential store** (Marsad 255c4de): `marsad-store/src/secrets.rs`,
   keyring 3 (Windows Credential Manager, macOS Keychain, Secret Service via the pure-Rust
   zbus client — no libdbus; musl helper unaffected). Chosen at store open by a write/read/
   delete probe (5 s); a Secret Service is never D-Bus-activated (an activated, locked keyring
   waits for a password prompt — seen locally); blocking calls on their own thread with a
   deadline. Fallback: owner-only `secrets.json`, with the reason. `secrets.json` values move to
   the system store once. `hosts.environment.secrets` → `SecretsLocation`; Hosts → Environment
   says where (warning for the file). Verified on CI: Keychain (macOS), Credential Manager
   (Windows), gnome-keyring under `dbus-run-session` (Linux step), file fallback (headless).
6. **`MARSAD_SURFACE` removed** (Marsad a7d08a9, a8a7b36): the WebDriver suite could not drive
   the app on the hosted runner (tauri-driver 2.1.0 + msedgedriver 157 matching WebView2
   157.0.4341.46: msedgedriver launched the app with `--remote-debugging-port=0` in
   `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` but never found `DevToolsActivePort`; 2 CI attempts,
   verbose driver log in the run), so `e2e-real/translucency.ps1` clicks the real Settings →
   Window background control through Windows UI Automation (mouse at the element), samples the
   screen over a magenta window, and checks the shell's Acrylic answer (its log), every area
   differs from Solid, Solid is solid. Green: content/top bar/rail/status bar differ by 18–19.
   Hosted runners draw Acrylic as its solid fallback (no GPU composition), so "the colour behind
   shows through" is reported, not required — real blur is unverified on real hardware.
7. **Changes tab colouring** (Marsad 58e666f): `DiffView` uses the one highlighter
   (`useHighlighted`/`Tokens`); old side (context+deletions) and new side are each highlighted
   whole by the file's language. vitest.
8. **`ask_user` on interrupt** (Wasit 1313078, Marsad 152cb25): a CLI waits for its tool call
   before acting on a cancel, so the turn never ended. The session's `Interrupt` input after the
   gate went up withdraws the gate and answers the tool; turn ends/interrupts before the gate's
   `GateRequested` are ignored (the old "earlier turn" gap) (changed in the session-flows round: the
   session's own reducer resolves a turn-bound gate; no watcher withdraws it any more). Fake OpenCode/Kilo now call MCP
   servers from `OPENCODE_CONFIG_CONTENT`/`KILO_CONFIG_CONTENT`; test
   `daemon.rs::ask_user_gate_is_withdrawn_when_the_turn_is_interrupted` (hung before the fix).
9. **Deadline inheritance** (Wasl 6ddfab9): a delegated task gets min(own, parent's deadline);
   none left → rejected (`expired`). Test `tasks.rs::delegated_task_inherits_the_parent_deadline`.
10. **daemon-kill-recovery flake** (Marsad 1ecc230): reproduced 2/10 in a codex scenario chain.
   Cause: the hub started the local session feed before the registry put the local host into
   the fleet; the fleet drops feeds of unknown hosts, so when the feed's first event (the full
   list) won the race after a restart the UI showed 0 sessions. Feed now starts after. After:
   15/15 (chain), 20/20 isolated, 20/20 (gate chain), full suite 35/35. `run.mjs --repeat N`.
   Also: `host_env` failed once on Linux CI (a restarted host daemon never got its settings
   back). Root cause (Wasit 86e0a59): the link supervisor announced `Connected` before
   installing the client, so the settings apply that `HostEnvs` starts on that transition could
   be routed in between and fail with "host is not connected". Reproduced 7/60 (8 parallel runs
   on 4 cores, pre-retry code); with the Wasit order fix alone 60/60; final 80/80 + 60/60.
   Wasit test `hub.rs::a_link_announced_connected_is_usable_at_once` (deterministic: the
   state callback checks the client; failed before). The retry schedule (558d7c4) is removed
   (Marsad 063aac2); `HostEnvs` also follows `Hub::events` (every transition; the states watch
   may skip a quick reconnect) and, on lag, ensures every connected host. A failing apply is
   logged and the host shows its settings as not applied (Hosts → Environment).
11. **codex** (Wasit 7856987): `live_codex_image` — a PNG uploaded to a codex 0.169.1 session goes
   out as `localImage`, codex sends it to the (fake) Responses model as an `input_image` data
   URL with the same bytes, the model's reply names it. Not verifiable without an account: real
   OpenAI auth/login, real model behaviour (reasoning items, real image understanding, rate
   limits and real error shapes), anything a real model chooses to do with tools.

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
  (the `MARSAD_SURFACE` / `MARSAD_WINDOW_EFFECT` test hooks have since been removed); the shell reports a hint when Windows'
  Transparency effects are off (Acrylic is then drawn solid — the runner's default, and a
  likely cause of the owner's "varying shade"). CI desktop (windows) job screenshots over
  magenta (`e2e-real/translucency.ps1`, artifact `translucency-windows`). Observed on the
  hosted runner: Acrylic applied + window active + effects on → solid fallback grey
  (220,221,222 vs solid 246,247,250) — no GPU composition; once, without an effect, the
  content showed the magenta through the whole stack (250,139,252), top bar/rail 253,208,254.
  CI (now through the real Settings control over WebDriver) asserts Acrylic and that content/
  top bar/rail/status bar differ from solid.
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
  shells via `/bin/sh` (csh, tcsh and bsd-csh changed in the session-flows round: their own shell and syntax). **Native Windows: PowerShell `-NoProfile -NonInteractive` in the
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
  values absent); secret values in the OS credential store (gap round, item 5), else
  `secrets.json` 0600; every store file is created 0600. A secret is sent to its host only inside
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
  `command →` and warnings; New session dialog and Config tab show the command.
- **Tests**: Wasit `tests/host_setup.rs` (bash login shell + rc-defined `module`; kilo/genie/
  lister; order; secrets scrubbed and absent from session.json/journal; failing/exit/malformed/
  hanging setups), `tests/host_setup_windows.rs` (PowerShell, Windows CI), version parser units.
  Marsad `tests/host_env.rs` (real sshd: module system, genie session, terminals, local host,
  secret only where the hub says secrets are kept and in no log/journal/reply, failing setup, daemon restart
  ⇒ re-applied), windows-e2e WSL (setup puts a genie wrapper on PATH; Test finds it), vitest
  (editing, write-only secret, failure, warning), real-app scenario `host-environment`.
- **Also this round**: `MARSAD_WINDOW_EFFECT` removed (shell + CI script). Flaky Playwright
  "html preview": cause found (Chromium kept the first `srcdoc` document when `srcdoc` changed
  while loading); the preview frame is keyed per document. Wasit `SessionManager::delete` now
  waits for the aborted actor, loader and `session.json` writer before removing the directory
  (macOS CI "Directory not empty").
- **Setup syntax errors**: macOS `/bin/bash` 3.2 in `-l -i` mode prints the syntax error of an
  `eval` and returns status 0, so POSIX shells now parse the lines with `-n` first and report a
  syntax error without running anything (Wasit 17541f2).
- (The not-done items of this round are closed in the gap-closing round above.)

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
- codex image input: verified through the real codex app-server with the fake Responses model
  (gap round, item 11); never against a real OpenAI model.

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
  extension, shell commands, JSON, unified diffs) and Changes-tab diffs (gap round, item 7).
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
  it. Withdrawal = `gate_resolved { reason: "withdrawn" }` (changed in the session-flows round: the reason is a
  typed `{code, detail}`, e.g. `turn_interrupted`, `turn_ended`, `cancelled`, `caller_gone`; old journals read as
  `withdrawn`), gone from the snapshot, late answer →
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
Default (fake CLIs): 35 scenarios (incl. `spawn`, `translucent-window`, `media`, `polish`, `host-environment`), last full run 35/35 (gap round; not run in the session-flows round, whose script edits — "End session", "Sign in" — are text only) — incl. notifications per prefs (Settings
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

Two scripts live OUTSIDE the repos, on the development box, in `/work/mewais/Marsad/.notes/`:
- `build-windows.sh [branch]` cross-builds the Windows x64 NSIS installer from a clean local clone of
  Marsad's `session-flows` (no path patches, so the committed `Cargo.lock` pins must be current and
  Wasit/Wasl pushed): `marsad-agent` for `x86_64-pc-windows-msvc` (`cargo xwin`) and
  `x86_64-unknown-linux-musl` with `--locked`, `scripts/stage-bundle.sh`, then `pnpm tauri build --runner
  cargo-xwin --target x86_64-pc-windows-msvc --bundles nsis -- --locked`; it copies
  `Marsad_<version>_x64-setup-<commit>.exe` to the owner's Windows Downloads folder (path set in the
  script). Needs cargo-xwin and makensis; about 18 minutes. It bundles only the Windows x64 and linux-musl
  x64 helpers (no aarch64 or macOS). The installer it produced was never run by the builder.
- `verify-all.sh` is the integrated verification of the three checkouts (with the path patches): per
  repo build of all targets, tests, clippy `-D warnings`, fmt; `cargo xwin check` for Windows (the whole Wasit
  workspace, only `marsad-agent` and its tests for Marsad); Marsad also regenerates the TypeScript types and counts diffs, `pnpm build`, vitest and the
  shell's `cargo test`; it restores Marsad's two lock files at the end. It sets `WASIT_TEST_SHELL_DIR` to a
  directory holding real csh/tcsh/bsd-csh (Wasit's shell tests fail when a shell is missing) and runs
  Marsad's clippy with `-A clippy::nonminimal_bool`.

## Not done / known
- Free models are slow/flaky: `opencode run` occasionally stalls (retried); a /review on the
  free model can run for minutes (the slash scenario interrupts after a minute). A failing live
  scenario can cascade into the next ones on the same session — rerun with `--only`.
- Spawning: `--real` e2e has no live spawn scenario (Rust live test only).
- The fake CLI's `mcp_call` step is synchronous, so the interrupt-while-a-tool-blocks tests drive
  `wasl.spawn` over RPC during a `wait_for_interrupt` turn instead of through the fake agent.
- Session-flows round, open:
  - **B4 (a prompt after a finished turn never arrived) is not established.** Only the owner's real CLI can
    show it; ask for a diagnostics export (Settings → Diagnostics) and read the session's prompt path.
  - **Shared ssh connection:** not verified on macOS (socket path bound computed, not measured), with
    `ProxyJump` hosts (measured by hand in the investigation, no test), or against servers with `MaxSessions 1`.
    A signed-out key host connects again by itself at the next hub start (the sign-out is not stored).
  - In the app, "queued" means a prompt still held by Marsad's driver and not yet written to the agent; one
    written to an agent that queues prompts itself shows as sent, and the app has no separate mark for
    "written but not yet acknowledged by the agent" (`features/session/transcript.ts`; the acknowledgement is
    in the session trail in Developer mode). For B4 this means a prompt the agent never took looks like any
    sent prompt in the transcript.
  - After the last background task ends the session reads Idle for 10–100 ms (in the recordings) until
    the turn the CLI starts by itself: the CLI gives no notice, so this is the truthful reading. If no turn
    follows, no "finished" notification is raised at all (`src-tauri/src/notify.rs` documents it).
  - Earlier history is bounded (most recent 64 MiB, items cut at 256 KiB, a main file above 1 GiB not
    imported) and has no load-older: omitted messages are only counted. A subagent that appears only in
    earlier history is not in the snapshot's subagent list, so its transcript view shows the generic label
    "Subagent" and says the session has no record of it (`SubagentView.tsx`).
  - A gate raised after its turn ended (between turns, or while only background work runs) belongs to the
    session, not to a turn, and is not withdrawn when a turn is interrupted or ends (Wasit §3.7).
  - One-key gate answers are unchanged: `a`, `d` and `1`–`4` answer the active gate without confirmation
    (`features/gates/PermissionGate.tsx`; the investigation listed this as the owner's product rule).
  - Nothing in Marsad or Wasit stops two sessions, or a session and the user's own terminal, from driving the
    same agent conversation at once (no check found in `manager.rs` create or the hub; the investigation
    proposed refusing it on one host, not implemented): the transcript can branch.
  - csh and tcsh cannot check setup commands without running them: for csh and bsd-csh the commands before a
    syntax error have already run; only a real tcsh pre-checks, and lexical errors only. Wasit's
    `ARCHITECTURE.md` §3.5 and the module doc of `wasit-runtime/src/environment.rs` describe this.
  - The terminal's close grace is `HANG_UP_GRACE` = 2 s (then the shell is killed; reaping bounded by
    `KILL_WAIT` = 10 s) in `crates/marsad-agent/src/host/pty.rs`; the values are not measured, the code
    comment only says a shell takes milliseconds.
  - Not verified: any of this round on Windows or macOS (compile check only), real Codex and OpenCode
    (their earlier history, acknowledgements and queue behaviour were tested against the stand-ins and
    recorded fixtures only), the real-app suite `e2e-real`, the cross-built installer, and the visual
    result of the new UI (tool-run row, not-sent bubbles, model menu, sidebar, sign-in dialog) by a
    person. The owner answered "Defaults are good" to the diagnostics defaults only. The interactive-login rule
    and the "Allow and remember" listing were built as the manager recommended; the owner was asked at
    checkpoint 1 and has not answered yet.
