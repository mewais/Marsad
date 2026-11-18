# Marsad architecture

The authoritative target architecture for Marsad, Wasl and Wasit is
[`Wasit/ARCHITECTURE.md`](https://github.com/mewais/Wasit/blob/claude/awesome-euler-r7x45r/ARCHITECTURE.md)
(§1 rules are non-negotiable; Marsad's parts are §2.3, §4.1, §6, §7). This file only records
Marsad-specific decisions.

## Crates

| Crate | Role |
|---|---|
| `marsad-proto` | Every Marsad wire type + method/topic names, re-exported Wasit wire types, the pure `effective_config` merge. `ts` feature → TypeScript. |
| `marsad-store` | Hub persistence: one writer task, atomic JSON documents (hosts, settings, notification prefs, override layers, seen-markers, recent projects, opaque workstream docs). |
| `marsad-agent` | The one daemon binary: `serve [--hub]`, `bridge`, `stop`, `shim`, `mcp` (stub), `--version`. |
| `marsad-client` | Typed client of the local daemon over a supervised link; starts the daemon on demand. |
| `marsad-fake-cli` | Test support only: builds Wasit's fake agent CLI into this workspace's target dir. |

`apps/desktop` (Tauri + React) is outside the Rust workspace and is being rebuilt separately.

## Daemon composition

Every daemon serves, in one `wasit_control::ServiceMux`: the Wasit `RuntimeService` (`host.info`,
`drivers.*`, `sessions.*`, `session.*`) and Marsad's host services (`git.*`, `pty.*`, `fs.*`,
`agentconfig.base`, `daemon.*`). The GUI machine's daemon runs with `--hub` and additionally
serves `hosts.*`, `store.*`, `fleet`, `agentconfig.effective` and `sessions.launch`; host
services and runtime methods carrying `host = <id>` are forwarded by `RoutingService` over that
host's link.

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
`sessions.create`, stores the session layer. Automatic items (the future Wasl MCP server, locked,
provenance `automatic`) come from `hub::AutomaticItems` providers.

## Store

JSON documents rather than sqlite: tiny data, human-inspectable, no native dependency for the
cross-compiled targets; one writer task gives serialisable writes without locks.
