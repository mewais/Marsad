# Marsad architecture

The authoritative target architecture for Marsad, Wasl and Wasit is
[`Wasit/ARCHITECTURE.md`](https://github.com/mewais/Wasit/blob/claude/awesome-euler-r7x45r/ARCHITECTURE.md)
(§1 rules are non-negotiable; Marsad's parts are §2.3, §4.1, §6, §7). This file only records
Marsad-specific decisions.

## Crates

| Crate | Role |
|---|---|
| `marsad-proto` | Every Marsad wire type + method/topic names, re-exported Wasit wire types, the pure `effective_config` merge. `ts` feature → TypeScript. |
| `marsad-store` | Hub persistence: one writer task, atomic JSON documents (hosts, settings, notification prefs, override layers, seen-markers, recent projects, workstreams, workstream templates). |
| `marsad-agent` | The one daemon binary: `serve [--hub]`, `bridge`, `stop`, `shim`, `mcp` (Wasl MCP server), `--version`. |
| `marsad-client` | Typed client of the local daemon over a supervised link; starts the daemon on demand. |
| `marsad-fake-cli` | Test support only: builds Wasit's fake agent CLI into this workspace's target dir. |

`apps/desktop` is built and shipped: `src-tauri` is a thin Tauri v2 shell over `marsad-client` (its own cargo package, not a workspace member), `src` is the React/TypeScript UI, and the release workflow builds its installers.

## Files on a machine (shared homes)

`marsad_proto::layout` (Wasit ARCHITECTURE §4.2): the data root `~/.marsad` may be shared by
several machines, so durable data lives in `~/.marsad/machines/<machine key>/` (sessions, wasl,
store, log) and sockets/locks in a machine-local runtime dir (`/tmp/marsad-<uid>/<tag>/` unless
the user lingers, then `/run/user/<uid>/marsad/<tag>/`; macOS the per-user temp dir; Windows
`%LOCALAPPDATA%\Marsad\run`). The helper cache `~/.marsad/bin/<os>-<arch>/` stays shared
(installed via a unique temp name + rename). `src/migrate.rs` adopts the flat layout of earlier
builds. `stop` also stops a daemon of an earlier build at the old socket. The hub records each
host's machine key (`HostEntry.machine`, from `Hello.machine`), warns when two hosts are one
machine, and refuses to restart a daemon away from its twin.

## Daemon composition

Every daemon serves, in one `wasit_control::ServiceMux`: the Wasit `RuntimeService` (`host.info`,
`drivers.*`, `sessions.*`, `session.*`), an embedded Wasl node (`wasl.*`, journal in
`<data>/machines/<key>/wasl/`) and Marsad's host services (`git.*`, `pty.*`, `fs.*`, `agentconfig.base`,
`daemon.*`). The GUI machine's daemon runs with `--hub` and additionally serves `hosts.*`,
`store.*`, `fleet`, `workstreams[.*]`, `agentconfig.effective` and `sessions.launch`; host
services, `wasl.*` and runtime methods carrying `host = <id>` are forwarded by
`RoutingService` over that host's link. On a remote daemon the hub's link (role `hub`) is the
Wasl node's uplink and supervisor; on the hub the node relays over the hub's own links and
always raises handoff gates (the hub is the supervisor).

Remote daemons are started by the link's bootstrap as `bridge --host-id <hub's id for the
host>`, so their Wasl node addresses agents with the id the hub routes by; a running daemon
of the same build but another id is restarted by the next attempt.

### Terminals (`pty.*`)

A session has at most one terminal, identified by the session and owned by the daemon of the
session's host (`host/pty.rs`). `pty.start` for a session is "open": it returns the live terminal
(`created: false`), replaces an exited one, or starts a shell (`created: true`); clients attach
with `pty.output:<id>` (replay from a byte offset, then live) and detaching is just
unsubscribing. A terminal lives until its shell exits, a client closes it, its session is
deleted (the daemon follows the runtime's `host.sessions` feed: the `Removed` event, and a check
against the manager after a `Reset`), or the daemon shuts down (every shell is hung up, killed
if it ignores that, and reaped before the daemon exits). An *ended* session keeps its terminal:
the shell is the user's, not the agent's. A shell that exited stays readable, with its status
and an `exited` event at the end of its stream that a later subscriber replays, until it is
closed or replaced, so a session holds one entry at most. Directory terminals (no session) have
no slot and are capped (`limit_reached`). The app's Terminal tab asks the host which terminal the
session has, starts one only when there is none, and shows only what the host confirmed; its
Close terminal button is the only thing besides the shell's own exit that ends it.

## Workstreams (§5)

The hub owns typed workstream documents (`marsad_proto::workstream`): nodes (a session
`(host, id)` or a launch spec with its own override layer, stored as the `workstream_node`
layer), connections with an `EdgePolicy` (`ask` / `always_allow` / `deny` = blocked), limits,
layout, state (`draft | running | paused | archived`). One actor (`hub::workstreams`) owns the
documents, the fleet's session snapshots and every node's outbound tasks; every committed
change re-derives the Wasl topology and, if it differs, bumps `topology_version` (persisted
first) and pushes it to every host the workstream ever touched (`wasl.topology.set`,
idempotent, versioned). It re-pushes to a host whenever its link connects and to all hosts on
hub start. Pause is the topology's `paused` flag (new delegations rejected). "Always allow"
chosen in a handoff gate arrives as a `wasl.edges` decision event and is written into the
document only if it was taken under the topology version in force (replays and overtaken
decisions are ignored). The actor reaches all nodes, local or remote, through an in-process
supervised connection to the hub's routing service. `workstreams.launch` launches nodes through
`sessions.launch` (labels `marsad.workstream` / `marsad.node`), pushes the topology and only
then sends first prompts. The `workstreams` topic carries, per workstream, the document,
node session snapshots, edges with in-flight tasks, pending handoff gates (from the sending
session's `pending_gates`) and counters, topology sync per host and a cost roll-up, pending spawn
gates, and the latest ended spawn requests of its agents (`recent_spawns`, newest first, at most
20: launched or why not). The hub only relays those: each host node publishes its ended spawn
requests (`wasl.spawns`, from its own journal record, Wasl's `End` vocabulary); nothing is stored
by the hub, and after a restart the feeds replay the nodes' journals.

Every session Marsad launches gets the Wasl MCP server (`marsad-agent mcp --socket <that
host's daemon socket> --session <id>`, from `daemon.info`) as a locked automatic item marked
`trusted`, so a running session can join a workstream without a restart. Sessions spawned by
agents are a later round (`hub::workstreams::Workstreams::edit` is the extension point).

## Links to remote hosts (deviates from Wasit ARCHITECTURE §4.1 by owner decision)

* On a Windows hub (and for `wsl.exe`) one ssh invocation per connect attempt does probe,
  upload-if-needed and `bridge`, with `ControlMaster=no ControlPath=none` so a user's ssh
  configuration cannot put it on a shared connection. On a Linux or macOS hub every ssh host
  has ONE shared connection (`hub/master.rs`, `hub/transport.rs` `SshRoute`), opened by the
  attempt's sign-in step and ridden by every link step: per attempt, (a) `ssh -O check` on the
  host's control socket (`<run>/ssh/<16 hex of host id + route>`, local, no network; a socket
  left by a killed master is removed); (b) with none open, a host that needs the user at login
  (or that the user signed out of) and no click is parked as `auth_needed`/`held` without
  starting ssh, anything else runs the sign-in step `ssh -M -N -f -o ControlMaster=yes -o
  ControlPersist=yes -E <socket>.log` with the askpass helper, where every prompt of the attempt
  happens and whose exit with status 0 (ssh detached once authenticated) is the signal that the
  connection is open; its failure is classified from its exit status and the log, as the one
  command was; (c) the link step is the bootstrap below with `ControlMaster=no
  BatchMode=yes` on the same socket: it can never prompt (with no master it connects directly
  for a key host and fails without asking for a host that needs the user). The connection is ssh's
  own detached process (`ControlPersist=yes`) and outlives the hub daemon; it ends at sign-out
  (`hosts.signout`), host removal or an edit of the host's route (`ssh -O exit`), or when it
  dies (the reason is the tail of its log, in the trail `ssh.*` and in the wait's words). So a
  link that drops while its connection lives reconnects by itself even for a host that needs
  the user, a restarted hub finds the connection with `-O check` and reconnects silently, and a
  restart attempt rides it (no second prompt). Human time is the prompts' (each bounded by the
  prompt timeout), machine time ssh's `ConnectTimeout`; the whole attempt keeps the
  supervisor's one bound. The wire carries `HostEntry.shared_connection` (derived by the hub,
  false on Windows) so the UI offers *Sign out* only where it works.
