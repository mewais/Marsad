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

`apps/desktop` (Tauri + React) is outside the Rust workspace and is being rebuilt separately.

## Files on a machine (shared homes)

`marsad_proto::layout` (Wasit ARCHITECTURE §4.2): the data root `~/.marsad` may be shared by
several machines, so durable data lives in `~/.marsad/machines/<machine key>/` (sessions, wasl,
store, log) and sockets/locks in a machine-local runtime dir (`/tmp/marsad-<uid>/<tag>/` unless
the user lingers, then `$XDG_RUNTIME_DIR/marsad/<tag>/`; macOS `$TMPDIR`; Windows
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
session's `pending_gates`) and counters, topology sync per host and a cost roll-up.

Every session Marsad launches gets the Wasl MCP server (`marsad-agent mcp --socket <that
host's daemon socket> --session <id>`, from `daemon.info`) as a locked automatic item marked
`trusted`, so a running session can join a workstream without a restart. Sessions spawned by
agents are a later round (`hub::workstreams::Workstreams::edit` is the extension point).

## Links to remote hosts (deviates from Wasit ARCHITECTURE §4.1 by owner decision)

* One ssh (or `wsl.exe`) invocation per connect attempt does probe, upload-if-needed and
  `bridge`: a bootstrap prints `MARSAD-BOOT <os> <arch> <installed build>`, takes `RUN`,
  `RESTART` or `UPLOAD <n>` + n bytes, installs atomically, stops the old daemon, answers
  `MARSAD-OK` and execs `marsad-agent bridge`. A running daemon of another build (seen in the
  hello) is restarted by the next attempt (wasit-control's `Provisioner` hook).
* No `BatchMode`. ssh runs with `SSH_ASKPASS=<marsad-agent>` and `SSH_ASKPASS_REQUIRE=force`;
  every prompt (password, passphrase, OTP, keyboard-interactive, host key) is published as the
  host's `auth_prompt` in the `fleet` topic and answered with `hosts.auth_answer` /
  `hosts.auth_cancel`, or times out (`--auth-prompt-timeout-ms`). Answers are never logged or
  stored. Prompt state lives in Marsad (`FleetHost.auth_prompt`), next to Wasit's `LinkState`.
* A host that has needed an interactive answer (`HostEntry.interactive_auth`) never
  reconnects on its own: a dropped link parks in `auth_needed` until `hosts.reconnect`.
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