* The bootstrap of one connect attempt does probe, upload-if-needed and
  `bridge`: a bootstrap prints `MARSAD-BOOT <os> <arch> <installed build>`, takes `RUN`,
  `RESTART` or `UPLOAD <n>` + n bytes, installs atomically, stops the old daemon, answers
  `MARSAD-OK` and execs `marsad-agent bridge`. A running daemon of another build (seen in the
  hello) is restarted by the next attempt (wasit-control's `Provisioner` hook).
* No `BatchMode` on the step that may prompt (the whole command on a Windows hub, the sign-in
  step elsewhere). ssh runs with `SSH_ASKPASS=<marsad-agent>` and `SSH_ASKPASS_REQUIRE=force`;
  every prompt (password, passphrase, OTP, keyboard-interactive, host key) is published as the
  host's `auth_prompt` in the `fleet` topic and answered with `hosts.auth_answer` /
  `hosts.auth_cancel`, or times out (`--auth-prompt-timeout-ms`). Answers are never logged or
  stored. Prompt state lives in Marsad (`FleetHost.auth_prompt`), next to Wasit's `LinkState`.
* A host that has needed an interactive answer (`HostEntry.interactive_auth`) never
  starts a sign-in on its own: when no shared connection is open (always, on a Windows hub), a
  dropped link parks in `auth_needed` until `hosts.reconnect`.
  Key/agent hosts reconnect with backoff. Sessions are held by shims and catch up from cursors.
* Binaries: the running executable for its own target triple, else
  `$MARSAD_AGENT_BIN_DIR/<triple>/marsad-agent[.exe]`.

## Agent configuration (§6)

`agentconfig.base` reports what each CLI itself has configured (Wasit discovery
`DiscoveryKind::Config`), each item with scope and source. `effective_config(base, layers
Global→Host→WorkstreamNode→Session, automatic)` is pure; inherited items are never re-rendered,
only changes/additions/disables, rendered by the driver's `ConfigRenderer`, which also reports
what it cannot honour. `sessions.launch` computes it, picks the session id, calls the runtime's
`sessions.create`, stores the session layer. Automatic items (the Wasl MCP server, locked,
provenance `automatic`) come from `hub::AutomaticItems` providers.

## Store

JSON documents rather than sqlite: tiny data, human-inspectable, no native dependency for the
cross-compiled targets; one writer task gives serialisable writes without locks.

## Diagnostics

Rules for events, fields and files are in Wasit `ARCHITECTURE.md` §9 "Diagnostics
(`wasit-diag`)"; this section only says where Marsad applies them.

**Declaring sites** (`wasit_diag::diag_events!` / `diag_enum!` / `diag_data!`):
`marsad-proto/src/diag.rs` (`MarsadEvent`, `UiEvent` for the webview, `AppEvent` for the shell);
`marsad-proto/src/export.rs` (`ExportLine`, the export file's wire format);
`marsad-agent/src/hub_events.rs` (`HubEvent`) and `hub/fleet_events.rs` (`FleetDiagEvent`);
`marsad-client/src/diag.rs` (`ClientEvent`); `marsad-store/src/diag.rs` (`StoreEvent`);
Wasit's `wasit-shim/src/diag.rs` (`ShimEvent`) and `wasit-runtime/src/diag.rs`.

**Sinks and files** (under `<data>/machines/<key>/`, see `marsad-proto/src/layout.rs`):

* The daemon (`serve`) promotes `diag/aux.ndjson` to `diag/daemon.ndjson` after it takes the
  lock (`marsad-agent/src/diag.rs:130`). Short-lived roles (`Bridge`, `Stop`, `Askpass`, and a
  daemon that loses the lock race) append to `aux.ndjson`.
* The desktop shell writes `diag/app.ndjson` (`apps/desktop/src-tauri/src/diag.rs`). The webview
  writes only through `diag_log`, which validates each event with `UiEvent::from_wire` and
  refuses the rest as `app.failed`.
* Each session's shim writes `sessions/<id>/shim.log` (and `shim.stderr`).
* `tracing` is counted into the sink (`trace.counted`) and printed as text only with
  `MARSAD_LOG` set (`marsad-agent/src/diag.rs:153-176`).
* `marsad-agent.log` is raw stderr only; it is never a record and never exported.

**Host logs.** A host's sink files are read through `wasit_runtime::DiagService`: `diag.sources`,
`diag.read`, and topic `diag.tail`, on `DIAG_ROUTES` (`["diag."]`). It is mounted in
`marsad-agent/src/serve.rs` (about lines 286-291 and 558). The hub routes `diag.` by host
(`marsad-proto/src/lib.rs`, about line 428). Only files the sink itself lists can be read.

**Export.** The hub's `Exports` actor (`marsad-agent/src/hub/export/mod.rs`) runs one export at a
time. Its job task (`job.rs`) collects each connected host's sources, and `gzip.rs` writes
`diag/exports/marsad-diagnostics-<host>-<id>.ndjson.gz`. Start is `diagnostics.export.start`;
progress is topic `diagnostics.exports` (`ExportsEvent`). Limits in `job.rs` `Limits::default`:
24 h window by default (31 d maximum), 20 MiB total, 16 MiB per host, 30 s per call, 180 s per
host, and the newest 3 finished exports are kept. The CLI `marsad-agent diagnostics export`
(`export_cli.rs`) uses the same job through the daemon, or `local_files` mode when there is no
hub.

**Trace id.** The webview makes `u-` plus 16 hex digits (`apps/desktop/src/bridge/api.ts:7-8`).
It is passed through `invoke("rpc", …)` to `bridge.rs` `call_traced`, to
`marsad-client/src/lib.rs` `call_traced` (carried in `RequestOpts`), to the hub's
`RoutingService`, which forwards `ctx.trace` (`wasit-control/src/hub.rs`). The session manager's
`command_traced` (`wasit-runtime/src/manager.rs`) journals it with the command as an `Intent`
(`wasit-runtime/src/diag.rs`), and it is shown on the session's `input` event.

**Source-scan tests.** `marsad-agent/tests/diag_sites.rs` scans the source tree with
`wasit_process::scan` for the declaration and free-text patterns. Two tests pin free text:
`error_text_from_display_only_in_reviewed_places` (`ErrorInfo::ours(` only in
`marsad-agent/src/diag.rs`) and `free_text_from_a_transport_only_in_reviewed_places`
(`DiagText::new(` only in the reviewed places). `field_types_and_free_text_only_in_reviewed_places`
pins where field types are declared, and `the_allow_lists_are_live` and `the_scan_sees_each_pattern`
check the scanner itself.
